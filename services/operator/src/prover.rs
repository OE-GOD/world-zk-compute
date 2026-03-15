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
    http_client: reqwest::Client,
}

impl ProofManager {
    /// Create a new ProofManager. Reads `PROVER_TIMEOUT_SECS` from env for
    /// the HTTP request timeout. Prefer `with_config_timeout()` when the
    /// caller already has a resolved config value.
    pub fn new(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
    ) -> Self {
        let request_timeout = std::env::var("PROVER_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PROVER_REQUEST_TIMEOUT_SECS);

        Self::build_client(
            precompute_bin,
            model_path,
            proofs_dir,
            max_retries,
            retry_delay_secs,
            request_timeout,
        )
    }

    /// Create a ProofManager with an explicit HTTP request timeout (seconds).
    ///
    /// This avoids redundant env-var reads when the caller already loaded
    /// the timeout value via `Config::from_env()`.
    pub fn with_config_timeout(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
        request_timeout_secs: u64,
    ) -> Self {
        Self::build_client(
            precompute_bin,
            model_path,
            proofs_dir,
            max_retries,
            retry_delay_secs,
            request_timeout_secs,
        )
    }

    /// Internal builder shared by all constructors.
    fn build_client(
        precompute_bin: &str,
        model_path: &str,
        proofs_dir: &str,
        max_retries: u32,
        retry_delay_secs: u64,
        request_timeout_secs: u64,
    ) -> Self {
        let mode = ProverMode::from_env();

        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(DEFAULT_PROVER_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()
            .expect("failed to build prover HTTP client");

        tracing::info!(
            "ProofManager initialized with mode: {:?}, max_retries: {}, retry_delay_secs: {}, prover_timeout_secs: {}",
            mode,
            max_retries,
            retry_delay_secs,
            request_timeout_secs,
        );
        Self {
            precompute_bin: precompute_bin.to_string(),
            model_path: model_path.to_string(),
            proofs_dir: PathBuf::from(proofs_dir),
            mode,
            max_retries,
            retry_delay_secs,
            http_client,
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

    /// Minimum acceptable response body size in bytes.
    /// Any proof response smaller than this is almost certainly invalid.
    const MIN_RESPONSE_SIZE: usize = 10;

    /// Generate a proof by calling the warm prover HTTP endpoint.
    ///
    /// Validates the response before writing to disk:
    /// 1. Content-Type must be application/json or application/octet-stream (if present)
    /// 2. Response body must be non-empty and meet minimum size threshold
    /// 3. Response body must be valid JSON
    /// 4. JSON must contain a recognized proof field (proof, proof_hex, or receipt)
    /// 5. Proof field values must be non-null and non-empty
    /// 6. StoredProof format (proof_hex) must include all required companion fields
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

        // 1. Validate response status.
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

        // 2. Check Content-Type header before consuming body.
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_lowercase());
        match content_type {
            Some(ref ct) if !ct.contains("json") && !ct.contains("octet-stream") => {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    content_type = %ct,
                    "Prover response has unexpected Content-Type"
                );
                anyhow::bail!(
                    "Prover response has unexpected Content-Type: {}; expected application/json or application/octet-stream",
                    ct
                );
            }
            None => {
                tracing::warn!(
                    result_id = result_id,
                    endpoint = %endpoint,
                    "Prover response missing Content-Type header; proceeding with body validation"
                );
            }
            _ => {} // acceptable Content-Type
        }

        // 3. Validate response body is not empty and meets minimum size.
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
        std::env::remove_var("PROVER_TIMEOUT_SECS");
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
        // Should not panic -- the custom timeout is picked up.
        let _pm = ProofManager::new("x", "y", "/tmp/test", 0, 10);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
    }

    #[test]
    fn test_proof_manager_with_config_timeout() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        // with_config_timeout should accept an explicit timeout and not panic.
        let pm = ProofManager::with_config_timeout("x", "y", "/tmp/test", 2, 5, 600);
        assert_eq!(pm.max_retries, 2);
        assert_eq!(pm.retry_delay_secs, 5);
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_empty_body() {
        // Start a mock HTTP server that returns 200 with empty body.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let _ = io.read(&mut buf).await;
            let response = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });

        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", &mock_url);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let dir = tempfile::tempdir().unwrap();
        let pm = ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1);
        std::env::remove_var("PROVER_URL");

        let result = pm
            .generate_proof_http(&mock_url, "test-result-1", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty"),
            "Expected empty body error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_invalid_json() {
        // Start a mock HTTP server that returns 200 with non-JSON body.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let _ = io.read(&mut buf).await;
            let body = "this is not json!!!";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });

        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", &mock_url);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let dir = tempfile::tempdir().unwrap();
        let pm = ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1);
        std::env::remove_var("PROVER_URL");

        let result = pm
            .generate_proof_http(&mock_url, "test-result-2", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not valid JSON"),
            "Expected JSON validation error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_validates_empty_proof_hex() {
        // Start a mock HTTP server that returns valid JSON with empty proof_hex.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let _ = io.read(&mut buf).await;
            let body = r#"{"proof_hex":"","circuit_hash":"0x1234","public_inputs_hex":"0xab","gens_hex":"0xcd"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });

        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", &mock_url);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let dir = tempfile::tempdir().unwrap();
        let pm = ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1);
        std::env::remove_var("PROVER_URL");

        let result = pm
            .generate_proof_http(&mock_url, "test-result-3", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty proof_hex"),
            "Expected empty proof_hex error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_non_200() {
        // Start a mock HTTP server that returns 500.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let _ = io.read(&mut buf).await;
            let body = "Internal Server Error";
            let response = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });

        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", &mock_url);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let dir = tempfile::tempdir().unwrap();
        let pm = ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1);
        std::env::remove_var("PROVER_URL");

        let result = pm
            .generate_proof_http(&mock_url, "test-result-4", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("500"),
            "Expected HTTP 500 error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_proof_manager_with_config_timeout() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("PROVER_URL");
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let pm = ProofManager::with_config_timeout("bin", "model", "/tmp/test", 2, 5, 600);
        assert_eq!(pm.precompute_bin, "bin");
        assert_eq!(pm.model_path, "model");
        assert_eq!(pm.max_retries, 2);
        assert_eq!(pm.retry_delay_secs, 5);
        assert!(matches!(pm.mode, ProverMode::Subprocess));
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_bad_content_type() {
        // Start a mock HTTP server that returns 200 with text/html content type.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let _ = io.read(&mut buf).await;
            let body = "<html>Not Found</html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });

        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", &mock_url);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let dir = tempfile::tempdir().unwrap();
        let pm = ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1);
        std::env::remove_var("PROVER_URL");

        let result = pm
            .generate_proof_http(&mock_url, "test-result-ct", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Content-Type"),
            "Expected Content-Type error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_proof_http_rejects_missing_proof_field() {
        // Start a mock HTTP server that returns valid JSON without any proof field.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let io = tokio::io::BufStream::new(stream);
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            let mut io = io;
            let _ = io.read(&mut buf).await;
            let body = r#"{"status":"ok","message":"no proof here"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = io.write_all(response.as_bytes()).await;
            let _ = io.flush().await;
        });

        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("PROVER_URL", &mock_url);
        std::env::remove_var("PROVER_TIMEOUT_SECS");
        let dir = tempfile::tempdir().unwrap();
        let pm = ProofManager::new("x", "y", dir.path().to_str().unwrap(), 0, 1);
        std::env::remove_var("PROVER_URL");

        let result = pm
            .generate_proof_http(&mock_url, "test-result-noproof", "[1.0, 2.0]")
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing required field"),
            "Expected missing proof field error, got: {}",
            err_msg
        );
    }
}
