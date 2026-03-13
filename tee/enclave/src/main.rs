//! TEE Enclave Application for ML Inference (XGBoost / LightGBM).
//!
//! Runs inside a TEE enclave (AWS Nitro / Intel TDX). Accepts inference requests
//! via HTTP, runs tree ensemble prediction (XGBoost or LightGBM), and returns an
//! ECDSA-signed attestation compatible with TEEMLVerifier.sol.

mod attestation;
mod config;
mod metrics;
mod model;
mod nitro;
mod validation;
mod watchdog;

use std::sync::Arc;

use alloy_primitives::keccak256;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use attestation::Attestor;
use config::Config;
use metrics::{Metrics, MetricsSnapshot};
use model::XgboostModel;
use nitro::{AttestationDocument, NitroAttestor};
use watchdog::{DetailedHealth, Watchdog};

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

/// Mutable model-related data that can be swapped via hot-reload.
/// Protected by `std::sync::RwLock` for concurrent read access on the hot path
/// and exclusive write access during model reload.
struct ModelState {
    model: XgboostModel,
    model_hash: alloy_primitives::B256,
    #[allow(dead_code)]
    model_bytes: Vec<u8>,
    /// Human-readable model name (derived from the model file path).
    model_name: String,
    /// SHA-256 hex digest of the raw model bytes (for integrity verification).
    model_sha256: String,
}

/// Simple sliding-window rate limiter.
/// Tracks request timestamps in a VecDeque; on each check, removes entries
/// older than 1 minute and compares count against the configured max.
struct RateLimiter {
    window: std::sync::Mutex<std::collections::VecDeque<std::time::Instant>>,
    max_requests_per_minute: u64,
}

impl RateLimiter {
    fn new(max_requests_per_minute: u64) -> Self {
        Self {
            window: std::sync::Mutex::new(std::collections::VecDeque::new()),
            max_requests_per_minute,
        }
    }

    /// Check if a request is allowed. Returns Ok(()) if allowed,
    /// Err(retry_after_secs) if rate limited.
    ///
    /// If the internal mutex is poisoned (a previous holder panicked), the rate
    /// limiter fails open -- allowing the request rather than crashing the server.
    fn check(&self) -> Result<(), u64> {
        let mut window = match self.window.lock() {
            Ok(guard) => guard,
            Err(_poisoned) => {
                // Fail-open: if the mutex is poisoned, allow the request rather
                // than panicking the entire server.
                tracing::warn!("RateLimiter mutex poisoned -- failing open");
                return Ok(());
            }
        };
        let now = std::time::Instant::now();
        let one_minute_ago = now - std::time::Duration::from_secs(60);

        // Remove expired entries
        while window.front().is_some_and(|&t| t < one_minute_ago) {
            window.pop_front();
        }

        if window.len() as u64 >= self.max_requests_per_minute {
            // Calculate retry-after from the oldest entry in the window.
            // front() is guaranteed Some because len >= max_requests_per_minute > 0.
            if let Some(oldest) = window.front() {
                let retry_after = 60u64.saturating_sub(oldest.elapsed().as_secs());
                return Err(retry_after.max(1));
            }
            return Err(1);
        }

        window.push_back(now);
        Ok(())
    }
}

struct AppState {
    /// Model data protected by a RwLock for hot-reload support.
    /// Read locks are acquired on the inference hot path (fast, non-blocking
    /// for concurrent readers). Write lock is only acquired during model reload.
    model_state: std::sync::RwLock<ModelState>,
    attestor: Attestor,
    nitro_attestor: std::sync::Mutex<NitroAttestor>,
    enclave_address: alloy_primitives::Address,
    /// Metrics counters (lock-free atomics). Wrapped in `Arc` so the
    /// watchdog background task can hold a reference independently.
    metrics: Arc<Metrics>,
    /// Optional admin API key. If `None`, admin endpoints return 403.
    admin_api_key: Option<String>,
    /// Rate limiter for POST /infer endpoint.
    rate_limiter: RateLimiter,
    /// Optional watchdog for background health monitoring.
    /// `None` when WATCHDOG_ENABLED=false.
    watchdog: Option<Arc<Watchdog>>,
    /// Optional expected SHA-256 hash for model validation (from EXPECTED_MODEL_HASH env var).
    expected_model_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct InferRequest {
    features: Vec<f64>,
    /// Optional per-request chain_id override. If not provided, uses the
    /// enclave-wide CHAIN_ID env var (default: 1 for Ethereum mainnet).
    #[serde(default)]
    chain_id: Option<u64>,
}

#[derive(Serialize)]
struct InferResponse {
    /// Hex-encoded result bytes (the JSON-serialized prediction scores).
    result: String,
    /// 0x-prefixed hex keccak256 of the model file.
    model_hash: String,
    /// 0x-prefixed hex keccak256 of the JSON-serialized input features.
    input_hash: String,
    /// 0x-prefixed hex keccak256 of the result bytes.
    result_hash: String,
    /// 0x-prefixed hex 65-byte ECDSA signature.
    attestation: String,
    /// 0x-prefixed hex Ethereum address of the enclave signer.
    enclave_address: String,
    /// Chain ID used in the attestation signature (for replay protection).
    chain_id: u64,
    /// Monotonic nonce used in the attestation signature (for replay protection).
    nonce: u64,
    /// Unix timestamp (seconds) used in the attestation signature.
    timestamp: u64,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    model_loaded: bool,
    /// SHA-256 hex digest of the loaded model bytes.
    model_hash: String,
}

#[derive(Serialize)]
struct InfoResponse {
    enclave_address: String,
    model_hash: String,
}

#[derive(Deserialize)]
struct AttestationQuery {
    /// Optional hex-encoded nonce (with or without 0x prefix).
    nonce: Option<String>,
}

#[derive(Deserialize)]
struct ReloadModelRequest {
    /// Path to the new model file on disk.
    model_path: String,
    /// Optional model format override. Defaults to "auto".
    #[serde(default = "default_model_format_str")]
    model_format: String,
}

fn default_model_format_str() -> String {
    "auto".to_string()
}

#[derive(Serialize, Deserialize)]
struct ReloadModelResponse {
    success: bool,
    num_trees: usize,
    num_features: usize,
    model_hash: String,
    /// SHA-256 hex digest of the new model bytes.
    model_sha256: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `(500, message)` error tuple for poisoned Mutex / RwLock.
///
/// This is used throughout the axum handlers so that a poisoned lock returns
/// an HTTP 500 instead of panicking the server process.
fn lock_error(name: &str) -> (StatusCode, String) {
    tracing::error!("{} lock poisoned", name);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("internal lock error: {} poisoned", name),
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health(
    State(state): State<Arc<AppState>>,
) -> Result<Json<HealthResponse>, (StatusCode, String)> {
    let ms = state
        .model_state
        .read()
        .map_err(|_| lock_error("model_state"))?;
    Ok(Json(HealthResponse {
        status: "ok".to_string(),
        model_loaded: true,
        model_hash: ms.model_sha256.clone(),
    }))
}

async fn health_detailed(
    State(state): State<Arc<AppState>>,
) -> Json<DetailedHealth> {
    match &state.watchdog {
        Some(wd) => Json(wd.detailed_health()),
        None => {
            // Watchdog disabled -- compute a one-shot health evaluation.
            Json(Watchdog::evaluate_health(&state.metrics))
        }
    }
}

async fn info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<InfoResponse>, (StatusCode, String)> {
    let ms = state
        .model_state
        .read()
        .map_err(|_| lock_error("model_state"))?;
    Ok(Json(InfoResponse {
        enclave_address: format!("{}", state.attestor.address()),
        model_hash: format!("0x{}", hex::encode(ms.model_hash)),
    }))
}

async fn attestation(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<AttestationQuery>,
) -> Result<Json<AttestationDocument>, (StatusCode, String)> {
    let model_hash = {
        let ms = state
            .model_state
            .read()
            .map_err(|_| lock_error("model_state"))?;
        ms.model_hash
    };

    if let Some(nonce_hex) = &query.nonce {
        // If nonce provided, generate a fresh attestation with this nonce
        let nonce_bytes = hex::decode(nonce_hex.strip_prefix("0x").unwrap_or(nonce_hex))
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid nonce hex: {}", e)))?;

        let mut attestor = state
            .nitro_attestor
            .lock()
            .map_err(|_| lock_error("nitro_attestor"))?;
        attestor
            .refresh(state.enclave_address, model_hash, Some(&nonce_bytes))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        state.metrics.record_attestation_refresh();

        match attestor.attestation_document() {
            Some(doc) => Ok(Json(doc.clone())),
            None => Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "No attestation document available".to_string(),
            )),
        }
    } else {
        let attestor = state
            .nitro_attestor
            .lock()
            .map_err(|_| lock_error("nitro_attestor"))?;
        match attestor.attestation_document() {
            Some(doc) => Ok(Json(doc.clone())),
            None => Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "No attestation document available".to_string(),
            )),
        }
    }
}

async fn infer(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InferRequest>,
) -> Result<Json<InferResponse>, (StatusCode, String)> {
    // Rate limiting check (applies only to /infer)
    if let Err(retry_after) = state.rate_limiter.check() {
        state.metrics.record_rate_limited();
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::json!({
                "error": "rate limit exceeded",
                "retry_after_secs": retry_after
            })
            .to_string(),
        ));
    }

    let start = std::time::Instant::now();

    // Acquire read lock on model state for the duration of inference.
    // This is a std::sync::RwLock read lock -- fast, non-contending for readers.
    let ms = state
        .model_state
        .read()
        .map_err(|_| lock_error("model_state"))?;

    // Validate feature count
    if req.features.len() != ms.model.num_features {
        let elapsed = start.elapsed();
        state.metrics.record_error(elapsed);
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Expected {} features, got {}",
                ms.model.num_features,
                req.features.len()
            ),
        ));
    }

    // Run inference
    let scores = model::predict(&ms.model, &req.features);
    let model_hash = ms.model_hash;

    // Drop the read lock before doing I/O-bound work (signing, serialization)
    drop(ms);

    // Encode result as JSON bytes (e.g. "[0.85]" for binary, "[0.1,0.8,0.1]" for multi-class)
    let result_bytes = serde_json::to_vec(&scores).map_err(|e| {
        let elapsed = start.elapsed();
        state.metrics.record_error(elapsed);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize result: {}", e),
        )
    })?;

    // Compute input hash from JSON-serialized features
    let input_json = serde_json::to_vec(&req.features).map_err(|e| {
        let elapsed = start.elapsed();
        state.metrics.record_error(elapsed);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize input: {}", e),
        )
    })?;
    let input_hash = keccak256(&input_json);

    // Validate per-request chain_id override (if provided, must match configured chain_id)
    if let Some(override_chain_id) = req.chain_id {
        if override_chain_id != state.attestor.chain_id() {
            let elapsed = start.elapsed();
            state.metrics.record_error(elapsed);
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Requested chain_id {} does not match enclave's configured chain_id {}. \
                     Set CHAIN_ID={} env var to use this chain.",
                    override_chain_id,
                    state.attestor.chain_id(),
                    override_chain_id,
                ),
            ));
        }
    }

    // Sign attestation (includes chain_id, nonce, and timestamp for replay protection)
    let attestation = state
        .attestor
        .sign_attestation(model_hash, input_hash, &result_bytes)
        .map_err(|e| {
            let elapsed = start.elapsed();
            state.metrics.record_error(elapsed);
            (StatusCode::INTERNAL_SERVER_ERROR, e)
        })?;

    let elapsed = start.elapsed();
    state.metrics.record_inference(elapsed);

    Ok(Json(InferResponse {
        result: format!("0x{}", hex::encode(&result_bytes)),
        model_hash: format!("0x{}", hex::encode(model_hash)),
        input_hash: format!("0x{}", hex::encode(input_hash)),
        result_hash: format!("0x{}", hex::encode(attestation.result_hash)),
        attestation: format!("0x{}", hex::encode(&attestation.signature)),
        enclave_address: format!("{}", state.attestor.address()),
        chain_id: attestation.chain_id,
        nonce: attestation.nonce,
        timestamp: attestation.timestamp,
    }))
}

async fn metrics_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<MetricsSnapshot>, (StatusCode, String)> {
    let ms = state
        .model_state
        .read()
        .map_err(|_| lock_error("model_state"))?;
    let snap = state.metrics.snapshot(
        &ms.model_name,
        &format!("0x{}", hex::encode(ms.model_hash)),
        ms.model.trees.len(),
        ms.model.num_features,
        ms.model.num_classes,
    );
    drop(ms);
    Ok(Json(snap))
}

/// Admin endpoint: hot-reload the model without restarting the enclave.
///
/// Protected by `ADMIN_API_KEY`. If not set, returns 403.
/// Expects `Authorization: Bearer <key>` header.
async fn reload_model(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ReloadModelRequest>,
) -> Result<Json<ReloadModelResponse>, (StatusCode, String)> {
    // Check admin API key
    let expected_key = match &state.admin_api_key {
        Some(key) => key,
        None => {
            return Err((
                StatusCode::FORBIDDEN,
                "Admin endpoint disabled: ADMIN_API_KEY not configured".to_string(),
            ));
        }
    };

    // Extract Bearer token from Authorization header
    let provided_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");

    if provided_key != expected_key {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Invalid or missing admin API key".to_string(),
        ));
    }

    // Parse model format
    let format =
        model::ModelFormat::from_str(&req.model_format).unwrap_or(model::ModelFormat::Auto);

    // Load and validate the new model in a temporary variable
    let new_model_bytes = std::fs::read(&req.model_path).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Failed to read model file '{}': {}", req.model_path, e),
        )
    })?;

    // Validate model integrity (SHA-256 check against EXPECTED_MODEL_HASH if set)
    let new_model_sha256 = validation::validate_model(
        &new_model_bytes,
        state.expected_model_hash.as_deref(),
    )
    .map_err(|e| {
        tracing::error!("Model validation failed during hot-reload: {}", e);
        (StatusCode::BAD_REQUEST, format!("Model validation failed: {}", e))
    })?;

    let new_model_hash = keccak256(&new_model_bytes);

    let new_model = model::load_model_with_format(&req.model_path, format).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Failed to parse model '{}': {}", req.model_path, e),
        )
    })?;

    let num_trees = new_model.trees.len();
    let num_features = new_model.num_features;

    // Derive model name from path
    let new_model_name = std::path::Path::new(&req.model_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("unknown")
        .to_string();

    let old_model_name = state
        .model_state
        .read()
        .map(|ms| ms.model_name.clone())
        .unwrap_or_else(|_| "<locked>".to_string());

    tracing::info!(
        "Hot-reloading model: {} -> {} ({} trees, {} features)",
        old_model_name,
        new_model_name,
        num_trees,
        num_features,
    );

    // Swap the model under write lock
    {
        let mut ms = state
            .model_state
            .write()
            .map_err(|_| lock_error("model_state"))?;
        ms.model = new_model;
        ms.model_hash = new_model_hash;
        ms.model_bytes = new_model_bytes;
        ms.model_name = new_model_name;
        ms.model_sha256 = new_model_sha256.clone();
    }

    tracing::info!(
        "Model hot-reload complete. New hash: 0x{}, SHA-256: {}",
        hex::encode(new_model_hash),
        new_model_sha256,
    );

    Ok(Json(ReloadModelResponse {
        success: true,
        num_trees,
        num_features,
        model_hash: format!("0x{}", hex::encode(new_model_hash)),
        model_sha256: new_model_sha256,
    }))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    // Load model (auto-detects XGBoost vs LightGBM unless MODEL_FORMAT is set)
    let model_bytes = std::fs::read(&config.model_path)
        .map_err(|e| format!("Failed to read model file '{}': {}", config.model_path, e))?;

    // Validate model integrity (SHA-256 check against EXPECTED_MODEL_HASH if set)
    let model_sha256 = validation::validate_model(
        &model_bytes,
        config.expected_model_hash.as_deref(),
    )
    .map_err(|e| format!("Model validation failed: {}", e))?;
    tracing::info!("Model SHA-256: {}", model_sha256);

    let model_hash = keccak256(&model_bytes);
    let model = model::load_model_with_format(&config.model_path, config.model_format)
        .map_err(|e| format!("Failed to parse model '{}': {}", config.model_path, e))?;
    tracing::info!("Model format: {:?}", config.model_format);

    // Create attestor.
    // When NITRO_ENABLED=true and no private key is set, generate a random key.
    // The Nitro attestation will bind this key to the enclave image.
    let mut attestor = match &config.private_key {
        Some(key) => Attestor::from_private_key(key)
            .map_err(|e| format!("Invalid ENCLAVE_PRIVATE_KEY: {}", e))?,
        None => {
            if config.nitro_enabled {
                tracing::info!("Nitro mode: generating random enclave key (bound via attestation)");
            }
            let a = Attestor::random();
            tracing::info!("Generated enclave key: {}", a.address());
            a
        }
    };

    // Set chain ID from configuration for replay protection
    attestor.set_chain_id(config.chain_id);
    tracing::info!("Chain ID: {}", config.chain_id);

    let enclave_address = attestor.address();

    // Create Nitro attestor (produces mock attestation in dev mode)
    let nitro_attestor = NitroAttestor::new(config.nitro_enabled, enclave_address, model_hash)
        .unwrap_or_else(|e| {
            tracing::warn!("Nitro attestation unavailable: {}", e);
            // Fall back to mock attestation so the server can still start.
            // mock_fallback constructs a dev-mode attestor without any
            // fallible operations, so it cannot panic.
            NitroAttestor::mock_fallback(enclave_address, model_hash)
        });

    tracing::info!(
        "Model loaded: {} features, {} classes, {} trees",
        model.num_features,
        model.num_classes,
        model.trees.len()
    );
    tracing::info!("Model hash: 0x{}", hex::encode(model_hash));
    tracing::info!("Enclave address: {}", enclave_address);
    tracing::info!(
        "Attestation mode: {}",
        if nitro_attestor.is_nitro() {
            "AWS Nitro"
        } else {
            "dev-mock"
        }
    );

    if config.admin_api_key.is_some() {
        tracing::info!("Admin API key configured: POST /admin/reload-model enabled");
    } else {
        tracing::info!("No ADMIN_API_KEY set: admin endpoints disabled");
    }

    // Derive a human-readable model name from the file path (e.g. "model.json")
    let model_name = std::path::Path::new(&config.model_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("unknown")
        .to_string();

    let metrics = Arc::new(Metrics::new());

    // Optionally spawn the background watchdog.
    let watchdog = if config.watchdog_enabled {
        tracing::info!("Watchdog enabled (interval: {}s)", watchdog::DEFAULT_CHECK_INTERVAL_SECS);
        Some(watchdog::spawn_watchdog(
            metrics.clone(),
            watchdog::DEFAULT_CHECK_INTERVAL_SECS,
        ))
    } else {
        tracing::info!("Watchdog disabled (WATCHDOG_ENABLED=false)");
        None
    };

    let state = Arc::new(AppState {
        model_state: std::sync::RwLock::new(ModelState {
            model,
            model_hash,
            model_bytes,
            model_name,
            model_sha256,
        }),
        attestor,
        nitro_attestor: std::sync::Mutex::new(nitro_attestor),
        enclave_address,
        metrics,
        admin_api_key: config.admin_api_key,
        rate_limiter: RateLimiter::new(config.max_requests_per_minute),
        watchdog,
        expected_model_hash: config.expected_model_hash,
    });

    // Background attestation refresh (every 5 minutes)
    {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // First tick is immediate, skip it
            loop {
                interval.tick().await;
                let model_hash = match state_clone.model_state.read() {
                    Ok(ms) => ms.model_hash,
                    Err(_) => {
                        tracing::warn!(
                            "model_state RwLock poisoned in background refresh -- skipping"
                        );
                        continue;
                    }
                };
                let mut attestor = match state_clone.nitro_attestor.lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        tracing::warn!(
                            "nitro_attestor Mutex poisoned in background refresh -- skipping"
                        );
                        continue;
                    }
                };
                if let Err(e) = attestor.refresh(state_clone.enclave_address, model_hash, None) {
                    tracing::warn!("Failed to refresh attestation: {}", e);
                } else {
                    state_clone.metrics.record_attestation_refresh();
                    tracing::debug!("Attestation document refreshed");
                }
            }
        });
    }

    let app = Router::new()
        .route("/health", get(health))
        .route("/health/detailed", get(health_detailed))
        .route("/info", get(info))
        .route("/attestation", get(attestation))
        .route("/infer", post(infer))
        .route("/metrics", get(metrics_handler))
        .route("/admin/reload-model", post(reload_model))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Failed to bind to {}: {}", addr, e))?;
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("Server error: {}", e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    /// Build a test AppState backed by the test model at `tee/test-model/model.json`.
    fn test_app_state() -> Arc<AppState> {
        test_app_state_with_admin_key(None)
    }

    /// Build a test AppState with an optional admin API key.
    fn test_app_state_with_admin_key(admin_api_key: Option<String>) -> Arc<AppState> {
        let model_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-model/model.json");
        let model_path_str = model_path.to_str().unwrap();
        let model_bytes = std::fs::read(model_path_str).unwrap();
        let model_sha256 = validation::compute_model_hash(&model_bytes);
        let model_hash = keccak256(&model_bytes);
        let model = model::load_model(model_path_str).unwrap();
        let attestor = Attestor::random();
        let enclave_address = attestor.address();
        let nitro_attestor = NitroAttestor::new(false, enclave_address, model_hash).unwrap();

        let metrics = Arc::new(Metrics::new());
        let watchdog = Some(Arc::new(Watchdog::new(metrics.clone())));

        Arc::new(AppState {
            model_state: std::sync::RwLock::new(ModelState {
                model,
                model_hash,
                model_bytes,
                model_name: "model.json".to_string(),
                model_sha256,
            }),
            attestor,
            nitro_attestor: std::sync::Mutex::new(nitro_attestor),
            enclave_address,
            metrics,
            admin_api_key,
            rate_limiter: RateLimiter::new(120), // generous limit for tests
            watchdog,
            expected_model_hash: None,
        })
    }

    /// Build a Router wired to the test AppState.
    fn test_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/health/detailed", get(health_detailed))
            .route("/info", get(info))
            .route("/attestation", get(attestation))
            .route("/infer", post(infer))
            .route("/metrics", get(metrics_handler))
            .route("/admin/reload-model", post(reload_model))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_metrics_endpoint_initial_state() {
        let state = test_app_state();
        let app = test_app(state);

        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&body).unwrap();

        assert_eq!(snap.total_inferences, 0);
        assert_eq!(snap.total_errors, 0);
        assert_eq!(snap.avg_inference_ms, 0.0);
        assert_eq!(snap.attestation_refreshes, 0);
        assert_eq!(snap.model_name, "model.json");
        assert!(snap.model_hash.starts_with("0x"));
        assert_eq!(snap.num_features, 4);
        assert_eq!(snap.num_classes, 2);
        assert_eq!(snap.num_trees, 3);
        // uptime should be very small (just created)
        assert!(snap.uptime_secs <= 2);
    }

    #[tokio::test]
    async fn test_metrics_after_successful_inference() {
        let state = test_app_state();
        let app = test_app(state.clone());

        // POST /infer with valid features (4 features for the test model)
        let infer_req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[5.0,3.5,1.5,0.3]}"#))
            .unwrap();

        let infer_resp = app.oneshot(infer_req).await.unwrap();
        assert_eq!(infer_resp.status(), StatusCode::OK);

        // Now check metrics
        let app2 = test_app(state);
        let metrics_req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let metrics_resp = app2.oneshot(metrics_req).await.unwrap();
        let body = axum::body::to_bytes(metrics_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&body).unwrap();

        assert_eq!(snap.total_inferences, 1);
        assert_eq!(snap.total_errors, 0);
        assert!(snap.avg_inference_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_metrics_after_failed_inference() {
        let state = test_app_state();
        let app = test_app(state.clone());

        // POST /infer with wrong number of features (model expects 4)
        let infer_req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[1.0,2.0]}"#))
            .unwrap();

        let infer_resp = app.oneshot(infer_req).await.unwrap();
        assert_eq!(infer_resp.status(), StatusCode::BAD_REQUEST);

        // Now check metrics
        let app2 = test_app(state);
        let metrics_req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let metrics_resp = app2.oneshot(metrics_req).await.unwrap();
        let body = axum::body::to_bytes(metrics_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&body).unwrap();

        assert_eq!(snap.total_inferences, 0);
        assert_eq!(snap.total_errors, 1);
    }

    #[tokio::test]
    async fn test_metrics_after_multiple_requests() {
        let state = test_app_state();

        // Successful inference
        let app = test_app(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[5.0,3.5,1.5,0.3]}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Another successful inference
        let app = test_app(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[6.0,2.5,4.5,1.3]}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Failed inference (wrong feature count)
        let app = test_app(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[1.0]}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Check cumulative metrics
        let app = test_app(state);
        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&body).unwrap();

        assert_eq!(snap.total_inferences, 2);
        assert_eq!(snap.total_errors, 1);
        assert!(snap.avg_inference_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_metrics_does_not_expose_secrets() {
        let state = test_app_state();
        let app = test_app(state);

        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();

        // Verify no private key or sensitive data is leaked
        assert!(!body_str.contains("private_key"));
        assert!(!body_str.contains("secret"));
        assert!(!body_str.contains("signer"));
        // Should only contain expected fields
        assert!(body_str.contains("total_inferences"));
        assert!(body_str.contains("model_name"));
        assert!(body_str.contains("model_hash"));
        assert!(body_str.contains("num_trees"));
    }

    #[tokio::test]
    async fn test_metrics_attestation_refresh_counter() {
        let state = test_app_state();

        // Request attestation with a nonce (triggers refresh)
        let app = test_app(state.clone());
        let req = Request::builder()
            .uri("/attestation?nonce=deadbeef")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Check metrics
        let app = test_app(state);
        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&body).unwrap();

        assert_eq!(snap.attestation_refreshes, 1);
    }

    #[tokio::test]
    async fn test_metrics_model_metadata() {
        let state = test_app_state();
        let app = test_app(state);

        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&body).unwrap();

        // The test model has 4 features, 2 classes (binary), 3 trees
        assert_eq!(snap.num_features, 4);
        assert_eq!(snap.num_classes, 2);
        assert_eq!(snap.num_trees, 3);
        assert_eq!(snap.model_name, "model.json");
        // model_hash should be a 0x-prefixed 64 hex chars (32 bytes)
        assert!(snap.model_hash.starts_with("0x"));
        assert_eq!(snap.model_hash.len(), 66); // "0x" + 64 hex chars
    }

    // -----------------------------------------------------------------------
    // Watchdog / health/detailed tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_health_detailed_endpoint_initial() {
        let state = test_app_state();
        let app = test_app(state);

        let req = Request::builder()
            .uri("/health/detailed")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let health: DetailedHealth = serde_json::from_slice(&body).unwrap();

        assert_eq!(health.status, watchdog::HealthStatus::Healthy);
        assert_eq!(health.error_rate, 0.0);
        assert_eq!(health.avg_latency_ms, 0.0);
        assert_eq!(health.checks.error_rate, watchdog::CheckResult::Ok);
        assert_eq!(health.checks.latency, watchdog::CheckResult::Ok);
    }

    #[tokio::test]
    async fn test_health_detailed_reflects_errors() {
        let state = test_app_state();

        // Trigger some failed inferences to raise error rate
        for _ in 0..5 {
            let app = test_app(state.clone());
            let req = Request::builder()
                .method("POST")
                .uri("/infer")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"features":[1.0]}"#)) // wrong count
                .unwrap();
            let _ = app.oneshot(req).await.unwrap();
        }

        // Trigger one successful inference
        let app = test_app(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[5.0,3.5,1.5,0.3]}"#))
            .unwrap();
        let _ = app.oneshot(req).await.unwrap();

        // Manually trigger a watchdog check so the snapshot updates
        if let Some(wd) = &state.watchdog {
            wd.check();
        }

        // Query /health/detailed
        let app = test_app(state);
        let req = Request::builder()
            .uri("/health/detailed")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let health: DetailedHealth = serde_json::from_slice(&body).unwrap();

        // 5 errors, 1 success = 83% error rate => unhealthy
        assert_eq!(health.status, watchdog::HealthStatus::Unhealthy);
        assert_eq!(health.checks.error_rate, watchdog::CheckResult::Critical);
        assert!(health.error_rate > 0.5);
    }

    #[tokio::test]
    async fn test_health_detailed_json_has_expected_fields() {
        let state = test_app_state();
        let app = test_app(state);

        let req = Request::builder()
            .uri("/health/detailed")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("\"status\""));
        assert!(body_str.contains("\"error_rate\""));
        assert!(body_str.contains("\"avg_latency_ms\""));
        assert!(body_str.contains("\"uptime_secs\""));
        assert!(body_str.contains("\"total_rate_limited\""));
        assert!(body_str.contains("\"checks\""));
    }

    // -----------------------------------------------------------------------
    // Hot-reload tests
    // -----------------------------------------------------------------------

    /// Helper: get a path to a valid alternate model for reload testing.
    /// We write a minimal XGBoost model JSON to a temp file.
    fn write_alternate_model() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("tee_reload_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("alternate_model.json");
        // Minimal XGBoost model with 2 features, 1 tree
        let json = r#"{
            "learner": {
                "learner_model_param": {
                    "num_feature": "2",
                    "num_class": "0",
                    "base_score": "5E-1"
                },
                "gradient_booster": {
                    "model": {
                        "trees": [
                            {
                                "left_children": [1, -1, -1],
                                "right_children": [2, -1, -1],
                                "split_indices": [0, 0, 0],
                                "split_conditions": [0.5, 0.0, 0.0],
                                "base_weights": [0.0, -0.1, 0.1]
                            }
                        ]
                    }
                }
            }
        }"#;
        std::fs::write(&path, json).unwrap();
        path
    }

    /// Helper: write an invalid model file for testing reload failure.
    fn write_invalid_model() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("tee_reload_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("invalid_model.json");
        std::fs::write(&path, r#"{"not": "a valid model"}"#).unwrap();
        path
    }

    #[tokio::test]
    async fn test_reload_model_success() {
        let admin_key = "test-admin-key-12345".to_string();
        let state = test_app_state_with_admin_key(Some(admin_key.clone()));

        // Verify initial model has 4 features, 3 trees
        {
            let ms = state.model_state.read().unwrap();
            assert_eq!(ms.model.num_features, 4);
            assert_eq!(ms.model.trees.len(), 3);
        }

        let alt_model_path = write_alternate_model();

        // Reload with the alternate model
        let app = test_app(state.clone());
        let body = serde_json::json!({
            "model_path": alt_model_path.to_str().unwrap(),
        });
        let req = Request::builder()
            .method("POST")
            .uri("/admin/reload-model")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", admin_key))
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp_body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let reload_resp: ReloadModelResponse = serde_json::from_slice(&resp_body).unwrap();
        assert!(reload_resp.success);
        assert_eq!(reload_resp.num_trees, 1);
        assert_eq!(reload_resp.num_features, 2);
        assert!(reload_resp.model_hash.starts_with("0x"));

        // Verify the model was actually swapped
        {
            let ms = state.model_state.read().unwrap();
            assert_eq!(ms.model.num_features, 2);
            assert_eq!(ms.model.trees.len(), 1);
            assert_eq!(ms.model_name, "alternate_model.json");
        }

        // Verify inference works with the new model (2 features)
        let app2 = test_app(state.clone());
        let infer_req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[0.3, 0.7]}"#))
            .unwrap();
        let infer_resp = app2.oneshot(infer_req).await.unwrap();
        assert_eq!(infer_resp.status(), StatusCode::OK);

        // Verify metrics endpoint reflects new model
        let app3 = test_app(state);
        let metrics_req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let metrics_resp = app3.oneshot(metrics_req).await.unwrap();
        let metrics_body = axum::body::to_bytes(metrics_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let snap: MetricsSnapshot = serde_json::from_slice(&metrics_body).unwrap();
        assert_eq!(snap.num_features, 2);
        assert_eq!(snap.num_trees, 1);
        assert_eq!(snap.model_name, "alternate_model.json");

        // Cleanup
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("tee_reload_test"));
    }

    #[tokio::test]
    async fn test_reload_model_invalid_preserves_old() {
        let admin_key = "test-admin-key-12345".to_string();
        let state = test_app_state_with_admin_key(Some(admin_key.clone()));

        // Record the original model hash
        let original_hash = {
            let ms = state.model_state.read().unwrap();
            format!("0x{}", hex::encode(ms.model_hash))
        };

        let invalid_path = write_invalid_model();

        // Try to reload with an invalid model
        let app = test_app(state.clone());
        let body = serde_json::json!({
            "model_path": invalid_path.to_str().unwrap(),
        });
        let req = Request::builder()
            .method("POST")
            .uri("/admin/reload-model")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", admin_key))
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Verify old model is preserved
        {
            let ms = state.model_state.read().unwrap();
            assert_eq!(ms.model.num_features, 4);
            assert_eq!(ms.model.trees.len(), 3);
            assert_eq!(format!("0x{}", hex::encode(ms.model_hash)), original_hash);
        }

        // Verify inference still works with old model
        let app2 = test_app(state);
        let infer_req = Request::builder()
            .method("POST")
            .uri("/infer")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"features":[5.0,3.5,1.5,0.3]}"#))
            .unwrap();
        let infer_resp = app2.oneshot(infer_req).await.unwrap();
        assert_eq!(infer_resp.status(), StatusCode::OK);

        // Cleanup
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("tee_reload_test"));
    }

    #[tokio::test]
    async fn test_reload_model_wrong_api_key() {
        let admin_key = "correct-key".to_string();
        let state = test_app_state_with_admin_key(Some(admin_key));

        let alt_model_path = write_alternate_model();

        // Try with wrong key
        let app = test_app(state.clone());
        let body = serde_json::json!({
            "model_path": alt_model_path.to_str().unwrap(),
        });
        let req = Request::builder()
            .method("POST")
            .uri("/admin/reload-model")
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong-key")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Try with no key at all
        let app2 = test_app(state.clone());
        let req2 = Request::builder()
            .method("POST")
            .uri("/admin/reload-model")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp2 = app2.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED);

        // Verify model was not changed
        {
            let ms = state.model_state.read().unwrap();
            assert_eq!(ms.model.num_features, 4);
            assert_eq!(ms.model.trees.len(), 3);
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("tee_reload_test"));
    }

    #[tokio::test]
    async fn test_reload_model_disabled_without_admin_key() {
        // No admin key configured
        let state = test_app_state_with_admin_key(None);

        let alt_model_path = write_alternate_model();

        let app = test_app(state.clone());
        let body = serde_json::json!({
            "model_path": alt_model_path.to_str().unwrap(),
        });
        let req = Request::builder()
            .method("POST")
            .uri("/admin/reload-model")
            .header("content-type", "application/json")
            .header("authorization", "Bearer some-key")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Verify model was not changed
        {
            let ms = state.model_state.read().unwrap();
            assert_eq!(ms.model.num_features, 4);
            assert_eq!(ms.model.trees.len(), 3);
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("tee_reload_test"));
    }
}
