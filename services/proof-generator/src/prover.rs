//! Real proof generation via the xgboost-remainder prover.
//!
//! The `xgboost-remainder` crate is excluded from the workspace (it depends on
//! `remainder_ce` which has heavy build requirements). Instead of linking it as
//! a library, we communicate with it over HTTP:
//!
//! - **Managed mode** (default): On model registration the manager spawns an
//!   `xgboost-remainder --serve` child process bound to a free port. Proof
//!   requests are forwarded to that process via its `/prove/bundle` endpoint.
//!   The child is killed when its handle is dropped (`remove_prover`, or the
//!   manager itself); deactivating a model does not stop its prover. Nothing
//!   in the service calls `remove_prover`, and a generator that exits on a
//!   signal leaves its children running.
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
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

/// Number of health probes a managed prover gets to become ready, one every
/// 500 ms plus the time the probe itself takes.
const READY_PROBES: u32 = 60;

/// Time limit for one health probe of a managed prover.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Arguments that start `xgboost-remainder` as a warm prover HTTP server.
///
/// Server mode is the `--serve` flag of that CLI; it has no `serve` subcommand.
fn serve_args(model_path: &std::path::Path, port: u16) -> Vec<std::ffi::OsString> {
    vec![
        "--serve".into(),
        "--model".into(),
        model_path.into(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string().into(),
        "--rate-limit".into(),
        "0".into(), // No rate limiting for internal use
    ]
}

/// Parse a `/prove/bundle` response into our result type.
///
/// The proof, the circuit hash, the public inputs, the generators and the
/// predicted class are required; a response without one of them is an error.
fn parse_bundle(bundle: &serde_json::Value) -> anyhow::Result<RealProofResult> {
    let required_str = |field: &str| {
        bundle[field]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Prover response has no string field '{}'", field))
    };

    let proof_hex = required_str("proof_hex")?;
    let proof_size_bytes = proof_hex.strip_prefix("0x").unwrap_or(&proof_hex).len() / 2;
    if proof_size_bytes == 0 {
        anyhow::bail!("Prover response has an empty proof_hex");
    }
    let predicted_class = bundle["predicted_class"]
        .as_u64()
        .and_then(|class| u32::try_from(class).ok())
        .ok_or_else(|| anyhow::anyhow!("Prover response has no integer field 'predicted_class'"))?;

    Ok(RealProofResult {
        predicted_class,
        proof_hex,
        circuit_hash: required_str("circuit_hash")?,
        public_inputs_hex: required_str("public_inputs_hex")?,
        gens_hex: required_str("gens_hex")?,
        dag_circuit_description: bundle["dag_circuit_description"]
            .as_str()
            .map(|s| s.to_string()),
        proof_size_bytes,
        prove_time_ms: bundle["prove_time_ms"].as_u64().unwrap_or(0),
    })
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
/// In managed mode, each model gets its own `xgboost-remainder --serve` child
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
    /// Held while a prover is being registered (see `ensure_prover`).
    register_lock: Mutex<()>,
    /// Failed start attempts per model: how many, and the last error.
    failed_starts: std::sync::Mutex<HashMap<String, (u64, String)>>,
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
            register_lock: Mutex::new(()),
            failed_starts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Start a prover for the model unless one is already registered.
    ///
    /// Registration runs under a lock, so concurrent first requests for a
    /// model start one prover and the others wait for it. If that start
    /// fails, the requests that were waiting get the same error instead of
    /// each starting the prover again; a request that arrives later tries
    /// again.
    pub async fn ensure_prover(
        &self,
        model_id: &str,
        model_json: &str,
        model_hash: &str,
    ) -> anyhow::Result<()> {
        if self.has_prover(model_id).await {
            return Ok(());
        }
        let (failures_before, _) = self.failed_starts(model_id);
        let _registering = self.register_lock.lock().await;
        if self.has_prover(model_id).await {
            return Ok(());
        }
        let (failures_now, last_error) = self.failed_starts(model_id);
        if failures_now != failures_before {
            anyhow::bail!(last_error);
        }
        match self.register_model(model_id, model_json, model_hash).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let mut failed = self
                    .failed_starts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let entry = failed.entry(model_id.to_string()).or_default();
                *entry = (entry.0 + 1, format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// Number of failed start attempts for the model, and the last error.
    fn failed_starts(&self, model_id: &str) -> (u64, String) {
        self.failed_starts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(model_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Register a model and start its prover instance.
    ///
    /// In managed mode, this spawns an `xgboost-remainder --serve` child process
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

        // Spawn the warm prover process. It is killed when `child` is dropped,
        // which also covers every early return below.
        let mut child = tokio::process::Command::new(&self.binary_path)
            .args(serve_args(&model_path, port))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
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
        for attempt in 0..READY_PROBES {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Some(status) = child.try_wait()? {
                anyhow::bail!(
                    "Managed prover for model {} exited during startup: {}",
                    model_id,
                    status
                );
            }
            let probe = self
                .client
                .get(&health_url)
                .timeout(HEALTH_PROBE_TIMEOUT)
                .send();
            match probe.await {
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
                "Managed prover for model {} did not become ready after {} health probes",
                model_id,
                READY_PROBES
            );
        }

        // Get circuit hash from health
        let resp = self
            .client
            .get(&health_url)
            .timeout(HEALTH_PROBE_TIMEOUT)
            .send()
            .await?;
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
        parse_bundle(&bundle)
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
            // A managed child is spawned with `kill_on_drop`, so it is killed
            // once the last clone of the handle is dropped (a `prove` call in
            // flight holds one until it returns). We log the removal.
            tracing::info!(model_id, "Removed prover instance");
            drop(handle);
        }
    }
}

/// Stand-ins for the prover in tests: an HTTP server and a child process.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn complete_bundle() -> serde_json::Value {
        serde_json::json!({
            "proof_hex": "0xdeadbeef",
            "gens_hex": "0x01",
            "public_inputs_hex": "0x02",
            "dag_circuit_description": "00",
            "circuit_hash": "0xc1",
            "predicted_class": 1,
            "prove_time_ms": 7,
        })
    }

    /// Serve `/health` and `/prove/bundle` like a warm prover, answering every
    /// proof request with `bundle`. The first `unanswered_probes` health
    /// requests are accepted and never answered; after those, health is 503
    /// until `ready_file` (if given) exists. Returns the port.
    pub async fn stand_in_prover_http(
        bundle: serde_json::Value,
        unanswered_probes: usize,
        ready_file: Option<std::path::PathBuf>,
    ) -> u16 {
        use axum::http::StatusCode;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let probes = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .route(
                "/health",
                axum::routing::get(move || {
                    let probes = probes.clone();
                    let ready_file = ready_file.clone();
                    async move {
                        if probes.fetch_add(1, Ordering::SeqCst) < unanswered_probes {
                            std::future::pending::<()>().await;
                        }
                        if ready_file.is_some_and(|file| !file.exists()) {
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(Default::default()),
                            );
                        }
                        let health = serde_json::json!({"status": "ok", "circuit_hash": "c1"});
                        (StatusCode::OK, axum::Json(health))
                    }
                }),
            )
            .route(
                "/prove/bundle",
                axum::routing::post(move || {
                    let bundle = bundle.clone();
                    async move { axum::Json(bundle) }
                }),
            );
        loop {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            // The manager computes the port after the one it assigns.
            if port == u16::MAX {
                continue;
            }
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            return port;
        }
    }

    /// Held by every test that starts a child process: a child forked while
    /// another test has its script open for writing inherits that descriptor,
    /// and the script then cannot be executed ("text file busy").
    #[cfg(unix)]
    pub static SPAWNING: Mutex<()> = Mutex::const_new(());

    /// Script that logs a line per start, then a line every 100 ms for 30 s.
    #[cfg(unix)]
    pub const RUNS_FOR_30S: &str = r#"#!/bin/sh
echo started >> "$0.starts"
i=0
while [ $i -lt 300 ]; do
    echo alive >> "$0.alive"
    sleep 0.1
    i=$((i + 1))
done
"#;

    #[cfg(unix)]
    pub const EXITS_AT_ONCE: &str = "#!/bin/sh\nexit 3\n";

    /// Script that logs a line per start and then exits with status 3.
    #[cfg(unix)]
    pub const STARTS_THEN_EXITS: &str = "#!/bin/sh\necho started >> \"$0.starts\"\nexit 3\n";

    /// A shell script standing in for the prover binary, with
    /// `stand_in_prover_http` answering on its port once the script runs.
    #[cfg(unix)]
    pub struct StandIn {
        dir: tempfile::TempDir,
        /// The model the test registers; its temp copy is removed on drop.
        pub model_id: String,
    }

    #[cfg(unix)]
    impl StandIn {
        pub fn new(script: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let dir = tempfile::TempDir::new().unwrap();
            let stand_in = Self {
                dir,
                model_id: format!("test-{}", uuid::Uuid::new_v4()),
            };
            std::fs::write(stand_in.script(), script).unwrap();
            std::fs::set_permissions(stand_in.script(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            stand_in
        }

        fn script(&self) -> std::path::PathBuf {
            self.dir.path().join("prover.sh")
        }

        /// A file the script appends to: `starts` or `alive`.
        fn log(&self, suffix: &str) -> std::path::PathBuf {
            self.dir.path().join(format!("prover.sh.{suffix}"))
        }

        pub fn lines(&self, suffix: &str) -> usize {
            std::fs::read_to_string(self.log(suffix))
                .map(|log| log.lines().count())
                .unwrap_or(0)
        }

        /// A manager whose first child gets the port of a stand-in HTTP
        /// server. Its first `unanswered_probes` health requests get no
        /// answer; then it is ready as soon as the script has logged.
        pub async fn manager(&self, unanswered_probes: usize) -> ProverManager {
            let ready_file = Some(self.log("alive"));
            let port = stand_in_prover_http(complete_bundle(), unanswered_probes, ready_file).await;
            let mgr = ProverManager::new(self.script().display().to_string(), None);
            *mgr.next_port.write().await = port;
            mgr
        }
    }

    #[cfg(unix)]
    impl Drop for StandIn {
        fn drop(&mut self) {
            // The model copy `register_model` leaves in the temp directory.
            let copy = std::env::temp_dir().join(format!("proof-gen-{}", self.model_id));
            let _ = std::fs::remove_dir_all(copy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    /// The prover CLI (examples/xgboost-remainder/src/main.rs) takes `--serve`
    /// as a flag. A positional `serve` makes it exit with "unexpected argument",
    /// so the managed prover never came up.
    #[test]
    fn test_serve_args_use_serve_flag() {
        let args = serve_args(std::path::Path::new("/tmp/m/model.json"), 13100);
        let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--serve",
                "--model",
                "/tmp/m/model.json",
                "--host",
                "127.0.0.1",
                "--port",
                "13100",
                "--rate-limit",
                "0"
            ]
        );
    }

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

    #[test]
    fn test_parse_bundle_complete() {
        let result = parse_bundle(&complete_bundle()).unwrap();
        assert_eq!(result.predicted_class, 1);
        assert_eq!(result.proof_hex, "0xdeadbeef");
        assert_eq!(result.proof_size_bytes, 4);
        assert_eq!(result.circuit_hash, "0xc1");
        assert_eq!(result.public_inputs_hex, "0x02");
        assert_eq!(result.gens_hex, "0x01");
        assert_eq!(result.dag_circuit_description.as_deref(), Some("00"));
        assert_eq!(result.prove_time_ms, 7);
    }

    #[test]
    fn test_parse_bundle_requires_proof_fields() {
        for field in [
            "proof_hex",
            "gens_hex",
            "public_inputs_hex",
            "circuit_hash",
            "predicted_class",
        ] {
            let mut bundle = complete_bundle();
            bundle.as_object_mut().unwrap().remove(field);
            let err = parse_bundle(&bundle).expect_err(field).to_string();
            assert!(err.contains(field), "{field}: {err}");
        }

        let mut bundle = complete_bundle();
        bundle["proof_hex"] = "0x".into();
        assert!(parse_bundle(&bundle).is_err());

        // The fields the prover may leave out.
        let mut bundle = complete_bundle();
        bundle.as_object_mut().unwrap().remove("prove_time_ms");
        bundle
            .as_object_mut()
            .unwrap()
            .remove("dag_circuit_description");
        assert!(parse_bundle(&bundle).is_ok());
    }

    #[tokio::test]
    async fn test_prove_rejects_reply_without_proof_fields() {
        let port = stand_in_prover_http(serde_json::json!({}), 0, None).await;
        let mgr = ProverManager::new(
            "unused".to_string(),
            Some(format!("http://127.0.0.1:{port}")),
        );
        mgr.ensure_prover("m", "{}", "0x00").await.unwrap();

        let err = mgr.prove("m", &[1.0]).await.unwrap_err().to_string();
        assert!(err.contains("proof_hex"), "{err}");
    }

    #[tokio::test]
    async fn test_prove_returns_complete_reply() {
        let port = stand_in_prover_http(complete_bundle(), 0, None).await;
        let mgr = ProverManager::new(
            "unused".to_string(),
            Some(format!("http://127.0.0.1:{port}")),
        );
        mgr.ensure_prover("m", "{}", "0x00").await.unwrap();

        let result = mgr.prove("m", &[1.0]).await.unwrap();
        assert_eq!(result.proof_hex, "0xdeadbeef");
        assert_eq!(result.predicted_class, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_concurrent_first_requests_start_one_prover() {
        let _spawning = SPAWNING.lock().await;
        let stand_in = StandIn::new(RUNS_FOR_30S);
        let mgr = stand_in.manager(0).await;

        let id = stand_in.model_id.as_str();
        let first_requests = async {
            tokio::join!(
                mgr.ensure_prover(id, "{}", "0x00"),
                mgr.ensure_prover(id, "{}", "0x00"),
                mgr.ensure_prover(id, "{}", "0x00"),
                mgr.ensure_prover(id, "{}", "0x00"),
            )
        };
        let results = tokio::time::timeout(Duration::from_secs(10), first_requests).await;

        assert_eq!(stand_in.lines("starts"), 1, "provers started");
        let (a, b, c, d) = results.expect("registration did not finish");
        for result in [a, b, c, d] {
            result.unwrap();
        }
        assert!(mgr.has_prover(id).await);
    }

    /// Requests that were waiting while a start failed get that failure and
    /// do not start the prover again.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_waiting_requests_share_a_failed_start() {
        let _spawning = SPAWNING.lock().await;
        let stand_in = StandIn::new(STARTS_THEN_EXITS);
        let mgr = stand_in.manager(0).await;

        let id = stand_in.model_id.as_str();
        let first_requests = async {
            tokio::join!(
                mgr.ensure_prover(id, "{}", "0x00"),
                mgr.ensure_prover(id, "{}", "0x00"),
                mgr.ensure_prover(id, "{}", "0x00"),
            )
        };
        let (a, b, c) = tokio::time::timeout(Duration::from_secs(10), first_requests)
            .await
            .expect("still waiting for a start that has failed");

        for result in [a, b, c] {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("exited during startup"), "{err}");
        }
        assert_eq!(stand_in.lines("starts"), 1, "provers started");

        // A request that arrives after the failure tries again.
        assert!(mgr.ensure_prover(id, "{}", "0x00").await.is_err());
        assert_eq!(stand_in.lines("starts"), 2, "provers started");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_removed_prover_is_killed() {
        let _spawning = SPAWNING.lock().await;
        let stand_in = StandIn::new(RUNS_FOR_30S);
        let mgr = stand_in.manager(0).await;
        let id = stand_in.model_id.as_str();
        mgr.ensure_prover(id, "{}", "0x00").await.unwrap();

        mgr.remove_prover(id).await;

        // Give the kill time to land, then the child must have stopped logging.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let alive = stand_in.lines("alive");
        assert!(alive > 0);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(stand_in.lines("alive"), alive, "child is still running");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_registration_stops_when_child_has_exited() {
        let _spawning = SPAWNING.lock().await;
        let stand_in = StandIn::new(EXITS_AT_ONCE);
        let mgr = stand_in.manager(0).await;

        let id = stand_in.model_id.as_str();
        let result =
            tokio::time::timeout(Duration::from_secs(10), mgr.ensure_prover(id, "{}", "0x00"))
                .await
                .expect("still waiting for a child that has exited");

        let err = result.unwrap_err().to_string();
        assert!(err.contains("exited during startup"), "{err}");
        assert!(err.contains("exit status: 3"), "{err}");
        assert!(!mgr.has_prover(id).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_unanswered_health_probe_times_out() {
        let _spawning = SPAWNING.lock().await;
        let stand_in = StandIn::new(RUNS_FOR_30S);
        let mgr = stand_in.manager(1).await;

        // The first probe gets no answer; the ones after it do.
        let id = stand_in.model_id.as_str();
        tokio::time::timeout(Duration::from_secs(15), mgr.ensure_prover(id, "{}", "0x00"))
            .await
            .expect("an unanswered probe was not abandoned")
            .unwrap();
    }
}
