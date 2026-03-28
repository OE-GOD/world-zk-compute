//! Proof comparison -- verify two proofs produce the same output.
//!
//! Banks need to validate that a new model version gives the same outputs as
//! the old one. This module compares two stored proofs and reports whether
//! their model hashes, circuit hashes, and output data match.

use serde::Serialize;

use crate::db::StoredProof;
use zkml_verifier::ProofBundle;

/// Result of comparing two proofs.
#[derive(Debug, Clone, Serialize)]
pub struct ComparisonResult {
    /// ID of the first proof.
    pub proof_a_id: String,
    /// ID of the second proof.
    pub proof_b_id: String,
    /// Whether the proof outputs (public inputs) match exactly.
    pub outputs_match: bool,
    /// Model hash of proof A.
    pub model_a_hash: String,
    /// Model hash of proof B.
    pub model_b_hash: String,
    /// Circuit hash of proof A.
    pub circuit_a_hash: String,
    /// Circuit hash of proof B.
    pub circuit_b_hash: String,
    /// Whether both proofs use the same model.
    pub same_model: bool,
    /// Whether both proofs use the same circuit.
    pub same_circuit: bool,
    /// Human-readable details about the comparison.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Compare two stored proofs by their metadata and, optionally, their output data.
///
/// When `bundle_a` and `bundle_b` are provided, the comparison also checks whether
/// the proof output data (`public_inputs_hex`) matches. Without bundles, only
/// metadata (model hash, circuit hash) is compared and `outputs_match` defaults
/// to `false` with an explanatory detail message.
pub fn compare_proofs(
    proof_a: &StoredProof,
    proof_b: &StoredProof,
    bundle_a: Option<&ProofBundle>,
    bundle_b: Option<&ProofBundle>,
) -> ComparisonResult {
    let same_model = !proof_a.model_hash.is_empty()
        && !proof_b.model_hash.is_empty()
        && proof_a.model_hash == proof_b.model_hash;

    let same_circuit = !proof_a.circuit_hash.is_empty()
        && !proof_b.circuit_hash.is_empty()
        && proof_a.circuit_hash == proof_b.circuit_hash;

    let (outputs_match, details) = match (bundle_a, bundle_b) {
        (Some(ba), Some(bb)) => compare_outputs(ba, bb),
        _ => (
            false,
            Some("bundles not available for output comparison".to_string()),
        ),
    };

    let mut detail_parts: Vec<String> = Vec::new();

    if same_model {
        detail_parts.push("same model".to_string());
    } else {
        detail_parts.push(format!(
            "different models: {} vs {}",
            proof_a.model_hash, proof_b.model_hash
        ));
    }

    if same_circuit {
        detail_parts.push("same circuit".to_string());
    } else {
        detail_parts.push(format!(
            "different circuits: {} vs {}",
            proof_a.circuit_hash, proof_b.circuit_hash
        ));
    }

    if let Some(d) = &details {
        detail_parts.push(d.clone());
    } else if outputs_match {
        detail_parts.push("outputs match".to_string());
    } else {
        detail_parts.push("outputs differ".to_string());
    }

    ComparisonResult {
        proof_a_id: proof_a.id.clone(),
        proof_b_id: proof_b.id.clone(),
        outputs_match,
        model_a_hash: proof_a.model_hash.clone(),
        model_b_hash: proof_b.model_hash.clone(),
        circuit_a_hash: proof_a.circuit_hash.clone(),
        circuit_b_hash: proof_b.circuit_hash.clone(),
        same_model,
        same_circuit,
        details: Some(detail_parts.join("; ")),
    }
}

/// Compare the output data of two proof bundles.
///
/// For the remainder GKR proof system, the "outputs" are embedded in the proof
/// binary. Since the proof binary includes commitments and Fiat-Shamir
/// challenges that differ across runs even for identical inputs, we compare
/// the `public_inputs_hex` field which represents the public inputs/outputs
/// that both proofs claim to prove.
///
/// Returns `(outputs_match, optional_detail_message)`.
fn compare_outputs(a: &ProofBundle, b: &ProofBundle) -> (bool, Option<String>) {
    let a_hex = normalize_hex(&a.public_inputs_hex);
    let b_hex = normalize_hex(&b.public_inputs_hex);

    if a_hex.is_empty() && b_hex.is_empty() {
        // Both have empty public inputs -- this is common when public inputs
        // are embedded in the proof itself. Fall back to comparing the
        // dag_circuit_description outputs if present.
        let a_outputs = extract_embedded_outputs(&a.dag_circuit_description);
        let b_outputs = extract_embedded_outputs(&b.dag_circuit_description);
        match (a_outputs, b_outputs) {
            (Some(ao), Some(bo)) => {
                if ao == bo {
                    (true, None)
                } else {
                    (false, Some("embedded outputs differ".to_string()))
                }
            }
            (None, None) => (
                true,
                Some("no public inputs or embedded outputs to compare".to_string()),
            ),
            _ => (
                false,
                Some("one proof has embedded outputs but the other does not".to_string()),
            ),
        }
    } else if a_hex == b_hex {
        (true, None)
    } else {
        (false, Some("public_inputs_hex differ".to_string()))
    }
}

/// Normalize a hex string by stripping the "0x" prefix and converting to lowercase.
fn normalize_hex(s: &str) -> String {
    s.trim()
        .strip_prefix("0x")
        .unwrap_or(s.trim())
        .to_lowercase()
}

/// Try to extract output values from the DAG circuit description JSON.
///
/// The circuit description may contain an "outputs" or "public_values" field
/// depending on the prover version.
fn extract_embedded_outputs(desc: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(outputs) = desc.get("outputs") {
        if !outputs.is_null() {
            return Some(outputs.clone());
        }
    }
    if let Some(pub_values) = desc.get("public_values") {
        if !pub_values.is_null() {
            return Some(pub_values.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_stored_proof(id: &str, model_hash: &str, circuit_hash: &str) -> StoredProof {
        StoredProof {
            id: id.to_string(),
            model_hash: model_hash.to_string(),
            circuit_hash: circuit_hash.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            verified: Some(true),
            verified_at: Some("2024-01-01T00:00:01Z".to_string()),
            proof_size_bytes: 1024,
            deleted: false,
        }
    }

    fn make_bundle(public_inputs: &str) -> ProofBundle {
        ProofBundle {
            proof_hex: "0xdeadbeef".to_string(),
            public_inputs_hex: public_inputs.to_string(),
            gens_hex: "0xcafe".to_string(),
            dag_circuit_description: serde_json::json!({"num_compute_layers": 4}),
            model_hash: Some("0xmodel".to_string()),
            timestamp: Some(1700000000),
            prover_version: Some("0.1.0".to_string()),
            circuit_hash: Some("0xcircuit".to_string()),
        }
    }

    #[test]
    fn test_same_model_same_output() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");
        let bundle_a = make_bundle("0xaabb");
        let bundle_b = make_bundle("0xaabb");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(result.same_model);
        assert!(result.same_circuit);
        assert!(result.outputs_match);
        assert_eq!(result.proof_a_id, "a");
        assert_eq!(result.proof_b_id, "b");
    }

    #[test]
    fn test_same_model_different_output() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");
        let bundle_a = make_bundle("0xaabb");
        let bundle_b = make_bundle("0xccdd");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(result.same_model);
        assert!(result.same_circuit);
        assert!(!result.outputs_match);
    }

    #[test]
    fn test_different_model_same_output() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel2", "0xcircuit2");
        let bundle_a = make_bundle("0xaabb");
        let bundle_b = make_bundle("0xaabb");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(!result.same_model);
        assert!(!result.same_circuit);
        assert!(result.outputs_match);
    }

    #[test]
    fn test_different_model_different_output() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel2", "0xcircuit2");
        let bundle_a = make_bundle("0xaabb");
        let bundle_b = make_bundle("0xccdd");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(!result.same_model);
        assert!(!result.same_circuit);
        assert!(!result.outputs_match);
    }

    #[test]
    fn test_empty_public_inputs_no_embedded_outputs() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");
        let bundle_a = make_bundle("");
        let bundle_b = make_bundle("");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        // Both empty, no embedded outputs => vacuously match
        assert!(result.outputs_match);
        assert!(result
            .details
            .as_ref()
            .unwrap()
            .contains("no public inputs"));
    }

    #[test]
    fn test_empty_vs_nonempty_public_inputs() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");
        let bundle_a = make_bundle("");
        let bundle_b = make_bundle("0xaabb");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(!result.outputs_match);
    }

    #[test]
    fn test_no_bundles_available() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");

        let result = compare_proofs(&proof_a, &proof_b, None, None);
        assert!(!result.outputs_match);
        assert!(result
            .details
            .as_ref()
            .unwrap()
            .contains("bundles not available"));
    }

    #[test]
    fn test_hex_normalization() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");
        let bundle_a = make_bundle("0xAABB");
        let bundle_b = make_bundle("aabb");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(result.outputs_match);
    }

    #[test]
    fn test_embedded_outputs_match() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");

        let mut bundle_a = make_bundle("");
        bundle_a.dag_circuit_description =
            serde_json::json!({"num_compute_layers": 4, "outputs": [1, 2, 3]});
        let mut bundle_b = make_bundle("");
        bundle_b.dag_circuit_description =
            serde_json::json!({"num_compute_layers": 4, "outputs": [1, 2, 3]});

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(result.outputs_match);
    }

    #[test]
    fn test_embedded_outputs_differ() {
        let proof_a = make_stored_proof("a", "0xmodel1", "0xcircuit1");
        let proof_b = make_stored_proof("b", "0xmodel1", "0xcircuit1");

        let mut bundle_a = make_bundle("");
        bundle_a.dag_circuit_description =
            serde_json::json!({"num_compute_layers": 4, "outputs": [1, 2, 3]});
        let mut bundle_b = make_bundle("");
        bundle_b.dag_circuit_description =
            serde_json::json!({"num_compute_layers": 4, "outputs": [4, 5, 6]});

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        assert!(!result.outputs_match);
        assert!(result
            .details
            .as_ref()
            .unwrap()
            .contains("embedded outputs differ"));
    }

    #[test]
    fn test_empty_hashes_not_considered_same() {
        let proof_a = make_stored_proof("a", "", "");
        let proof_b = make_stored_proof("b", "", "");

        let result = compare_proofs(&proof_a, &proof_b, None, None);
        assert!(!result.same_model);
        assert!(!result.same_circuit);
    }

    #[test]
    fn test_comparison_result_serializes() {
        let proof_a = make_stored_proof("a", "0xm1", "0xc1");
        let proof_b = make_stored_proof("b", "0xm2", "0xc2");
        let bundle_a = make_bundle("0x1234");
        let bundle_b = make_bundle("0x1234");

        let result = compare_proofs(&proof_a, &proof_b, Some(&bundle_a), Some(&bundle_b));
        let json = serde_json::to_string(&result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["proof_a_id"], "a");
        assert_eq!(parsed["proof_b_id"], "b");
        assert_eq!(parsed["outputs_match"], true);
        assert_eq!(parsed["same_model"], false);
        assert_eq!(parsed["same_circuit"], false);
        assert_eq!(parsed["model_a_hash"], "0xm1");
        assert_eq!(parsed["model_b_hash"], "0xm2");
        assert_eq!(parsed["circuit_a_hash"], "0xc1");
        assert_eq!(parsed["circuit_b_hash"], "0xc2");
    }
}
