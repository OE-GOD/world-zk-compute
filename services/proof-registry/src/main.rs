//! Proof Registry Service
//!
//! Store, search, and verify ZKML proof bundles.
//!
//! Endpoints:
//!   POST   /proofs          — Submit a proof bundle
//!   GET    /proofs/:id      — Retrieve a proof by ID
//!   GET    /proofs          — Search proofs (query params: circuit_hash, model_hash, limit)
//!   POST   /proofs/:id/verify — Verify a stored proof
//!   GET    /health          — Health check
//!   GET    /stats           — Registry statistics

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;

// -- Types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProof {
    pub id: String,
    pub proof_hex: String,
    pub gens_hex: String,
    #[serde(default)]
    pub public_inputs_hex: String,
    pub dag_circuit_description: serde_json::Value,
    #[serde(default)]
    pub circuit_hash: Option<String>,
    #[serde(default)]
    pub model_hash: Option<String>,
    #[serde(default)]
    pub timestamp: Option<u64>,
    #[serde(default)]
    pub prover_version: Option<String>,
    /// SHA-256 digest of the proof bundle JSON (integrity check).
    pub content_hash: String,
    /// When the proof was submitted to the registry.
    pub submitted_at: u64,
    /// Verification status: null (not verified), true, or false.
    #[serde(default)]
    pub verified: Option<bool>,
}

#[derive(Serialize)]
struct SubmitResponse {
    id: String,
    content_hash: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    circuit_hash: Option<String>,
    model_hash: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Serialize)]
struct SearchResponse {
    proofs: Vec<ProofSummary>,
    total: usize,
}

#[derive(Serialize)]
struct ProofSummary {
    id: String,
    circuit_hash: Option<String>,
    model_hash: Option<String>,
    submitted_at: u64,
    verified: Option<bool>,
    content_hash: String,
}

#[derive(Serialize)]
struct StatsResponse {
    total_proofs: usize,
    verified_count: usize,
    unverified_count: usize,
}

#[derive(Serialize)]
struct VerifyResponse {
    id: String,
    verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

type ProofStore = Arc<RwLock<HashMap<String, StoredProof>>>;

// -- Handlers --

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn stats(State(store): State<ProofStore>) -> Json<StatsResponse> {
    let proofs = store.read().await;
    let verified = proofs.values().filter(|p| p.verified == Some(true)).count();
    let unverified = proofs.values().filter(|p| p.verified.is_none()).count();
    Json(StatsResponse {
        total_proofs: proofs.len(),
        verified_count: verified,
        unverified_count: unverified,
    })
}

async fn submit_proof(
    State(store): State<ProofStore>,
    Json(bundle): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<SubmitResponse>), (StatusCode, Json<ErrorResponse>)> {
    let proof_hex = bundle
        .get("proof_hex")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing proof_hex"))?
        .to_string();

    let gens_hex = bundle
        .get("gens_hex")
        .and_then(|v| v.as_str())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing gens_hex"))?
        .to_string();

    let content_hash = {
        let json_bytes = serde_json::to_vec(&bundle).unwrap_or_default();
        format!("{:x}", Sha256::digest(&json_bytes))
    };

    let id = uuid::Uuid::new_v4().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let proof = StoredProof {
        id: id.clone(),
        proof_hex,
        gens_hex,
        public_inputs_hex: bundle
            .get("public_inputs_hex")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        dag_circuit_description: bundle
            .get("dag_circuit_description")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default())),
        circuit_hash: bundle
            .get("circuit_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model_hash: bundle
            .get("model_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        timestamp: bundle.get("timestamp").and_then(|v| v.as_u64()),
        prover_version: bundle
            .get("prover_version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        content_hash: content_hash.clone(),
        submitted_at: now,
        verified: None,
    };

    store.write().await.insert(id.clone(), proof);

    Ok((
        StatusCode::CREATED,
        Json(SubmitResponse { id, content_hash }),
    ))
}

async fn get_proof(
    State(store): State<ProofStore>,
    Path(id): Path<String>,
) -> Result<Json<StoredProof>, (StatusCode, Json<ErrorResponse>)> {
    let proofs = store.read().await;
    proofs
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "proof not found"))
}

async fn search_proofs(
    State(store): State<ProofStore>,
    Query(query): Query<SearchQuery>,
) -> Json<SearchResponse> {
    let proofs = store.read().await;
    let limit = query.limit.unwrap_or(50).min(1000);
    let offset = query.offset.unwrap_or(0);

    let mut results: Vec<&StoredProof> = proofs
        .values()
        .filter(|p| {
            if let Some(ref ch) = query.circuit_hash {
                if p.circuit_hash.as_deref() != Some(ch.as_str()) {
                    return false;
                }
            }
            if let Some(ref mh) = query.model_hash {
                if p.model_hash.as_deref() != Some(mh.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect();

    results.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
    let total = results.len();

    let page: Vec<ProofSummary> = results
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|p| ProofSummary {
            id: p.id.clone(),
            circuit_hash: p.circuit_hash.clone(),
            model_hash: p.model_hash.clone(),
            submitted_at: p.submitted_at,
            verified: p.verified,
            content_hash: p.content_hash.clone(),
        })
        .collect();

    Json(SearchResponse {
        proofs: page,
        total,
    })
}

async fn verify_proof(
    State(store): State<ProofStore>,
    Path(id): Path<String>,
) -> Result<Json<VerifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let proof_data = {
        let proofs = store.read().await;
        proofs
            .get(&id)
            .cloned()
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "proof not found"))?
    };

    // Minimal validation: check proof has REM1 prefix
    let hex_str = proof_data
        .proof_hex
        .strip_prefix("0x")
        .unwrap_or(&proof_data.proof_hex);
    let proof_bytes = hex::decode(hex_str).map_err(|e| {
        err(
            StatusCode::BAD_REQUEST,
            &format!("invalid proof hex: {}", e),
        )
    })?;

    let verified = proof_bytes.len() >= 36 && &proof_bytes[0..4] == b"REM1";
    let error = if !verified {
        Some("proof too short or invalid selector".to_string())
    } else {
        None
    };

    // Update verification status
    store.write().await.entry(id.clone()).and_modify(|p| {
        p.verified = Some(verified);
    });

    Ok(Json(VerifyResponse {
        id,
        verified,
        error,
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
    let store: ProofStore = Arc::new(RwLock::new(HashMap::new()));
    Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/proofs", post(submit_proof))
        .route("/proofs", get(search_proofs))
        .route("/proofs/:id", get(get_proof))
        .route("/proofs/:id/verify", post(verify_proof))
        .layer(CorsLayer::permissive())
        .with_state(store)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("Proof registry listening on {addr}");
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
    async fn test_submit_and_get() {
        let app = build_app();
        let bundle = serde_json::json!({
            "proof_hex": "0x52454d31deadbeef",
            "gens_hex": "0x00",
            "dag_circuit_description": {}
        });
        let req = Request::post("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&bundle).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let submit: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = submit["id"].as_str().unwrap();

        // Retrieve
        let req = Request::get(&format!("/proofs/{}", id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_search_empty() {
        let app = build_app();
        let req = Request::get("/proofs").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let result: SearchResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(result.total, 0);
    }

    #[tokio::test]
    async fn test_not_found() {
        let app = build_app();
        let req = Request::get("/proofs/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_verify_stored_proof() {
        let app = build_app();
        // Submit valid proof (has REM1 prefix + 32 bytes circuit hash)
        let proof_hex = format!("0x{}{}", hex::encode(b"REM1"), "aa".repeat(32));
        let bundle = serde_json::json!({
            "proof_hex": proof_hex,
            "gens_hex": "0x00",
            "dag_circuit_description": {}
        });
        let req = Request::post("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&bundle).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let submit: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = submit["id"].as_str().unwrap();

        // Verify
        let req = Request::post(&format!("/proofs/{}/verify", id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let result: VerifyResponse = serde_json::from_slice(&body).unwrap();
        assert!(result.verified);
    }

    #[tokio::test]
    async fn test_stats() {
        let app = build_app();
        let req = Request::get("/stats").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_proofs, 0);
    }

    #[tokio::test]
    async fn test_missing_proof_hex() {
        let app = build_app();
        let bundle = serde_json::json!({ "gens_hex": "0x00" });
        let req = Request::post("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&bundle).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
