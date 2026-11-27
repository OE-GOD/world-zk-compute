//! HTTP route handlers for the proof registry API.

use std::ops::Bound;
use std::sync::Arc;

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
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
    pub webhooks: crate::webhook::WebhookStore,
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
    pub transparency_healthy: bool,
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

/// A value of the `from` or `to` search parameter.
enum DateParam {
    Instant(DateTime<Utc>),
    Day(NaiveDate),
}

/// Parse an RFC 3339 timestamp or a `YYYY-MM-DD` date (UTC).
fn parse_date_param(
    name: &str,
    value: &str,
) -> Result<DateParam, (StatusCode, Json<ErrorResponse>)> {
    if let Ok(t) = DateTime::parse_from_rfc3339(value) {
        return Ok(DateParam::Instant(t.to_utc()));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        // chrono also takes signs, spaces and missing leading zeros.
        .filter(|d| d.format("%Y-%m-%d").to_string() == value)
        .map(DateParam::Day)
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                format!(
                    "invalid '{name}' parameter '{value}': expected an RFC 3339 timestamp \
                     (with '+' sent as %2B) or a YYYY-MM-DD date"
                ),
            )
        })
}

/// Midnight UTC at the start of `day`.
fn start_of_day(day: NaiveDate) -> DateTime<Utc> {
    day.and_time(NaiveTime::MIN).and_utc()
}

/// Lower bound (inclusive) of the `from` parameter.
fn parse_from(value: &str) -> Result<DateTime<Utc>, (StatusCode, Json<ErrorResponse>)> {
    Ok(match parse_date_param("from", value)? {
        DateParam::Instant(t) => t,
        DateParam::Day(d) => start_of_day(d),
    })
}

/// Upper bound of the `to` parameter; a plain date includes the whole day.
fn parse_to(value: &str) -> Result<Bound<DateTime<Utc>>, (StatusCode, Json<ErrorResponse>)> {
    Ok(match parse_date_param("to", value)? {
        DateParam::Instant(t) => Bound::Included(t),
        DateParam::Day(d) => match d.checked_add_days(Days::new(1)) {
            Some(next_day) => Bound::Excluded(start_of_day(next_day)),
            // The last day chrono can represent: nothing is stored after it.
            None => Bound::Unbounded,
        },
    })
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
///
/// `from` and `to` are RFC 3339 timestamps or `YYYY-MM-DD` dates (UTC); both
/// ends are inclusive, and a plain date as `to` includes that whole day.
pub async fn search_proofs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = params.limit.unwrap_or(50).min(1000);
    let from = params.from.as_deref().map(parse_from).transpose()?;
    let to = match params.to.as_deref() {
        Some(value) => parse_to(value)?,
        None => Bound::Unbounded,
    };

    let db = state.db.lock().await;
    let proofs = db
        .search(params.model.as_deref(), from, to, limit)
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

    let result =
        crate::compare::compare_proofs(&proof_a, &proof_b, bundle_a.as_ref(), bundle_b.as_ref());

    Ok(Json(result))
}

/// GET /health — health check with proof DB and transparency log status.
pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let db_healthy = state.db.lock().await.is_healthy();
    let transparency_healthy = state.transparency_log.lock().await.is_healthy();
    Json(HealthResponse {
        status: if db_healthy && transparency_healthy {
            "ok"
        } else {
            "degraded"
        },
        db_healthy,
        transparency_healthy,
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

// -- Webhook routes --

#[derive(Deserialize)]
pub struct RegisterWebhookRequest {
    pub url: String,
    #[serde(default)]
    pub events: Vec<String>,
}

pub async fn register_webhook(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterWebhookRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    if req.url.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "url is required"})),
        ));
    }
    let id = state.webhooks.register(&req.url, req.events);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"id": id, "status": "registered"})),
    ))
}

pub async fn list_webhooks(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let hooks = state.webhooks.list();
    Json(serde_json::json!({"webhooks": hooks, "count": hooks.len()}))
}

pub async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> StatusCode {
    if state.webhooks.unregister(&id) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use tower::ServiceExt;

    /// Create a test state backed by temporary SQLite databases.
    fn make_test_state() -> Arc<AppState> {
        let (state, tmp) = make_test_state_in_dir();
        // Leak the tempdir so the files remain alive for the test duration.
        std::mem::forget(tmp);
        state
    }

    /// Like [`make_test_state`], but returns the directory of the databases
    /// (`test.db` and `transparency.db`), which is removed when dropped.
    fn make_test_state_in_dir() -> (Arc<AppState>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let storage_dir = tmp.path().join("bundles");
        let tlog_path = tmp.path().join("transparency.db");
        let db = crate::db::ProofDb::new(db_path.to_str().unwrap(), storage_dir.to_str().unwrap())
            .unwrap();
        let tlog = TransparencyLog::new(tlog_path.to_str().unwrap()).unwrap();
        let signing_key = SigningKey::from_slice(&[42u8; 32]).unwrap();

        let state = Arc::new(AppState {
            db: Mutex::new(db),
            transparency_log: Mutex::new(tlog),
            signing_key,
            api_keys: vec![],
            webhooks: crate::webhook::WebhookStore::new(),
        });
        (state, tmp)
    }

    /// Helper: run SQL on a database file through a connection of its own.
    fn exec_sql(path: &std::path::Path, sql: &str) {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
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
        let app = crate::build_app_with_state(state);

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["status"], "ok");
        assert_eq!(json["db_healthy"], true);
        assert_eq!(json["transparency_healthy"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_health_reports_unreadable_tables() {
        // (database file, table, db_healthy, transparency_healthy)
        for (file, table, db_ok, tlog_ok) in [
            ("transparency.db", "transparency_log", true, false),
            ("test.db", "proofs", false, true),
        ] {
            let (state, tmp) = make_test_state_in_dir();
            let app = crate::build_app_with_state(state);
            exec_sql(
                &tmp.path().join(file),
                &format!("ALTER TABLE {table} RENAME TO t_gone;"),
            );

            let req = Request::get("/health").body(Body::empty()).unwrap();
            let resp = app.oneshot(req).await.unwrap();

            assert_eq!(resp.status(), StatusCode::OK, "{table}");
            let json = body_json(resp).await;
            assert_eq!(json["status"], "degraded", "{table}");
            assert_eq!(json["db_healthy"], db_ok, "{table}");
            assert_eq!(json["transparency_healthy"], tlog_ok, "{table}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_stats_empty() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

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
        let app = crate::build_app_with_state(state);

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
        let req = Request::get(format!("/proofs/{proof_id}"))
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
        let app = crate::build_app_with_state(state);

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
        let app = crate::build_app_with_state(state);

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
        let req = Request::get(format!("/proofs/{proof_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_search_proofs() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

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
        let req = Request::get("/proofs?limit=50")
            .body(Body::empty())
            .unwrap();
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

    /// Helper: GET /proofs with a query string, return the status and body.
    async fn search(app: &Router, query: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::get(format!("/proofs?{query}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        (status, body_json(resp).await)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_search_date_params() {
        let (state, tmp) = make_test_state_in_dir();
        let app = crate::build_app_with_state(state);
        submit_bundle(&app, &make_test_bundle_json()).await;
        exec_sql(
            &tmp.path().join("test.db"),
            "UPDATE proofs SET created_at = '2026-11-24T09:19:16.010210+00:00';",
        );

        for (query, count) in [
            // Plain dates: from the start of `from` to the end of `to`.
            ("from=2026-11-24&to=2026-11-24", 1),
            ("to=2026-11-24", 1),
            ("to=2026-11-23", 0),
            ("from=2026-11-24", 1),
            ("from=2026-11-25", 0),
            ("from=0001-01-01&to=9999-12-31", 1),
            // RFC 3339 in UTC, with Z and at several precisions.
            ("from=2026-11-24T09:19:16Z", 1),
            ("from=2026-11-24T09:19:16.010210Z", 1),
            ("from=2026-11-24T09:19:16.010211Z", 0),
            ("to=2026-11-24T09:19:16.010210Z", 1),
            ("to=2026-11-24T09:19:16.01021Z", 1),
            ("to=2026-11-24T09:19:16.010209Z", 0),
            ("to=2026-11-24T09:19:16Z", 0),
            // Other offsets: 09:19:16 UTC is 11:19:16+02:00 and 04:19:16-05:00.
            ("from=2026-11-24T11:19:16%2B02:00", 1),
            ("from=2026-11-24T11:19:17%2B02:00", 0),
            ("to=2026-11-24T11:19:17%2B02:00", 1),
            ("to=2026-11-24T11:19:16%2B02:00", 0),
            ("from=2026-11-24T04:19:17-05:00", 0),
            ("to=2026-11-24T04:19:17-05:00", 1),
            // Dates after 9999 take a sign, up to the last day chrono can
            // represent, which has no next day.
            ("to=%2B10000-01-01", 1),
            ("from=%2B10000-01-01", 0),
            ("to=%2B262142-12-31", 1),
            ("from=%2B262142-12-31", 0),
            // Years before 1 take a '-', which sorts before every digit, so
            // their text compares as the earlier time without a clamp: the
            // proof is after from=-0001-01-01 and after the whole of that day.
            ("from=-0001-01-01", 1),
            ("to=-0001-01-01", 0),
        ] {
            let (status, json) = search(&app, query).await;
            assert_eq!(status, StatusCode::OK, "{query}: {json}");
            assert_eq!(json["count"], count, "{query}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_search_plain_date_covers_exactly_one_day() {
        let (state, tmp) = make_test_state_in_dir();
        let app = crate::build_app_with_state(state);
        submit_bundle(&app, &make_test_bundle_json()).await;

        for (created_at, count) in [
            ("2026-11-23T23:59:59.999999999+00:00", 0),
            ("2026-11-24T00:00:00+00:00", 1),
            ("2026-11-24T23:59:59.999999999+00:00", 1),
            ("2026-11-25T00:00:00+00:00", 0),
        ] {
            exec_sql(
                &tmp.path().join("test.db"),
                &format!("UPDATE proofs SET created_at = '{created_at}';"),
            );
            let (status, json) = search(&app, "from=2026-11-24&to=2026-11-24").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(json["count"], count, "{created_at}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_search_rejects_malformed_dates() {
        let (state, _tmp) = make_test_state_in_dir();
        let app = crate::build_app_with_state(state);

        for name in ["from", "to"] {
            for value in [
                "",
                "yesterday",
                "2026-11",
                "2026-11-31",
                "2026-1-5",
                "%2B2026-1-05",
                "%202026-11-24",
                "2026-11-24T09:19:16",
                "2026-11-24T25:00:00Z",
                "1764000000",
                // An unencoded '+' reaches the service as a space.
                "2026-11-24T09:19:16+00:00",
            ] {
                let (status, json) = search(&app, &format!("{name}={value}")).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{name}={value}");
                let error = json["error"].as_str().unwrap();
                assert!(
                    error.starts_with(&format!("invalid '{name}' parameter")),
                    "{error}"
                );
            }
        }
    }

    // -- Proof comparison endpoint tests --

    /// Helper: submit a proof bundle with custom model/circuit hash and return its ID.
    async fn submit_bundle_with_hashes(
        app: &Router,
        model_hash: &str,
        circuit_hash: &str,
        public_inputs: &str,
    ) -> String {
        let proof_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let gens_data: Vec<u8> = (0..128).map(|i| ((i * 7) % 256) as u8).collect();

        let bundle_json = serde_json::json!({
            "proof_hex": format!("0x{}", hex::encode(&proof_data)),
            "public_inputs_hex": public_inputs,
            "gens_hex": format!("0x{}", hex::encode(&gens_data)),
            "dag_circuit_description": {
                "num_compute_layers": 4,
                "layer_types": [0, 1, 0, 1],
            },
            "model_hash": model_hash,
            "timestamp": 1700000000u64,
            "prover_version": "0.1.0-test",
            "circuit_hash": circuit_hash,
        });

        let resp = submit_bundle(app, &bundle_json).await;
        resp["id"].as_str().unwrap().to_string()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_compare_same_model_same_output() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

        let id_a = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "0xaabbccdd").await;
        let id_b = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "0xaabbccdd").await;

        let compare_req = serde_json::json!({
            "proof_a": id_a,
            "proof_b": id_b,
        });

        let req = Request::builder()
            .method("POST")
            .uri("/proofs/compare")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&compare_req).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["proof_a_id"], id_a);
        assert_eq!(json["proof_b_id"], id_b);
        assert_eq!(json["same_model"], true);
        assert_eq!(json["same_circuit"], true);
        assert_eq!(json["outputs_match"], true);
        assert_eq!(json["model_a_hash"], "0xmodel1");
        assert_eq!(json["model_b_hash"], "0xmodel1");
        assert_eq!(json["circuit_a_hash"], "0xcircuit1");
        assert_eq!(json["circuit_b_hash"], "0xcircuit1");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_compare_same_model_different_output() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

        let id_a = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "0xaabbccdd").await;
        let id_b = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "0x11223344").await;

        let compare_req = serde_json::json!({
            "proof_a": id_a,
            "proof_b": id_b,
        });

        let req = Request::builder()
            .method("POST")
            .uri("/proofs/compare")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&compare_req).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["same_model"], true);
        assert_eq!(json["same_circuit"], true);
        assert_eq!(json["outputs_match"], false);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_compare_different_models() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

        let id_a = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "0xaabbccdd").await;
        let id_b = submit_bundle_with_hashes(&app, "0xmodel2", "0xcircuit2", "0xaabbccdd").await;

        let compare_req = serde_json::json!({
            "proof_a": id_a,
            "proof_b": id_b,
        });

        let req = Request::builder()
            .method("POST")
            .uri("/proofs/compare")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&compare_req).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["same_model"], false);
        assert_eq!(json["same_circuit"], false);
        assert_eq!(json["outputs_match"], true);
        assert_eq!(json["model_a_hash"], "0xmodel1");
        assert_eq!(json["model_b_hash"], "0xmodel2");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_compare_nonexistent_proof() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

        let id_a = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "0xaabb").await;

        let compare_req = serde_json::json!({
            "proof_a": id_a,
            "proof_b": "nonexistent-id",
        });

        let req = Request::builder()
            .method("POST")
            .uri("/proofs/compare")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&compare_req).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_compare_empty_public_inputs() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

        let id_a = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "").await;
        let id_b = submit_bundle_with_hashes(&app, "0xmodel1", "0xcircuit1", "").await;

        let compare_req = serde_json::json!({
            "proof_a": id_a,
            "proof_b": id_b,
        });

        let req = Request::builder()
            .method("POST")
            .uri("/proofs/compare")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&compare_req).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["same_model"], true);
        assert_eq!(json["same_circuit"], true);
        // Both empty, no embedded outputs, so vacuously match.
        assert_eq!(json["outputs_match"], true);
    }

    // -- Webhook endpoint tests --

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_webhook_register_list_delete() {
        let state = make_test_state();
        let app = crate::build_app_with_state(state);

        // Register.
        let register_req = serde_json::json!({
            "url": "http://127.0.0.1:9/hook",
            "events": ["proof.verified"],
        });
        let req = Request::builder()
            .method("POST")
            .uri("/webhooks")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&register_req).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let json = body_json(resp).await;
        assert_eq!(json["status"], "registered");
        let webhook_id = json["id"].as_str().unwrap().to_string();

        // List.
        let req = Request::get("/webhooks").body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["count"], 1);
        assert_eq!(json["webhooks"][0]["id"], webhook_id);
        assert_eq!(json["webhooks"][0]["url"], "http://127.0.0.1:9/hook");

        // Delete, then delete again.
        for expected in [StatusCode::OK, StatusCode::NOT_FOUND] {
            let req = Request::builder()
                .method("DELETE")
                .uri(format!("/webhooks/{webhook_id}"))
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected);
        }

        let req = Request::get("/webhooks").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(body_json(resp).await["count"], 0);
    }
}
