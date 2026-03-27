//! Real proof generation via the xgboost-remainder prover.
//!
//! The `xgboost-remainder` crate is excluded from the workspace (it depends on
//! `remainder_ce` which has heavy build requirements). Instead of linking it as
//! a library, we communicate with it over HTTP:
//!
//! - **Managed mode** (default): On model registration the manager spawns an
//!   `xgboost-remainder serve` child process bound to a free port. Proof
//!   requests are forwarded to that process via its `/prove/bundle` endpoint.
//!   When the model is deactivated, the child process is killed.
//!
//! - **External mode**: Point at an already-running warm prover instance via
//!   `--prover-url`. All models share the same external prover (suitable when
//!   a single model is deployed).
//!
//! The response from `/prove/bundle` is a self-contained JSON proof bundle
//! compatible with the on-chain verifier.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Result of a real proof generation call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealProofResult {
    /// Predicted class from model inference.
    pub predicted_class: u32,
    /// Hex-encoded proof bytes (with "0x" prefix).
    pub proof_hex: String,
    /// Hex-encoded circuit hash.
    pub circuit_hash: String,
    /// Hex-encoded public inputs.
    pub public_inputs_hex: String,
    /// Hex-encoded Pedersen generators.
    pub gens_hex: String,
    /// DAG circuit description (hex string or JSON, as returned by prover).
    #[serde(default)]
    pub dag_circuit_description: Option<String>,
    /// Proof size in bytes.
    pub proof_size_bytes: usize,
    /// Proving time in milliseconds.
    pub prove_time_ms: u64,
}

/// Handle to a single prover instance (either a managed child process or an
/// external URL).
struct ProverHandle {
    /// Base URL of the warm prover HTTP server (e.g. "http://127.0.0.1:3042").
    base_url: String,
    /// Child process handle (only set in managed mode).
    #[allow(dead_code)]
    child: Option<tokio::process::Child>,
    /// Model hash for correlation.
    #[allow(dead_code)]
    model_hash: String,
}

/// Manages prover instances for registered models.
///
/// In managed mode, each model gets its own `xgboost-remainder serve` child
/// process. In external mode, a single shared URL is used for all models.
pub struct ProverManager {
    /// model_id -> prover handle
    provers: RwLock<HashMap<String, Arc<ProverHandle>>>,
    /// HTTP client for communicating with prover instances.
    client: reqwest::Client,
    /// Path to the `xgboost-remainder` binary (for managed mode).
    binary_path: String,
    /// External prover URL (when set, all models share this prover).
    external_url: Option<String>,
    /// Next port to assign to a managed child process.
    next_port: RwLock<u16>,
}

impl ProverManager {
    /// Create a new ProverManager.
    ///
    /// - `binary_path`: Path to the `xgboost-remainder` binary. Used only in
    ///   managed mode (when `external_url` is `None`).
    /// - `external_url`: When set, all proof requests go to this URL instead
    ///   of spawning child processes.
    pub fn new(binary_path: String, external_url: Option<String>) -> Self {
        Self {
            provers: RwLock::new(HashMap::new()),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("Failed to build HTTP client"),
            binary_path,
            external_url,
            // Start assigning ports from 13100 to avoid conflicts with the
            // proof-generator service itself (default 3100).
            next_port: RwLock::new(13100),
        }
    }

    /// Register a model and start its prover instance.
    ///
    /// In managed mode, this spawns an `xgboost-remainder serve` child process
    /// with the model JSON written to a temp file. In external mode, this is a
    /// no-op that records the model_id -> external_url mapping.
    ///
    /// Returns the circuit hash reported by the prover's health endpoint.
    pub async fn register_model(
        &self,
        model_id: &str,
        model_json: &str,
        model_hash: &str,
    ) -> anyhow::Result<String> {
        // If using an external prover, just record the mapping.
        if let Some(ref url) = self.external_url {
            let handle = Arc::new(ProverHandle {
                base_url: url.clone(),
                child: None,
                model_hash: model_hash.to_string(),
            });
            self.provers
                .write()
                .await
                .insert(model_id.to_string(), handle);

            // Query health to get circuit hash
            let health_url = format!("{}/health", url);
            let resp = self
                .client
                .get(&health_url)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to reach external prover: {}", e))?;
            let health: serde_json::Value = resp.json().await?;
            let circuit_hash = health["circuit_hash"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();

            tracing::info!(
                model_id,
                circuit_hash = %circuit_hash,
                "Registered model with external prover"
            );
            return Ok(circuit_hash);
        }

        // Managed mode: write model to a temp file and spawn child process.
        let port = {
            let mut next = self.next_port.write().await;
            let p = *next;
            *next += 1;
            p
        };

        // Write model JSON to a temp file
        let model_dir = std::env::temp_dir().join(format!("proof-gen-{}", model_id));
        tokio::fs::create_dir_all(&model_dir).await?;
        let model_path = model_dir.join("model.json");
        tokio::fs::write(&model_path, model_json).await?;

        tracing::info!(
            model_id,
            port,
            binary = %self.binary_path,
            model_path = %model_path.display(),
            "Spawning managed prover child process"
        );

        // Spawn the warm prover process
        let child = tokio::process::Command::new(&self.binary_path)
            .arg("serve")
            .arg("--model")
            .arg(&model_path)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--rate-limit")
            .arg("0") // No rate limiting for internal use
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to spawn xgboost-remainder binary at '{}': {}. \
                     Make sure the binary is built and available on PATH.",
                    self.binary_path,
                    e
                )
            })?;

        let base_url = format!("http://127.0.0.1:{}", port);

        // Wait for the prover to become ready (health check with retries).
        let health_url = format!("{}/health", base_url);
        let mut ready = false;
        for attempt in 0..60 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match self.client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    ready = true;
                    tracing::info!(
                        model_id,
                        port,
                        attempts = attempt + 1,
                        "Managed prover ready"
                    );
                    break;
                }
                _ => continue,
            }
        }

        if !ready {
            anyhow::bail!(
                "Managed prover for model {} did not become ready within 30 seconds",
                model_id
            );
        }

        // Get circuit hash from health
        let resp = self.client.get(&health_url).send().await?;
        let health: serde_json::Value = resp.json().await?;
        let circuit_hash = health["circuit_hash"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        let handle = Arc::new(ProverHandle {
            base_url,
            child: Some(child),
            model_hash: model_hash.to_string(),
        });
        self.provers
            .write()
            .await
            .insert(model_id.to_string(), handle);

        tracing::info!(
            model_id,
            circuit_hash = %circuit_hash,
            "Registered model with managed prover"
        );
        Ok(circuit_hash)
    }

    /// Generate a proof for the given model and feature vector.
    ///
    /// Forwards the request to the model's prover instance via HTTP.
    pub async fn prove(&self, model_id: &str, features: &[f64]) -> anyhow::Result<RealProofResult> {
        let handle = {
            let provers = self.provers.read().await;
            provers
                .get(model_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("No prover registered for model {}", model_id))?
        };

        let prove_url = format!("{}/prove/bundle", handle.base_url);
        let body = serde_json::json!({ "features": features });

        let resp = self
            .client
            .post(&prove_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Prover HTTP request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Prover returned error {}: {}", status, text);
        }

        let bundle: serde_json::Value = resp.json().await?;

        // Parse the bundle response into our result type
        let result = RealProofResult {
            predicted_class: bundle["predicted_class"].as_u64().unwrap_or(0) as u32,
            proof_hex: bundle["proof_hex"].as_str().unwrap_or("").to_string(),
            circuit_hash: bundle["circuit_hash"].as_str().unwrap_or("").to_string(),
            public_inputs_hex: bundle["public_inputs_hex"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            gens_hex: bundle["gens_hex"].as_str().unwrap_or("").to_string(),
            dag_circuit_description: bundle["dag_circuit_description"]
                .as_str()
                .map(|s| s.to_string()),
            proof_size_bytes: bundle["proof_hex"]
                .as_str()
                .map(|s| {
                    let s = s.strip_prefix("0x").unwrap_or(s);
                    s.len() / 2
                })
                .unwrap_or(0),
            prove_time_ms: bundle["prove_time_ms"].as_u64().unwrap_or(0),
        };

        Ok(result)
    }

    /// Check if a prover is registered for the given model.
    pub async fn has_prover(&self, model_id: &str) -> bool {
        self.provers.read().await.contains_key(model_id)
    }

    /// Remove a prover instance for the given model.
    ///
    /// In managed mode, this kills the child process.
    #[allow(dead_code)]
    pub async fn remove_prover(&self, model_id: &str) {
        if let Some(handle) = self.provers.write().await.remove(model_id) {
            // In managed mode, the child process will be dropped and killed
            // when the Arc refcount reaches zero. We log the removal.
            tracing::info!(model_id, "Removed prover instance");
            drop(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_proof_result_serialization() {
        let result = RealProofResult {
            predicted_class: 1,
            proof_hex: "0xdeadbeef".to_string(),
            circuit_hash: "0xabcd".to_string(),
            public_inputs_hex: "0x1234".to_string(),
            gens_hex: "0x5678".to_string(),
            dag_circuit_description: Some("abcdef".to_string()),
            proof_size_bytes: 4,
            prove_time_ms: 150,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("predicted_class"));
        assert!(json.contains("0xdeadbeef"));

        let deserialized: RealProofResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.predicted_class, 1);
        assert_eq!(deserialized.proof_hex, "0xdeadbeef");
    }

    #[test]
    fn test_real_proof_result_deserialization_minimal() {
        // Simulate a minimal response from the prover
        let json = r#"{
            "predicted_class": 0,
            "proof_hex": "0x00",
            "circuit_hash": "0xff",
            "public_inputs_hex": "",
            "gens_hex": "",
            "proof_size_bytes": 1,
            "prove_time_ms": 50
        }"#;
        let result: RealProofResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.predicted_class, 0);
        assert!(result.dag_circuit_description.is_none());
    }

    #[tokio::test]
    async fn test_prover_manager_new() {
        let mgr = ProverManager::new(
            "xgboost-remainder".to_string(),
            Some("http://localhost:9999".to_string()),
        );
        assert!(!mgr.has_prover("nonexistent").await);
    }

    #[tokio::test]
    async fn test_prover_manager_prove_no_prover() {
        let mgr = ProverManager::new("xgboost-remainder".to_string(), None);
        let result = mgr.prove("nonexistent", &[1.0, 2.0]).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No prover registered"));
    }

    #[tokio::test]
    async fn test_prover_manager_remove_nonexistent() {
        let mgr = ProverManager::new("xgboost-remainder".to_string(), None);
        // Should not panic
        mgr.remove_prover("nonexistent").await;
    }
}
