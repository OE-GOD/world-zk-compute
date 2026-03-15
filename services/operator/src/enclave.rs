use serde::Deserialize;
use std::time::Duration;

/// Default connection timeout for enclave HTTP requests (seconds).
const DEFAULT_ENCLAVE_CONNECT_TIMEOUT_SECS: u64 = 5;

/// Default request timeout for enclave HTTP requests (seconds).
const DEFAULT_ENCLAVE_REQUEST_TIMEOUT_SECS: u64 = 30;

/// HTTP client for the TEE enclave API.
pub struct EnclaveClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct InferResponse {
    pub result: String,
    pub model_hash: String,
    pub input_hash: String,
    pub result_hash: String,
    pub attestation: String,
    #[allow(dead_code)]
    pub enclave_address: String,
}

#[derive(Debug, Deserialize)]
pub struct EnclaveInfo {
    pub enclave_address: String,
    #[allow(dead_code)]
    pub model_hash: String,
}

/// Attestation response from the enclave's /attestation endpoint.
#[derive(Debug, Deserialize)]
pub struct AttestationResponse {
    pub document: String,
    #[allow(dead_code)]
    pub enclave_address: String,
    pub is_nitro: bool,
    pub pcr0: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct HealthResponse {
    #[allow(dead_code)]
    status: String,
}

impl EnclaveClient {
    /// Create a new client using env var `ENCLAVE_TIMEOUT_SECS` or defaults.
    #[allow(dead_code)]
    pub fn new(base_url: &str) -> Self {
        let timeout_secs = std::env::var("ENCLAVE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());

        let connect_timeout = DEFAULT_ENCLAVE_CONNECT_TIMEOUT_SECS;
        let request_timeout = timeout_secs.unwrap_or(DEFAULT_ENCLAVE_REQUEST_TIMEOUT_SECS);

        Self::with_timeouts(base_url, connect_timeout, request_timeout)
    }

    /// Create a new client using a pre-resolved config timeout (seconds).
    ///
    /// This avoids redundant env-var reads when the caller already loaded
    /// the timeout value via `Config::from_env()`.
    pub fn with_config_timeout(base_url: &str, request_timeout_secs: u64) -> Self {
        Self::with_timeouts(
            base_url,
            DEFAULT_ENCLAVE_CONNECT_TIMEOUT_SECS,
            request_timeout_secs,
        )
    }

    /// Create a new client with explicit timeout values (in seconds).
    pub fn with_timeouts(
        base_url: &str,
        connect_timeout_secs: u64,
        request_timeout_secs: u64,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(connect_timeout_secs))
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()
            .expect("failed to build reqwest client");

        tracing::debug!(
            connect_timeout_secs = connect_timeout_secs,
            request_timeout_secs = request_timeout_secs,
            "EnclaveClient configured with timeouts"
        );

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Check if the enclave is healthy.
    pub async fn health(&self) -> anyhow::Result<bool> {
        let resp = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .await?;
        Ok(resp.status().is_success())
    }

    /// Get enclave info (address + model hash).
    pub async fn info(&self) -> anyhow::Result<EnclaveInfo> {
        let resp = self
            .client
            .get(format!("{}/info", self.base_url))
            .send()
            .await?
            .json::<EnclaveInfo>()
            .await?;
        Ok(resp)
    }

    /// Fetch attestation document from the enclave.
    ///
    /// If `nonce` is provided, it is passed as a query parameter to the enclave
    /// so the attestation document includes it for freshness verification.
    pub async fn attestation(&self, nonce: Option<&str>) -> anyhow::Result<AttestationResponse> {
        let start = std::time::Instant::now();
        let mut url = format!("{}/attestation", self.base_url);
        if let Some(n) = nonce {
            url = format!("{}?nonce={}", url, n);
        }
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Attestation fetch failed ({}): {}", status, text);
        }

        let att_resp = resp.json::<AttestationResponse>().await?;
        tracing::debug!(
            enclave_url = %self.base_url,
            latency_ms = start.elapsed().as_millis() as u64,
            "Attestation fetched"
        );
        Ok(att_resp)
    }

    /// Run inference on the given features.
    pub async fn infer(&self, features: &[f64]) -> anyhow::Result<InferResponse> {
        let start = std::time::Instant::now();
        let body = serde_json::json!({ "features": features });
        let resp = self
            .client
            .post(format!("{}/infer", self.base_url))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Enclave inference failed ({}): {}", status, text);
        }

        let infer_resp = resp.json::<InferResponse>().await?;
        tracing::debug!(
            enclave_url = %self.base_url,
            latency_ms = start.elapsed().as_millis() as u64,
            num_features = features.len(),
            "Inference completed"
        );
        Ok(infer_resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enclave_client_new() {
        let client = EnclaveClient::new("http://localhost:8080/");
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_enclave_client_with_timeouts() {
        let client = EnclaveClient::with_timeouts("http://example.com", 5, 15);
        assert_eq!(client.base_url, "http://example.com");
    }

    #[test]
    fn test_enclave_client_with_timeouts_strips_trailing_slash() {
        let client = EnclaveClient::with_timeouts("http://example.com/", 10, 30);
        assert_eq!(client.base_url, "http://example.com");
    }

    #[test]
    fn test_default_timeout_constants() {
        assert_eq!(DEFAULT_ENCLAVE_CONNECT_TIMEOUT_SECS, 5);
        assert_eq!(DEFAULT_ENCLAVE_REQUEST_TIMEOUT_SECS, 30);
    }

    #[test]
    fn test_enclave_client_with_config_timeout() {
        let client = EnclaveClient::with_config_timeout("http://example.com/", 60);
        assert_eq!(client.base_url, "http://example.com");
    }
}
