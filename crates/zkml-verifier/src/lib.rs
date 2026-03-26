//! Standalone BN254 GKR DAG verifier for ZKML proof bundles.
//!
//! This crate implements a complete GKR (Goldwasser-Kalai-Rothblum) DAG verifier
//! for BN254 field arithmetic. It can verify proofs from the Remainder prover
//! without any blockchain dependencies.
//!
//! ## Quick start
//!
//! ```no_run
//! use zkml_verifier::{ProofBundle, verify};
//!
//! let bundle = ProofBundle::from_file("proof_bundle.json").unwrap();
//! let result = verify(&bundle).unwrap();
//! assert!(result.verified);
//! ```

// Module declarations
pub mod bundle;
pub mod decode;
pub mod ec;
pub mod error;
pub mod ffi;
pub mod field;
pub mod gkr;
pub mod hyrax;
pub mod poseidon;
pub mod sumcheck;
pub mod test_utils;
pub mod transcript;
pub mod types;
pub mod wasm;

// Re-exports
pub use bundle::{HybridVerificationResult, ProofBundle, VerificationResult};
pub use error::{Result, VerifyError};
pub use types::ProofMetadata;

// Imports
use crate::decode::ProofDecoder;
use crate::field::{Fq, U256};
use crate::gkr::DAGCircuitDescription;

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

fn read_usize_from(dec: &mut ProofDecoder) -> usize {
    let v = dec.read_u256_public();
    assert!(
        v.0[1] == 0 && v.0[2] == 0 && v.0[3] == 0,
        "value too large for usize"
    );
    v.0[0] as usize
}

fn read_u8_array(dec: &mut ProofDecoder) -> Vec<u8> {
    let len = read_usize_from(dec);
    (0..len)
        .map(|_| dec.read_u256_public().0[0] as u8)
        .collect()
}

fn read_usize_array(dec: &mut ProofDecoder) -> Vec<usize> {
    let len = read_usize_from(dec);
    (0..len).map(|_| read_usize_from(dec)).collect()
}

fn read_u64_array(dec: &mut ProofDecoder) -> Vec<u64> {
    let len = read_usize_from(dec);
    (0..len).map(|_| dec.read_u256_public().0[0]).collect()
}

fn read_bool_array(dec: &mut ProofDecoder) -> Vec<bool> {
    let len = read_usize_from(dec);
    (0..len)
        .map(|_| !dec.read_u256_public().is_zero())
        .collect()
}

fn read_u256_array(dec: &mut ProofDecoder) -> Vec<U256> {
    let len = read_usize_from(dec);
    (0..len).map(|_| dec.read_u256_public()).collect()
}

/// Verify a DAG GKR proof end-to-end.
pub fn verify_dag_proof_inner(
    proof_data: &[u8],
    _public_inputs: &[u8],
    gens_data: &[u8],
    circuit_desc_data: &[u8],
) -> bool {
    assert!(proof_data.len() >= 4, "proof data too short for selector");
    assert!(&proof_data[0..4] == b"REM1", "invalid selector");
    let proof_bytes = &proof_data[4..];
    assert!(
        proof_bytes.len() >= 32,
        "proof body too short for circuit hash"
    );
    let mut circuit_hash = [0u8; 32];
    circuit_hash.copy_from_slice(&proof_bytes[0..32]);
    let mut decoder = ProofDecoder::new(proof_bytes);
    let (gkr_proof, embedded_pub_inputs, dag_input_proofs, public_value_claims) =
        decoder.decode_proof_for_dag();
    let embedded_fqs: Vec<Fq> = embedded_pub_inputs.iter().map(|u| Fq(*u)).collect();
    let gens = ProofDecoder::decode_pedersen_gens(gens_data);
    let desc = decode_circuit_description(circuit_desc_data);
    let mut input_commit_coords: Vec<U256> = Vec::new();
    for dag_proof in &dag_input_proofs {
        for row in &dag_proof.commitment_rows {
            input_commit_coords.push(row.x);
            input_commit_coords.push(row.y);
        }
    }
    let mut sponge =
        transcript::setup_transcript(&circuit_hash, &embedded_fqs, &input_commit_coords);
    let ctx = gkr::verify_compute_layers(&gkr_proof, &desc, &gens, &mut sponge);
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

/// Hybrid DAG GKR verification.
pub fn verify_dag_proof_hybrid_inner(
    proof_data: &[u8],
    _public_inputs: &[u8],
    gens_data: &[u8],
    circuit_desc_data: &[u8],
) -> gkr::FrOutputCollector {
    assert!(proof_data.len() >= 4, "proof data too short for selector");
    assert!(&proof_data[0..4] == b"REM1", "invalid selector");
    let proof_bytes = &proof_data[4..];
    assert!(
        proof_bytes.len() >= 32,
        "proof body too short for circuit hash"
    );
    let mut circuit_hash = [0u8; 32];
    circuit_hash.copy_from_slice(&proof_bytes[0..32]);
    let mut decoder = ProofDecoder::new(proof_bytes);
    let (gkr_proof, embedded_pub_inputs, dag_input_proofs, public_value_claims) =
        decoder.decode_proof_for_dag();
    let embedded_fqs: Vec<Fq> = embedded_pub_inputs.iter().map(|u| Fq(*u)).collect();
    let gens = ProofDecoder::decode_pedersen_gens(gens_data);
    let desc = decode_circuit_description(circuit_desc_data);
    let mut input_commit_coords: Vec<U256> = Vec::new();
    for dag_proof in &dag_input_proofs {
        for row in &dag_proof.commitment_rows {
            input_commit_coords.push(row.x);
            input_commit_coords.push(row.y);
        }
    }
    let mut sponge =
        transcript::setup_transcript(&circuit_hash, &embedded_fqs, &input_commit_coords);
    let (ctx, compute_fr) =
        gkr::verify_compute_layers_hybrid(&gkr_proof, &desc, &gens, &mut sponge);
    let _input_fr = hyrax::verify_input_layers_hybrid(
        &gkr_proof,
        &desc,
        &gens,
        &ctx,
        &mut sponge,
        &embedded_pub_inputs,
        &dag_input_proofs,
        &public_value_claims,
    );
    compute_fr
}

/// Encode hybrid verification Fr outputs as a flat byte array.
pub fn encode_hybrid_fr_outputs(result: &HybridVerificationResult) -> Vec<u8> {
    let mut buf = Vec::new();
    let n = result.compute_fr.rlc_betas.len();
    buf.extend_from_slice(&U256::from_u64(n as u64).to_be_bytes());
    for b in &result.compute_fr.rlc_betas {
        buf.extend_from_slice(&b.to_be_bytes());
    }
    for z in &result.compute_fr.z_dot_j_stars {
        buf.extend_from_slice(&z.to_be_bytes());
    }
    buf.extend_from_slice(&U256::from_u64(result.input_fr.z_dot_rs.len() as u64).to_be_bytes());
    buf.extend_from_slice(
        &U256::from_u64(result.input_fr.l_tensor_flat.len() as u64).to_be_bytes(),
    );
    for v in &result.input_fr.l_tensor_flat {
        buf.extend_from_slice(&v.to_be_bytes());
    }
    for v in &result.input_fr.z_dot_rs {
        buf.extend_from_slice(&v.to_be_bytes());
    }
    buf.extend_from_slice(&U256::from_u64(result.input_fr.mle_evals.len() as u64).to_be_bytes());
    for v in &result.input_fr.mle_evals {
        buf.extend_from_slice(&v.to_be_bytes());
    }
    buf
}

// --------------------------------------------------------
// Safe (non-panicking) circuit description decoder
// --------------------------------------------------------

fn try_read_usize_from(dec: &mut ProofDecoder) -> Result<usize> {
    let v = dec.try_read_u256()?;
    if v.0[1] != 0 || v.0[2] != 0 || v.0[3] != 0 {
        return Err(VerifyError::DecodeError(
            "value too large for usize".to_string(),
        ));
    }
    Ok(v.0[0] as usize)
}

fn try_read_u8_array(dec: &mut ProofDecoder) -> Result<Vec<u8>> {
    let len = try_read_usize_from(dec)?;
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(dec.try_read_u256()?.0[0] as u8);
    }
    Ok(result)
}

fn try_read_usize_array(dec: &mut ProofDecoder) -> Result<Vec<usize>> {
    let len = try_read_usize_from(dec)?;
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(try_read_usize_from(dec)?);
    }
    Ok(result)
}

fn try_read_u64_array(dec: &mut ProofDecoder) -> Result<Vec<u64>> {
    let len = try_read_usize_from(dec)?;
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(dec.try_read_u256()?.0[0]);
    }
    Ok(result)
}

fn try_read_bool_array(dec: &mut ProofDecoder) -> Result<Vec<bool>> {
    let len = try_read_usize_from(dec)?;
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(!dec.try_read_u256()?.is_zero());
    }
    Ok(result)
}

fn try_read_u256_array(dec: &mut ProofDecoder) -> Result<Vec<U256>> {
    let len = try_read_usize_from(dec)?;
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(dec.try_read_u256()?);
    }
    Ok(result)
}

fn try_decode_circuit_description(data: &[u8]) -> Result<DAGCircuitDescription> {
    let mut dec = ProofDecoder::new(data);
    let num_compute_layers = try_read_usize_from(&mut dec)?;
    let num_input_layers = try_read_usize_from(&mut dec)?;
    let layer_types = try_read_u8_array(&mut dec)?;
    let num_sumcheck_rounds = try_read_usize_array(&mut dec)?;
    let atom_offsets = try_read_usize_array(&mut dec)?;
    let atom_target_layers = try_read_usize_array(&mut dec)?;
    let atom_commit_idxs = try_read_usize_array(&mut dec)?;
    let pt_offsets = try_read_usize_array(&mut dec)?;
    let pt_data = try_read_u64_array(&mut dec)?;
    let input_is_committed = try_read_bool_array(&mut dec)?;
    let oracle_product_offsets = try_read_usize_array(&mut dec)?;
    let oracle_result_idxs = try_read_usize_array(&mut dec)?;
    let oracle_expr_coeffs = try_read_u256_array(&mut dec)?;
    Ok(DAGCircuitDescription {
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
    })
}

/// Verify a DAG GKR proof end-to-end, returning Result instead of panicking.
///
/// This is the safe version of `verify_dag_proof_inner` that propagates all
/// errors through Result instead of using assert/panic. It is suitable for
/// use in contexts where panic=abort (e.g., fuzz testing, WASM).
fn try_verify_dag_proof_inner(
    proof_data: &[u8],
    _public_inputs: &[u8],
    gens_data: &[u8],
    circuit_desc_data: &[u8],
) -> Result<bool> {
    if proof_data.len() < 4 {
        return Err(VerifyError::InvalidFormat(
            "proof data too short for selector".to_string(),
        ));
    }
    if &proof_data[0..4] != b"REM1" {
        return Err(VerifyError::InvalidFormat(
            "invalid selector (expected REM1)".to_string(),
        ));
    }
    let proof_bytes = &proof_data[4..];
    if proof_bytes.len() < 32 {
        return Err(VerifyError::InvalidFormat(
            "proof body too short for circuit hash".to_string(),
        ));
    }
    let mut circuit_hash = [0u8; 32];
    circuit_hash.copy_from_slice(&proof_bytes[0..32]);

    let mut decoder = ProofDecoder::new(proof_bytes);
    let (gkr_proof, embedded_pub_inputs, dag_input_proofs, public_value_claims) =
        decoder.try_decode_proof_for_dag()?;

    let embedded_fqs: Vec<Fq> = embedded_pub_inputs.iter().map(|u| Fq(*u)).collect();
    let gens = ProofDecoder::try_decode_pedersen_gens(gens_data)?;
    let desc = try_decode_circuit_description(circuit_desc_data)?;

    let mut input_commit_coords: Vec<U256> = Vec::new();
    for dag_proof in &dag_input_proofs {
        for row in &dag_proof.commitment_rows {
            input_commit_coords.push(row.x);
            input_commit_coords.push(row.y);
        }
    }

    // The GKR compute/input layer verification still uses assert internally,
    // so we wrap it in catch_unwind as a defense-in-depth measure.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut sponge =
            transcript::setup_transcript(&circuit_hash, &embedded_fqs, &input_commit_coords);
        let ctx = gkr::verify_compute_layers(&gkr_proof, &desc, &gens, &mut sponge);
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
    }));

    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown verification panic".to_string()
            };
            Err(VerifyError::VerificationFailed(msg))
        }
    }
}

/// Verify a GKR DAG proof from raw byte slices.
///
/// This is the low-level verification entry point. For a higher-level API that
/// accepts JSON proof bundles, see [`verify`].
///
/// # Arguments
/// * `proof_data` - Proof blob with "REM1" 4-byte selector prefix
/// * `generators` - Serialized Pedersen generators
/// * `circuit_desc` - ABI-encoded DAG circuit description
///
/// # Returns
/// `Ok(VerificationResult)` on success, `Err(VerifyError)` on failure.
pub fn verify_raw(
    proof_data: &[u8],
    generators: &[u8],
    circuit_desc: &[u8],
) -> Result<VerificationResult> {
    if proof_data.len() < 36 {
        return Err(VerifyError::InvalidFormat(
            "proof data too short (need at least 36 bytes)".to_string(),
        ));
    }
    if &proof_data[0..4] != b"REM1" {
        return Err(VerifyError::InvalidFormat(
            "invalid selector (expected REM1)".to_string(),
        ));
    }
    let mut circuit_hash = [0u8; 32];
    circuit_hash.copy_from_slice(&proof_data[4..36]);

    // Use the safe (non-panicking) decode + verify path.
    // This properly returns Err on malformed input without relying on
    // catch_unwind (which doesn't work under panic=abort).
    match try_verify_dag_proof_inner(proof_data, &[], generators, circuit_desc) {
        Ok(true) => Ok(VerificationResult {
            circuit_hash,
            verified: true,
        }),
        Ok(false) => Ok(VerificationResult {
            circuit_hash,
            verified: false,
        }),
        Err(e) => Err(e),
    }
}

/// Verify a proof bundle (full verification with EC checks).
pub fn verify(bundle: &ProofBundle) -> Result<VerificationResult> {
    let proof_data = bundle.proof_data()?;
    if proof_data.len() < 36 {
        return Err(VerifyError::InvalidProof("proof too short".to_string()));
    }
    if &proof_data[0..4] != b"REM1" {
        return Err(VerifyError::InvalidProof(
            "invalid selector (expected REM1)".to_string(),
        ));
    }
    let mut circuit_hash = [0u8; 32];
    circuit_hash.copy_from_slice(&proof_data[4..36]);
    let public_inputs = bundle.public_inputs_data()?;
    let gens_data = bundle.gens_data()?;
    let circuit_desc_data =
        test_utils::encode_circuit_desc_from_json(&bundle.dag_circuit_description);

    // Use the safe (non-panicking) decode + verify path.
    match try_verify_dag_proof_inner(&proof_data, &public_inputs, &gens_data, &circuit_desc_data) {
        Ok(true) => Ok(VerificationResult {
            circuit_hash,
            verified: true,
        }),
        Ok(false) => Ok(VerificationResult {
            circuit_hash,
            verified: false,
        }),
        Err(e) => Err(e),
    }
}

/// Verify a proof bundle in hybrid mode.
pub fn verify_hybrid(bundle: &ProofBundle) -> Result<HybridVerificationResult> {
    let proof_data = bundle.proof_data()?;
    if proof_data.len() < 36 {
        return Err(VerifyError::InvalidProof("proof too short".to_string()));
    }
    if &proof_data[0..4] != b"REM1" {
        return Err(VerifyError::InvalidProof(
            "invalid selector (expected REM1)".to_string(),
        ));
    }
    let _public_inputs = bundle.public_inputs_data()?;
    let gens_data = bundle.gens_data()?;
    let circuit_desc_data =
        test_utils::encode_circuit_desc_from_json(&bundle.dag_circuit_description);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof_bytes = &proof_data[4..];
        let mut ch = [0u8; 32];
        ch.copy_from_slice(&proof_bytes[0..32]);
        let mut decoder = ProofDecoder::new(proof_bytes);
        let (gkr_proof, embedded_pub_inputs, dag_input_proofs, public_value_claims) =
            decoder.decode_proof_for_dag();
        let embedded_fqs: Vec<Fq> = embedded_pub_inputs.iter().map(|u| Fq(*u)).collect();
        let gens = ProofDecoder::decode_pedersen_gens(&gens_data);
        let desc = decode_circuit_description(&circuit_desc_data);
        let mut icc: Vec<U256> = Vec::new();
        for dp in &dag_input_proofs {
            for row in &dp.commitment_rows {
                icc.push(row.x);
                icc.push(row.y);
            }
        }
        let mut sponge = transcript::setup_transcript(&ch, &embedded_fqs, &icc);
        let (ctx, compute_fr) =
            gkr::verify_compute_layers_hybrid(&gkr_proof, &desc, &gens, &mut sponge);
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
        let digest_fq = sponge.squeeze();
        let mut td = [0u8; 32];
        td.copy_from_slice(&digest_fq.0.to_be_bytes());
        (ch, td, compute_fr, input_fr)
    }));
    match result {
        Ok((ch, td, compute_fr, input_fr)) => Ok(HybridVerificationResult {
            circuit_hash: ch,
            transcript_digest: td,
            compute_fr,
            input_fr,
        }),
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown verification panic".to_string()
            };
            Err(VerifyError::VerificationFailed(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{encode_circuit_desc_from_json, push_usize};

    #[test]
    #[should_panic(expected = "proof data too short for selector")]
    fn test_verify_rejects_short_proof() {
        let _ = verify_dag_proof_inner(&[0u8; 3], &[], &[], &[]);
    }

    #[test]
    #[should_panic(expected = "invalid selector")]
    fn test_verify_rejects_wrong_selector() {
        let mut data = vec![0u8; 68];
        data[0..4].copy_from_slice(b"BAAD");
        let _ = verify_dag_proof_inner(&data, &[], &[], &[]);
    }

    #[test]
    #[should_panic(expected = "proof body too short for circuit hash")]
    fn test_verify_rejects_no_circuit_hash() {
        let mut data = vec![0u8; 20];
        data[0..4].copy_from_slice(b"REM1");
        let _ = verify_dag_proof_inner(&data, &[], &[], &[]);
    }

    fn build_minimal_circuit_desc_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        for _ in 0..4 {
            push_usize(&mut buf, 0);
        }
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 0);
        for _ in 0..3 {
            push_usize(&mut buf, 0);
        }
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 0);
        for _ in 0..2 {
            push_usize(&mut buf, 0);
        }
        push_usize(&mut buf, 1);
        push_usize(&mut buf, 0);
        for _ in 0..2 {
            push_usize(&mut buf, 0);
        }
        buf
    }

    #[test]
    fn test_decode_circuit_description_minimal() {
        let data = build_minimal_circuit_desc_bytes();
        let desc = decode_circuit_description(&data);
        assert_eq!(desc.num_compute_layers, 0);
        assert_eq!(desc.num_input_layers, 0);
    }

    #[test]
    fn test_verify_api_rejects_short_proof() {
        let bundle = ProofBundle {
            proof_hex: "0x0102".to_string(),
            public_inputs_hex: String::new(),
            gens_hex: "0x00".to_string(),
            dag_circuit_description: serde_json::json!({}),
            model_hash: None,
            timestamp: None,
            prover_version: None,
            circuit_hash: None,
        };
        let result = verify(&bundle);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VerifyError::InvalidProof(_)));
    }

    #[test]
    fn test_proof_bundle_deserialize() {
        let json = r#"{"proof_hex":"0xdeadbeef","gens_hex":"0x00","dag_circuit_description":{}}"#;
        let bundle: ProofBundle = serde_json::from_str(json).unwrap();
        assert_eq!(bundle.proof_hex, "0xdeadbeef");
        assert!(bundle.public_inputs_hex.is_empty());
    }

    #[test]
    fn test_error_api() {
        assert_eq!(
            format!("{}", VerifyError::InvalidProof("test".into())),
            "invalid proof: test"
        );
        assert_eq!(
            format!("{}", VerifyError::InvalidFormat("fmt".into())),
            "invalid proof format: fmt"
        );
        assert_eq!(
            format!("{}", VerifyError::BundleParse("bad".into())),
            "bundle parse error: bad"
        );
        assert_eq!(
            format!("{}", VerifyError::VerificationFailed("x".into())),
            "verification failed: x"
        );
        assert_eq!(
            format!("{}", VerifyError::DecodeError("dec".into())),
            "decode error: dec"
        );
    }

    #[test]
    fn test_verify_raw_rejects_short_proof() {
        let result = verify_raw(&[0u8; 3], &[], &[]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VerifyError::InvalidFormat(_)));
    }

    #[test]
    fn test_verify_raw_rejects_wrong_selector() {
        let mut data = vec![0u8; 68];
        data[0..4].copy_from_slice(b"BAAD");
        let result = verify_raw(&data, &[], &[]);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VerifyError::InvalidFormat(_)));
    }

    #[test]
    fn test_proof_metadata_default() {
        let meta = ProofMetadata::default();
        assert!(meta.model_hash.is_empty());
        assert_eq!(meta.timestamp, 0);
        assert!(meta.prover_version.is_empty());
    }

    #[test]
    #[should_panic(expected = "invalid selector")]
    fn test_hybrid_rejects_wrong_selector() {
        let mut data = vec![0u8; 68];
        data[0..4].copy_from_slice(b"BAAD");
        let _ = verify_dag_proof_hybrid_inner(&data, &[], &[], &[]);
    }

    #[test]
    #[should_panic(expected = "proof data too short for selector")]
    fn test_hybrid_rejects_short_proof() {
        let _ = verify_dag_proof_hybrid_inner(&[0u8; 3], &[], &[], &[]);
    }

    #[test]
    #[ignore]
    fn test_verify_dag_proof_e2e() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contracts/test/fixtures/phase1a_dag_fixture.json"
        );
        let fixture_str = std::fs::read_to_string(fixture_path).expect("failed to read fixture");
        let fixture: serde_json::Value = serde_json::from_str(&fixture_str).unwrap();
        let proof_data = hex::decode(&fixture["proof_hex"].as_str().unwrap()[2..]).unwrap();
        let public_inputs =
            hex::decode(&fixture["public_inputs_hex"].as_str().unwrap()[2..]).unwrap_or_default();
        let gens_data = hex::decode(&fixture["gens_hex"].as_str().unwrap()[2..]).unwrap();
        let circuit_desc_data = encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);
        assert!(verify_dag_proof_inner(
            &proof_data,
            &public_inputs,
            &gens_data,
            &circuit_desc_data
        ));
    }
}
