//! Proof Generation Service
//!
//! Accepts a model + input and produces a verified proof bundle.
//! Orchestrates: load model → run inference → generate proof → verify → return bundle.
//!
//! Endpoints:
//!   POST /prove       — Generate a proof from model + features
//!   GET  /models      — List available models
//!   POST /models      — Register a model
//!   GET  /health      — Health check

use axum::{
    extract::{Json, State},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegisteredModel {
    id: String,
    name: String,
    format: String,
    model_hash: String,
    num_features: usize,
    registered_at: u64,
}

#[derive(Deserialize)]
struct RegisterModelRequest {
    id: String,
    name: String,
    format: String,
    num_features: usize,
}

#[derive(Deserialize)]
struct ProveRequest {
    model_id: String,
    features: Vec<f64>,
}

#[derive(Serialize)]
struct ProveResponse {
    proof_id: String,
    model_id: String,
    predicted_class: u32,
    proof_bundle: serde_json::Value,
    prove_time_ms: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

type ModelStore = Arc<RwLock<HashMap<String, RegisteredModel>>>;

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "proof-generator" }))
}

async fn list_models(State(store): State<ModelStore>) -> Json<serde_json::Value> {
    let models = store.read().await;
    let list: Vec<&RegisteredModel> = models.values().collect();
    Json(serde_json::json!({ "models": list, "count": list.len() }))
}

async fn register_model(
    State(store): State<ModelStore>,
    Json(req): Json<RegisterModelRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<ErrorResponse>)> {
    if req.id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "id is required"));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let model = RegisteredModel {
        id: req.id.clone(),
        name: req.name,
        format: req.format,
        model_hash: String::new(),
        num_features: req.num_features,
        registered_at: now,
    };

    store.write().await.insert(req.id.clone(), model);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": req.id, "status": "registered" })),
    ))
}

async fn prove(
    State(store): State<ModelStore>,
    Json(req): Json<ProveRequest>,
) -> Result<Json<ProveResponse>, (StatusCode, Json<ErrorResponse>)> {
    let models = store.read().await;
    let model = models
        .get(&req.model_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "model not registered"))?;

    if req.features.len() != model.num_features {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!(
                "expected {} features, got {}",
                model.num_features,
                req.features.len()
            ),
        ));
    }

    let start = std::time::Instant::now();

    // In a real implementation, this would call the xgboost-remainder prover.
    // For now, return a placeholder proof bundle.
    let proof_id = uuid::Uuid::new_v4().to_string();
    let bundle = serde_json::json!({
        "proof_hex": "0x52454d31" ,
        "gens_hex": "0x00",
        "public_inputs_hex": "",
        "dag_circuit_description": {},
        "model_hash": &model.model_hash,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "prover_version": env!("CARGO_PKG_VERSION"),
    });

    Ok(Json(ProveResponse {
        proof_id,
        model_id: req.model_id,
        predicted_class: 0,
        proof_bundle: bundle,
        prove_time_ms: start.elapsed().as_millis() as u64,
    }))
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        code,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

fn build_app() -> Router {
    let store: ModelStore = Arc::new(RwLock::new(HashMap::new()));
    Router::new()
        .route("/health", get(health))
        .route("/models", get(list_models))
        .route("/models", post(register_model))
        .route("/prove", post(prove))
        .layer(CorsLayer::permissive())
        .with_state(store)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let port = std::env::var("PORT").unwrap_or_else(|_| "3002".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("Proof generator on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, build_app()).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn test_health() {
        let app = build_app();
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_register_and_list_models() {
        let app = build_app();
        let body = serde_json::json!({
            "id": "credit-v1",
            "name": "Credit Scoring",
            "format": "xgboost",
            "num_features": 6
        });
        let req = Request::post("/models")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let req = Request::get("/models").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 1);
    }

    #[tokio::test]
    async fn test_prove_unknown_model() {
        let app = build_app();
        let body = serde_json::json!({ "model_id": "unknown", "features": [1.0] });
        let req = Request::post("/prove")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
