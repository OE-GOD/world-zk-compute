//! API route definitions for the TEE indexer.
//!
//! All data endpoints live under `/api/v1/`. Infrastructure endpoints
//! (`/health`, `/ws/events`) remain at the root (unversioned).
//! Old unversioned paths (`/results`, `/stats`) are preserved as
//! backward-compatible aliases.

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;

use super::rate_limit::{RateLimitConfig, RateLimitLayer};
use super::websocket::{self, EventBroadcaster};
use super::{AppState, ResultFilter, ResultRow, StatsResponse, Storage};

/// The current API version string.
pub const API_VERSION: &str = "v1";

/// X-API-Version header name (lowercase for HTTP/2 compat).
static X_API_VERSION: HeaderName = HeaderName::from_static("x-api-version");

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Maximum allowed value for the `limit` query parameter.
const MAX_LIMIT: u32 = 1000;

/// Maximum allowed length for a result ID path parameter.
const MAX_ID_LENGTH: usize = 128;

/// Maximum allowed length for any single string query parameter.
const MAX_STRING_LENGTH: usize = 256;

/// Maximum allowed total length of the raw query string (4 KB).
const MAX_QUERY_STRING_LENGTH: usize = 4096;

/// Valid values for the `status` filter parameter.
const VALID_STATUSES: &[&str] = &["submitted", "challenged", "finalized", "resolved"];

/// Check whether `s` is a valid hex string: starts with `0x` followed by
/// one or more hex digits.
fn is_valid_hex(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix("0x") {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit())
    } else {
        false
    }
}

/// Validate a raw query string length. Must be called before the
/// deserialized filter is inspected.
fn validate_raw_query(
    raw: &Option<String>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if let Some(ref qs) = raw {
        if qs.len() > MAX_QUERY_STRING_LENGTH {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "query string too long ({} bytes); maximum is {MAX_QUERY_STRING_LENGTH}",
                        qs.len()
                    ),
                }),
            ));
        }
    }
    Ok(())
}

/// Validate all query-parameter constraints on a `ResultFilter` at once.
///
/// Returns `Ok(())` when the filter is acceptable, or a 400 error response
/// listing every validation violation found.
fn validate_query_params(
    filter: &ResultFilter,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let mut violations: Vec<String> = Vec::new();

    // --- limit ---
    if let Some(limit) = filter.limit {
        if limit == 0 || limit > MAX_LIMIT {
            violations.push(format!(
                "limit must be between 1 and {MAX_LIMIT}, got {limit}"
            ));
        }
    }

    // --- status ---
    if let Some(ref status) = filter.status {
        if status.len() > MAX_STRING_LENGTH {
            violations.push(format!(
                "status too long ({} chars); maximum is {MAX_STRING_LENGTH}",
                status.len()
            ));
        } else if !VALID_STATUSES.contains(&status.as_str()) {
            violations.push(format!(
                "invalid status '{}'; must be one of: {}",
                status,
                VALID_STATUSES.join(", ")
            ));
        }
    }

    // --- submitter ---
    if let Some(ref submitter) = filter.submitter {
        if submitter.len() > MAX_STRING_LENGTH {
            violations.push(format!(
                "submitter too long ({} chars); maximum is {MAX_STRING_LENGTH}",
                submitter.len()
            ));
        } else if !is_valid_hex(submitter) {
            violations.push(format!(
                "submitter must be a hex string starting with 0x, got '{submitter}'"
            ));
        }
    }

    // --- model_hash ---
    if let Some(ref model_hash) = filter.model_hash {
        if model_hash.len() > MAX_STRING_LENGTH {
            violations.push(format!(
                "model_hash too long ({} chars); maximum is {MAX_STRING_LENGTH}",
                model_hash.len()
            ));
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: violations.join("; "),
            }),
        ))
    }
}

/// Validate a result ID path parameter.
fn validate_id(id: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if id.len() > MAX_ID_LENGTH {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "result id too long ({} chars); maximum is {MAX_ID_LENGTH}",
                    id.len()
                ),
            }),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ErrorResponse {
    error: String,
}

pub(crate) async fn handle_list_results(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
    Query(filter): Query<ResultFilter>,
) -> Result<Json<Vec<ResultRow>>, (StatusCode, Json<ErrorResponse>)> {
    validate_raw_query(&raw_query)?;
    validate_query_params(&filter)?;
    state.storage.list_results(&filter).map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })
}

pub(crate) async fn handle_get_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResultRow>, (StatusCode, Json<ErrorResponse>)> {
    validate_id(&id)?;
    match state.storage.get_result(&id) {
        Ok(Some(row)) => Ok(Json(row)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "result not found".to_string(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

pub(crate) async fn handle_stats(
    State(state): State<AppState>,
) -> Result<Json<StatsResponse>, (StatusCode, Json<ErrorResponse>)> {
    state.storage.get_stats().map(Json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })
}

pub(crate) async fn handle_health(State(state): State<AppState>) -> impl IntoResponse {
    let healthy = state.storage.is_healthy();
    let body = super::HealthResponse {
        status: if healthy {
            "ok".to_string()
        } else {
            "degraded".to_string()
        },
        last_indexed_block: state.storage.get_last_indexed_block(),
        total_results: state.storage.get_total_results(),
    };
    if healthy {
        (StatusCode::OK, Json(body))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(body))
    }
}

pub(crate) async fn handle_admin_reset(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // Authenticate via X-Admin-Key header
    let expected_key = std::env::var("ADMIN_API_KEY").unwrap_or_default();
    let provided_key = headers
        .get("x-admin-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if expected_key.is_empty() || provided_key != expected_key {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "missing or invalid X-Admin-Key header".to_string(),
            }),
        ));
    }

    state.storage.reset_lock_state();

    Ok(Json(serde_json::json!({
        "status": "ok",
        "message": "Lock state reset"
    })))
}

// ---------------------------------------------------------------------------
// Router construction
// ---------------------------------------------------------------------------

/// Build the full application router with versioned API, backward-compat
/// aliases, infrastructure endpoints, `X-API-Version` response header,
/// and per-IP rate limiting.
///
/// The `rate_limit_config` controls the maximum number of requests per IP
/// within a sliding time window. Set via `RATE_LIMIT_PER_IP` env var
/// (default 100 req/min).
pub fn build_app(
    storage: Arc<dyn Storage>,
    broadcaster: Arc<EventBroadcaster>,
    rate_limit_config: RateLimitConfig,
) -> Router {
    let state = AppState {
        storage,
        broadcaster: broadcaster.clone(),
    };

    // Versioned API routes under /api/v1
    let v1 = Router::new()
        .route("/results", get(handle_list_results))
        .route("/results/:id", get(handle_get_result))
        .route("/stats", get(handle_stats))
        .route("/admin/reset", post(handle_admin_reset));

    // WebSocket route uses its own state type
    let ws_routes = Router::new()
        .route("/ws/events", get(websocket::ws_handler))
        .with_state(broadcaster);

    // Use new_without_cleanup so tests don't need a tokio runtime at layer
    // construction time. In production the main() function constructs the
    // layer with cleanup enabled via build_app_with_rate_limit.
    let rate_layer = RateLimitLayer::new_without_cleanup(rate_limit_config);

    Router::new()
        // Versioned API under /api/v1/
        .nest("/api/v1", v1.clone())
        // Backward compat: keep unversioned routes working
        .route("/results", get(handle_list_results))
        .route("/results/:id", get(handle_get_result))
        .route("/stats", get(handle_stats))
        // Unversioned infrastructure endpoints
        .route("/health", get(handle_health))
        .with_state(state)
        // WebSocket routes (different state type)
        .merge(ws_routes)
        // Per-IP rate limiting
        .layer(rate_layer)
        // Add X-API-Version header to all responses
        .layer(SetResponseHeaderLayer::if_not_present(
            X_API_VERSION.clone(),
            HeaderValue::from_static(API_VERSION),
        ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::SqliteStorage;
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_storage() -> Arc<dyn Storage> {
        Arc::new(SqliteStorage::open_in_memory().unwrap())
    }

    fn test_broadcaster() -> Arc<EventBroadcaster> {
        Arc::new(EventBroadcaster::with_max_connections(64, 100))
    }

    /// Rate limit config for tests: effectively unlimited so existing tests
    /// are not affected by the rate limiter.
    fn test_rate_limit_config() -> RateLimitConfig {
        RateLimitConfig {
            max_requests: 100_000,
            window: std::time::Duration::from_secs(60),
        }
    }

    #[tokio::test]
    async fn test_versioned_results_endpoint() {
        let s = test_storage();
        s.insert_result("0xabc", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Check X-API-Version header
        let version = resp.headers().get("x-api-version").unwrap();
        assert_eq!(version, "v1");

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn test_versioned_result_by_id() {
        let s = test_storage();
        s.insert_result("0xdef", "0xm", "0xi", "0xa", 42).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results/0xdef")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("x-api-version").unwrap(), "v1");

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let row: ResultRow = serde_json::from_slice(&body).unwrap();
        assert_eq!(row.id, "0xdef");
    }

    #[tokio::test]
    async fn test_versioned_stats() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/stats")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("x-api-version").unwrap(), "v1");
    }

    #[tokio::test]
    async fn test_backward_compat_results() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        // Old unversioned path should still work
        let req = axum::http::Request::builder()
            .uri("/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("x-api-version").unwrap(), "v1");
    }

    #[tokio::test]
    async fn test_health_no_version_prefix() {
        let s = test_storage();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Health endpoint still gets X-API-Version header from layer
        assert!(resp.headers().get("x-api-version").is_some());
    }

    #[tokio::test]
    async fn test_versioned_not_found() {
        let s = test_storage();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results/0xnonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_api_version_constant() {
        assert_eq!(API_VERSION, "v1");
    }

    #[tokio::test]
    async fn test_list_results_filter_by_status() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.update_result_status("0x02", "challenged", Some("0xc"))
            .unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?status=challenged")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "0x02");
        assert_eq!(rows[0].status, "challenged");
        assert_eq!(rows[0].challenger.as_deref(), Some("0xc"));
    }

    #[tokio::test]
    async fn test_list_results_filter_by_submitter() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xalice", 1)
            .unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xbob", 2)
            .unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?submitter=0xbob")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].submitter, "0xbob");
    }

    #[tokio::test]
    async fn test_stats_accuracy_across_status_transitions() {
        let s = test_storage();
        // Insert 3 results, transition through various statuses
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
        s.update_result_status("0x02", "challenged", Some("0xc"))
            .unwrap();
        s.update_result_status("0x03", "finalized", None).unwrap();

        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/stats")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let stats: StatsResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.total_submitted, 1);
        assert_eq!(stats.total_challenged, 1);
        assert_eq!(stats.total_finalized, 1);
        assert_eq!(stats.total_resolved, 0);
    }

    #[tokio::test]
    async fn test_health_reports_block_number() {
        let s = test_storage();
        s.set_last_indexed_block(12345).unwrap();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 100)
            .unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let health: super::super::HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.last_indexed_block, 12345);
        assert_eq!(health.total_results, 1);
    }

    #[tokio::test]
    async fn test_list_results_ordered_by_block_desc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 50).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 30).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "0x02"); // block 50 first
        assert_eq!(rows[1].id, "0x03"); // block 30
        assert_eq!(rows[2].id, "0x01"); // block 10 last
    }

    #[tokio::test]
    async fn test_list_results_with_limit() {
        let s = test_storage();
        for i in 0..10 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?limit=3")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn test_duplicate_insert_is_idempotent() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        // Second insert with same ID should be ignored (INSERT OR IGNORE)
        s.insert_result("0x01", "0xdifferent", "0xi", "0xb", 2)
            .unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results/0x01")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let row: ResultRow = serde_json::from_slice(&body).unwrap();
        // Should retain original values
        assert_eq!(row.model_hash, "0xm");
        assert_eq!(row.submitter, "0xa");
    }

    // -----------------------------------------------------------------------
    // Input validation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_invalid_status_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?status=invalid")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("invalid status"),
            "expected 'invalid status' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_limit_zero_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?limit=0")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("limit must be between 1 and 1000"),
            "expected limit error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_limit_exceeds_max_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?limit=5000")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("limit must be between 1 and 1000"),
            "expected limit error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_result_id_too_long_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let long_id = "x".repeat(200);
        let req = axum::http::Request::builder()
            .uri(&format!("/api/v1/results/{}", long_id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("too long"),
            "expected 'too long' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_submitter_invalid_hex_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?submitter=not_hex")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("submitter must be a hex string"),
            "expected hex error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_submitter_valid_hex_accepted() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xabcdef1234567890", 1)
            .unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?submitter=0xabcdef1234567890")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_submitter_0x_only_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?submitter=0x")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("submitter must be a hex string"),
            "expected hex error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_status_too_long_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let long_status = "a".repeat(300);
        let req = axum::http::Request::builder()
            .uri(&format!("/api/v1/results?status={}", long_status))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("status too long"),
            "expected 'status too long' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_submitter_too_long_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let long_submitter = format!("0x{}", "a".repeat(300));
        let req = axum::http::Request::builder()
            .uri(&format!("/api/v1/results?submitter={}", long_submitter))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("submitter too long"),
            "expected 'submitter too long' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_model_hash_too_long_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let long_hash = "h".repeat(300);
        let req = axum::http::Request::builder()
            .uri(&format!("/api/v1/results?model_hash={}", long_hash))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("model_hash too long"),
            "expected 'model_hash too long' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_multiple_violations_returned_together() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?limit=0&submitter=not_hex")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        // Both violations should appear separated by "; "
        assert!(
            err.error.contains("limit must be between") && err.error.contains("submitter must be a hex"),
            "expected multiple violations, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_query_string_too_long_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        // Build a query string > 4KB
        let long_value = "a".repeat(4100);
        let uri = format!("/api/v1/results?status={}", long_value);
        let req = axum::http::Request::builder()
            .uri(&uri)
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("query string too long"),
            "expected 'query string too long' in error, got: {}",
            err.error
        );
    }

    #[test]
    fn test_is_valid_hex() {
        use super::is_valid_hex;

        assert!(is_valid_hex("0xabc"));
        assert!(is_valid_hex("0xABCDEF0123456789"));
        assert!(is_valid_hex("0x1"));
        assert!(!is_valid_hex("0x"));       // no digits after prefix
        assert!(!is_valid_hex("abc"));       // missing 0x prefix
        assert!(!is_valid_hex("0xZZZ"));     // invalid hex digits
        assert!(!is_valid_hex(""));          // empty
        assert!(!is_valid_hex("0xab cd"));   // space
    }

    // -----------------------------------------------------------------------
    // Rate limiting tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_rate_limit_returns_429_when_exceeded() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();

        // Allow only 3 requests per minute
        let rl_config = RateLimitConfig {
            max_requests: 3,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        // First 3 requests should succeed
        for i in 0..3 {
            let req = axum::http::Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap();

            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "request {} should succeed",
                i + 1
            );
        }

        // 4th request should be rate limited
        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Check Retry-After header is present
        let retry_after = resp.headers().get("retry-after");
        assert!(
            retry_after.is_some(),
            "response should include Retry-After header"
        );
        let retry_secs: u64 = retry_after
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            retry_secs > 0 && retry_secs <= 61,
            "retry-after should be between 1 and 61 seconds, got {}",
            retry_secs
        );

        // Check response body has error message
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("Too many requests"),
            "expected 'Too many requests' in error, got: {}",
            json
        );
    }

    #[tokio::test]
    async fn test_rate_limit_different_ips_independent() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();

        // Allow only 1 request per minute
        let rl_config = RateLimitConfig {
            max_requests: 1,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        // First request from IP 10.0.0.1 should succeed
        let req = axum::http::Request::builder()
            .uri("/health")
            .header("x-forwarded-for", "10.0.0.1")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second request from same IP should be blocked
        let req = axum::http::Request::builder()
            .uri("/health")
            .header("x-forwarded-for", "10.0.0.1")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Request from different IP should still succeed
        let req = axum::http::Request::builder()
            .uri("/health")
            .header("x-forwarded-for", "10.0.0.2")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_applies_to_all_endpoints() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();

        // Allow only 2 requests per minute
        let rl_config = RateLimitConfig {
            max_requests: 2,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        // Use up quota across different endpoints
        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let req = axum::http::Request::builder()
            .uri("/api/v1/stats")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Third request to any endpoint should be blocked
        let req = axum::http::Request::builder()
            .uri("/api/v1/results")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
