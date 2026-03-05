#![cfg_attr(target_arch = "wasm32", no_main)]
#![cfg_attr(target_arch = "wasm32", no_std)]

//! Stylus GKR DAG Verifier -- entry point.
//!
//! This crate implements a BN254 GKR (Goldwasser-Kalai-Rothblum) DAG verifier
//! targeting the Arbitrum Stylus (WASM) runtime.  It can also be compiled as a
//! native library for testing.
//!
//! ## Verification pipeline
//!
//! 1. Strip the 4-byte "REM1" selector prefix from the proof blob.
//! 2. Decode the flat, 32-byte-aligned proof using [`decode::ProofDecoder`].
//! 3. Decode Pedersen generators from a separate byte slice.
//! 4. Decode the DAG circuit description from a separate byte slice.
//! 5. Extract embedded public inputs and input-layer commitment coordinates.
//! 6. Initialise the Fiat-Shamir transcript (Poseidon sponge) from the
//!    circuit hash, public inputs, and EC commitment coordinates.
//! 7. Verify all compute layers (sumcheck + proof-of-product per layer).
//! 8. Verify all input layers (committed PODP and/or public-value MLE checks).
//! 9. Return `true` if every check passes; panic otherwise.

// The alloc crate is needed unconditionally because stylus-sdk is no_std
// and its proc macros generate code referencing alloc::vec.  On std builds
// alloc is a subset of std and is always available.
extern crate alloc;

// ------------------------------------------------------------------
// Module declarations
// ------------------------------------------------------------------

pub mod field;
pub mod poseidon;
pub mod ec;
pub mod transcript;
pub mod gkr;
pub mod sumcheck;
pub mod hyrax;
pub mod decode;

// ------------------------------------------------------------------
// Imports
// ------------------------------------------------------------------

use alloc::vec;
use alloc::vec::Vec;

use crate::field::{Fq, U256};
use crate::gkr::DAGCircuitDescription;
use crate::decode::ProofDecoder;

// ------------------------------------------------------------------
// Stylus SDK imports
// ------------------------------------------------------------------

#[cfg(feature = "stylus")]
use stylus_sdk::prelude::*;

// ------------------------------------------------------------------
// Circuit description decoder
// ------------------------------------------------------------------

/// Decode a `DAGCircuitDescription` from a flat byte slice.
///
/// The encoding is a sequential stream of 32-byte big-endian U256 values.
/// Variable-length arrays are length-prefixed: a U256 count followed by
/// that many U256 elements.
///
/// Layout (in order):
///   num_compute_layers          (u256)
///   num_input_layers            (u256)
///   layer_types                 (length-prefixed u256[])
///   num_sumcheck_rounds         (length-prefixed u256[])
///   atom_offsets                (length-prefixed u256[])
///   atom_target_layers          (length-prefixed u256[])
///   atom_commit_idxs            (length-prefixed u256[])
///   pt_offsets                  (length-prefixed u256[])
///   pt_data                     (length-prefixed u256[])
///   input_is_committed          (length-prefixed u256[], 0 or 1)
///   oracle_product_offsets      (length-prefixed u256[])
///   oracle_result_idxs          (length-prefixed u256[])
///   oracle_expr_coeffs          (length-prefixed u256[])
fn decode_circuit_description(data: &[u8]) -> DAGCircuitDescription {
    let mut dec = ProofDecoder::new(data);

    let num_compute_layers = read_usize_from(&mut dec);
    let num_input_layers = read_usize_from(&mut dec);

    let layer_types = read_u8_array(&mut dec);
    let num_sumcheck_rounds = read_usize_array(&mut dec);
    let atom_offsets = read_usize_array(&mut dec);
    let atom_target_layers = read_usize_array(&mut dec);
    let atom_commit_idxs = read_usize_array(&mut dec);
    let pt_offsets = read_usize_array(&mut dec);
    let pt_data = read_u64_array(&mut dec);
    let input_is_committed = read_bool_array(&mut dec);
    let oracle_product_offsets = read_usize_array(&mut dec);
    let oracle_result_idxs = read_usize_array(&mut dec);
    let oracle_expr_coeffs = read_u256_array(&mut dec);

    DAGCircuitDescription {
        num_compute_layers,
        num_input_layers,
        layer_types,
        num_sumcheck_rounds,
        atom_offsets,
        atom_target_layers,
        atom_commit_idxs,
        pt_offsets,
        pt_data,
        input_is_committed,
        oracle_product_offsets,
        oracle_result_idxs,
        oracle_expr_coeffs,
    }
}

/// Read a single U256 from the decoder, returning its low-order usize value.
fn read_usize_from(dec: &mut ProofDecoder) -> usize {
    let v = dec.read_u256_public();
    // Only use the lowest 64 bits; panic if larger.
    assert!(
        v.0[1] == 0 && v.0[2] == 0 && v.0[3] == 0,
        "value too large for usize"
    );
    v.0[0] as usize
}

/// Read a length-prefixed array of U256 values interpreted as `u8`.
fn read_u8_array(dec: &mut ProofDecoder) -> Vec<u8> {
    let len = read_usize_from(dec);
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        let v = dec.read_u256_public();
        result.push(v.0[0] as u8);
    }
    result
}

/// Read a length-prefixed array of U256 values interpreted as `usize`.
fn read_usize_array(dec: &mut ProofDecoder) -> Vec<usize> {
    let len = read_usize_from(dec);
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(read_usize_from(dec));
    }
    result
}

/// Read a length-prefixed array of U256 values interpreted as `u64`.
fn read_u64_array(dec: &mut ProofDecoder) -> Vec<u64> {
    let len = read_usize_from(dec);
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        let v = dec.read_u256_public();
        result.push(v.0[0]);
    }
    result
}

/// Read a length-prefixed array of U256 values interpreted as `bool` (0 = false, nonzero = true).
fn read_bool_array(dec: &mut ProofDecoder) -> Vec<bool> {
    let len = read_usize_from(dec);
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        let v = dec.read_u256_public();
        result.push(!v.is_zero());
    }
    result
}

/// Read a length-prefixed array of raw U256 values.
fn read_u256_array(dec: &mut ProofDecoder) -> Vec<U256> {
    let len = read_usize_from(dec);
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(dec.read_u256_public());
    }
    result
}

// ------------------------------------------------------------------
// Core verification logic (also usable as a library function)
// ------------------------------------------------------------------

/// Verify a DAG GKR proof end-to-end.
///
/// # Arguments
///
/// * `proof_data` -- The proof blob prefixed with a 4-byte selector (`REM1`).
///   Everything after the selector is a flat array of 32-byte big-endian fields
///   conforming to the layout expected by `decode::ProofDecoder`.
///
/// * `_public_inputs` -- ABI-encoded public input values (currently unused;
///   the verifier extracts embedded public inputs from the proof body).
///
/// * `gens_data` -- Serialised Pedersen generators (message generators followed
///   by the scalar generator and blinding generator, each as 64-byte G1 affine
///   points with a leading length field).
///
/// * `circuit_desc_data` -- ABI-encoded DAG circuit topology describing layer
///   types, atom routing, point templates, and oracle expressions.
///
/// # Returns
///
/// `true` when verification succeeds. The function **panics** on any
/// verification failure (invalid sumcheck, mismatched commitment, etc.).
pub fn verify_dag_proof_inner(
    proof_data: &[u8],
    _public_inputs: &[u8],
    gens_data: &[u8],
    circuit_desc_data: &[u8],
) -> bool {
    // ----------------------------------------------------------------
    // 1. Strip "REM1" 4-byte selector
    // ----------------------------------------------------------------
    assert!(proof_data.len() >= 4, "proof data too short for selector");
    assert!(&proof_data[0..4] == b"REM1", "invalid selector");
    let proof_bytes = &proof_data[4..];

    // ----------------------------------------------------------------
    // 2. Extract circuit hash (first 32 bytes of proof body)
    // ----------------------------------------------------------------
    assert!(
        proof_bytes.len() >= 32,
        "proof body too short for circuit hash"
    );
    let mut circuit_hash = [0u8; 32];
    circuit_hash.copy_from_slice(&proof_bytes[0..32]);

    // ----------------------------------------------------------------
    // 3. Decode the proof
    // ----------------------------------------------------------------
    let mut decoder = ProofDecoder::new(proof_bytes);
    let (gkr_proof, embedded_pub_inputs, dag_input_proofs, public_value_claims) =
        decoder.decode_proof_for_dag();

    // ----------------------------------------------------------------
    // 4. Convert embedded public inputs to Fq for transcript
    // ----------------------------------------------------------------
    let embedded_fqs: Vec<Fq> = embedded_pub_inputs.iter().map(|u| Fq(*u)).collect();

    // ----------------------------------------------------------------
    // 5. Decode Pedersen generators
    // ----------------------------------------------------------------
    let gens = ProofDecoder::decode_pedersen_gens(gens_data);

    // ----------------------------------------------------------------
    // 6. Decode circuit description
    // ----------------------------------------------------------------
    let desc = decode_circuit_description(circuit_desc_data);

    // ----------------------------------------------------------------
    // 7. Collect input commitment coordinates for transcript
    // ----------------------------------------------------------------
    let mut input_commit_coords: Vec<U256> = Vec::new();
    for dag_proof in &dag_input_proofs {
        for row in &dag_proof.commitment_rows {
            input_commit_coords.push(row.x);
            input_commit_coords.push(row.y);
        }
    }

    // ----------------------------------------------------------------
    // 8. Setup Fiat-Shamir transcript
    // ----------------------------------------------------------------
    let mut sponge = transcript::setup_transcript(
        &circuit_hash,
        &embedded_fqs,
        &input_commit_coords,
    );

    // ----------------------------------------------------------------
    // 9. Verify compute layers
    // ----------------------------------------------------------------
    let ctx = gkr::verify_compute_layers(
        &gkr_proof,
        &desc,
        &gens,
        &mut sponge,
    );

    // ----------------------------------------------------------------
    // 10. Verify input layers
    // ----------------------------------------------------------------
    hyrax::verify_input_layers(
        &gkr_proof,
        &desc,
        &gens,
        &ctx,
        &mut sponge,
        &embedded_pub_inputs,
        &dag_input_proofs,
        &public_value_claims,
    );

    true
}

// ------------------------------------------------------------------
// Stylus contract
// ------------------------------------------------------------------

#[cfg(feature = "stylus")]
sol_storage! {
    /// On-chain GKR DAG verifier contract deployed on Arbitrum Stylus.
    ///
    /// This contract is stateless -- it stores no data.  All inputs are
    /// provided via calldata to the single public method.
    #[entrypoint]
    pub struct GKRVerifier {}
}

#[cfg(feature = "stylus")]
#[public]
impl GKRVerifier {
    /// Verify a DAG GKR proof on-chain.
    ///
    /// # Arguments
    ///
    /// * `proof_data` -- Proof blob with 4-byte "REM1" selector prefix.
    /// * `public_inputs` -- ABI-encoded public input values.
    /// * `gens_data` -- ABI-encoded Pedersen generators.
    /// * `circuit_desc_data` -- ABI-encoded DAG circuit description.
    ///
    /// # Returns
    ///
    /// `true` if verification passes; reverts otherwise.
    pub fn verify_dag_proof(
        &self,
        proof_data: Vec<u8>,
        public_inputs: Vec<u8>,
        gens_data: Vec<u8>,
        circuit_desc_data: Vec<u8>,
    ) -> bool {
        verify_dag_proof_inner(
            &proof_data,
            &public_inputs,
            &gens_data,
            &circuit_desc_data,
        )
    }
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::field::U256;
    use crate::gkr::DAGCircuitDescription;

    /// Proof data shorter than 4 bytes must be rejected.
    #[test]
    #[should_panic(expected = "proof data too short for selector")]
    fn test_verify_rejects_short_proof() {
        let short = [0u8; 3];
        let _ = verify_dag_proof_inner(&short, &[], &[], &[]);
    }

    /// Wrong selector must be rejected.
    #[test]
    #[should_panic(expected = "invalid selector")]
    fn test_verify_rejects_wrong_selector() {
        let mut data = vec![0u8; 68]; // 4 selector + 64 body
        data[0..4].copy_from_slice(b"BAAD");
        let _ = verify_dag_proof_inner(&data, &[], &[], &[]);
    }

    /// Proof body (after selector) shorter than 32 bytes must be rejected.
    #[test]
    #[should_panic(expected = "proof body too short for circuit hash")]
    fn test_verify_rejects_no_circuit_hash() {
        let mut data = vec![0u8; 20]; // 4 selector + 16 body
        data[0..4].copy_from_slice(b"REM1");
        let _ = verify_dag_proof_inner(&data, &[], &[], &[]);
    }

    // --------------------------------------------------------
    // Circuit description decoder tests
    // --------------------------------------------------------

    /// Helper: encode a U256 as 32 big-endian bytes and append.
    fn push_u256(buf: &mut Vec<u8>, val: U256) {
        buf.extend_from_slice(&val.to_be_bytes());
    }

    /// Helper: encode a usize as a U256 and append.
    fn push_usize(buf: &mut Vec<u8>, val: usize) {
        push_u256(buf, U256::from_u64(val as u64));
    }

    /// Build a minimal encoded circuit description (0 compute layers, 0 input layers,
    /// all arrays empty except offsets which need at least one sentinel).
    fn build_minimal_circuit_desc_bytes() -> Vec<u8> {
        let mut buf = Vec::new();

        push_usize(&mut buf, 0); // num_compute_layers
        push_usize(&mut buf, 0); // num_input_layers
        push_usize(&mut buf, 0); // layer_types (length 0)
        push_usize(&mut buf, 0); // num_sumcheck_rounds (length 0)
        push_usize(&mut buf, 1); // atom_offsets (length 1)
        push_usize(&mut buf, 0); //   atom_offsets[0] = 0
        push_usize(&mut buf, 0); // atom_target_layers (length 0)
        push_usize(&mut buf, 0); // atom_commit_idxs (length 0)
        push_usize(&mut buf, 1); // pt_offsets (length 1)
        push_usize(&mut buf, 0); //   pt_offsets[0] = 0
        push_usize(&mut buf, 0); // pt_data (length 0)
        push_usize(&mut buf, 0); // input_is_committed (length 0)
        push_usize(&mut buf, 1); // oracle_product_offsets (length 1)
        push_usize(&mut buf, 0); //   oracle_product_offsets[0] = 0
        push_usize(&mut buf, 0); // oracle_result_idxs (length 0)
        push_usize(&mut buf, 0); // oracle_expr_coeffs (length 0)

        buf
    }

    #[test]
    fn test_decode_circuit_description_minimal() {
        let data = build_minimal_circuit_desc_bytes();
        let desc = decode_circuit_description(&data);

        assert_eq!(desc.num_compute_layers, 0);
        assert_eq!(desc.num_input_layers, 0);
        assert!(desc.layer_types.is_empty());
        assert!(desc.num_sumcheck_rounds.is_empty());
        assert_eq!(desc.atom_offsets, vec![0]);
        assert!(desc.atom_target_layers.is_empty());
        assert!(desc.atom_commit_idxs.is_empty());
        assert_eq!(desc.pt_offsets, vec![0]);
        assert!(desc.pt_data.is_empty());
        assert!(desc.input_is_committed.is_empty());
        assert_eq!(desc.oracle_product_offsets, vec![0]);
        assert!(desc.oracle_result_idxs.is_empty());
        assert!(desc.oracle_expr_coeffs.is_empty());
    }

    #[test]
    fn test_decode_circuit_description_single_layer() {
        let mut buf = Vec::new();

        push_usize(&mut buf, 1); // num_compute_layers
        push_usize(&mut buf, 1); // num_input_layers

        // layer_types: [1] (multiply)
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 1);

        // num_sumcheck_rounds: [3]
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 3);

        // atom_offsets: [0, 1]
        push_usize(&mut buf, 2);
        push_usize(&mut buf, 0);
        push_usize(&mut buf, 1);

        // atom_target_layers: [1]
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 1);

        // atom_commit_idxs: [0]
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 0);

        // pt_offsets: [0, 2]
        push_usize(&mut buf, 2);
        push_usize(&mut buf, 0);
        push_usize(&mut buf, 2);

        // pt_data: [0, 1] (binding refs)
        push_usize(&mut buf, 2);
        push_usize(&mut buf, 0);
        push_usize(&mut buf, 1);

        // input_is_committed: [true]
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 1);

        // oracle_product_offsets: [0, 1]
        push_usize(&mut buf, 2);
        push_usize(&mut buf, 0);
        push_usize(&mut buf, 1);

        // oracle_result_idxs: [0]
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 0);

        // oracle_expr_coeffs: [1]
        push_usize(&mut buf, 1);
        push_u256(&mut buf, U256::ONE);

        let desc = decode_circuit_description(&buf);

        assert_eq!(desc.num_compute_layers, 1);
        assert_eq!(desc.num_input_layers, 1);
        assert_eq!(desc.layer_types, vec![1]);
        assert_eq!(desc.num_sumcheck_rounds, vec![3]);
        assert_eq!(desc.atom_offsets, vec![0, 1]);
        assert_eq!(desc.atom_target_layers, vec![1]);
        assert_eq!(desc.atom_commit_idxs, vec![0]);
        assert_eq!(desc.pt_offsets, vec![0, 2]);
        assert_eq!(desc.pt_data, vec![0, 1]);
        assert_eq!(desc.input_is_committed, vec![true]);
        assert_eq!(desc.oracle_product_offsets, vec![0, 1]);
        assert_eq!(desc.oracle_result_idxs, vec![0]);
        assert_eq!(desc.oracle_expr_coeffs, vec![U256::ONE]);
    }

    /// Build a minimal (empty) circuit description for panic-path tests.
    #[allow(dead_code)]
    fn dummy_circuit_desc() -> DAGCircuitDescription {
        DAGCircuitDescription {
            num_compute_layers: 0,
            num_input_layers: 0,
            layer_types: Vec::new(),
            num_sumcheck_rounds: Vec::new(),
            atom_offsets: vec![0],
            atom_target_layers: Vec::new(),
            atom_commit_idxs: Vec::new(),
            pt_offsets: vec![0],
            pt_data: Vec::new(),
            input_is_committed: Vec::new(),
            oracle_product_offsets: vec![0],
            oracle_result_idxs: Vec::new(),
            oracle_expr_coeffs: Vec::new(),
        }
    }

    // --------------------------------------------------------
    // Integration test: real proof fixture
    // --------------------------------------------------------

    /// Encode the `dag_circuit_description` JSON object into the flat binary format
    /// expected by `decode_circuit_description`.
    fn encode_circuit_desc_from_json(desc: &serde_json::Value) -> Vec<u8> {
        let mut buf = Vec::new();

        // Helper: push hex string as raw 32-byte value
        fn push_hex(buf: &mut Vec<u8>, hex_str: &str) {
            let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
            let decoded = hex::decode(stripped).expect("invalid hex in oracleExprCoeffs");
            assert_eq!(decoded.len(), 32, "oracleExprCoeff must be 32 bytes");
            buf.extend_from_slice(&decoded);
        }

        // num_compute_layers, num_input_layers
        push_usize(&mut buf, desc["numComputeLayers"].as_u64().unwrap() as usize);
        push_usize(&mut buf, desc["numInputLayers"].as_u64().unwrap() as usize);

        // Length-prefixed integer arrays
        for key in &[
            "layerTypes",
            "numSumcheckRounds",
            "atomOffsets",
            "atomTargetLayers",
            "atomCommitIdxs",
            "ptOffsets",
            "ptData",
        ] {
            let arr = desc[key].as_array().unwrap();
            push_usize(&mut buf, arr.len());
            for v in arr {
                push_usize(&mut buf, v.as_u64().unwrap() as usize);
            }
        }

        // inputIsCommitted (booleans → 0 or 1)
        let bools = desc["inputIsCommitted"].as_array().unwrap();
        push_usize(&mut buf, bools.len());
        for v in bools {
            push_usize(&mut buf, if v.as_bool().unwrap() { 1 } else { 0 });
        }

        // More integer arrays
        for key in &["oracleProductOffsets", "oracleResultIdxs"] {
            let arr = desc[key].as_array().unwrap();
            push_usize(&mut buf, arr.len());
            for v in arr {
                push_usize(&mut buf, v.as_u64().unwrap() as usize);
            }
        }

        // oracleExprCoeffs (hex strings → raw U256 bytes)
        let coeffs = desc["oracleExprCoeffs"].as_array().unwrap();
        push_usize(&mut buf, coeffs.len());
        for v in coeffs {
            push_hex(&mut buf, v.as_str().unwrap());
        }

        buf
    }

    /// End-to-end integration test using the real 88-layer XGBoost DAG proof fixture.
    /// This exercises the complete verification pipeline: proof decoding, transcript
    /// setup, compute layer verification, and input layer verification.
    #[test]
    #[ignore] // Takes ~30s due to EC scalar multiplications in pure Rust
    fn test_verify_dag_proof_e2e() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../contracts/test/fixtures/phase1a_dag_fixture.json"
        );
        let fixture_str = std::fs::read_to_string(fixture_path)
            .expect("failed to read phase1a_dag_fixture.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&fixture_str).expect("failed to parse fixture JSON");

        // 1. proof_data: direct hex decode (already has REM1 selector)
        let proof_hex = fixture["proof_hex"].as_str().unwrap();
        let proof_data = hex::decode(&proof_hex[2..]).expect("invalid proof_hex");
        assert_eq!(&proof_data[0..4], b"REM1", "fixture must start with REM1");

        // 2. public_inputs: unused by verifier, but decode anyway
        let pub_hex = fixture["public_inputs_hex"].as_str().unwrap();
        let public_inputs = hex::decode(&pub_hex[2..]).unwrap_or_default();

        // 3. gens_data: direct hex decode
        let gens_hex = fixture["gens_hex"].as_str().unwrap();
        let gens_data = hex::decode(&gens_hex[2..]).expect("invalid gens_hex");

        // 4. circuit_desc_data: encode from JSON structure
        let circuit_desc_data =
            encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);

        // Run the full verification pipeline
        let result =
            verify_dag_proof_inner(&proof_data, &public_inputs, &gens_data, &circuit_desc_data);
        assert!(result, "DAG proof verification should succeed");
    }
}
