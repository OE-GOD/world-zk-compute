//! Private Input Server
//!
//! Reference implementation for hosting private data behind an auth server.
//! Provers authenticate with a wallet-signed request, and the server verifies
//! the prover has claimed the job on-chain before releasing data.
//!
//! ## Endpoints
//!
//! - `POST /inputs/:request_id` — Authenticated fetch (prover sends signed AuthRequest)
//! - `POST /inputs/:request_id/upload` — Admin upload endpoint
//! - `GET /health` — Health check

mod auth;
mod store;

use alloy::primitives::Address;
use auth::AuthRequest;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use serde::Serialize;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

/// Private Input Server CLI
#[derive(Parser, Debug)]
#[command(name = "private-input-server")]
#[command(about = "Private Input Server for World ZK Compute")]
struct Args {
    /// Server host
    #[arg(long, default_value = "0.0.0.0", env = "HOST")]
    host: String,

    /// Server port
    #[arg(long, default_value = "8090", env = "PORT")]
    port: u16,

    /// RPC URL for on-chain verification
    #[arg(long, env = "RPC_URL")]
    rpc_url: String,

    /// ExecutionEngine contract address
    #[arg(long, env = "ENGINE_ADDRESS")]
    engine_address: String,

    /// Storage directory for persistent inputs (optional, in-memory if not set)
    #[arg(long, env = "STORAGE_DIR")]
    storage_dir: Option<String>,

    /// Skip on-chain verification (for testing only!)
    #[arg(long, env = "SKIP_CHAIN_VERIFICATION")]
    skip_chain_verification: bool,
}

/// Application state
struct AppState {
    /// Input storage
    store: store::InputStore,
    /// RPC URL for on-chain checks
    rpc_url: String,
    /// Engine contract address
    engine_address: Address,
    /// Skip on-chain verification (testing only)
    skip_chain_verification: bool,
}

// === Response Types ===

#[derive(Serialize)]
struct UploadResponse {
    request_id: u64,
    digest: String,
    size: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    inputs_stored: usize,
}

// === Main ===

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("private_input_server=info".parse()?),
        )
        .init();

    let args = Args::parse();
    let engine_address: Address = args.engine_address.parse()?;

    let input_store = match args.storage_dir {
        Some(ref dir) => {
            info!("Using file-backed storage: {}", dir);
            store::InputStore::with_storage_dir(dir.into())?
        }
        None => {
            info!("Using in-memory storage (data lost on restart)");
            store::InputStore::new()
        }
    };

    let state = Arc::new(AppState {
        store: input_store,
        rpc_url: args.rpc_url.clone(),
        engine_address,
        skip_chain_verification: args.skip_chain_verification,
    });

    let app = Router::new()
        .route("/inputs/:request_id", post(fetch_input))
        .route("/inputs/:request_id/upload", post(upload_input))
        .route("/health", get(health_check))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("{}:{}", args.host, args.port);
    info!("Starting Private Input Server on {}", addr);
    info!("Engine address: {}", engine_address);
    info!("RPC URL: {}", args.rpc_url);
    if args.skip_chain_verification {
        warn!("On-chain verification is DISABLED (testing mode)");
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::Signer;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    fn test_app() -> Router {
        let state = Arc::new(AppState {
            store: store::InputStore::new(),
            rpc_url: "http://localhost:8545".to_string(),
            engine_address: Address::ZERO,
            skip_chain_verification: true,
        });

        Router::new()
            .route("/inputs/:request_id", post(fetch_input))
            .route("/inputs/:request_id/upload", post(upload_input))
            .route("/health", get(health_check))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_health() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_upload_and_fetch() {
        let state = Arc::new(AppState {
            store: store::InputStore::new(),
            rpc_url: "http://localhost:8545".to_string(),
            engine_address: Address::ZERO,
            skip_chain_verification: true,
        });

        let app = Router::new()
            .route("/inputs/:request_id", post(fetch_input))
            .route("/inputs/:request_id/upload", post(upload_input))
            .with_state(state);

        // Upload
        let upload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/inputs/42/upload")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from(b"test input data".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload_response.status(), StatusCode::OK);

        // Fetch with valid auth
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let message = format!(
            "world-zk-compute:fetch-input:{}:{}:{}",
            42,
            address.to_checksum(None),
            now,
        );
        let sig = signer.sign_message(message.as_bytes()).await.unwrap();

        let auth = AuthRequest {
            request_id: 42,
            prover_address: address.to_checksum(None),
            timestamp: now,
            signature: format!("0x{}", hex::encode(sig.as_bytes())),
        };

        let fetch_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/inputs/42")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&auth).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(fetch_response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(fetch_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"test input data");
    }

    #[tokio::test]
    async fn test_fetch_not_found() {
        let app = test_app();

        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let message = format!(
            "world-zk-compute:fetch-input:{}:{}:{}",
            99,
            address.to_checksum(None),
            now,
        );
        let sig = signer.sign_message(message.as_bytes()).await.unwrap();

        let auth = AuthRequest {
            request_id: 99,
            prover_address: address.to_checksum(None),
            timestamp: now,
            signature: format!("0x{}", hex::encode(sig.as_bytes())),
        };

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/inputs/99")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&auth).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
