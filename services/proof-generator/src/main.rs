//! Production proof generation service.
//!
//! Pre-loads models at startup, generates proofs on demand. Supports model
//! upload with format auto-detection, model listing, and proof generation.
//!
//! In `--mock` mode, proof generation returns deterministic mock responses
//! so the service can be tested without the real prover (which requires the
//! `remainder_ce` crate and heavy circuit construction).
//!
//! # Endpoints
//!
//! - `POST /prove`                    -- generate a proof for given model + features
//! - `POST /models`                   -- upload a new model version (JSON body)
//! - `GET  /models`                   -- list all registered models
//! - `GET  /models/:id`               -- get a single model's metadata + version info
//! - `DELETE /models/:id`             -- deactivate a model version (soft delete)
//! - `POST /models/:id/activate`      -- activate a specific model version
//! - `POST /models/:id/deactivate`    -- deactivate a specific model version
//! - `GET  /models/:name/versions`    -- list all versions for a model name
//! - `POST /models/:name/rollback`    -- rollback to the previous version
//! - `GET  /health`                   -- health check

mod batch;
mod jobs;
mod model_registry;
mod models;
mod prover;
mod registry_client;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use clap::Parser;
use jobs::JobStore;
use model_registry::{ModelRegistry, VersionInfo};
use models::{ModelMetadata, ModelStore};
use prover::ProverManager;
use registry_client::RegistryClient;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

/// Proof generation service CLI.
#[derive(Parser)]
#[command(name = "proof-generator")]
struct Cli {
    /// Host to bind to.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 3100)]
    port: u16,

    /// Directory for model storage on disk.
    #[arg(long, default_value = "./data/models")]
    model_dir: String,

    /// Run in mock mode (no real proof generation).
    #[arg(long)]
    mock: bool,

    /// Enable CORS for all origins.
    #[arg(long)]
    enable_cors: bool,

    /// Path to the `xgboost-remainder` binary for managed prover mode.
    ///
    /// In non-mock mode, this binary is spawned as a child process for each
    /// registered model. The child runs a warm prover server that the
    /// proof-generator communicates with over HTTP.
    #[arg(long, default_value = "xgboost-remainder")]
    prover_binary: String,

    /// URL of an external warm prover instance.
    ///
    /// When set, all proof requests are forwarded to this URL instead of
    /// spawning managed child processes. Useful when the prover runs as a
    /// separate deployment.
    #[arg(long)]
    prover_url: Option<String>,
}

/// Shared application state.
struct AppState {
    model_store: ModelStore,
    /// Model registry with versioning and activation controls.
    registry: ModelRegistry,
    /// When true, proof requests return mock responses.
    mock_mode: bool,
    /// Real prover manager (used when mock_mode is false).
    prover_manager: Option<ProverManager>,
    /// Optional client for auto-submitting proofs to the proof-registry.
    /// Configured via the `REGISTRY_URL` environment variable.
    registry_client: Option<RegistryClient>,
    /// In-memory store for async proof jobs.
    job_store: Arc<JobStore>,
    start_time: Instant,
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for `POST /prove`.
#[derive(Debug, Deserialize)]
struct ProveRequest {
    /// ID of a previously uploaded model.
    model_id: String,
    /// Feature vector for inference.
    features: Vec<f64>,
}

/// Response body for `POST /prove`.
#[derive(Debug, Serialize, Deserialize)]
struct ProveResponse {
    /// Unique proof identifier.
    proof_id: String,
    /// Model hash (SHA-256 of model JSON).
    model_hash: String,
    /// Circuit hash (deterministic from model).
    circuit_hash: String,
    /// Predicted output label.
    output: String,
    /// Whether this is a mock proof.
    mock: bool,
    /// Proving time in milliseconds.
    prove_time_ms: u64,
    /// The proof bundle (hex-encoded bytes in production, placeholder in mock).
    proof_bundle: ProofBundle,
    /// Proof ID assigned by the proof-registry, if auto-submit is configured.
    /// `None` when `REGISTRY_URL` is not set or submission failed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    registry_proof_id: Option<String>,
}

/// Proof bundle data returned inside a prove response.
#[derive(Debug, Serialize, Deserialize)]
struct ProofBundle {
    /// Hex-encoded proof bytes.
    proof_hex: String,
    /// Hex-encoded public inputs.
    public_inputs_hex: String,
    /// Proof size in bytes.
    proof_size_bytes: usize,
}

/// Request body for `POST /models`.
#[derive(Debug, Deserialize)]
struct UploadModelRequest {
    /// Human-readable model name.
    name: String,
    /// Raw model JSON (the full model file contents).
    model_json: serde_json::Value,
    /// Optional format hint: "auto", "xgboost", "lightgbm", "random_forest",
    /// "logistic_regression". Defaults to "auto" if omitted.
    #[serde(default = "default_format")]
    format: String,
}

fn default_format() -> String {
    "auto".to_string()
}

/// Response body for `POST /models`.
#[derive(Debug, Serialize, Deserialize)]
struct UploadModelResponse {
    model_id: String,
    model_hash: String,
    circuit_hash: String,
    format: String,
    active: bool,
}

/// Response body for `GET /models`.
#[derive(Debug, Serialize, Deserialize)]
struct ListModelsResponse {
    models: Vec<ModelSummary>,
}

/// Summary of a single model.
#[derive(Debug, Serialize, Deserialize)]
struct ModelSummary {
    id: String,
    name: String,
    format: String,
    model_hash: String,
    circuit_hash: String,
    active: bool,
    created_at: String,
    file_size_bytes: u64,
}

/// Response body for `GET /health`.
#[derive(Debug, Serialize, Deserialize)]
struct HealthResponse {
    status: String,
    version: String,
    mock_mode: bool,
    /// Whether a real prover manager is available (false in mock mode).
    prover_available: bool,
    uptime_secs: u64,
    model_count: usize,
}

/// Response body for version listing endpoints.
#[derive(Debug, Serialize, Deserialize)]
struct ListVersionsResponse {
    name: String,
    versions: Vec<VersionSummary>,
}

/// Summary of a single model version.
#[derive(Debug, Serialize, Deserialize)]
struct VersionSummary {
    id: String,
    version: u32,
    created_at: String,
    activated_at: Option<String>,
    is_active: bool,
    model_hash: String,
    circuit_hash: String,
    format: String,
    file_size_bytes: u64,
}

/// Standard error response body.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /health`
async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let model_count = state.model_store.list_models().await.len();
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        mock_mode: state.mock_mode,
        prover_available: state.prover_manager.is_some(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        model_count,
    })
}

/// `POST /models` -- upload a new model version.
///
/// If a model with the same name already exists, a new version is created.
/// The new version is automatically activated and any previous active version
/// of the same name is deactivated.
async fn upload_model_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UploadModelRequest>,
) -> Result<(StatusCode, Json<UploadModelResponse>), (StatusCode, Json<ErrorResponse>)> {
    let raw_json = serde_json::to_string(&req.model_json).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Failed to serialize model JSON: {}", e),
            }),
        )
    })?;

    let format_hint = if req.format == "auto" {
        None
    } else {
        Some(req.format.clone())
    };

    let version_info = state
        .registry
        .upload(req.name, raw_json, format_hint)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("identical model hash") {
                (StatusCode::CONFLICT, Json(ErrorResponse { error: msg }))
            } else {
                (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
            }
        })?;

    Ok((
        StatusCode::CREATED,
        Json(UploadModelResponse {
            model_id: version_info.id,
            model_hash: version_info.model_hash,
            circuit_hash: version_info.circuit_hash,
            format: version_info.format,
            active: version_info.is_active,
        }),
    ))
}

/// `GET /models` -- list all models.
async fn list_models_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let models: Vec<ModelSummary> = state
        .model_store
        .list_models()
        .await
        .into_iter()
        .map(|m: ModelMetadata| ModelSummary {
            id: m.id,
            name: m.name,
            format: m.format,
            model_hash: m.model_hash,
            circuit_hash: m.circuit_hash,
            active: m.active,
            created_at: m.created_at,
            file_size_bytes: m.file_size_bytes,
        })
        .collect();

    Json(ListModelsResponse { models })
}

/// `GET /models/:id` -- get a single model's metadata.
async fn get_model_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ModelSummary>, (StatusCode, Json<ErrorResponse>)> {
    let model = state.model_store.get_model(&id).await.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Model not found: {}", id),
            }),
        )
    })?;

    Ok(Json(ModelSummary {
        id: model.id,
        name: model.name,
        format: model.format,
        model_hash: model.model_hash,
        circuit_hash: model.circuit_hash,
        active: model.active,
        created_at: model.created_at,
        file_size_bytes: model.raw_json.len() as u64,
    }))
}

/// `DELETE /models/:id` -- deactivate a model (soft delete).
async fn deactivate_model_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    state.registry.deactivate(&id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /models/:id/activate` -- activate a specific model version.
///
/// Deactivates any other active version of the same model name.
async fn activate_model_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VersionSummary>, (StatusCode, Json<ErrorResponse>)> {
    let info = state.registry.activate(&id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(version_info_to_summary(&info)))
}

/// `POST /models/:id/deactivate` -- deactivate a specific model version.
async fn deactivate_version_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VersionSummary>, (StatusCode, Json<ErrorResponse>)> {
    let info = state.registry.deactivate(&id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(version_info_to_summary(&info)))
}

/// `GET /models/:name/versions` -- list all versions for a model name.
async fn list_versions_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ListVersionsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let versions = state.registry.list_versions(&name).await.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Model name not found: {}", name),
            }),
        )
    })?;

    Ok(Json(ListVersionsResponse {
        name,
        versions: versions.iter().map(version_info_to_summary).collect(),
    }))
}

/// `POST /models/:name/rollback` -- rollback to the previous version.
async fn rollback_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<VersionSummary>, (StatusCode, Json<ErrorResponse>)> {
    let info = state.registry.rollback(&name).await.map_err(|e| {
        let msg = e.to_string();
        let status = if msg.contains("not found") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        };
        (status, Json(ErrorResponse { error: msg }))
    })?;

    Ok(Json(version_info_to_summary(&info)))
}

/// Convert a `VersionInfo` to a `VersionSummary` for API responses.
fn version_info_to_summary(info: &VersionInfo) -> VersionSummary {
    VersionSummary {
        id: info.id.clone(),
        version: info.version,
        created_at: info.created_at.clone(),
        activated_at: info.activated_at.clone(),
        is_active: info.is_active,
        model_hash: info.model_hash.clone(),
        circuit_hash: info.circuit_hash.clone(),
        format: info.format.clone(),
        file_size_bytes: info.file_size_bytes,
    }
}

/// `POST /prove` -- generate a proof.
///
/// In mock mode, returns a deterministic mock proof. In production mode,
/// this is where the real prover would be invoked.
async fn prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProveRequest>,
) -> Result<Json<ProveResponse>, (StatusCode, Json<ErrorResponse>)> {
    let model = state
        .model_store
        .get_model(&req.model_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Model not found: {}", req.model_id),
                }),
            )
        })?;

    if !model.active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Model {} is deactivated", req.model_id),
            }),
        ));
    }

    if req.features.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Features vector must not be empty".to_string(),
            }),
        ));
    }

    let start = Instant::now();
    let proof_id = uuid::Uuid::new_v4().to_string();

    if state.mock_mode {
        let mock = generate_mock_proof(&model, &req.features);
        let prove_time_ms = start.elapsed().as_millis() as u64;

        // Auto-submit to registry if configured.
        let registry_proof_id =
            submit_to_registry(&state, &model, &mock.bundle, prove_time_ms).await;

        return Ok(Json(ProveResponse {
            proof_id,
            model_hash: model.model_hash,
            circuit_hash: model.circuit_hash,
            output: mock.output,
            mock: true,
            prove_time_ms,
            proof_bundle: mock.bundle,
            registry_proof_id,
        }));
    }

    // Real proving via the ProverManager.
    let prover_mgr = state.prover_manager.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Prover manager not initialized. Service misconfigured.".to_string(),
            }),
        )
    })?;

    // Ensure the model has a registered prover instance. If not, attempt
    // lazy registration (the model was loaded from disk at startup before
    // the prover manager existed, or was uploaded while the prover was
    // still starting).
    if !prover_mgr.has_prover(&req.model_id).await {
        // Auto-register: read the raw JSON from the model store and spin up
        // a prover instance for this model.
        if let Err(e) = prover_mgr
            .register_model(&req.model_id, &model.raw_json, &model.model_hash)
            .await
        {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: format!(
                        "Failed to start prover for model {}: {}. \
                         Is the xgboost-remainder binary available?",
                        req.model_id, e
                    ),
                }),
            ));
        }
    }

    let result = prover_mgr
        .prove(&req.model_id, &req.features)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Proof generation failed: {}", e),
                }),
            )
        })?;

    let prove_time_ms = start.elapsed().as_millis() as u64;

    // Determine output label from predicted class
    let output = if result.predicted_class > 0 {
        "approved".to_string()
    } else {
        "denied".to_string()
    };

    let bundle = ProofBundle {
        proof_hex: result.proof_hex,
        public_inputs_hex: result.public_inputs_hex,
        proof_size_bytes: result.proof_size_bytes,
    };

    // Auto-submit to registry if configured.
    let registry_proof_id = submit_to_registry(&state, &model, &bundle, prove_time_ms).await;

    Ok(Json(ProveResponse {
        proof_id,
        model_hash: model.model_hash,
        circuit_hash: result.circuit_hash.clone(),
        output,
        mock: false,
        prove_time_ms,
        proof_bundle: bundle,
        registry_proof_id,
    }))
}

/// `POST /v1/prove/batch` -- generate proofs for a batch of inputs.
///
/// Accepts an array of feature vectors and proves them all with configurable
/// concurrency. Each individual failure does not block the rest of the batch.
async fn batch_prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<batch::BatchProveRequest>,
) -> Result<Json<batch::BatchProveResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate request structure.
    if let Err(msg) = batch::validate_batch_request(&req) {
        return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg })));
    }

    // Verify model exists and is active.
    let model = state
        .model_store
        .get_model(&req.model_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Model not found: {}", req.model_id),
                }),
            )
        })?;

    if !model.active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Model {} is deactivated", req.model_id),
            }),
        ));
    }

    let batch_id = uuid::Uuid::new_v4().to_string();
    let total = req.inputs.len();
    let concurrency = req.effective_concurrency();

    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let model_arc = Arc::new(model);
    let state_arc = state.clone();

    // Spawn a task for each input, governed by the semaphore.
    let mut handles = Vec::with_capacity(total);
    for (index, features) in req.inputs.into_iter().enumerate() {
        let sem = semaphore.clone();
        let model_ref = model_arc.clone();
        let st = state_arc.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            prove_single(index, &features, &model_ref, &st).await
        });
        handles.push(handle);
    }

    // Collect results in order.
    let mut results = Vec::with_capacity(total);
    let mut completed = 0usize;
    let mut failed = 0usize;
    for handle in handles {
        match handle.await {
            Ok(result) => {
                if result.error.is_some() {
                    failed += 1;
                } else {
                    completed += 1;
                }
                results.push(result);
            }
            Err(e) => {
                // Task panicked or was cancelled.
                failed += 1;
                results.push(batch::BatchProveResult {
                    index: results.len(),
                    proof_id: None,
                    output: None,
                    error: Some(format!("Internal error: {}", e)),
                });
            }
        }
    }

    Ok(Json(batch::BatchProveResponse {
        batch_id,
        total,
        completed,
        failed,
        results,
    }))
}

/// Prove a single input within a batch. Returns a `BatchProveResult` with
/// either the proof data or an error message (never panics).
async fn prove_single(
    index: usize,
    features: &[f64],
    model: &models::LoadedModel,
    state: &AppState,
) -> batch::BatchProveResult {
    let start = Instant::now();
    let proof_id = uuid::Uuid::new_v4().to_string();

    if state.mock_mode {
        let mock = generate_mock_proof(model, features);
        let _prove_time_ms = start.elapsed().as_millis() as u64;
        return batch::BatchProveResult {
            index,
            proof_id: Some(proof_id),
            output: Some(mock.output),
            error: None,
        };
    }

    // Real proving via the ProverManager.
    let prover_mgr = match state.prover_manager.as_ref() {
        Some(mgr) => mgr,
        None => {
            return batch::BatchProveResult {
                index,
                proof_id: None,
                output: None,
                error: Some("Prover manager not initialized".to_string()),
            };
        }
    };

    // Ensure prover is registered for this model.
    if !prover_mgr.has_prover(&model.id).await {
        if let Err(e) = prover_mgr
            .register_model(&model.id, &model.raw_json, &model.model_hash)
            .await
        {
            return batch::BatchProveResult {
                index,
                proof_id: None,
                output: None,
                error: Some(format!("Failed to start prover: {}", e)),
            };
        }
    }

    match prover_mgr.prove(&model.id, features).await {
        Ok(result) => {
            let output = if result.predicted_class > 0 {
                "approved".to_string()
            } else {
                "denied".to_string()
            };
            batch::BatchProveResult {
                index,
                proof_id: Some(proof_id),
                output: Some(output),
                error: None,
            }
        }
        Err(e) => batch::BatchProveResult {
            index,
            proof_id: None,
            output: None,
            error: Some(format!("Proof generation failed: {}", e)),
        },
    }
}

/// Submit a proof bundle to the proof-registry if `REGISTRY_URL` is configured.
///
/// Returns `Some(proof_id)` on success, `None` if the registry is not
/// configured or submission failed (failures are logged but do not block
/// the prove response).
async fn submit_to_registry(
    state: &AppState,
    model: &models::LoadedModel,
    bundle: &ProofBundle,
    prove_time_ms: u64,
) -> Option<String> {
    let client = state.registry_client.as_ref()?;

    // Build the registry submission payload. We include the proof bundle
    // fields along with model metadata so the registry has full context.
    let payload = serde_json::json!({
        "proof_hex": bundle.proof_hex,
        "public_inputs_hex": bundle.public_inputs_hex,
        "model_hash": model.model_hash,
        "circuit_hash": model.circuit_hash,
        "prover_version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().timestamp(),
    });

    match client.submit_proof(&payload).await {
        Ok(result) => {
            tracing::info!(
                registry_proof_id = %result.id,
                registry_url = %client.url(),
                prove_time_ms,
                "Auto-submitted proof to registry"
            );
            Some(result.id)
        }
        Err(e) => {
            tracing::warn!(
                registry_url = %client.url(),
                error = %e,
                "Failed to auto-submit proof to registry (non-fatal)"
            );
            None
        }
    }
}

/// Mock proof output.
struct MockProof {
    output: String,
    bundle: ProofBundle,
}

/// Generate a deterministic mock proof from model + features.
///
/// The mock proof is a SHA-256 hash of (model_hash || features), which makes
/// it deterministic and testable. The "output" is derived from the feature
/// sum: positive sum -> "approved", non-positive -> "denied".
fn generate_mock_proof(model: &models::LoadedModel, features: &[f64]) -> MockProof {
    let feature_sum: f64 = features.iter().sum();
    let output = if feature_sum > 0.0 {
        "approved"
    } else {
        "denied"
    };

    // Deterministic mock proof bytes
    let mut hasher = Sha256::new();
    hasher.update(model.model_hash.as_bytes());
    for f in features {
        hasher.update(f.to_le_bytes());
    }
    let proof_bytes = hasher.finalize();
    let proof_hex = format!("0x{}", hex::encode(proof_bytes));

    // Mock public inputs: hash of features
    let mut pi_hasher = Sha256::new();
    for f in features {
        pi_hasher.update(f.to_le_bytes());
    }
    let pi_bytes = pi_hasher.finalize();
    let public_inputs_hex = format!("0x{}", hex::encode(pi_bytes));

    MockProof {
        output: output.to_string(),
        bundle: ProofBundle {
            proof_hex,
            public_inputs_hex,
            proof_size_bytes: 32,
        },
    }
}

// ---------------------------------------------------------------------------
// Async prove types and handlers
// ---------------------------------------------------------------------------

/// Request body for `POST /v1/prove/async` (same shape as sync prove).
#[derive(Debug, Deserialize)]
struct AsyncProveRequest {
    /// ID of a previously uploaded model.
    model_id: String,
    /// Feature vector for inference.
    features: Vec<f64>,
}

/// Response body for `POST /v1/prove/async`.
#[derive(Debug, Serialize, Deserialize)]
struct AsyncProveResponse {
    /// Job ID to poll for status.
    job_id: String,
    /// Initial status (always "pending").
    status: String,
}

/// Query parameters for `GET /v1/jobs`.
#[derive(Debug, Deserialize)]
struct ListJobsQuery {
    /// Maximum number of jobs to return. Defaults to 20.
    #[serde(default = "default_job_limit")]
    limit: usize,
}

fn default_job_limit() -> usize {
    20
}

/// Response body for `GET /v1/jobs`.
#[derive(Debug, Serialize, Deserialize)]
struct ListJobsResponse {
    jobs: Vec<jobs::Job>,
}

/// `POST /v1/prove/async` -- submit an async proof request.
///
/// Returns immediately with a job ID. The proof is generated in a background
/// task. Poll `GET /v1/jobs/{id}` for status.
async fn async_prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AsyncProveRequest>,
) -> Result<(StatusCode, Json<AsyncProveResponse>), (StatusCode, Json<ErrorResponse>)> {
    // Validate: model exists and is active.
    let model = state
        .model_store
        .get_model(&req.model_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Model not found: {}", req.model_id),
                }),
            )
        })?;

    if !model.active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Model {} is deactivated", req.model_id),
            }),
        ));
    }

    if req.features.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Features vector must not be empty".to_string(),
            }),
        ));
    }

    // Create the job.
    let job_id = state.job_store.create(&req.model_id).await;

    // Spawn background proving task.
    let state_bg = state.clone();
    let job_id_bg = job_id.clone();
    let features = req.features;
    let model_id = req.model_id.clone();

    tokio::spawn(async move {
        // Transition to proving.
        state_bg
            .job_store
            .update_status(&job_id_bg, jobs::JobStatus::Proving)
            .await;

        // Run the proof (reuse the same logic as the sync handler).
        let result = run_prove(&state_bg, &model_id, &features).await;

        match result {
            Ok((proof_id, output)) => {
                state_bg
                    .job_store
                    .complete(&job_id_bg, &proof_id, &output)
                    .await;
            }
            Err(err_msg) => {
                state_bg.job_store.fail(&job_id_bg, &err_msg).await;
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(AsyncProveResponse {
            job_id,
            status: "pending".to_string(),
        }),
    ))
}

/// `GET /v1/jobs/{id}` -- get job status.
async fn get_job_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<jobs::Job>, (StatusCode, Json<ErrorResponse>)> {
    state.job_store.get(&id).await.map(Json).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Job not found: {}", id),
            }),
        )
    })
}

/// `GET /v1/jobs` -- list recent jobs.
async fn list_jobs_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListJobsQuery>,
) -> Json<ListJobsResponse> {
    let limit = params.limit.min(100); // Cap at 100
    let jobs = state.job_store.list(limit).await;
    Json(ListJobsResponse { jobs })
}

/// Core prove logic shared by the sync handler and async background task.
///
/// Returns `Ok((proof_id, output))` on success, or `Err(error_message)` on failure.
async fn run_prove(
    state: &AppState,
    model_id: &str,
    features: &[f64],
) -> Result<(String, String), String> {
    let model = state
        .model_store
        .get_model(model_id)
        .await
        .ok_or_else(|| format!("Model not found: {}", model_id))?;

    let start = Instant::now();
    let proof_id = uuid::Uuid::new_v4().to_string();

    if state.mock_mode {
        let mock = generate_mock_proof(&model, features);
        let prove_time_ms = start.elapsed().as_millis() as u64;
        // Auto-submit to registry if configured.
        let _registry_proof_id =
            submit_to_registry(state, &model, &mock.bundle, prove_time_ms).await;
        return Ok((proof_id, mock.output));
    }

    // Real proving via the ProverManager.
    let prover_mgr = state
        .prover_manager
        .as_ref()
        .ok_or_else(|| "Prover manager not initialized. Service misconfigured.".to_string())?;

    // Lazy registration.
    if !prover_mgr.has_prover(model_id).await {
        prover_mgr
            .register_model(model_id, &model.raw_json, &model.model_hash)
            .await
            .map_err(|e| format!("Failed to start prover for model {}: {}", model_id, e))?;
    }

    let result = prover_mgr
        .prove(model_id, features)
        .await
        .map_err(|e| format!("Proof generation failed: {}", e))?;

    let prove_time_ms = start.elapsed().as_millis() as u64;
    let output = if result.predicted_class > 0 {
        "approved".to_string()
    } else {
        "denied".to_string()
    };

    let bundle = ProofBundle {
        proof_hex: result.proof_hex,
        public_inputs_hex: result.public_inputs_hex,
        proof_size_bytes: result.proof_size_bytes,
    };

    let _registry_proof_id = submit_to_registry(state, &model, &bundle, prove_time_ms).await;

    Ok((proof_id, output))
}

// ---------------------------------------------------------------------------
// Router construction (used by both main and tests)
// ---------------------------------------------------------------------------

/// Build the application router with all routes.
fn build_router(state: Arc<AppState>, enable_cors: bool) -> Router {
    let mut app = Router::new()
        .route("/health", get(health_handler))
        .route("/models", post(upload_model_handler))
        .route("/models", get(list_models_handler))
        .route("/models/:id", get(get_model_handler))
        .route("/models/:id", delete(deactivate_model_handler))
        // Versioning endpoints
        .route("/models/:id/activate", post(activate_model_handler))
        .route("/models/:id/deactivate", post(deactivate_version_handler))
        .route("/models/:name/versions", get(list_versions_handler))
        .route("/models/:name/rollback", post(rollback_handler))
        .route("/prove", post(prove_handler))
        .route("/v1/prove/batch", post(batch_prove_handler))
        .route("/v1/prove/async", post(async_prove_handler))
        .route("/v1/jobs", get(list_jobs_handler))
        .route("/v1/jobs/:id", get(get_job_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    if enable_cors {
        app = app.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }

    app
}

// ---------------------------------------------------------------------------
// Structured logging
// ---------------------------------------------------------------------------

/// Initialise the `tracing_subscriber` with structured JSON output.
///
/// - `LOG_FORMAT` (default `"json"`) selects between JSON and human-readable output.
/// - `RUST_LOG` / default `"info"` controls the log level.
fn init_tracing() {
    let format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "json".into());
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());

    match format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();

    tracing::info!("Starting proof-generator service (mock={})", cli.mock);

    let model_store = ModelStore::new(cli.model_dir.into()).await?;
    let model_count = model_store.list_models().await.len();
    let registry = ModelRegistry::new(model_store.clone()).await;

    // Initialize the prover manager (only when not in mock mode).
    let prover_manager = if cli.mock {
        None
    } else {
        let mgr = ProverManager::new(cli.prover_binary.clone(), cli.prover_url.clone());
        tracing::info!(
            binary = %cli.prover_binary,
            external_url = ?cli.prover_url,
            "Prover manager initialized (real proving enabled)"
        );
        Some(mgr)
    };

    // Initialize the registry client if REGISTRY_URL is set.
    let registry_client = std::env::var("REGISTRY_URL").ok().map(|url| {
        tracing::info!(registry_url = %url, "Auto-submit to proof-registry enabled");
        RegistryClient::new(url)
    });

    let state = Arc::new(AppState {
        model_store,
        registry,
        mock_mode: cli.mock,
        prover_manager,
        registry_client,
        job_store: Arc::new(JobStore::new()),
        start_time: Instant::now(),
    });

    let app = build_router(state, cli.enable_cors);

    let bind_addr = format!("{}:{}", cli.host, cli.port);
    tracing::info!("Listening on {} ({} models loaded)", bind_addr, model_count);
    if cli.mock {
        tracing::info!("Mock mode enabled: proof requests return deterministic mock responses");
    } else {
        tracing::info!(
            "Real proving mode enabled. Prover binary: {}, external URL: {:?}",
            cli.prover_binary,
            cli.prover_url
        );
    }

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Create a test app with mock mode and a temp model directory.
    async fn test_app() -> (Router, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let registry = ModelRegistry::new(store.clone()).await;
        let state = Arc::new(AppState {
            model_store: store,
            registry,
            mock_mode: true,
            prover_manager: None,
            registry_client: None,
            job_store: Arc::new(JobStore::new()),
            start_time: Instant::now(),
        });
        (build_router(state, false), tmp)
    }

    /// Create a test app with a mock registry server for auto-submit testing.
    async fn test_app_with_registry(registry_url: String) -> (Router, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf()).await.unwrap();
        let registry = ModelRegistry::new(store.clone()).await;
        let state = Arc::new(AppState {
            model_store: store,
            registry,
            mock_mode: true,
            prover_manager: None,
            registry_client: Some(RegistryClient::new(registry_url)),
            job_store: Arc::new(JobStore::new()),
            start_time: Instant::now(),
        });
        (build_router(state, false), tmp)
    }

    /// Helper to upload a model and return its ID.
    async fn upload_test_model(app: &Router) -> String {
        let body = serde_json::json!({
            "name": "test-credit-model",
            "model_json": {"learner": {"gradient_booster": {"model": {"trees": []}}}},
            "format": "auto"
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let upload_resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        upload_resp["model_id"].as_str().unwrap().to_string()
    }

    // ----- Health check -----

    #[tokio::test]
    async fn test_health_check() {
        let (app, _tmp) = test_app().await;

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let health: HealthResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(health.status, "ok");
        assert!(health.mock_mode);
        assert!(!health.prover_available);
        assert_eq!(health.model_count, 0);
    }

    // ----- Model upload -----

    #[tokio::test]
    async fn test_upload_model() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;
        assert!(!model_id.is_empty());
    }

    #[tokio::test]
    async fn test_upload_model_invalid_format() {
        let (app, _tmp) = test_app().await;

        let body = serde_json::json!({
            "name": "bad-format",
            "model_json": {"some_key": 42},
            "format": "pytorch"
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_upload_model_auto_detect() {
        let (app, _tmp) = test_app().await;

        let body = serde_json::json!({
            "name": "lightgbm-model",
            "model_json": {"tree_info": [{"tree_index": 0}]}
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let upload_resp: UploadModelResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(upload_resp.format, "lightgbm");
    }

    #[tokio::test]
    async fn test_upload_duplicate_model_rejected() {
        let (app, _tmp) = test_app().await;

        let body = serde_json::json!({
            "name": "dup-model",
            "model_json": {"learner": {"unique_key": "test_dup"}}
        });

        // First upload succeeds
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Second upload with same JSON fails
        let req2 = axum::http::Request::builder()
            .method("POST")
            .uri("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp2 = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    // ----- Model listing -----

    #[tokio::test]
    async fn test_list_models_empty() {
        let (app, _tmp) = test_app().await;

        let req = axum::http::Request::builder()
            .uri("/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: ListModelsResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert!(list.models.is_empty());
    }

    #[tokio::test]
    async fn test_list_models_after_upload() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let req = axum::http::Request::builder()
            .uri("/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: ListModelsResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(list.models.len(), 1);
        assert_eq!(list.models[0].id, model_id);
    }

    // ----- Get model -----

    #[tokio::test]
    async fn test_get_model_by_id() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let req = axum::http::Request::builder()
            .uri(&format!("/models/{}", model_id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let model: ModelSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(model.id, model_id);
        assert_eq!(model.name, "test-credit-model");
    }

    #[tokio::test]
    async fn test_get_model_not_found() {
        let (app, _tmp) = test_app().await;

        let req = axum::http::Request::builder()
            .uri("/models/nonexistent-id")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ----- Deactivate model -----

    #[tokio::test]
    async fn test_deactivate_model() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let req = axum::http::Request::builder()
            .method("DELETE")
            .uri(&format!("/models/{}", model_id))
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify deactivated via GET
        let req2 = axum::http::Request::builder()
            .uri(&format!("/models/{}", model_id))
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        let body_bytes = resp2.into_body().collect().await.unwrap().to_bytes();
        let model: ModelSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!model.active);
    }

    // ----- Prove (mock mode) -----

    #[tokio::test]
    async fn test_prove_mock_approved() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let body = serde_json::json!({
            "model_id": model_id,
            "features": [52000.0, 0.38, 710.0, 4.0]
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();

        assert!(prove_resp.mock);
        assert_eq!(prove_resp.output, "approved");
        assert!(!prove_resp.proof_id.is_empty());
        assert!(prove_resp.proof_bundle.proof_hex.starts_with("0x"));
        assert!(prove_resp.proof_bundle.public_inputs_hex.starts_with("0x"));
        assert!(prove_resp.model_hash.starts_with("0x"));
        // No registry configured, so registry_proof_id should be None.
        assert!(prove_resp.registry_proof_id.is_none());
    }

    #[tokio::test]
    async fn test_prove_mock_denied() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let body = serde_json::json!({
            "model_id": model_id,
            "features": [-10.0, -5.0, -3.0]
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(prove_resp.output, "denied");
    }

    #[tokio::test]
    async fn test_prove_model_not_found() {
        let (app, _tmp) = test_app().await;

        let body = serde_json::json!({
            "model_id": "nonexistent",
            "features": [1.0, 2.0]
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_prove_empty_features_rejected() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let body = serde_json::json!({
            "model_id": model_id,
            "features": []
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_prove_deactivated_model_rejected() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        // Deactivate
        let del_req = axum::http::Request::builder()
            .method("DELETE")
            .uri(&format!("/models/{}", model_id))
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(del_req).await.unwrap();

        // Try to prove
        let body = serde_json::json!({
            "model_id": model_id,
            "features": [1.0, 2.0]
        });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ----- Mock proof determinism -----

    #[tokio::test]
    async fn test_mock_proof_deterministic() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let features = vec![1.0, 2.0, 3.0];

        let mut proof_hexes = Vec::new();
        for _ in 0..2 {
            let body = serde_json::json!({
                "model_id": model_id,
                "features": features,
            });
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/prove")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();

            let resp = app.clone().oneshot(req).await.unwrap();
            let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();
            proof_hexes.push(prove_resp.proof_bundle.proof_hex);
        }

        assert_eq!(
            proof_hexes[0], proof_hexes[1],
            "Mock proof should be deterministic"
        );
    }

    // ----- Versioning integration tests -----

    /// Helper: upload a model with a specific name and unique JSON content.
    async fn upload_named_model(app: &Router, name: &str, unique_key: &str) -> String {
        let body = serde_json::json!({
            "name": name,
            "model_json": {"learner": {"gradient_booster": {"model": {"trees": [], "id": unique_key}}}},
            "format": "auto"
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let upload_resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        upload_resp["model_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn test_upload_same_model_creates_v1_and_v2() {
        let (app, _tmp) = test_app().await;

        let v1_id = upload_named_model(&app, "credit-model", "v1-content").await;
        let v2_id = upload_named_model(&app, "credit-model", "v2-content").await;

        assert_ne!(v1_id, v2_id);

        // List versions
        let req = axum::http::Request::builder()
            .uri("/models/credit-model/versions")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let versions: ListVersionsResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(versions.versions.len(), 2);
        assert_eq!(versions.versions[0].version, 1);
        assert_eq!(versions.versions[1].version, 2);

        // v1 is deactivated, v2 is active.
        assert!(!versions.versions[0].is_active);
        assert!(versions.versions[1].is_active);
    }

    #[tokio::test]
    async fn test_activate_v2_prove_uses_v2() {
        let (app, _tmp) = test_app().await;

        let v1_id = upload_named_model(&app, "prove-model", "p1").await;
        let v2_id = upload_named_model(&app, "prove-model", "p2").await;

        // v2 is active by default. Prove should work.
        let body = serde_json::json!({
            "model_id": v2_id,
            "features": [1.0, 2.0, 3.0]
        });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // v1 is deactivated — prove should fail.
        let body = serde_json::json!({
            "model_id": v1_id,
            "features": [1.0, 2.0, 3.0]
        });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_rollback_activates_v1() {
        let (app, _tmp) = test_app().await;

        let v1_id = upload_named_model(&app, "rollback-model", "rb1").await;
        let v2_id = upload_named_model(&app, "rollback-model", "rb2").await;

        // Rollback should activate v1.
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models/rollback-model/rollback")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rolled: VersionSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(rolled.id, v1_id);
        assert_eq!(rolled.version, 1);
        assert!(rolled.is_active);

        // v2 should now be deactivated.
        let req = axum::http::Request::builder()
            .uri(&format!("/models/{}", v2_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let model: ModelSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!model.active);

        // Prove with v1 should work now.
        let body = serde_json::json!({
            "model_id": v1_id,
            "features": [1.0, 2.0]
        });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_deactivated_model_rejects_prove() {
        let (app, _tmp) = test_app().await;
        let model_id = upload_named_model(&app, "deact-prove", "dp1").await;

        // Deactivate via POST /models/:id/deactivate
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(&format!("/models/{}/deactivate", model_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let info: VersionSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!info.is_active);

        // Prove should be rejected.
        let body = serde_json::json!({
            "model_id": model_id,
            "features": [1.0, 2.0]
        });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_activate_and_deactivate_via_endpoints() {
        let (app, _tmp) = test_app().await;

        let v1_id = upload_named_model(&app, "ad-model", "ad1").await;
        let v2_id = upload_named_model(&app, "ad-model", "ad2").await;

        // v2 is active. Activate v1.
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(&format!("/models/{}/activate", v1_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let info: VersionSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert!(info.is_active);
        assert_eq!(info.version, 1);

        // Check v2 is deactivated.
        let req = axum::http::Request::builder()
            .uri(&format!("/models/{}", v2_id))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let model: ModelSummary = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!model.active);
    }

    #[tokio::test]
    async fn test_list_versions_not_found() {
        let (app, _tmp) = test_app().await;

        let req = axum::http::Request::builder()
            .uri("/models/nonexistent-name/versions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_rollback_single_version_fails() {
        let (app, _tmp) = test_app().await;
        upload_named_model(&app, "single-ver", "sv1").await;

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/models/single-ver/rollback")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ----- Registry auto-submit tests -----

    /// Spin up a mock registry server that accepts POST /proofs and returns a
    /// canned response with a known proof ID.
    async fn start_mock_registry() -> (String, tokio::task::JoinHandle<()>) {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let mock_app = axum::Router::new().route(
            "/proofs",
            post(move |Json(body): Json<serde_json::Value>| {
                let count = call_count_clone.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    // Validate that expected fields are present.
                    assert!(body.get("proof_hex").is_some(), "missing proof_hex");
                    assert!(body.get("model_hash").is_some(), "missing model_hash");
                    (
                        StatusCode::CREATED,
                        Json(serde_json::json!({
                            "id": format!("registry-proof-{}", n),
                            "status": "stored",
                            "transparency_index": n,
                            "proof_hash": "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
                        })),
                    )
                }
            }),
        );

        // Bind to a random available port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            axum::serve(listener, mock_app).await.unwrap();
        });

        // Brief delay to let the server start.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (url, handle)
    }

    #[tokio::test]
    async fn test_prove_with_registry_auto_submit() {
        let (registry_url, _server_handle) = start_mock_registry().await;
        let (app, _tmp) = test_app_with_registry(registry_url).await;
        let model_id = upload_test_model(&app).await;

        let body = serde_json::json!({
            "model_id": model_id,
            "features": [52000.0, 0.38, 710.0, 4.0]
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();

        assert!(prove_resp.mock);
        assert_eq!(prove_resp.output, "approved");
        // Registry auto-submit should have succeeded.
        assert!(
            prove_resp.registry_proof_id.is_some(),
            "Expected registry_proof_id to be set"
        );
        assert!(prove_resp
            .registry_proof_id
            .as_ref()
            .unwrap()
            .starts_with("registry-proof-"));
    }

    #[tokio::test]
    async fn test_prove_with_registry_unreachable() {
        // Point at a port that is not listening.
        let (app, _tmp) = test_app_with_registry("http://127.0.0.1:19999".to_string()).await;
        let model_id = upload_test_model(&app).await;

        let body = serde_json::json!({
            "model_id": model_id,
            "features": [1.0, 2.0, 3.0]
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // Proof generation should still succeed even if registry is down.
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();

        assert!(prove_resp.mock);
        assert_eq!(prove_resp.output, "approved");
        // Registry was unreachable, so registry_proof_id should be None.
        assert!(
            prove_resp.registry_proof_id.is_none(),
            "Expected registry_proof_id to be None when registry is unreachable"
        );
    }

    #[tokio::test]
    async fn test_prove_without_registry_config() {
        // Standard test app has no registry configured.
        let (app, _tmp) = test_app().await;
        let model_id = upload_test_model(&app).await;

        let body = serde_json::json!({
            "model_id": model_id,
            "features": [1.0, 2.0]
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();

        // No registry configured -> field absent.
        assert!(prove_resp.registry_proof_id.is_none());
    }

    #[tokio::test]
    async fn test_prove_multiple_proofs_get_different_registry_ids() {
        let (registry_url, _server_handle) = start_mock_registry().await;
        let (app, _tmp) = test_app_with_registry(registry_url).await;
        let model_id = upload_test_model(&app).await;

        let mut registry_ids = Vec::new();
        for i in 0..3 {
            let body = serde_json::json!({
                "model_id": model_id,
                "features": [1.0 + i as f64, 2.0]
            });

            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/prove")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();

            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let prove_resp: ProveResponse = serde_json::from_slice(&body_bytes).unwrap();

            assert!(prove_resp.registry_proof_id.is_some());
            registry_ids.push(prove_resp.registry_proof_id.unwrap());
        }

        // Each proof should get a unique registry ID.
        assert_eq!(registry_ids[0], "registry-proof-0");
        assert_eq!(registry_ids[1], "registry-proof-1");
        assert_eq!(registry_ids[2], "registry-proof-2");
    }
}
