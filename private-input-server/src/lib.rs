//! Private Input Server library
//!
//! Exposes the application router builder and core types so that
//! integration tests (and other binaries) can construct a testable
//! server without starting a real listener.

pub mod auth;
pub mod store;

use alloy::primitives::Address;
use auth::AuthRequest;
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

/// Application state shared across all handlers.
pub struct AppState {
    /// Input storage
    pub store: store::InputStore,
    /// RPC URL for on-chain checks
    pub rpc_url: String,
    /// Engine contract address
    pub engine_address: Address,
    /// Skip on-chain verification (testing only)
    pub skip_chain_verification: bool,
}

// === Response Types ===

#[derive(Serialize, Deserialize, Debug)]
pub struct UploadResponse {
    pub request_id: u64,
    pub digest: String,
    pub size: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HealthResponse {
    pub status: String,
    pub inputs_stored: usize,
}

/// Build the application router with the given shared state.
///
/// This is the canonical way to construct the server for both
/// production use (in `main`) and integration tests.
pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/inputs/:request_id", post(fetch_input))
        .route("/inputs/:request_id/upload", post(upload_input))
        .route("/health", get(health_check))
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024)) // 10 MB limit
        .with_state(state)
}

// === Handlers ===

/// Upload input data for a request ID (admin endpoint)
/// POST /inputs/:request_id/upload
///
/// Body: raw bytes (application/octet-stream)
async fn upload_input(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<u64>,
    body: axum::body::Bytes,
) -> Result<Json<UploadResponse>, (StatusCode, Json<ErrorResponse>)> {
    if body.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Empty body".to_string(),
            }),
        ));
    }

    let size = body.len();
    let digest = state.store.put(request_id, body.to_vec());

    info!(
        "Input uploaded: request_id={}, size={}, digest={}",
        request_id,
        size,
        hex::encode(digest)
    );

    Ok(Json(UploadResponse {
        request_id,
        digest: format!("0x{}", hex::encode(digest)),
        size,
    }))
}

/// Fetch private input (authenticated prover endpoint)
/// POST /inputs/:request_id
///
/// Body: JSON AuthRequest { request_id, prover_address, timestamp, signature }
async fn fetch_input(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<u64>,
    Json(auth): Json<AuthRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    // Validate request_id matches path
    if auth.request_id != request_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "Path request_id ({}) doesn't match body ({})",
                    request_id, auth.request_id
                ),
            }),
        ));
    }

    // Step 1: Verify signature and timestamp
    let prover_address = auth::verify_auth_request(&auth).map_err(|e| {
        (
            e.status_code(),
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    // Step 2: Verify on-chain claim (unless disabled for testing)
    if !state.skip_chain_verification {
        auth::verify_on_chain_claim(
            &state.rpc_url,
            &state.engine_address,
            request_id,
            &prover_address,
        )
        .await
        .map_err(|e| {
            (
                e.status_code(),
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    }

    // Step 3: Retrieve input data
    let stored = state.store.get(request_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("No input stored for request {}", request_id),
            }),
        )
    })?;

    info!(
        "Input served: request_id={}, prover={}, size={}",
        request_id,
        prover_address,
        stored.data.len()
    );

    // Return raw bytes (same format the prover expects)
    Ok((
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        stored.data,
    ))
}

/// Health check
/// GET /health
async fn health_check(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        inputs_stored: state.store.len(),
    })
}
