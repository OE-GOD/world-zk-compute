use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// Default connection timeout for prover HTTP requests (seconds).
const DEFAULT_PROVER_CONNECT_TIMEOUT_SECS: u64 = 5;

/// Default request timeout for prover HTTP requests (seconds).
/// Proving can take several minutes, so default is generous.
const DEFAULT_PROVER_REQUEST_TIMEOUT_SECS: u64 = 300;

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
    /// Base delay in milliseconds for retry backoff.
    retry_base_delay_ms: u64,
    /// Maximum delay in milliseconds (caps exponential backoff).
    retry_max_delay_ms: u64,
    http_client: reqwest::Client,
}

impl ProofManager {
    pub fn new(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
    ) -> Self {
        Self::with_retry_config(
            precompute_bin,
            model_path,
            proofs_dir,
            max_retries,
            retry_delay_secs,
            retry_delay_secs * 1000,      // convert legacy seconds to ms
            retry_delay_secs * 16 * 1000, // legacy cap was 16x base
        )
    }

    /// Create a ProofManager with an explicit prover timeout (seconds).
    ///
    /// This avoids redundant env-var reads when the caller already loaded
    /// the timeout value via `Config::from_env()`.
    pub fn with_config_timeout(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
        prover_timeout_secs: u64,
    ) -> Self {
        Self::build_internal(
            precompute_bin,
            model_path,
            proofs_dir,
            max_retries,
            retry_delay_secs,
            retry_delay_secs * 1000,
            retry_delay_secs * 16 * 1000,
            prover_timeout_secs,
        )
    }

    /// Create a ProofManager with explicit retry parameters (millisecond precision).
    pub fn with_retry_config(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
        retry_base_delay_ms: u64,
        retry_max_delay_ms: u64,
    ) -> Self {
        let request_timeout = std::env::var("PROVER_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PROVER_REQUEST_TIMEOUT_SECS);

        Self::build_internal(
            precompute_bin,
            model_path,
            proofs_dir,
            max_retries,
            retry_delay_secs,
            retry_base_delay_ms,
            retry_max_delay_ms,
            request_timeout,
        )
    }

    /// Internal builder shared by all constructors.
    fn build_internal(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
        retry_base_delay_ms: u64,
        retry_max_delay_ms: u64,
        request_timeout_secs: u64,
    ) -> Self {
        let mode = ProverMode::from_env();

        let request_timeout = request_timeout_secs;

        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(DEFAULT_PROVER_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(request_timeout))
            .build()
            .expect("failed to build prover HTTP client");

        tracing::info!(
            "ProofManager initialized: mode={:?}, max_retries={}, base_delay_ms={}, max_delay_ms={}, timeout={}s",
            mode,
            max_retries,
            retry_base_delay_ms,
            retry_max_delay_ms,
            request_timeout,
        );
        Self {
            precompute_bin: precompute_bin.to_string(),
            model_path: model_path.to_string(),
            proofs_dir: PathBuf::from(proofs_dir),
            mode,
            max_retries,
            retry_delay_secs,
            retry_base_delay_ms,
            retry_max_delay_ms,
            http_client,
        }
    }

    /// Compute the retry delay in milliseconds for a given attempt number.
    ///
    /// Uses exponential backoff: `base_ms * 2^(attempt-1)`, capped at `max_delay_ms`.
    /// Returns 0 for attempt 0 (no delay on first try).
    pub fn compute_retry_delay_ms(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }
        let base = self.retry_base_delay_ms;
        let exp = (attempt - 1).min(20);
        let delay = base.saturating_mul(1u64 << exp);
        delay.min(self.retry_max_delay_ms)
    }

    /// Trigger proof generation for given features.
    ///
    /// On failure, retries up to `max_retries` times with exponential backoff
    /// starting from `retry_base_delay_ms` (capped at `retry_max_delay_ms`).
    pub async fn generate_proof(&self, result_id: &str, features: &str) -> anyhow::Result<()> {
        let mut last_error = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay_ms = self.compute_retry_delay_ms(attempt);
                tracing::warn!(
                    attempt = attempt,
                    delay_ms = delay_ms,
                    result_id = result_id,
                    "Retrying proof generation"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }

            let result = match &self.mode {
                ProverMode::Subprocess => {
                    self.generate_proof_subprocess(result_id, features).await
                }
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

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("Proof generation failed after retries")))
    }

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

        let resp = self
            .http_client
            .post(&endpoint)
            .json(&serde_json::json!({ "features": feats }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Prover HTTP request failed ({}): {}", status, text);
        }

        let body = resp.text().await?;
        if body.is_empty() {
            anyhow::bail!("Prover returned empty response body");
        }

        let json_value: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            anyhow::anyhow!("Prover response is not valid JSON: {}", e)
        })?;

        if let Some(proof_hex) = json_value.get("proof_hex").and_then(|v| v.as_str()) {
            let stripped = proof_hex.strip_prefix("0x").unwrap_or(proof_hex);
            if stripped.is_empty() {
                anyhow::bail!("Prover response contains empty proof_hex");
            }
        }

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

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_proof_manager_new() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
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
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::new("x", "y", "/tmp/nonexistent-proof-dir-12345", 3, 10);
        assert!(!pm.has_proof("does-not-exist"));
    }

    #[test]
    fn test_read_proof_missing() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::new("x", "y", "/tmp/nonexistent-proof-dir-12345", 3, 10);
        let result = pm.read_proof("does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_prover_mode_from_env_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        assert!(matches!(ProverMode::from_env(), ProverMode::Subprocess));
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
        assert!(matches!(ProverMode::from_env(), ProverMode::Subprocess));
        std::env::remove_var("PROVER_URL");
    }

    #[test]
    fn test_proof_manager_http_mode() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", "http://warm-prover:8080");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::new("unused", "unused", "/tmp/proofs", 3, 10);
        assert!(matches!(pm.mode, ProverMode::Http { .. }));
        std::env::remove_var("PROVER_URL");
    }

    #[test]
    fn test_proof_manager_new_with_retries() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::new("x", "y", "/tmp/test", 5, 15);
        assert_eq!(pm.max_retries, 5);
        assert_eq!(pm.retry_delay_secs, 15);
    }

    #[test]
    fn test_proof_manager_zero_retries() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::new("x", "y", "/tmp/test", 0, 10);
        assert_eq!(pm.max_retries, 0);
        assert_eq!(pm.retry_delay_secs, 10);
    }

    #[test]
    fn test_default_prover_timeout_constants() {
        assert_eq!(DEFAULT_PROVER_CONNECT_TIMEOUT_SECS, 5);
        assert_eq!(DEFAULT_PROVER_REQUEST_TIMEOUT_SECS, 300);
    }

    #[test]
    fn test_proof_manager_custom_timeout_env() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::set_var("PROVER_TIMEOUT_SECS", "600");
        let _pm = ProofManager::new("x", "y", "/tmp/test", 0, 10);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
    }

    // ======================== Configurable retry tests ========================

    #[test]
    fn test_with_retry_config() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::with_retry_config("x", "y", "/tmp/test", 5, 10, 2000, 60000);
        assert_eq!(pm.max_retries, 5);
        assert_eq!(pm.retry_delay_secs, 10);
        assert_eq!(pm.retry_base_delay_ms, 2000);
        assert_eq!(pm.retry_max_delay_ms, 60000);
    }

    #[test]
    fn test_new_sets_legacy_retry_ms_values() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::new("x", "y", "/tmp/test", 3, 10);
        assert_eq!(pm.retry_base_delay_ms, 10000);
        assert_eq!(pm.retry_max_delay_ms, 160000);
    }

    #[test]
    fn test_compute_retry_delay_ms_attempt_zero() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::with_retry_config("x", "y", "/tmp/test", 3, 1, 1000, 30000);
        assert_eq!(pm.compute_retry_delay_ms(0), 0);
    }

    #[test]
    fn test_compute_retry_delay_ms_exponential() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::with_retry_config("x", "y", "/tmp/test", 10, 1, 1000, 30000);
        assert_eq!(pm.compute_retry_delay_ms(1), 1000);
        assert_eq!(pm.compute_retry_delay_ms(2), 2000);
        assert_eq!(pm.compute_retry_delay_ms(3), 4000);
        assert_eq!(pm.compute_retry_delay_ms(4), 8000);
        assert_eq!(pm.compute_retry_delay_ms(5), 16000);
        assert_eq!(pm.compute_retry_delay_ms(6), 30000);
    }

    #[test]
    fn test_compute_retry_delay_ms_capped_at_max() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::with_retry_config("x", "y", "/tmp/test", 10, 1, 5000, 10000);
        assert_eq!(pm.compute_retry_delay_ms(1), 5000);
        assert_eq!(pm.compute_retry_delay_ms(2), 10000);
        assert_eq!(pm.compute_retry_delay_ms(3), 10000);
        assert_eq!(pm.compute_retry_delay_ms(20), 10000);
    }

    #[test]
    fn test_compute_retry_delay_ms_no_overflow() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::with_retry_config("x", "y", "/tmp/test", 100, 1, 60000, 600000);
        assert_eq!(pm.compute_retry_delay_ms(21), 600000);
        assert_eq!(pm.compute_retry_delay_ms(100), 600000);
    }
}
