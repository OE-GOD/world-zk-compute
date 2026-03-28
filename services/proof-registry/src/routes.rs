//! HTTP route handlers for the proof registry API.

use std::sync::Arc;

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::db::{DbStats, ProofDb, ProofMetadata, StoredProof};
use crate::receipt::VerificationReceipt;
use crate::transparency::{self, TransparencyLog};
use k256::ecdsa::SigningKey;
use zkml_verifier::ProofBundle;

/// Shared application state.
pub struct AppState {
    pub db: Mutex<ProofDb>,
    pub transparency_log: Mutex<TransparencyLog>,
    pub signing_key: SigningKey,
    pub api_keys: Vec<String>,
}

// -- Request/Response types --

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Serialize)]
pub struct SubmitResponse {
    pub id: String,
    pub status: &'static str,
    /// Leaf index in the transparency log.
    pub transparency_index: u64,
    /// SHA-256 hash of the proof bundle (hex).
    pub proof_hash: String,
}

#[derive(Serialize)]
pub struct VerifyResponse {
    pub verified: bool,
    pub receipt_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct DeleteResponse {
    pub id: String,
    pub status: &'static str,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub db_healthy: bool,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub proofs: Vec<StoredProof>,
    pub count: usize,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub model: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct SubmitRequest {
    /// The proof bundle to store.
    #[serde(flatten)]
    pub bundle: ProofBundle,
}

#[derive(Deserialize)]
pub struct CompareRequest {
    /// ID of the first proof to compare.
    pub proof_a: String,
    /// ID of the second proof to compare.
    pub proof_b: String,
}

// -- Helper --

fn err(code: StatusCode, msg: String) -> (StatusCode, Json<ErrorResponse>) {
    (code, Json(ErrorResponse { error: msg }))
}

// -- Handlers --

/// POST /proofs — submit a proof bundle.
pub async fn submit_proof(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubmitRequest>,
) -> Result<(StatusCode, Json<SubmitResponse>), (StatusCode, Json<ErrorResponse>)> {
    let id = uuid::Uuid::new_v4().to_string();

    let metadata = ProofMetadata {
        model_hash: req.bundle.model_hash.clone().unwrap_or_default(),
        circuit_hash: req.bundle.circuit_hash.clone().unwrap_or_default(),
    };

    // Compute proof hash for the transparency log before storing.
    let bundle_json = serde_json::to_vec(&req.bundle).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize bundle: {e}"),
        )
    })?;
    let proof_hash = transparency::hash_proof_bundle(&bundle_json);

    let db = state.db.lock().await;
    db.store(&id, &req.bundle, &metadata)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    drop(db);

    // Append to transparency log.
    let mut tlog = state.transparency_log.lock().await;
    let transparency_index = tlog.append(proof_hash, &id).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("transparency log error: {e}"),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(SubmitResponse {
            id,
            status: "stored",
            transparency_index,
            proof_hash: hex::encode(proof_hash),
        }),
    ))
}

/// GET /proofs/:id — retrieve a stored proof.
pub async fn get_proof(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<StoredProof>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.lock().await;
    match db.get(&id) {
        Ok(Some(proof)) => Ok(Json(proof)),
        Ok(None) => Err(err(
            StatusCode::NOT_FOUND,
            format!("proof '{id}' not found"),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// GET /proofs — search proofs by model hash and/or date range.
pub async fn search_proofs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = params.limit.unwrap_or(50).min(1000);

    let db = state.db.lock().await;
    let proofs = db
        .search(
            params.model.as_deref(),
            params.from.as_deref(),
            params.to.as_deref(),
            limit,
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let count = proofs.len();
    Ok(Json(SearchResponse { proofs, count }))
}

/// POST /proofs/:id/verify — verify a proof on demand.
pub async fn verify_proof(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VerifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Load the bundle and metadata from the filesystem.
    let (bundle, model_hash) = {
        let db = state.db.lock().await;

        // Check proof exists and get metadata.
        let stored = db
            .get(&id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("proof '{id}' not found")))?;

        let bundle = db
            .load_bundle(&id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

        (bundle, stored.model_hash)
    };

    // Run verification in a blocking thread (CPU-intensive).
    let verify_result = tokio::task::spawn_blocking(move || zkml_verifier::verify(&bundle))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (verified, circuit_hash_hex, error_msg) = match verify_result {
        Ok(r) => (
            r.verified,
            format!("0x{}", hex::encode(r.circuit_hash)),
            None,
        ),
        Err(e) => (false, String::new(), Some(e.to_string())),
    };

    // Create and sign the verification receipt.
    let mut receipt = VerificationReceipt::new(
        &id,
        &circuit_hash_hex,
        &model_hash,
        verified,
        error_msg.clone(),
    );
    receipt.sign(&state.signing_key);

    // Store the result.
    let db = state.db.lock().await;
    let _ = db.mark_verified(&id, verified);

    let receipt_id = db
        .store_receipt(&receipt)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(VerifyResponse {
        verified,
        receipt_id,
        error: error_msg,
    }))
}

/// GET /proofs/:id/receipt — download verification receipt.
pub async fn get_receipt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VerificationReceipt>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.lock().await;
    match db.get_receipt(&id) {
        Ok(Some(receipt)) => Ok(Json(receipt)),
        Ok(None) => Err(err(
            StatusCode::NOT_FOUND,
            format!("no verification receipt for proof '{id}'"),
        )),
        Err(e) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

/// DELETE /proofs/:id — soft-delete a proof.
pub async fn delete_proof(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.lock().await;
    db.soft_delete(&id)
        .map_err(|e| err(StatusCode::NOT_FOUND, e))?;

    Ok(Json(DeleteResponse {
        id,
        status: "deleted",
    }))
}

/// POST /proofs/compare — compare two proofs for output equivalence.
pub async fn compare_proofs(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CompareRequest>,
) -> Result<Json<crate::compare::ComparisonResult>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.lock().await;

    let proof_a = db
        .get(&req.proof_a)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                format!("proof '{}' not found", req.proof_a),
            )
        })?;

    let proof_b = db
        .get(&req.proof_b)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| {
            err(
                StatusCode::NOT_FOUND,
                format!("proof '{}' not found", req.proof_b),
            )
        })?;

    // Attempt to load bundles for output comparison.
    let bundle_a = db.load_bundle(&req.proof_a).ok();
    let bundle_b = db.load_bundle(&req.proof_b).ok();

    let result = crate::compare::compare_proofs(
        &proof_a,
        &proof_b,
        bundle_a.as_ref(),
        bundle_b.as_ref(),
    );

    Ok(Json(result))
}

/// GET /health — health check with DB status.
pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let db = state.db.lock().await;
    let db_healthy = db.is_healthy();
    Json(HealthResponse {
        status: if db_healthy { "ok" } else { "degraded" },
        db_healthy,
    })
}

/// GET /stats — database statistics.
pub async fn stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DbStats>, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.lock().await;
    db.stats()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

// -- Transparency log handlers --

/// GET /transparency/root — current Merkle root and tree size.
pub async fn transparency_root(
    State(state): State<Arc<AppState>>,
) -> Result<Json<transparency::RootResponse>, (StatusCode, Json<ErrorResponse>)> {
    let tlog = state.transparency_log.lock().await;
    let root = tlog
        .root()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let tree_size = tlog
        .size()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(transparency::RootResponse {
        root: hex::encode(root),
        tree_size,
    }))
}

/// GET /transparency/proof/:index — Merkle inclusion proof for a leaf.
pub async fn transparency_proof(
    State(state): State<Arc<AppState>>,
    Path(index): Path<u64>,
) -> Result<Json<transparency::InclusionProof>, (StatusCode, Json<ErrorResponse>)> {
    let tlog = state.transparency_log.lock().await;

    let leaf = tlog
        .get_leaf(index)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("leaf {index} not found")))?;

    let proof = tlog
        .inclusion_proof(index)
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;

    let root = tlog
        .root()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let tree_size = tlog
        .size()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(transparency::InclusionProof {
        index,
        leaf_hash: hex::encode(leaf),
        proof: proof.iter().map(hex::encode).collect(),
        root: hex::encode(root),
        tree_size,
    }))
}

/// POST /transparency/verify — verify a Merkle inclusion proof.
pub async fn transparency_verify(
    Json(req): Json<transparency::VerifyRequest>,
) -> Result<Json<transparency::VerifyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let root: [u8; 32] = hex::decode(&req.root)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("invalid root hex: {e}")))?
        .try_into()
        .map_err(|_| err(StatusCode::BAD_REQUEST, "root must be 32 bytes".to_string()))?;

    let leaf: [u8; 32] = hex::decode(&req.leaf_hash)
        .map_err(|e| {
            err(
                StatusCode::BAD_REQUEST,
                format!("invalid leaf_hash hex: {e}"),
            )
        })?
        .try_into()
        .map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                "leaf_hash must be 32 bytes".to_string(),
            )
        })?;

    let proof: Vec<[u8; 32]> = req
        .proof
        .iter()
        .map(|h| {
            hex::decode(h)
                .map_err(|e| err(StatusCode::BAD_REQUEST, format!("invalid proof hex: {e}")))
                .and_then(|b| {
                    b.try_into().map_err(|_| {
                        err(
                            StatusCode::BAD_REQUEST,
                            "proof element must be 32 bytes".to_string(),
                        )
                    })
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let valid = transparency::verify_inclusion(&root, &leaf, req.index, &proof, req.tree_size);

    Ok(Json(transparency::VerifyResponse { valid }))
}

#[derive(Deserialize)]
pub struct EntriesParams {
    pub from: Option<u64>,
    pub count: Option<u64>,
}

/// GET /transparency/entries — list transparency log entries.
pub async fn transparency_entries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EntriesParams>,
) -> Result<Json<transparency::EntriesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let from = params.from.unwrap_or(0);
    let count = params.count.unwrap_or(100).min(1000);

    let tlog = state.transparency_log.lock().await;
    let total = tlog
        .size()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let entries = tlog
        .list_entries(from, count)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(transparency::EntriesResponse { entries, total }))
}

/// Simple API key auth middleware.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    // Health endpoint is always public.
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // If no API keys configured, auth is disabled.
    if state.api_keys.is_empty() {
        return next.run(request).await;
    }

    // Check X-API-Key header.
    let key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok());

    match key {
        Some(k) if state.api_keys.iter().any(|ak| ak == k) => next.run(request).await,
        Some(_) => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Invalid API key".to_string(),
            }),
        )
            .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Missing X-API-Key header".to_string(),
            }),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware as mw;
    use axum::routing::{delete, get, post};
    use axum::Router;
    use tower::ServiceExt;

    /// Build a test router wired to all route handlers.
    fn build_test_app(state: Arc<AppState>) -> Router {
        let protected = Router::new()
            .route("/proofs", post(submit_proof))
            .route("/proofs", get(search_proofs))
            .route("/proofs/:id", get(get_proof))
            .route("/proofs/:id/verify", post(verify_proof))
            .route("/proofs/:id/receipt", get(get_receipt))
            .route("/proofs/:id", delete(delete_proof))
            .route("/proofs/compare", post(compare_proofs))
            .route("/stats", get(stats))
            .route("/transparency/root", get(transparency_root))
            .route("/transparency/proof/:index", get(transparency_proof))
            .route("/transparency/verify", post(transparency_verify))
            .route("/transparency/entries", get(transparency_entries))
            .route_layer(mw::from_fn_with_state(state.clone(), auth_middleware))
            .with_state(state.clone());

        let health_router = Router::new()
            .route("/health", get(health))
            .with_state(state);

        Router::new().merge(health_router).merge(protected)
    }

    /// Create a test state backed by temporary SQLite databases.
    fn make_test_state() -> Arc<AppState> {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let storage_dir = tmp.path().join("bundles");
        let tlog_path = tmp.path().join("transparency.db");
        let db = crate::db::ProofDb::new(
            db_path.to_str().unwrap(),
            storage_dir.to_str().unwrap(),
        )
        .unwrap();
        let tlog = TransparencyLog::new(tlog_path.to_str().unwrap()).unwrap();
        let signing_key = SigningKey::from_slice(&[42u8; 32]).unwrap();

        // Leak the tempdir so the files remain alive for the test duration.
        std::mem::forget(tmp);

        Arc::new(AppState {
            db: Mutex::new(db),
            transparency_log: Mutex::new(tlog),
            signing_key,
            api_keys: vec![],
        })
    }

    /// Build a minimal proof bundle JSON payload for submission.
    fn make_test_bundle_json() -> serde_json::Value {
        let proof_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let gens_data: Vec<u8> = (0..128).map(|i| ((i * 7) % 256) as u8).collect();

        serde_json::json!({
            "proof_hex": format!("0x{}", hex::encode(&proof_data)),
            "public_inputs_hex": "",
            "gens_hex": format!("0x{}", hex::encode(&gens_data)),
            "dag_circuit_description": {
                "num_compute_layers": 4,
                "layer_types": [0, 1, 0, 1],
            },
            "model_hash": "0xabcdef1234567890",
            "timestamp": 1700000000u64,
            "prover_version": "0.1.0-test",
            "circuit_hash": "0xdeadbeef",
        })
    }

    /// Helper: read a response body as JSON.
    async fn body_json(resp: axum::http::Response<Body>) -> serde_json::Value {
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Helper: submit a proof bundle and return the JSON response.
    async fn submit_bundle(app: &Router, bundle: &serde_json::Value) -> serde_json::Value {
        let req = Request::builder()
            .method("POST")
            .uri("/proofs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(bundle).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        body_json(resp).await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_health() {
        let state = make_test_state();
        let app = build_test_app(state);

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["status"], "ok");
        assert_eq!(json["db_healthy"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_stats_empty() {
        let state = make_test_state();
        let app = build_test_app(state);

        let req = Request::get("/stats").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["total_proofs"], 0);
        assert_eq!(json["active_proofs"], 0);
        assert_eq!(json["verified_count"], 0);
        assert_eq!(json["failed_count"], 0);
        assert_eq!(json["total_storage_bytes"], 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_submit_and_get_proof() {
        let state = make_test_state();
        let app = build_test_app(state);

        let bundle_json = make_test_bundle_json();

        // Submit a proof.
        let submit_resp = submit_bundle(&app, &bundle_json).await;
        assert_eq!(submit_resp["status"], "stored");
        let proof_id = submit_resp["id"].as_str().unwrap().to_string();
        assert!(!proof_id.is_empty());
        // Transparency log fields should be present.
        assert!(submit_resp["transparency_index"].is_number());
        assert!(submit_resp["proof_hash"].as_str().unwrap().len() == 64);

        // Retrieve the proof by ID.
        let req = Request::get(&format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["id"], proof_id);
        assert_eq!(json["model_hash"], "0xabcdef1234567890");
        assert_eq!(json["circuit_hash"], "0xdeadbeef");
        assert!(!json["created_at"].as_str().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_get_nonexistent_proof() {
        let state = make_test_state();
        let app = build_test_app(state);

        let req = Request::get("/proofs/nonexistent-id-12345")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let json = body_json(resp).await;
        assert!(json["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_delete_proof() {
        let state = make_test_state();
        let app = build_test_app(state);

        let bundle_json = make_test_bundle_json();

        // Submit a proof.
        let submit_resp = submit_bundle(&app, &bundle_json).await;
        let proof_id = submit_resp["id"].as_str().unwrap().to_string();

        // Delete it.
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["id"], proof_id);
        assert_eq!(json["status"], "deleted");

        // Verify it is now gone (soft-deleted).
        let req = Request::get(&format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_search_proofs() {
        let state = make_test_state();
        let app = build_test_app(state);

        // Submit two proofs with the same circuit_hash.
        let bundle1 = make_test_bundle_json();
        submit_bundle(&app, &bundle1).await;
        submit_bundle(&app, &bundle1).await;

        // Submit one proof with a different model_hash.
        let mut bundle2 = make_test_bundle_json();
        bundle2["model_hash"] = serde_json::json!("0xother_model");
        bundle2["circuit_hash"] = serde_json::json!("0xothercircuit");
        submit_bundle(&app, &bundle2).await;

        // Search all proofs (no filter).
        let req = Request::get("/proofs?limit=50").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["count"], 3);
        assert_eq!(json["proofs"].as_array().unwrap().len(), 3);

        // Search by model_hash matching the first two.
        let req = Request::get("/proofs?model=0xabcdef1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["count"], 2);
        for proof in json["proofs"].as_array().unwrap() {
            assert_eq!(proof["model_hash"], "0xabcdef1234567890");
        }

        // Search with model_hash that matches one proof.
        let req = Request::get("/proofs?model=0xother_model")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["count"], 1);
        assert_eq!(json["proofs"][0]["circuit_hash"], "0xothercircuit");
    }
}
