//! TEE Enclave Application for XGBoost ML Inference.
//!
//! Runs inside a TEE enclave (AWS Nitro / Intel TDX). Accepts inference requests
//! via HTTP, runs XGBoost prediction, and returns an ECDSA-signed attestation
//! compatible with TEEMLVerifier.sol.

mod attestation;
mod config;
mod model;

use std::sync::Arc;

use alloy_primitives::keccak256;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use attestation::Attestor;
use config::Config;
use model::XgboostModel;

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

struct AppState {
    model: XgboostModel,
    model_hash: alloy_primitives::B256,
    #[allow(dead_code)]
    model_bytes: Vec<u8>,
    attestor: Attestor,
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct InferRequest {
    features: Vec<f64>,
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
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    model_loaded: bool,
}

#[derive(Serialize)]
struct InfoResponse {
    enclave_address: String,
    model_hash: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        model_loaded: true,
    })
}

async fn info(State(state): State<Arc<AppState>>) -> Json<InfoResponse> {
    Json(InfoResponse {
        enclave_address: format!("{}", state.attestor.address()),
        model_hash: format!("0x{}", hex::encode(state.model_hash)),
    })
}

async fn infer(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InferRequest>,
) -> Result<Json<InferResponse>, (StatusCode, String)> {
    // Validate feature count
    if req.features.len() != state.model.num_features {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Expected {} features, got {}",
                state.model.num_features,
                req.features.len()
            ),
        ));
    }

    // Run inference
    let scores = model::predict(&state.model, &req.features);

    // Encode result as JSON bytes (e.g. "[0.85]" for binary, "[0.1,0.8,0.1]" for multi-class)
    let result_bytes = serde_json::to_vec(&scores).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize result: {}", e),
        )
    })?;

    // Compute input hash from JSON-serialized features
    let input_json = serde_json::to_vec(&req.features).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize input: {}", e),
        )
    })?;
    let input_hash = keccak256(&input_json);

    // Sign attestation
    let attestation = state
        .attestor
        .sign_attestation(state.model_hash, input_hash, &result_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(InferResponse {
        result: format!("0x{}", hex::encode(&result_bytes)),
        model_hash: format!("0x{}", hex::encode(state.model_hash)),
        input_hash: format!("0x{}", hex::encode(input_hash)),
        result_hash: format!("0x{}", hex::encode(attestation.result_hash)),
        attestation: format!("0x{}", hex::encode(&attestation.signature)),
        enclave_address: format!("{}", state.attestor.address()),
    }))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    // Load model
    let model_bytes = std::fs::read(&config.model_path).expect("Failed to read model file");
    let model_hash = keccak256(&model_bytes);
    let model = model::load_model(&config.model_path).expect("Failed to parse model");

    // Create attestor
    let attestor = match &config.private_key {
        Some(key) => Attestor::from_private_key(key).expect("Invalid private key"),
        None => {
            let a = Attestor::random();
            tracing::info!("Generated enclave key: {}", a.address());
            a
        }
    };

    tracing::info!(
        "Model loaded: {} features, {} classes, {} trees",
        model.num_features,
        model.num_classes,
        model.trees.len()
    );
    tracing::info!("Model hash: 0x{}", hex::encode(model_hash));
    tracing::info!("Enclave address: {}", attestor.address());

    let state = Arc::new(AppState {
        model,
        model_hash,
        model_bytes,
        attestor,
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/infer", post(infer))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
