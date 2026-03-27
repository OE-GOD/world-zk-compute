//! Client for auto-submitting proof bundles to the proof-registry service.
//!
//! When `REGISTRY_URL` is configured, the proof-generator will POST each
//! generated proof bundle to the registry after successful generation. This
//! removes the need for callers to separately submit proofs.

use serde::{Deserialize, Serialize};

/// Result of submitting a proof to the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySubmitResult {
    /// Proof ID assigned by the registry.
    pub id: String,
    /// Status returned by the registry (e.g. "stored").
    pub status: String,
    /// Transparency log index, if available.
    pub transparency_index: Option<u64>,
    /// SHA-256 hash of the proof bundle, if available.
    pub proof_hash: Option<String>,
}

/// HTTP client for the proof-registry service.
#[derive(Clone)]
pub struct RegistryClient {
    /// Base URL of the proof-registry (e.g. "http://localhost:3200").
    url: String,
    /// Reusable HTTP client.
    client: reqwest::Client,
}

#[allow(dead_code)]
impl RegistryClient {
    /// Create a new registry client pointing at the given base URL.
    pub fn new(url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build registry HTTP client");
        Self { url, client }
    }

    /// Submit a proof bundle to the registry.
    ///
    /// The `proof_bundle` is sent as-is to `POST {url}/proofs`. Returns the
    /// registry's response containing the assigned proof ID.
    pub async fn submit_proof(
        &self,
        proof_bundle: &serde_json::Value,
    ) -> Result<RegistrySubmitResult, String> {
        let endpoint = format!("{}/proofs", self.url);

        let resp = self
            .client
            .post(&endpoint)
            .json(proof_bundle)
            .send()
            .await
            .map_err(|e| format!("Failed to reach proof-registry at {}: {}", endpoint, e))?;

        let status_code = resp.status();
        if !status_code.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "Proof-registry returned HTTP {}: {}",
                status_code, body
            ));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse registry response: {}", e))?;

        Ok(RegistrySubmitResult {
            id: body["id"].as_str().unwrap_or("").to_string(),
            status: body["status"].as_str().unwrap_or("unknown").to_string(),
            transparency_index: body["transparency_index"].as_u64(),
            proof_hash: body["proof_hash"].as_str().map(|s| s.to_string()),
        })
    }

    /// Return the configured registry URL.
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_client_creation() {
        let client = RegistryClient::new("http://localhost:3200".to_string());
        assert_eq!(client.url(), "http://localhost:3200");
    }

    #[test]
    fn test_registry_submit_result_serialization() {
        let result = RegistrySubmitResult {
            id: "abc-123".to_string(),
            status: "stored".to_string(),
            transparency_index: Some(42),
            proof_hash: Some("deadbeef".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("abc-123"));
        assert!(json.contains("stored"));

        let deserialized: RegistrySubmitResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "abc-123");
        assert_eq!(deserialized.transparency_index, Some(42));
    }

    #[test]
    fn test_registry_submit_result_minimal() {
        let result = RegistrySubmitResult {
            id: "x".to_string(),
            status: "stored".to_string(),
            transparency_index: None,
            proof_hash: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: RegistrySubmitResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.transparency_index.is_none());
        assert!(deserialized.proof_hash.is_none());
    }
}
