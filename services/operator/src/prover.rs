use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::fs;

/// Default connection timeout for prover HTTP requests (seconds).
const DEFAULT_PROVER_CONNECT_TIMEOUT_SECS: u64 = 5;

/// Default request timeout for prover HTTP requests (seconds).
/// Proving can take several minutes, so default is generous.
#[allow(dead_code)]
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
    #[allow(dead_code)]
    retry_delay_secs: u64,
    /// Base delay in milliseconds for retry backoff.
    retry_base_delay_ms: u64,
    /// Maximum delay in milliseconds (caps exponential backoff).
    retry_max_delay_ms: u64,
    http_client: reqwest::Client,
}

impl ProofManager {
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(clippy::too_many_arguments)]
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

        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(DEFAULT_PROVER_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()
            .expect("failed to build prover HTTP client");

        tracing::info!(
            "ProofManager initialized: mode={:?}, max_retries={}, base_delay_ms={}, max_delay_ms={}, timeout={}s",
            mode,
            max_retries,
            retry_base_delay_ms,
            retry_max_delay_ms,
            request_timeout_secs,
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
        fs::create_dir_all(&self.proofs_dir).await?;
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

    /// Minimum acceptable response body size in bytes.
    /// Any proof response smaller than this is almost certainly invalid.
    const MIN_RESPONSE_SIZE: usize = 10;

    /// Generate a proof by calling the warm prover HTTP endpoint.
    ///
    /// Validates the response before writing to disk:
    /// 1. HTTP status must be success (2xx)
    /// 2. Content-Type must be JSON-compatible (if present)
    /// 3. Response body must be non-empty and meet minimum size threshold
    /// 4. Response body must be valid JSON
    /// 5. JSON must contain a recognized proof field (`proof`, `proof_hex`, or `receipt`)
    /// 6. Proof field values must be non-null and non-empty
    /// 7. StoredProof format (`proof_hex`) must include companion fields (`circuit_hash`,
    ///    `public_inputs_hex`, `gens_hex`)
    async fn generate_proof_http(
        &self,
        prover_url: &str,
        result_id: &str,
        features: &str,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(&self.proofs_dir).await?;
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

        // 1. Validate HTTP status.
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(
                result_id = result_id,
                endpoint = %endpoint,
                status = %status,
                body_preview = %if text.len() > 200 { &text[..200] } else { &text },
                "Prover HTTP request returned non-success status"
            );
            anyhow::bail!("Prover HTTP request failed ({}): {}", status, text);
        }

        // 2. Check Content-Type header: if present and not JSON-compatible, reject early.
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_lowercase());
        if let Some(ref ct) = content_type {
            if !ct.contains("json") && !ct.contains("octet-stream") {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    content_type = %ct,
                    "Prover response has unexpected Content-Type"
                );
                anyhow::bail!(
                    "Prover response has unexpected Content-Type: {}; expected JSON",
                    ct
                );
            }
        }

        // 3. Validate response body is non-empty and meets minimum size.
        let body = resp.text().await?;
        if body.is_empty() {
            tracing::warn!(
                result_id = result_id,
                endpoint = %endpoint,
                "Prover returned empty response body"
            );
            anyhow::bail!("Prover returned empty response body");
        }
        if body.len() < Self::MIN_RESPONSE_SIZE {
            tracing::warn!(
                result_id = result_id,
                endpoint = %endpoint,
                body_len = body.len(),
                body_preview = %body,
                "Prover response too small ({} bytes, minimum {} bytes)",
                body.len(),
                Self::MIN_RESPONSE_SIZE,
            );
            anyhow::bail!(
                "Prover response too small ({} bytes, minimum {} bytes)",
                body.len(),
                Self::MIN_RESPONSE_SIZE,
            );
        }

        // 4. Validate the response is valid JSON.
        let json_value: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            tracing::warn!(
                result_id = result_id,
                endpoint = %endpoint,
                body_preview = %if body.len() > 200 { &body[..200] } else { &body },
                error = %e,
                "Prover response is not valid JSON"
            );
            anyhow::anyhow!("Prover response is not valid JSON: {}", e)
        })?;

        // 5. Validate that the JSON contains at least one recognized proof field.
        let has_proof_field = json_value.get("proof").is_some()
            || json_value.get("proof_hex").is_some()
            || json_value.get("receipt").is_some();
        if !has_proof_field {
            tracing::warn!(
                result_id = result_id,
                endpoint = %endpoint,
                keys = ?json_value.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                "Prover response JSON missing required proof field (expected 'proof', 'proof_hex', or 'receipt')"
            );
            anyhow::bail!(
                "Prover response JSON missing required field: expected 'proof', 'proof_hex', or 'receipt'"
            );
        }

        // 6. Deep validation of proof content based on format.
        if let Some(proof_hex_val) = json_value.get("proof_hex") {
            // StoredProof format: validate proof_hex is a non-empty string.
            let proof_hex = proof_hex_val.as_str().ok_or_else(|| {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    "Prover response proof_hex is not a string"
                );
                anyhow::anyhow!("Prover response proof_hex is not a string")
            })?;
            let stripped = proof_hex.strip_prefix("0x").unwrap_or(proof_hex);
            if stripped.is_empty() {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    "Prover response contains empty proof_hex"
                );
                anyhow::bail!("Prover response contains empty proof_hex");
            }

            // StoredProof requires circuit_hash, public_inputs_hex, gens_hex alongside proof_hex.
            let required_companions = ["circuit_hash", "public_inputs_hex", "gens_hex"];
            let mut missing = Vec::new();
            for field in &required_companions {
                match json_value.get(*field) {
                    Some(v) if v.is_string() && !v.as_str().unwrap_or("").is_empty() => {}
                    _ => missing.push(*field),
                }
            }
            if !missing.is_empty() {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    missing_fields = ?missing,
                    "Prover response (proof_hex format) missing required companion fields"
                );
                anyhow::bail!(
                    "Prover response missing required fields for StoredProof: {}",
                    missing.join(", ")
                );
            }
        } else if let Some(proof_val) = json_value.get("proof") {
            // Generic proof format: verify the value is non-null and non-empty.
            if proof_val.is_null() {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    "Prover response contains null proof field"
                );
                anyhow::bail!("Prover response contains null proof field");
            }
            if let Some(s) = proof_val.as_str() {
                if s.is_empty() {
                    tracing::warn!(
                        result_id = result_id,
                        endpoint = %endpoint,
                        "Prover response contains empty proof string"
                    );
                    anyhow::bail!("Prover response contains empty proof string");
                }
            }
        } else if let Some(receipt_val) = json_value.get("receipt") {
            // Receipt format: verify the value is non-null and non-empty.
            if receipt_val.is_null() {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    "Prover response contains null receipt field"
                );
                anyhow::bail!("Prover response contains null receipt field");
            }
            if let Some(s) = receipt_val.as_str() {
                if s.is_empty() {
                    tracing::warn!(
                        result_id = result_id,
                        endpoint = %endpoint,
                        "Prover response contains empty receipt string"
                    );
                    anyhow::bail!("Prover response contains empty receipt string");
                }
            }
        }

        fs::write(&output_path, &body).await?;
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

    // ======================== HTTP response validation tests ========================

    /// Helper: spawn a mock HTTP server that returns a fixed response on the first connection.
    async fn spawn_mock_server(response: String) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let mut buf = vec![0u8; 4096];
            let _ = io.read(&mut buf).await;
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });
        (url, handle)
    }

    /// Helper: create a ProofManager for testing (subprocess mode is fine since we call
    /// generate_proof_http directly with the mock URL).
    fn make_test_pm(dir: &tempfile::TempDir) -> ProofManager {
        // Force subprocess mode so the constructor does not need the mock URL.
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1)
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_empty_body() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let (mock_url, _h) = spawn_mock_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string(),
        )
        .await;

        let result = pm
            .generate_proof_http(&mock_url, "test-empty-body", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("empty"),
            "Expected empty body error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_too_small_response() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = "short"; // 5 bytes, under MIN_RESPONSE_SIZE=10
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-too-small", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("too small"),
            "Expected too-small error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_invalid_json() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = "this is not json at all!!!";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-bad-json", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not valid JSON"),
            "Expected JSON validation error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_empty_proof_hex() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"proof_hex":"","circuit_hash":"0x1234","public_inputs_hex":"0xab","gens_hex":"0xcd"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-empty-hex", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("empty proof_hex"),
            "Expected empty proof_hex error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_non_200() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = "Internal Server Error";
        let resp = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-500", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("500"),
            "Expected HTTP 500 error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_unexpected_content_type() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = "<html><body>Error page</body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-html", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Content-Type"),
            "Expected Content-Type error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_missing_proof_field() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"status":"ok","message":"no proof here"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-no-proof-field", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing required field"),
            "Expected missing field error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_accepts_valid_proof_hex_response() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"proof_hex":"0xdeadbeef","circuit_hash":"0x1234","public_inputs_hex":"0xab","gens_hex":"0xcd"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-valid-hex", "[1.0, 2.0]")
            .await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result);

        // Verify the file was written.
        let proof_path = dir.path().join("test-valid-hex.json");
        assert!(proof_path.exists(), "Proof file should have been saved");
        let contents = std::fs::read_to_string(&proof_path).unwrap();
        assert!(contents.contains("deadbeef"));
    }

    #[tokio::test]
    async fn test_generate_proof_http_accepts_receipt_field() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"receipt":"base64encodeddata","image_id":"0xabcd"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-receipt", "[1.0, 2.0]")
            .await;
        assert!(
            result.is_ok(),
            "Expected success with 'receipt' field, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_accepts_proof_field() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"proof":{"seal":"0x1234","journal":"0xabcd"}}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-proof-obj", "[1.0, 2.0]")
            .await;
        assert!(
            result.is_ok(),
            "Expected success with 'proof' field, got: {:?}",
            result
        );
    }

    // ======================== StoredProof companion field validation ========================

    #[tokio::test]
    async fn test_generate_proof_http_rejects_proof_hex_missing_companion_fields() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        // proof_hex present but circuit_hash, public_inputs_hex, gens_hex are absent.
        let body = r#"{"proof_hex":"0xdeadbeef"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-missing-companions", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing required fields"),
            "Expected missing fields error, got: {}",
            err_msg
        );
        assert!(err_msg.contains("circuit_hash"), "Should mention circuit_hash: {}", err_msg);
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_proof_hex_with_empty_circuit_hash() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"proof_hex":"0xdeadbeef","circuit_hash":"","public_inputs_hex":"0xab","gens_hex":"0xcd"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-empty-ch", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("circuit_hash"),
            "Expected circuit_hash in error, got: {}",
            err_msg
        );
    }

    // ======================== Null/empty proof and receipt validation ========================

    #[tokio::test]
    async fn test_generate_proof_http_rejects_null_proof_field() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"proof":null}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-null-proof", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("null proof"),
            "Expected null proof error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_empty_proof_string() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"proof":""}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-empty-proof-str", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty proof string"),
            "Expected empty proof string error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_null_receipt_field() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"receipt":null}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-null-receipt", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("null receipt"),
            "Expected null receipt error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_empty_receipt_string() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pm = make_test_pm(&dir);

        let body = r#"{"receipt":""}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body,
        );
        let (mock_url, _h) = spawn_mock_server(resp).await;

        let result = pm
            .generate_proof_http(&mock_url, "test-empty-receipt-str", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty receipt string"),
            "Expected empty receipt string error, got: {}",
            err_msg
        );
    }
}
