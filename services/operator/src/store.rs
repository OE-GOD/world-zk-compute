use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A stored ZK proof ready for on-chain dispute resolution.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredProof {
    pub proof_hex: String,
    pub circuit_hash: String,
    pub public_inputs_hex: String,
    pub gens_hex: String,
}

/// File-based proof storage.
#[allow(dead_code)]
pub struct ProofStore {
    dir: PathBuf,
}

#[allow(dead_code)]
impl ProofStore {
    pub fn new(dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: PathBuf::from(dir),
        })
    }

    /// Save a proof for a given result ID.
    pub fn save(&self, result_id: &str, proof: &StoredProof) -> anyhow::Result<()> {
        let path = self.dir.join(format!("{}.json", result_id));
        let json = serde_json::to_string_pretty(proof)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a proof by result ID, if it exists.
    pub fn load(&self, result_id: &str) -> anyhow::Result<Option<StoredProof>> {
        let path = self.dir.join(format!("{}.json", result_id));
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Check if a proof exists.
    pub fn has(&self, result_id: &str) -> bool {
        self.dir.join(format!("{}.json", result_id)).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_roundtrip() {
        let dir = std::env::temp_dir().join("tee-operator-test-store");
        let _ = std::fs::remove_dir_all(&dir);

        let store = ProofStore::new(dir.to_str().unwrap()).unwrap();

        let proof = StoredProof {
            proof_hex: "0xaabb".to_string(),
            circuit_hash: "0x1234".to_string(),
            public_inputs_hex: "0xccdd".to_string(),
            gens_hex: "0xeeff".to_string(),
        };

        store.save("test-result-1", &proof).unwrap();
        assert!(store.has("test-result-1"));
        assert!(!store.has("nonexistent"));

        let loaded = store.load("test-result-1").unwrap().unwrap();
        assert_eq!(loaded.proof_hex, "0xaabb");
        assert_eq!(loaded.circuit_hash, "0x1234");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
