//! TEE Enclave Application for XGBoost ML Inference.
//!
//! Runs inside a TEE enclave (AWS Nitro / Intel TDX). Accepts inference requests
//! via HTTP, runs XGBoost prediction, and returns an ECDSA-signed attestation
//! compatible with TEEMLVerifier.sol.

mod attestation;
mod config;
mod model;
mod nitro;

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
use nitro::{AttestationDocument, NitroAttestor};

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

struct AppState {
    model: XgboostModel,
    model_hash: alloy_primitives::B256,
    #[allow(dead_code)]
    model_bytes: Vec<u8>,
    attestor: Attestor,
    nitro_attestor: std::sync::Mutex<NitroAttestor>,
    enclave_address: alloy_primitives::Address,
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

#[derive(Deserialize)]
struct AttestationQuery {
    /// Optional hex-encoded nonce (with or without 0x prefix).
    nonce: Option<String>,
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

async fn attestation(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<AttestationQuery>,
) -> Result<Json<AttestationDocument>, (StatusCode, String)> {
    if let Some(nonce_hex) = &query.nonce {
        // If nonce provided, generate a fresh attestation with this nonce
        let nonce_bytes = hex::decode(nonce_hex.strip_prefix("0x").unwrap_or(nonce_hex))
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid nonce hex: {}", e)))?;

        let mut attestor = state.nitro_attestor.lock().unwrap();
        attestor
            .refresh(state.enclave_address, state.model_hash, Some(&nonce_bytes))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        match attestor.attestation_document() {
            Some(doc) => Ok(Json(doc.clone())),
            None => Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "No attestation document available".to_string(),
            )),
        }
    } else {
        let attestor = state.nitro_attestor.lock().unwrap();
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

    // Create attestor.
    // When NITRO_ENABLED=true and no private key is set, generate a random key.
    // The Nitro attestation will bind this key to the enclave image.
    let attestor = match &config.private_key {
        Some(key) => Attestor::from_private_key(key).expect("Invalid private key"),
        None => {
            if config.nitro_enabled {
                tracing::info!("Nitro mode: generating random enclave key (bound via attestation)");
            }
            let a = Attestor::random();
            tracing::info!("Generated enclave key: {}", a.address());
            a
        }
    };

    let enclave_address = attestor.address();

    // Create Nitro attestor (produces mock attestation in dev mode)
    let nitro_attestor = NitroAttestor::new(config.nitro_enabled, enclave_address, model_hash)
        .unwrap_or_else(|e| {
            tracing::warn!("Nitro attestation unavailable: {}", e);
            // Fall back to mock attestation so the server can still start
            NitroAttestor::new(false, enclave_address, model_hash)
                .expect("Mock attestation should always succeed")
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

    let state = Arc::new(AppState {
        model,
        model_hash,
        model_bytes,
        attestor,
        nitro_attestor: std::sync::Mutex::new(nitro_attestor),
        enclave_address,
    });

    // Background attestation refresh (every 5 minutes)
    {
        let state_clone = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // First tick is immediate, skip it
            loop {
                interval.tick().await;
                let mut attestor = state_clone.nitro_attestor.lock().unwrap();
                if let Err(e) =
                    attestor.refresh(state_clone.enclave_address, state_clone.model_hash, None)
                {
                    tracing::warn!("Failed to refresh attestation: {}", e);
                } else {
                    tracing::debug!("Attestation document refreshed");
                }
            }
        });
    }

    let app = Router::new()
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/attestation", get(attestation))
        .route("/infer", post(infer))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
