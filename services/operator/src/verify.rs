//! Local proof pre-verification using zkml-verifier.
//!
//! Provides a `ProofPreVerifier` that loads DAG circuit descriptions from a
//! configured directory and runs local GKR verification before on-chain submission.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::store::StoredProof;

/// Pre-verifier that caches circuit descriptions and runs local proof checks.
pub struct ProofPreVerifier {
    circuit_desc_dir: PathBuf,
    cache: RwLock<HashMap<String, serde_json::Value>>,
}

/// Result of local pre-verification.
#[derive(Debug)]
pub enum PreVerifyResult {
    /// Proof verified successfully.
    Verified,
    /// Verification failed with reason.
    Failed(String),
    /// No circuit description available — cannot pre-verify.
    Skipped(String),
}

impl ProofPreVerifier {
    /// Create a new pre-verifier. `circuit_desc_dir` should contain JSON files
    /// named `<circuit_hash_hex>.json` with the `dag_circuit_description` object.
    pub fn new(circuit_desc_dir: &str) -> Self {
        Self {
            circuit_desc_dir: PathBuf::from(circuit_desc_dir),
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Pre-verify a stored proof locally.
    pub fn pre_verify(&self, proof: &StoredProof) -> PreVerifyResult {
        let circuit_hash_hex = normalize_hex(&proof.circuit_hash);

        let desc = match self.load_circuit_desc(&circuit_hash_hex) {
            Some(d) => d,
            None => {
                return PreVerifyResult::Skipped(format!(
                    "no circuit description for {}",
                    circuit_hash_hex
                ))
            }
        };

        let bundle = zkml_verifier::ProofBundle {
            proof_hex: proof.proof_hex.clone(),
            public_inputs_hex: proof.public_inputs_hex.clone(),
            gens_hex: proof.gens_hex.clone(),
            dag_circuit_description: desc,
            model_hash: None,
            timestamp: None,
            prover_version: None,
            circuit_hash: None,
        };

        match zkml_verifier::verify(&bundle) {
            Ok(result) if result.verified => {
                tracing::info!(circuit_hash = %circuit_hash_hex, "Local pre-verification PASSED");
                PreVerifyResult::Verified
            }
            Ok(_) => {
                tracing::warn!(circuit_hash = %circuit_hash_hex, "Local pre-verification FAILED");
                PreVerifyResult::Failed(format!("proof failed for circuit {}", circuit_hash_hex))
            }
            Err(e) => {
                tracing::warn!(circuit_hash = %circuit_hash_hex, error = %e, "Pre-verification error");
                PreVerifyResult::Failed(format!("verification error: {}", e))
            }
        }
    }

    /// Pre-verify in hybrid mode (transcript replay only, no EC checks).
    pub fn pre_verify_hybrid(
        &self,
        proof: &StoredProof,
    ) -> Result<zkml_verifier::HybridVerificationResult, PreVerifyResult> {
        let circuit_hash_hex = normalize_hex(&proof.circuit_hash);

        let desc = match self.load_circuit_desc(&circuit_hash_hex) {
            Some(d) => d,
            None => {
                return Err(PreVerifyResult::Skipped(format!(
                    "no circuit description for {}",
                    circuit_hash_hex
                )))
            }
        };

        let bundle = zkml_verifier::ProofBundle {
            proof_hex: proof.proof_hex.clone(),
            public_inputs_hex: proof.public_inputs_hex.clone(),
            gens_hex: proof.gens_hex.clone(),
            dag_circuit_description: desc,
            model_hash: None,
            timestamp: None,
            prover_version: None,
            circuit_hash: None,
        };

        match zkml_verifier::verify_hybrid(&bundle) {
            Ok(result) => {
                tracing::info!(
                    circuit_hash = %circuit_hash_hex,
                    compute_layers = result.compute_fr.rlc_betas.len(),
                    "Hybrid pre-verification PASSED"
                );
                Ok(result)
            }
            Err(e) => Err(PreVerifyResult::Failed(format!("hybrid error: {}", e))),
        }
    }

    fn load_circuit_desc(&self, circuit_hash_hex: &str) -> Option<serde_json::Value> {
        {
            let cache = self.cache.read().ok()?;
            if let Some(desc) = cache.get(circuit_hash_hex) {
                return Some(desc.clone());
            }
        }

        let path = self.circuit_desc_dir.join(format!("{}.json", circuit_hash_hex));
        let desc = load_circuit_desc_file(&path)?;

        if let Ok(mut cache) = self.cache.write() {
            cache.insert(circuit_hash_hex.to_string(), desc.clone());
        }

        Some(desc)
    }
}

fn load_circuit_desc_file(path: &Path) -> Option<serde_json::Value> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None,
    };
    serde_json::from_str(&content).ok()
}

fn normalize_hex(hex: &str) -> String {
    hex.strip_prefix("0x")
        .or_else(|| hex.strip_prefix("0X"))
        .unwrap_or(hex)
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_hex() {
        assert_eq!(normalize_hex("0xABCD"), "abcd");
        assert_eq!(normalize_hex("0Xabcd"), "abcd");
        assert_eq!(normalize_hex("abcd"), "abcd");
    }

    #[test]
    fn test_pre_verify_skips_without_circuit_desc() {
        let dir = tempfile::tempdir().unwrap();
        let verifier = ProofPreVerifier::new(dir.path().to_str().unwrap());

        let proof = StoredProof {
            proof_hex: "0xaabb".to_string(),
            circuit_hash: "0x1234".to_string(),
            public_inputs_hex: "0x".to_string(),
            gens_hex: "0x".to_string(),
        };

        let result = verifier.pre_verify(&proof);
        assert!(matches!(result, PreVerifyResult::Skipped(_)));
    }

    #[test]
    fn test_pre_verify_fails_on_bad_proof() {
        let dir = tempfile::tempdir().unwrap();
        let desc = serde_json::json!({
            "numComputeLayers": 0, "numInputLayers": 0,
            "layerTypes": [], "numSumcheckRounds": [],
            "atomOffsets": [0], "atomTargetLayers": [], "atomCommitIdxs": [],
            "ptOffsets": [0], "ptData": [],
            "inputIsCommitted": [],
            "oracleProductOffsets": [0], "oracleResultIdxs": [], "oracleExprCoeffs": []
        });

        let hash_hex = "1234abcd";
        let desc_path = dir.path().join(format!("{}.json", hash_hex));
        std::fs::write(&desc_path, serde_json::to_string(&desc).unwrap()).unwrap();

        let verifier = ProofPreVerifier::new(dir.path().to_str().unwrap());

        let proof = StoredProof {
            proof_hex: "0x42414144".to_string(), // "BAAD"
            circuit_hash: format!("0x{}", hash_hex),
            public_inputs_hex: "0x".to_string(),
            gens_hex: "0x".to_string(),
        };

        let result = verifier.pre_verify(&proof);
        assert!(matches!(result, PreVerifyResult::Failed(_)), "expected Failed, got: {:?}", result);
    }
}
