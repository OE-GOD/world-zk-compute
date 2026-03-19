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

/// Assertion macro that strips error message strings from WASM builds.
/// On native, behaves like `assert!` with the message; on WASM, uses
/// `core::arch::wasm32::unreachable()` to trap without embedding string data.
///
/// # Why `unreachable()` instead of `panic!()`
///
/// `core::arch::wasm32::unreachable()` emits the WASM `unreachable` trap
/// instruction, which unconditionally aborts execution. Using it here is
/// sound because:
///   1. It is only reached when `$cond` is false, i.e., a verification
///      invariant has been violated and the program must abort.
///   2. In the Stylus WASM runtime, trapping is the correct way to revert a
///      transaction -- it is equivalent to a Solidity `revert()`.
///   3. No memory or state is accessed after the trap; the instruction has
///      return type `!` (never), so no subsequent code can observe invalid state.
///   4. This is preferred over `panic!()` because it avoids embedding format
///      strings in the WASM binary, keeping the binary under Stylus's 24KB
///      Brotli-compressed size limit.
#[cfg(target_arch = "wasm32")]
macro_rules! verify {
    ($cond:expr $(, $msg:expr)* $(,)?) => {
        if !$cond {
            // SAFETY: We are aborting because a verification check failed.
            // The WASM `unreachable` trap is the correct abort mechanism in
            // the Stylus runtime. No code executes after this point.
            unsafe { core::arch::wasm32::unreachable() }
        }
    };
}
#[cfg(not(target_arch = "wasm32"))]
macro_rules! verify {
    ($cond:expr, $msg:expr $(,)?) => {
        assert!($cond, "{}", $msg);
    };
    ($cond:expr $(,)?) => {
        assert!($cond);
    };
}

// ------------------------------------------------------------------
// Module declarations
// ------------------------------------------------------------------

pub mod decode;
pub mod ec;
pub mod field;
pub mod gkr;
pub mod hyrax;
pub mod poseidon;
pub mod sumcheck;
#[cfg(not(target_arch = "wasm32"))]
pub mod test_utils;
pub mod transcript;

// ------------------------------------------------------------------
// Imports
// ------------------------------------------------------------------

#[allow(unused_imports)]
use alloc::vec;
use alloc::vec::Vec;

use crate::decode::ProofDecoder;
use crate::field::{Fq, U256};
use crate::gkr::DAGCircuitDescription;
#[cfg(any(feature = "hybrid", not(target_arch = "wasm32")))]
use crate::gkr::FrOutputCollector;
#[cfg(any(feature = "hybrid", not(target_arch = "wasm32")))]
use crate::hyrax::InputFrOutputs;

// ------------------------------------------------------------------
// Stylus SDK imports
// ------------------------------------------------------------------

#[cfg(feature = "stylus")]
use stylus_sdk::prelude::*;

#[cfg(feature = "stylus")]
use stylus_sdk::abi::Bytes;

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
    verify!(
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
    verify!(proof_data.len() >= 4, "proof data too short for selector");
    verify!(&proof_data[0..4] == b"REM1", "invalid selector");
    let proof_bytes = &proof_data[4..];

    // ----------------------------------------------------------------
    // 2. Extract circuit hash (first 32 bytes of proof body)
    // ----------------------------------------------------------------
    verify!(
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
    let mut sponge =
        transcript::setup_transcript(&circuit_hash, &embedded_fqs, &input_commit_coords);

    // ----------------------------------------------------------------
    // 9. Verify compute layers
    // ----------------------------------------------------------------
    let ctx = gkr::verify_compute_layers(&gkr_proof, &desc, &gens, &mut sponge);

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
// Hybrid verification result
// ------------------------------------------------------------------

/// Result of hybrid verification: transcript digest + Fr-domain outputs.
///
/// The `transcript_digest` binds the sponge state after all transcript
/// operations. A Groth16 proof must attest that the same sponge state
/// would be produced by a full verifier, using the Fr outputs below.
#[cfg(any(feature = "hybrid", not(target_arch = "wasm32")))]
pub struct HybridVerificationResult {
    /// Circuit hash (first 32 bytes of proof body).
    pub circuit_hash: [u8; 32],
    /// Sponge state digest: one final squeeze after all hybrid operations.
    pub transcript_digest: [u8; 32],
    /// Per compute layer (0..N): rlc_beta and z_dot_j_star scalars.
    pub compute_fr: FrOutputCollector,
    /// Input layer Fr outputs: l_tensor_flat, z_dot_rs, mle_evals.
    pub input_fr: InputFrOutputs,
}

// ------------------------------------------------------------------
// Hybrid verification logic
// ------------------------------------------------------------------

/// Hybrid DAG GKR verification: transcript replay + Fr arithmetic, no EC ops.
///
/// Performs the same transcript operations as `verify_dag_proof_inner` (every
/// absorb/squeeze in the same order with the same values), but **skips all
/// EC operations**: ec_mul, ec_add, MSM, PODP equation checks, PoP checks,
/// Pedersen opening checks.
///
/// Instead of returning `bool`, returns a `HybridVerificationResult` containing:
/// - `circuit_hash`: the 32-byte circuit hash from the proof header
/// - `transcript_digest`: a final squeeze from the sponge (binding the state)
/// - `compute_fr`: per-layer rlc_beta and z_dot_j_star scalars
/// - `input_fr`: per-group l_tensor, z_dot_r, and per-claim mle_eval
///
/// These Fr outputs are used by a Groth16 proof to attest that the full
/// EC operations would have passed, without actually performing them on-chain.
///
/// # Panics
///
/// Panics on structural proof errors (wrong selector, truncated data, etc.)
/// but does NOT panic on EC equation failures (since those are not checked).
#[cfg(any(feature = "hybrid", not(target_arch = "wasm32")))]
pub fn verify_dag_proof_hybrid_inner(
    proof_data: &[u8],
    _public_inputs: &[u8],
    gens_data: &[u8],
    circuit_desc_data: &[u8],
) -> HybridVerificationResult {
    // ----------------------------------------------------------------
    // 1. Strip "REM1" 4-byte selector
    // ----------------------------------------------------------------
    verify!(proof_data.len() >= 4, "proof data too short for selector");
    verify!(&proof_data[0..4] == b"REM1", "invalid selector");
    let proof_bytes = &proof_data[4..];

    // ----------------------------------------------------------------
    // 2. Extract circuit hash (first 32 bytes of proof body)
    // ----------------------------------------------------------------
    verify!(
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
    // 5. Decode Pedersen generators (needed for signature but not for EC)
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
    // 8. Setup Fiat-Shamir transcript (IDENTICAL to full verifier)
    // ----------------------------------------------------------------
    let mut sponge =
        transcript::setup_transcript(&circuit_hash, &embedded_fqs, &input_commit_coords);

    // ----------------------------------------------------------------
    // 9. Hybrid compute layers: transcript replay + Fr outputs
    // ----------------------------------------------------------------
    let (ctx, compute_fr) =
        gkr::verify_compute_layers_hybrid(&gkr_proof, &desc, &gens, &mut sponge);

    // ----------------------------------------------------------------
    // 10. Hybrid input layers: transcript replay + Fr outputs
    // ----------------------------------------------------------------
    let input_fr = hyrax::verify_input_layers_hybrid(
        &gkr_proof,
        &desc,
        &gens,
        &ctx,
        &mut sponge,
        &embedded_pub_inputs,
        &dag_input_proofs,
        &public_value_claims,
    );

    // ----------------------------------------------------------------
    // 11. Final transcript digest (one squeeze to bind sponge state)
    // ----------------------------------------------------------------
    let digest_fq = sponge.squeeze();
    let mut transcript_digest = [0u8; 32];
    transcript_digest.copy_from_slice(&digest_fq.0.to_be_bytes());

    HybridVerificationResult {
        circuit_hash,
        transcript_digest,
        compute_fr,
        input_fr,
    }
}

/// Encode hybrid verification Fr outputs as a flat byte array.
///
/// Layout (all values as 32-byte big-endian U256):
///   1. num_compute_layers (U256)
///   2. Per compute layer (0..N): rlc_beta
///   3. Per compute layer (0..N): z_dot_j_star
///   4. num_eval_groups (U256)
///   5. Per eval group: l_tensor_flat length (U256), then l_tensor_flat entries
///   6. Per eval group: z_dot_r
///   7. num_mle_evals (U256)
///   8. Per public claim: mle_eval
///
/// This format is self-describing (length-prefixed) so a Groth16 verifier
/// can decode it without external metadata.
#[cfg(any(feature = "hybrid", not(target_arch = "wasm32")))]
pub fn encode_hybrid_fr_outputs(result: &HybridVerificationResult) -> Vec<u8> {
    let num_layers = result.compute_fr.rlc_betas.len();
    let num_groups = result.input_fr.z_dot_rs.len();
    let num_mle = result.input_fr.mle_evals.len();

    // Estimate capacity
    let capacity = 32
        * (1 + num_layers * 2
            + 1
            + num_groups
            + 1
            + num_mle
            + result.input_fr.l_tensor_flat.len());
    let mut buf = Vec::with_capacity(capacity);

    // Compute layer outputs
    push_u256_to_buf(&mut buf, &U256::from_u64(num_layers as u64));
    for beta in &result.compute_fr.rlc_betas {
        push_u256_to_buf(&mut buf, beta);
    }
    for z_dot in &result.compute_fr.z_dot_j_stars {
        push_u256_to_buf(&mut buf, z_dot);
    }

    // Input layer outputs -- eval groups
    push_u256_to_buf(&mut buf, &U256::from_u64(num_groups as u64));
    // l_tensor_flat entries (all flattened, groups are contiguous)
    push_u256_to_buf(
        &mut buf,
        &U256::from_u64(result.input_fr.l_tensor_flat.len() as u64),
    );
    for val in &result.input_fr.l_tensor_flat {
        push_u256_to_buf(&mut buf, val);
    }
    for z_dot_r in &result.input_fr.z_dot_rs {
        push_u256_to_buf(&mut buf, z_dot_r);
    }

    // MLE evals
    push_u256_to_buf(&mut buf, &U256::from_u64(num_mle as u64));
    for mle in &result.input_fr.mle_evals {
        push_u256_to_buf(&mut buf, mle);
    }

    buf
}

#[cfg(any(feature = "hybrid", not(target_arch = "wasm32")))]
fn push_u256_to_buf(buf: &mut Vec<u8>, val: &U256) {
    buf.extend_from_slice(&val.to_be_bytes());
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
        proof_data: Bytes,
        public_inputs: Bytes,
        gens_data: Bytes,
        circuit_desc_data: Bytes,
    ) -> bool {
        verify_dag_proof_inner(&proof_data, &public_inputs, &gens_data, &circuit_desc_data)
    }
}

#[cfg(all(feature = "stylus", feature = "hybrid"))]
#[public]
impl GKRVerifier {
    /// Hybrid DAG GKR verification: transcript replay + Fr outputs, no EC ops.
    ///
    /// Returns `(success, transcript_digest, encoded_fr_outputs)`:
    /// - `success`: always true if the function returns (panics on error)
    /// - `transcript_digest`: 32-byte Poseidon sponge digest binding the transcript
    /// - `encoded_fr_outputs`: ABI-encoded Fr scalar outputs for Groth16 verification
    pub fn verify_dag_proof_hybrid(
        &self,
        proof_data: Bytes,
        public_inputs: Bytes,
        gens_data: Bytes,
        circuit_desc_data: Bytes,
    ) -> (bool, stylus_sdk::alloy_primitives::FixedBytes<32>, Bytes) {
        let result = verify_dag_proof_hybrid_inner(
            &proof_data,
            &public_inputs,
            &gens_data,
            &circuit_desc_data,
        );
        let digest =
            stylus_sdk::alloy_primitives::FixedBytes::<32>::from(result.transcript_digest);
        let encoded = encode_hybrid_fr_outputs(&result);
        (true, digest, Bytes(encoded))
    }
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::U256;
    use crate::gkr::DAGCircuitDescription;
    use alloc::vec;

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

    use crate::test_utils::{encode_circuit_desc_from_json, push_u256, push_usize};

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

    /// End-to-end integration test using the real 88-layer XGBoost DAG proof fixture.
    /// This exercises the complete verification pipeline: proof decoding, transcript
    /// setup, compute layer verification, and input layer verification.
    #[test]
    #[ignore] // Takes ~10s in release mode due to EC scalar multiplications
    fn test_verify_dag_proof_e2e() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../contracts/test/fixtures/phase1a_dag_fixture.json"
        );
        let fixture_str =
            std::fs::read_to_string(fixture_path).expect("failed to read phase1a_dag_fixture.json");
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
        let circuit_desc_data = encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);

        // Run the full verification pipeline
        let result =
            verify_dag_proof_inner(&proof_data, &public_inputs, &gens_data, &circuit_desc_data);
        assert!(result, "DAG proof verification should succeed");
    }

    // --------------------------------------------------------
    // Hybrid verification tests
    // --------------------------------------------------------

    /// Helper: load the fixture and return (proof_data, public_inputs, gens_data, circuit_desc_data)
    fn load_fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../contracts/test/fixtures/phase1a_dag_fixture.json"
        );
        let fixture_str =
            std::fs::read_to_string(fixture_path).expect("failed to read phase1a_dag_fixture.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&fixture_str).expect("failed to parse fixture JSON");

        let proof_hex = fixture["proof_hex"].as_str().unwrap();
        let proof_data = hex::decode(&proof_hex[2..]).expect("invalid proof_hex");

        let pub_hex = fixture["public_inputs_hex"].as_str().unwrap();
        let public_inputs = hex::decode(&pub_hex[2..]).unwrap_or_default();

        let gens_hex = fixture["gens_hex"].as_str().unwrap();
        let gens_data = hex::decode(&gens_hex[2..]).expect("invalid gens_hex");

        let circuit_desc_data = encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);

        (proof_data, public_inputs, gens_data, circuit_desc_data)
    }

    /// Hybrid verification returns structured Fr outputs without panicking.
    /// This test runs the hybrid verifier on the real 88-layer fixture and
    /// checks that the output structure is non-trivial.
    #[test]
    #[ignore] // Requires fixture file; faster than full E2E (no EC ops)
    fn test_hybrid_verification_returns_outputs() {
        let (proof_data, public_inputs, gens_data, circuit_desc_data) = load_fixture();

        let result = verify_dag_proof_hybrid_inner(
            &proof_data,
            &public_inputs,
            &gens_data,
            &circuit_desc_data,
        );

        // Circuit hash should be non-zero
        assert!(
            result.circuit_hash.iter().any(|&b| b != 0),
            "circuit_hash should be non-zero"
        );

        // Transcript digest should be non-zero
        assert!(
            result.transcript_digest.iter().any(|&b| b != 0),
            "transcript_digest should be non-zero"
        );

        // 88 compute layers
        assert_eq!(
            result.compute_fr.rlc_betas.len(),
            88,
            "expected 88 rlc_betas for XGBoost circuit"
        );
        assert_eq!(
            result.compute_fr.z_dot_j_stars.len(),
            88,
            "expected 88 z_dot_j_stars for XGBoost circuit"
        );

        // Input layer outputs should be non-empty
        assert!(
            !result.input_fr.l_tensor_flat.is_empty(),
            "l_tensor_flat should not be empty"
        );
        assert!(
            !result.input_fr.z_dot_rs.is_empty(),
            "z_dot_rs should not be empty"
        );
        assert!(
            !result.input_fr.mle_evals.is_empty(),
            "mle_evals should not be empty"
        );

        // Verify encoding round-trips without error
        let encoded = encode_hybrid_fr_outputs(&result);
        assert!(
            encoded.len() > 32 * 88 * 2,
            "encoded output should be substantial"
        );
    }

    /// Verify that hybrid and full verifiers produce identical transcript states.
    ///
    /// Both paths initialize the sponge identically and perform the same absorb/squeeze
    /// operations. After all compute + input layer processing, we squeeze one more
    /// element from each sponge and compare. If they match, the hybrid verifier
    /// faithfully replayed the Fiat-Shamir transcript.
    #[test]
    #[ignore] // Requires fixture; full verifier is slow (~10s release)
    fn test_hybrid_transcript_matches_full() {
        let (proof_data, public_inputs, gens_data, circuit_desc_data) = load_fixture();

        // Run hybrid verification -- get the sponge digest
        let hybrid_result = verify_dag_proof_hybrid_inner(
            &proof_data,
            &public_inputs,
            &gens_data,
            &circuit_desc_data,
        );

        // Run the full verification pipeline manually to get the sponge state
        let proof_bytes = &proof_data[4..];
        let mut circuit_hash = [0u8; 32];
        circuit_hash.copy_from_slice(&proof_bytes[0..32]);

        let mut decoder = ProofDecoder::new(proof_bytes);
        let (gkr_proof, embedded_pub_inputs, dag_input_proofs, public_value_claims) =
            decoder.decode_proof_for_dag();
        let embedded_fqs: Vec<Fq> = embedded_pub_inputs.iter().map(|u| Fq(*u)).collect();
        let gens = ProofDecoder::decode_pedersen_gens(&gens_data);
        let desc = decode_circuit_description(&circuit_desc_data);

        let mut input_commit_coords: Vec<U256> = Vec::new();
        for dag_proof in &dag_input_proofs {
            for row in &dag_proof.commitment_rows {
                input_commit_coords.push(row.x);
                input_commit_coords.push(row.y);
            }
        }

        let mut sponge =
            transcript::setup_transcript(&circuit_hash, &embedded_fqs, &input_commit_coords);

        // Full compute layers
        let ctx = gkr::verify_compute_layers(&gkr_proof, &desc, &gens, &mut sponge);

        // Full input layers
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

        // Final squeeze from full sponge
        let full_digest_fq = sponge.squeeze();
        let mut full_digest = [0u8; 32];
        full_digest.copy_from_slice(&full_digest_fq.0.to_be_bytes());

        // Compare: hybrid and full transcript digests must be identical
        assert_eq!(
            hybrid_result.transcript_digest, full_digest,
            "hybrid and full verification transcript digests must match"
        );
    }

    /// Hybrid verification rejects wrong selector just like the full verifier.
    #[test]
    #[should_panic(expected = "invalid selector")]
    fn test_hybrid_rejects_wrong_selector() {
        let mut data = vec![0u8; 68];
        data[0..4].copy_from_slice(b"BAAD");
        let _ = verify_dag_proof_hybrid_inner(&data, &[], &[], &[]);
    }

    /// Hybrid verification rejects short proof data.
    #[test]
    #[should_panic(expected = "proof data too short for selector")]
    fn test_hybrid_rejects_short_proof() {
        let short = [0u8; 3];
        let _ = verify_dag_proof_hybrid_inner(&short, &[], &[], &[]);
    }
}
