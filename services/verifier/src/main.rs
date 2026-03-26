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
        model_hash: None,
        timestamp: None,
        prover_version: None,
        circuit_hash: None,
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
        model_hash: None,
        timestamp: None,
        prover_version: None,
        circuit_hash: None,
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

// -- Batch types --

#[derive(Deserialize)]
struct BatchVerifyRequest {
    bundles: Vec<ProofBundle>,
}

#[derive(Serialize)]
struct BatchVerifyResponse {
    results: Vec<BatchResultEntry>,
    total: usize,
    valid: usize,
}

#[derive(Serialize)]
struct BatchResultEntry {
    index: usize,
    verified: bool,
    circuit_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn verify_batch(
    Json(req): Json<BatchVerifyRequest>,
) -> Result<Json<BatchVerifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    if req.bundles.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "bundles array is empty".into()));
    }
    if req.bundles.len() > 100 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "maximum 100 bundles per batch".into(),
        ));
    }

    let bundles = req.bundles;
    let results = tokio::task::spawn_blocking(move || {
        bundles
            .iter()
            .enumerate()
            .map(|(i, bundle)| match verify(bundle) {
                Ok(r) => BatchResultEntry {
                    index: i,
                    verified: r.verified,
                    circuit_hash: format!("0x{}", hex::encode(r.circuit_hash)),
                    error: None,
                },
                Err(e) => BatchResultEntry {
                    index: i,
                    verified: false,
                    circuit_hash: String::new(),
                    error: Some(e.to_string()),
                },
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = results.len();
    let valid = results.iter().filter(|r| r.verified).count();
    Ok(Json(BatchVerifyResponse {
        results,
        total,
        valid,
    }))
}

fn err(code: StatusCode, msg: String) -> (StatusCode, Json<ErrorResponse>) {
    (code, Json(ErrorResponse { error: msg }))
}

fn build_app() -> Router {
    let store: CircuitStore = Arc::new(RwLock::new(HashMap::new()));
    Router::new()
        .route("/health", get(health))
        .route("/verify", post(verify_proof))
        .route("/verify/batch", post(verify_batch))
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
    use tower::util::ServiceExt;

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

    #[tokio::test]
    async fn test_verify_batch_empty() {
        let app = build_app();
        let body = serde_json::json!({ "bundles": [] });
        let req = Request::post("/verify/batch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_verify_batch_invalid_proofs() {
        let app = build_app();
        let body = serde_json::json!({
            "bundles": [
                { "proof_hex": "0xdead", "gens_hex": "0x00", "dag_circuit_description": {} },
                { "proof_hex": "0xbeef", "gens_hex": "0x00", "dag_circuit_description": {} }
            ]
        });
        let req = Request::post("/verify/batch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["total"], 2);
        assert_eq!(json["valid"], 0);
        assert_eq!(json["results"].as_array().unwrap().len(), 2);
        // Each result should have an error
        for r in json["results"].as_array().unwrap() {
            assert_eq!(r["verified"], false);
            assert!(r["error"].is_string());
        }
    }

    #[tokio::test]
    async fn test_register_circuit_empty_id() {
        let app = build_app();
        let body = serde_json::json!({
            "circuit_id": "",
            "gens_hex": "0x00",
            "dag_circuit_description": {}
        });
        let req = Request::post("/circuits")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["error"].as_str().unwrap().contains("circuit_id"));
    }

    #[tokio::test]
    async fn test_register_circuit_overwrites() {
        let app = build_app();

        // Register first time
        let body = serde_json::json!({
            "circuit_id": "dup-circuit",
            "gens_hex": "0xaa",
            "dag_circuit_description": {"version": 1}
        });
        let req = Request::post("/circuits")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Register second time with same id
        let body2 = serde_json::json!({
            "circuit_id": "dup-circuit",
            "gens_hex": "0xbb",
            "dag_circuit_description": {"version": 2}
        });
        let req = Request::post("/circuits")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body2).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // List should still show exactly one circuit
        let req = Request::get("/circuits").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["circuits"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_verify_hybrid_invalid_proof() {
        let app = build_app();
        let bundle = serde_json::json!({
            "proof_hex": "0xdead",
            "gens_hex": "0xbeef",
            "dag_circuit_description": {}
        });
        let req = Request::post("/verify/hybrid")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&bundle).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_warm_verify_hybrid_not_found() {
        let app = build_app();
        let body = serde_json::json!({
            "proof_hex": "0xdead"
        });
        let req = Request::post("/circuits/nonexistent/verify/hybrid")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_verify_batch_over_limit() {
        let app = build_app();
        let single = serde_json::json!({
            "proof_hex": "0xdead",
            "gens_hex": "0x00",
            "dag_circuit_description": {}
        });
        let bundles: Vec<_> = (0..101).map(|_| single.clone()).collect();
        let body = serde_json::json!({ "bundles": bundles });
        let req = Request::post("/verify/batch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["error"].as_str().unwrap().contains("100"));
    }

    #[tokio::test]
    async fn test_health_after_circuit_registration() {
        let app = build_app();

        // Register a circuit
        let body = serde_json::json!({
            "circuit_id": "health-test",
            "gens_hex": "0x00",
            "dag_circuit_description": {}
        });
        let req = Request::post("/circuits")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Health should show circuits_loaded=1
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["circuits_loaded"], 1);
    }

    #[tokio::test]
    async fn test_verify_missing_content_type() {
        let app = build_app();
        // POST /verify with a JSON body but no content-type header
        let bundle = serde_json::json!({
            "proof_hex": "0xdead",
            "gens_hex": "0xbeef",
            "dag_circuit_description": {}
        });
        let req = Request::post("/verify")
            .body(Body::from(serde_json::to_vec(&bundle).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        // Axum rejects missing content-type with 415 (Unsupported Media Type)
        // or 422 (Unprocessable Entity) depending on version
        assert!(
            status == 415 || status == 422,
            "expected 415 or 422, got {status}"
        );
    }

    #[tokio::test]
    async fn test_verify_malformed_json() {
        let app = build_app();
        let req = Request::post("/verify")
            .header("content-type", "application/json")
            .body(Body::from("not valid json {{{"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        // Axum returns 400 or 422 for malformed JSON
        assert!(
            status == 400 || status == 422,
            "expected 400 or 422, got {status}"
        );
    }
}
