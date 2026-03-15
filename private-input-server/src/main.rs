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

use alloy::primitives::Address;
use clap::Parser;
use private_input_server::{build_app, store, AppState};
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

    let app = build_app(state).layer(TraceLayer::new_for_http());

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

#[cfg(test)]
mod tests {
    use alloy::primitives::Address;
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::Signer;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use private_input_server::{auth::AuthRequest, build_app, store, AppState, ErrorResponse};
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn test_app() -> axum::Router {
        let state = Arc::new(AppState {
            store: store::InputStore::new(),
            rpc_url: "http://localhost:8545".to_string(),
            engine_address: Address::ZERO,
            skip_chain_verification: true,
        });
        build_app(state)
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

        let app = build_app(state);

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

    // Ensure error response body can be parsed
    #[tokio::test]
    async fn test_error_response_is_json() {
        let app = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/inputs/1/upload")
                    .header("content-type", "application/octet-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(err.error.contains("Empty body"));
    }
}
