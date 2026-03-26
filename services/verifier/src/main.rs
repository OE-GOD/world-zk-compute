use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use zkml_verifier::{verify, verify_hybrid, ProofBundle};

// -- Shared state for warm circuits --

/// Pre-registered circuit configuration (gens + circuit description).
#[derive(Clone)]
struct CircuitConfig {
    gens_hex: String,
    dag_circuit_description: serde_json::Value,
}

type CircuitStore = Arc<RwLock<HashMap<String, CircuitConfig>>>;

// -- Request/response types --

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    circuits_loaded: usize,
}

#[derive(Serialize)]
struct VerifyResponse {
    verified: bool,
    circuit_hash: String,
}

#[derive(Serialize)]
struct HybridResponse {
    circuit_hash: String,
    transcript_digest: String,
    num_compute_layers: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Deserialize)]
struct RegisterCircuitRequest {
    circuit_id: String,
    gens_hex: String,
    dag_circuit_description: serde_json::Value,
}

#[derive(Serialize)]
struct RegisterResponse {
    circuit_id: String,
    status: &'static str,
}

#[derive(Deserialize)]
struct WarmVerifyRequest {
    proof_hex: String,
    #[serde(default)]
    public_inputs_hex: String,
}

#[derive(Serialize)]
struct CircuitListResponse {
    circuits: Vec<String>,
}

// -- Handlers --

async fn health(State(store): State<CircuitStore>) -> Json<HealthResponse> {
    let count = store.read().await.len();
    Json(HealthResponse {
        status: "ok",
        circuits_loaded: count,
    })
}

async fn verify_proof(
    Json(bundle): Json<ProofBundle>,
) -> Result<Json<VerifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let result = tokio::task::spawn_blocking(move || verify(&bundle))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Ok(r) => Ok(Json(VerifyResponse {
            verified: r.verified,
            circuit_hash: format!("0x{}", hex::encode(r.circuit_hash)),
        })),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

async fn verify_hybrid_proof(
    Json(bundle): Json<ProofBundle>,
) -> Result<Json<HybridResponse>, (StatusCode, Json<ErrorResponse>)> {
    let result = tokio::task::spawn_blocking(move || verify_hybrid(&bundle))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Ok(r) => Ok(Json(HybridResponse {
            circuit_hash: format!("0x{}", hex::encode(r.circuit_hash)),
            transcript_digest: format!("0x{}", hex::encode(r.transcript_digest)),
            num_compute_layers: r.compute_fr.rlc_betas.len(),
        })),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

async fn register_circuit(
    State(store): State<CircuitStore>,
    Json(req): Json<RegisterCircuitRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, Json<ErrorResponse>)> {
    if req.circuit_id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "circuit_id is required".into()));
    }
    store.write().await.insert(
        req.circuit_id.clone(),
        CircuitConfig {
            gens_hex: req.gens_hex,
            dag_circuit_description: req.dag_circuit_description,
        },
    );
    Ok(Json(RegisterResponse {
        circuit_id: req.circuit_id,
        status: "registered",
    }))
}

async fn list_circuits(State(store): State<CircuitStore>) -> Json<CircuitListResponse> {
    let keys: Vec<String> = store.read().await.keys().cloned().collect();
    Json(CircuitListResponse { circuits: keys })
}

async fn warm_verify(
    State(store): State<CircuitStore>,
    Path(circuit_id): Path<String>,
    Json(req): Json<WarmVerifyRequest>,
) -> Result<Json<VerifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = store
        .read()
        .await
        .get(&circuit_id)
        .cloned()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("circuit '{circuit_id}' not registered")))?;

    let bundle = ProofBundle {
        proof_hex: req.proof_hex,
        public_inputs_hex: req.public_inputs_hex,
        gens_hex: config.gens_hex,
        dag_circuit_description: config.dag_circuit_description,
    };

    let result = tokio::task::spawn_blocking(move || verify(&bundle))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Ok(r) => Ok(Json(VerifyResponse {
            verified: r.verified,
            circuit_hash: format!("0x{}", hex::encode(r.circuit_hash)),
        })),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

async fn warm_verify_hybrid(
    State(store): State<CircuitStore>,
    Path(circuit_id): Path<String>,
    Json(req): Json<WarmVerifyRequest>,
) -> Result<Json<HybridResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = store
        .read()
        .await
        .get(&circuit_id)
        .cloned()
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("circuit '{circuit_id}' not registered")))?;

    let bundle = ProofBundle {
        proof_hex: req.proof_hex,
        public_inputs_hex: req.public_inputs_hex,
        gens_hex: config.gens_hex,
        dag_circuit_description: config.dag_circuit_description,
    };

    let result = tokio::task::spawn_blocking(move || verify_hybrid(&bundle))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Ok(r) => Ok(Json(HybridResponse {
            circuit_hash: format!("0x{}", hex::encode(r.circuit_hash)),
            transcript_digest: format!("0x{}", hex::encode(r.transcript_digest)),
            num_compute_layers: r.compute_fr.rlc_betas.len(),
        })),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

fn err(code: StatusCode, msg: String) -> (StatusCode, Json<ErrorResponse>) {
    (code, Json(ErrorResponse { error: msg }))
}

fn build_app() -> Router {
    let store: CircuitStore = Arc::new(RwLock::new(HashMap::new()));
    Router::new()
        .route("/health", get(health))
        .route("/verify", post(verify_proof))
        .route("/verify/hybrid", post(verify_hybrid_proof))
        .route("/circuits", get(list_circuits))
        .route("/circuits", post(register_circuit))
        .route("/circuits/:circuit_id/verify", post(warm_verify))
        .route("/circuits/:circuit_id/verify/hybrid", post(warm_verify_hybrid))
        .layer(CorsLayer::permissive())
        .with_state(store)
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");

    let app = build_app();

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    eprintln!("zkml-verifier-service listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = build_app();
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["circuits_loaded"], 0);
    }

    #[tokio::test]
    async fn test_list_circuits_empty() {
        let app = build_app();
        let req = Request::get("/circuits").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["circuits"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn test_register_and_list_circuit() {
        let app = build_app();

        // Register
        let register_body = serde_json::json!({
            "circuit_id": "test-circuit",
            "gens_hex": "0x00",
            "dag_circuit_description": {}
        });
        let req = Request::post("/circuits")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&register_body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // List
        let req = Request::get("/circuits").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let circuits = json["circuits"].as_array().unwrap();
        assert_eq!(circuits.len(), 1);
        assert_eq!(circuits[0], "test-circuit");
    }

    #[tokio::test]
    async fn test_verify_invalid_proof() {
        let app = build_app();
        let bundle = serde_json::json!({
            "proof_hex": "0xdead",
            "gens_hex": "0xbeef",
            "dag_circuit_description": {}
        });
        let req = Request::post("/verify")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&bundle).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_warm_verify_not_found() {
        let app = build_app();
        let body = serde_json::json!({
            "proof_hex": "0xdead"
        });
        let req = Request::post("/circuits/nonexistent/verify")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
