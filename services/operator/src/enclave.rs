use serde::Deserialize;

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

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct EnclaveInfo {
    pub enclave_address: String,
    pub model_hash: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct HealthResponse {
    #[allow(dead_code)]
    status: String,
}

impl EnclaveClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
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
    #[allow(dead_code)]
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

    /// Run inference on the given features.
    pub async fn infer(&self, features: &[f64]) -> anyhow::Result<InferResponse> {
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
}
