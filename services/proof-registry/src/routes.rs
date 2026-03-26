//! HTTP route handlers for the proof registry API.

use std::sync::Arc;

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::db::{DbStats, ProofDb, ProofMetadata, StoredProof, VerificationReceipt};
use zkml_verifier::ProofBundle;

/// Shared application state.
pub struct AppState {
    pub db: Mutex<ProofDb>,
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

    let db = state.db.lock().await;
    db.store(&id, &req.bundle, &metadata)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok((
        StatusCode::CREATED,
        Json(SubmitResponse {
            id,
            status: "stored",
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
        Ok(None) => Err(err(StatusCode::NOT_FOUND, format!("proof '{id}' not found"))),
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
    // Load the bundle from the filesystem.
    let bundle = {
        let db = state.db.lock().await;

        // Check proof exists.
        db.get(&id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("proof '{id}' not found")))?;

        db.load_bundle(&id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    };

    // Run verification in a blocking thread (CPU-intensive).
    let verify_result = tokio::task::spawn_blocking(move || zkml_verifier::verify(&bundle))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (verified, circuit_hash_hex, error_msg) = match verify_result {
        Ok(r) => (r.verified, format!("0x{}", hex::encode(r.circuit_hash)), None),
        Err(e) => (false, String::new(), Some(e.to_string())),
    };

    // Store the result.
    let db = state.db.lock().await;
    let _ = db.mark_verified(&id, verified);

    let receipt = VerificationReceipt {
        proof_id: id.clone(),
        verified,
        verified_at: chrono::Utc::now().to_rfc3339(),
        circuit_hash: circuit_hash_hex,
        error: error_msg.clone(),
    };
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

/// GET /health — health check with DB status.
pub async fn health(
    State(state): State<Arc<AppState>>,
) -> Json<HealthResponse> {
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
