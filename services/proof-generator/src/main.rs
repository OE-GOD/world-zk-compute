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
//! - `POST /prove`        -- generate a proof for given model + features
//! - `POST /models`       -- upload a new model (JSON body)
//! - `GET  /models`       -- list all registered models
//! - `GET  /models/:id`   -- get a single model's metadata
//! - `DELETE /models/:id` -- deactivate a model (soft delete)
//! - `GET  /health`       -- health check

mod models;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use clap::Parser;
use models::{ModelMetadata, ModelStore};
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
}

/// Shared application state.
struct AppState {
    model_store: ModelStore,
    /// When true, proof requests return mock responses.
    mock_mode: bool,
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
    uptime_secs: u64,
    model_count: usize,
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
        uptime_secs: state.start_time.elapsed().as_secs(),
        model_count,
    })
}

/// `POST /models` -- upload a new model.
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

    let meta = state
        .model_store
        .add_model(req.name, raw_json, format_hint)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("identical hash") {
                (StatusCode::CONFLICT, Json(ErrorResponse { error: msg }))
            } else {
                (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg }))
            }
        })?;

    Ok((
        StatusCode::CREATED,
        Json(UploadModelResponse {
            model_id: meta.id,
            model_hash: meta.model_hash,
            circuit_hash: meta.circuit_hash,
            format: meta.format,
            active: meta.active,
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
    state.model_store.deactivate_model(&id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
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

        return Ok(Json(ProveResponse {
            proof_id,
            model_hash: model.model_hash,
            circuit_hash: model.circuit_hash,
            output: mock.output,
            mock: true,
            prove_time_ms,
            proof_bundle: mock.bundle,
        }));
    }

    // Real proving: not yet integrated.
    // When the real prover is plugged in, this block would:
    //   1. Parse the model JSON into an XgboostModel (or other format)
    //   2. Build or retrieve a CachedProver for this model
    //   3. Call prover.prove(&features, predicted_class)
    //   4. Return the real proof bytes
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            error: "Real proof generation not yet integrated. Use --mock for testing.".to_string(),
        }),
    ))
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
        .route("/prove", post(prove_handler))
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
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    tracing::info!("Starting proof-generator service (mock={})", cli.mock);

    let model_store = ModelStore::new(cli.model_dir.into()).await?;
    let model_count = model_store.list_models().await.len();

    let state = Arc::new(AppState {
        model_store,
        mock_mode: cli.mock,
        start_time: Instant::now(),
    });

    let app = build_router(state, cli.enable_cors);

    let bind_addr = format!("{}:{}", cli.host, cli.port);
    tracing::info!("Listening on {} ({} models loaded)", bind_addr, model_count);
    if cli.mock {
        tracing::info!("Mock mode enabled: proof requests return deterministic mock responses");
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
        let state = Arc::new(AppState {
            model_store: store,
            mock_mode: true,
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
}
