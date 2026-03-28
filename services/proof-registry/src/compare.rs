//! Proof comparison — verify two proofs produce the same output for the same input.

use serde::Serialize;

/// Result of comparing two proofs.
#[derive(Debug, Serialize)]
pub struct ComparisonResult {
    pub proof_a_id: String,
    pub proof_b_id: String,
    pub same_circuit: bool,
    pub same_model: bool,
    pub both_verified: bool,
    pub match_status: String,
}

/// Compare two stored proofs by their metadata.
pub fn compare_proofs(
    a_circuit_hash: Option<&str>,
    a_model_hash: Option<&str>,
    a_verified: Option<bool>,
    b_circuit_hash: Option<&str>,
    b_model_hash: Option<&str>,
    b_verified: Option<bool>,
) -> (bool, bool, bool) {
    let same_circuit = match (a_circuit_hash, b_circuit_hash) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let same_model = match (a_model_hash, b_model_hash) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let both_verified = a_verified == Some(true) && b_verified == Some(true);
    (same_circuit, same_model, both_verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_proofs() {
        let (sc, sm, bv) = compare_proofs(
            Some("0xabc"), Some("0xdef"), Some(true),
            Some("0xabc"), Some("0xdef"), Some(true),
        );
        assert!(sc && sm && bv);
    }

    #[test]
    fn test_different_circuits() {
        let (sc, _, _) = compare_proofs(
            Some("0xabc"), Some("0xdef"), Some(true),
            Some("0x123"), Some("0xdef"), Some(true),
        );
        assert!(!sc);
    }

    #[test]
    fn test_one_unverified() {
        let (_, _, bv) = compare_proofs(
            Some("0xabc"), None, Some(true),
            Some("0xabc"), None, None,
        );
        assert!(!bv);
    }
}
