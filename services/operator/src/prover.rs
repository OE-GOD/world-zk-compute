use std::path::PathBuf;
use tokio::process::Command;

/// Manages ZK proof generation by spawning the precompute_proof binary.
pub struct ProofManager {
    #[allow(dead_code)]
    precompute_bin: String,
    #[allow(dead_code)]
    model_path: String,
    proofs_dir: PathBuf,
}

impl ProofManager {
    pub fn new(precompute_bin: &str, model_path: &str, proofs_dir: &str) -> Self {
        Self {
            precompute_bin: precompute_bin.to_string(),
            model_path: model_path.to_string(),
            proofs_dir: PathBuf::from(proofs_dir),
        }
    }

    /// Trigger proof generation for given features. Stores result at `{proofs_dir}/{result_id}.json`.
    #[allow(dead_code)]
    pub async fn generate_proof(&self, result_id: &str, features: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.proofs_dir)?;
        let output_path = self.proofs_dir.join(format!("{}.json", result_id));

        tracing::info!(
            "Generating proof: {} --model {} --features {} --output {}",
            self.precompute_bin,
            self.model_path,
            features,
            output_path.display()
        );

        let status = Command::new(&self.precompute_bin)
            .arg("--model")
            .arg(&self.model_path)
            .arg("--features")
            .arg(features)
            .arg("--output")
            .arg(&output_path)
            .status()
            .await?;

        if !status.success() {
            anyhow::bail!("Proof generation failed with status: {}", status);
        }

        tracing::info!("Proof generated: {}", output_path.display());
        Ok(())
    }

    /// Wait for a proof file to appear (poll with timeout).
    pub async fn wait_for_proof(&self, result_id: &str, timeout_secs: u64) -> anyhow::Result<bool> {
        let path = self.proofs_dir.join(format!("{}.json", result_id));
        let start = std::time::Instant::now();

        while start.elapsed().as_secs() < timeout_secs {
            if path.exists() {
                return Ok(true);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        Ok(false)
    }

    /// Read a stored proof from disk.
    pub fn read_proof(&self, result_id: &str) -> anyhow::Result<Option<crate::store::StoredProof>> {
        let path = self.proofs_dir.join(format!("{}.json", result_id));
        if !path.exists() {
            return Ok(None);
        }
        let json = std::fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// Check if a proof exists for this result ID.
    #[allow(dead_code)]
    pub fn has_proof(&self, result_id: &str) -> bool {
        self.proofs_dir.join(format!("{}.json", result_id)).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proof_manager_new() {
        let pm = ProofManager::new("precompute_proof", "./model.json", "/tmp/proofs");
        assert_eq!(pm.precompute_bin, "precompute_proof");
        assert_eq!(pm.model_path, "./model.json");
    }

    #[test]
    fn test_has_proof_missing() {
        let pm = ProofManager::new("x", "y", "/tmp/nonexistent-proof-dir-12345");
        assert!(!pm.has_proof("does-not-exist"));
    }

    #[test]
    fn test_read_proof_missing() {
        let pm = ProofManager::new("x", "y", "/tmp/nonexistent-proof-dir-12345");
        let result = pm.read_proof("does-not-exist").unwrap();
        assert!(result.is_none());
    }
}
