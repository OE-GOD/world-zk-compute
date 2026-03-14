use std::path::PathBuf;
use tokio::process::Command;

/// How the operator communicates with the prover.
#[derive(Debug, Clone)]
pub enum ProverMode {
    /// Spawn a subprocess (existing behavior).
    Subprocess,
    /// Call a warm prover HTTP server at the given URL.
    Http { url: String },
}

impl ProverMode {
    /// Determine prover mode from environment.
    /// If `PROVER_URL` is set and non-empty, use HTTP mode. Otherwise, use subprocess.
    pub fn from_env() -> Self {
        match std::env::var("PROVER_URL") {
            Ok(url) if !url.is_empty() => ProverMode::Http { url },
            _ => ProverMode::Subprocess,
        }
    }
}

/// Manages ZK proof generation via subprocess or HTTP warm prover.
pub struct ProofManager {
    precompute_bin: String,
    model_path: String,
    proofs_dir: PathBuf,
    mode: ProverMode,
    max_retries: u32,
    retry_delay_secs: u64,
}

impl ProofManager {
    pub fn new(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
    ) -> Self {
        let mode = ProverMode::from_env();
        tracing::info!(
            "ProofManager initialized with mode: {:?}, max_retries: {}, retry_delay_secs: {}",
            mode,
            max_retries,
            retry_delay_secs,
        );
        Self {
            precompute_bin: precompute_bin.to_string(),
            model_path: model_path.to_string(),
            proofs_dir: PathBuf::from(proofs_dir),
            mode,
            max_retries,
            retry_delay_secs,
        }
    }

    /// Trigger proof generation for given features. Stores result at `{proofs_dir}/{result_id}.json`.
    ///
    /// On failure, retries up to `max_retries` times with exponential backoff
    /// starting from `retry_delay_secs` (capped at 16x the base delay).
    pub async fn generate_proof(&self, result_id: &str, features: &str) -> anyhow::Result<()> {
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                // Exponential backoff: base * 2^(attempt-1), capped at 16x base
                let delay = self.retry_delay_secs * (1u64 << (attempt - 1).min(4));
                tracing::warn!(
                    attempt = attempt,
                    delay_secs = delay,
                    result_id = result_id,
                    "Retrying proof generation"
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }

            let result = match &self.mode {
                ProverMode::Subprocess => self.generate_proof_subprocess(result_id, features).await,
                ProverMode::Http { url } => {
                    self.generate_proof_http(url, result_id, features).await
                }
            };

            match result {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::error!(
                        attempt = attempt,
                        max_retries = self.max_retries,
                        error = %e,
                        result_id = result_id,
                        "Proof generation failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Proof generation failed after retries")))
    }

    /// Generate a proof by spawning a subprocess.
    async fn generate_proof_subprocess(
        &self,
        result_id: &str,
        features: &str,
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.proofs_dir)?;
        let output_path = self.proofs_dir.join(format!("{}.json", result_id));

        tracing::info!(
            "Generating proof (subprocess): {} --model {} --features {} --output {}",
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

        tracing::info!("Proof generated (subprocess): {}", output_path.display());
        Ok(())
    }

    /// Generate a proof by calling the warm prover HTTP endpoint.
    async fn generate_proof_http(
        &self,
        prover_url: &str,
        result_id: &str,
        features: &str,
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.proofs_dir)?;
        let output_path = self.proofs_dir.join(format!("{}.json", result_id));

        let endpoint = format!("{}/prove", prover_url.trim_end_matches('/'));
        tracing::info!("Generating proof (HTTP): POST {}", endpoint);

        let feats: Vec<f64> = serde_json::from_str(features)
            .map_err(|e| anyhow::anyhow!("Invalid features JSON: {}", e))?;

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .json(&serde_json::json!({ "features": feats }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Prover HTTP request failed ({}): {}", status, text);
        }

        // Save the response JSON directly as the proof file.
        let body = resp.text().await?;
        std::fs::write(&output_path, &body)?;

        tracing::info!("Proof received and saved (HTTP): {}", output_path.display());
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

    /// Return the current prover mode.
    #[allow(dead_code)]
    pub fn mode(&self) -> &ProverMode {
        &self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables,
    // preventing race conditions in parallel test execution.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_proof_manager_new() {
        let _lock = ENV_MUTEX.lock().unwrap();
        // Ensure subprocess mode when PROVER_URL is not set.
        std::env::remove_var("PROVER_URL");
        let pm = ProofManager::new("precompute_proof", "./model.json", "/tmp/proofs", 3, 10);
        assert_eq!(pm.precompute_bin, "precompute_proof");
        assert_eq!(pm.model_path, "./model.json");
        assert!(matches!(pm.mode, ProverMode::Subprocess));
        assert_eq!(pm.max_retries, 3);
        assert_eq!(pm.retry_delay_secs, 10);
    }

    #[test]
    fn test_has_proof_missing() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        let pm = ProofManager::new("x", "y", "/tmp/nonexistent-proof-dir-12345", 3, 10);
        assert!(!pm.has_proof("does-not-exist"));
    }

    #[test]
    fn test_read_proof_missing() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        let pm = ProofManager::new("x", "y", "/tmp/nonexistent-proof-dir-12345", 3, 10);
        let result = pm.read_proof("does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_prover_mode_from_env_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        let mode = ProverMode::from_env();
        assert!(matches!(mode, ProverMode::Subprocess));
    }

    #[test]
    fn test_prover_mode_from_env_http() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", "http://localhost:3000");
        let mode = ProverMode::from_env();
        assert!(matches!(mode, ProverMode::Http { .. }));
        if let ProverMode::Http { url } = &mode {
            assert_eq!(url, "http://localhost:3000");
        }
        std::env::remove_var("PROVER_URL");
    }

    #[test]
    fn test_prover_mode_from_env_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", "");
        let mode = ProverMode::from_env();
        assert!(matches!(mode, ProverMode::Subprocess));
        std::env::remove_var("PROVER_URL");
    }

    #[test]
    fn test_proof_manager_http_mode() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", "http://warm-prover:8080");
        let pm = ProofManager::new("unused", "unused", "/tmp/proofs", 3, 10);
        assert!(matches!(pm.mode, ProverMode::Http { .. }));
        if let ProverMode::Http { url } = &pm.mode {
            assert_eq!(url, "http://warm-prover:8080");
        }
        std::env::remove_var("PROVER_URL");
    }

    #[test]
    fn test_proof_manager_new_with_retries() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        let pm = ProofManager::new("x", "y", "/tmp/test", 5, 15);
        assert_eq!(pm.max_retries, 5);
        assert_eq!(pm.retry_delay_secs, 15);
    }

    #[test]
    fn test_proof_manager_zero_retries() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        let pm = ProofManager::new("x", "y", "/tmp/test", 0, 10);
        assert_eq!(pm.max_retries, 0);
        assert_eq!(pm.retry_delay_secs, 10);
    }
}
