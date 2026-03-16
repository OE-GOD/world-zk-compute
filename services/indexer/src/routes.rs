//! API route definitions for the TEE indexer.
//!
//! All data endpoints live under `/api/v1/`. Infrastructure endpoints
//! (`/health`, `/ws/events`) remain at the root (unversioned).
//! Old unversioned paths (`/results`, `/stats`) are preserved as
//! backward-compatible aliases.

use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

use super::rate_limit::{RateLimitConfig, RateLimitLayer};
use super::websocket::{self, EventBroadcaster};
use super::{AppState, PaginatedResponse, ResultFilter, ResultRow, StatsResponse, Storage};

/// The current API version string.
pub const API_VERSION: &str = "v1";

/// X-API-Version header name (lowercase for HTTP/2 compat).
static X_API_VERSION: HeaderName = HeaderName::from_static("x-api-version");

/// X-Request-ID header name for request correlation.
static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Maximum allowed value for the `limit` query parameter.
const MAX_LIMIT: u32 = 1000;

/// Maximum allowed value for `offset + limit` to prevent deep pagination abuse.
const MAX_OFFSET_PLUS_LIMIT: u32 = 10_000;

/// Maximum allowed length for a result ID path parameter.
const MAX_ID_LENGTH: usize = 128;

/// Maximum allowed length for any single string query parameter.
const MAX_STRING_LENGTH: usize = 256;

/// Maximum allowed total length of the raw query string (4 KB).
const MAX_QUERY_STRING_LENGTH: usize = 4096;

/// Valid values for the `status` filter parameter.
const VALID_STATUSES: &[&str] = &["submitted", "challenged", "finalized", "resolved"];

/// Valid values for the `sort_by` query parameter.
const VALID_SORT_BY: &[&str] = &["block_number", "submitted_at", "status", "submitter"];

/// Valid values for the `sort_order` query parameter.
const VALID_SORT_ORDER: &[&str] = &["asc", "desc"];

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
fn validate_raw_query(raw: &Option<String>) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
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
fn validate_query_params(filter: &ResultFilter) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
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

    // --- sort_by ---
    if let Some(ref sort_by) = filter.sort_by {
        if !VALID_SORT_BY.contains(&sort_by.as_str()) {
            violations.push(format!(
                "invalid sort_by '{}'; must be one of: {}",
                sort_by,
                VALID_SORT_BY.join(", ")
            ));
        }
    }

    // --- sort_order ---
    if let Some(ref sort_order) = filter.sort_order {
        if !VALID_SORT_ORDER.contains(&sort_order.as_str()) {
            violations.push(format!(
                "invalid sort_order '{}'; must be one of: {}",
                sort_order,
                VALID_SORT_ORDER.join(", ")
            ));
        }
    }

    // --- offset + after_id mutual exclusion ---
    if filter.offset.is_some() && filter.after_id.is_some() {
        violations
            .push("offset and after_id are mutually exclusive; use one or the other".to_string());
    }

    // --- offset + limit deep pagination guard ---
    if let Some(offset) = filter.offset {
        let limit = filter.limit.unwrap_or(50);
        let sum = offset.saturating_add(limit);
        if sum > MAX_OFFSET_PLUS_LIMIT {
            violations.push(format!(
                "offset + limit must not exceed {MAX_OFFSET_PLUS_LIMIT}, got {sum}"
            ));
        }
    }

    // --- after_id length ---
    if let Some(ref after_id) = filter.after_id {
        if after_id.len() > MAX_ID_LENGTH {
            violations.push(format!(
                "after_id too long ({} chars); maximum is {MAX_ID_LENGTH}",
                after_id.len()
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
// Request-ID middleware
// ---------------------------------------------------------------------------

/// Middleware that reads `X-Request-ID` from the incoming request header
/// (or generates a new UUID v4 if absent), attaches it to a tracing span
/// for the request, and includes it in the response headers.
async fn request_id_middleware(request: Request<axum::body::Body>, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %request.method(),
        uri = %request.uri(),
    );
    let _guard = span.enter();

    tracing::debug!(request_id = %request_id, "processing request");

    let mut response = next.run(request).await;

    if let Ok(val) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(X_REQUEST_ID.clone(), val);
    }

    response
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
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    validate_raw_query(&raw_query)?;
    validate_query_params(&filter)?;

    let map_storage_err = |e: anyhow::Error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    };

    // Get the total count of matching results (ignoring limit/offset/after_id).
    let total = state
        .storage
        .count_results(&filter)
        .map_err(map_storage_err)?;

    // Fetch the actual page of results.
    let rows = state
        .storage
        .list_results(&filter)
        .map_err(map_storage_err)?;

    // Determine if there are more results beyond this page.
    let limit = filter.limit.unwrap_or(50).min(1000);
    let offset = filter.offset.unwrap_or(0);
    let has_more = if filter.after_id.is_some() {
        // For cursor-based pagination, there are more if we got a full page.
        rows.len() as u32 >= limit
    } else {
        // For offset-based pagination, compare offset + rows against total.
        (offset as u64) + (rows.len() as u64) < total
    };

    let paginated = PaginatedResponse {
        data: rows,
        total,
        limit,
        offset,
        has_more,
    };

    // Build response with pagination headers for clients that prefer them.
    let mut response = Json(paginated).into_response();
    let hdrs = response.headers_mut();
    if let Ok(val) = HeaderValue::from_str(&total.to_string()) {
        hdrs.insert(HeaderName::from_static("x-total-count"), val);
    }
    hdrs.insert(
        HeaderName::from_static("x-has-more"),
        HeaderValue::from_static(if has_more { "true" } else { "false" }),
    );

    Ok(response)
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

/// Individual check status for the readiness probe.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReadinessChecks {
    /// Database connectivity: `"ok"` or `"error"`.
    pub db: String,
    /// Indexing progress: `"ok"` if at least one block has been indexed, `"stale"` otherwise.
    pub indexing: String,
}

/// Readiness response with per-check detail.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReadinessResponse {
    /// Overall readiness: `true` only when all checks pass.
    pub ready: bool,
    /// Per-subsystem check results.
    pub checks: ReadinessChecks,
    /// The last indexed block number (0 if none indexed yet).
    pub last_indexed_block: u64,
}

/// Readiness probe for Kubernetes.
///
/// Returns 200 if and only if:
/// 1. Storage backend is healthy (no lock poisoning)
/// 2. Database is reachable (connectivity check via trivial query)
/// 3. The indexer has processed at least one block (`last_indexed_block > 0`)
///
/// Otherwise returns 503 Service Unavailable so K8s does not route traffic
/// to a pod that has not caught up yet.
///
/// The response includes a `checks` object with individual check statuses:
/// - `db`: `"ok"` when the storage backend is healthy and reachable, `"error"` otherwise.
/// - `indexing`: `"ok"` when at least one block has been indexed, `"stale"` otherwise.
pub(crate) async fn handle_ready(State(state): State<AppState>) -> impl IntoResponse {
    let backend_healthy = state.storage.is_healthy();
    let connectivity_ok = state.storage.check_connectivity().is_ok();
    let db_ok = backend_healthy && connectivity_ok;

    let last_block = state.storage.get_last_indexed_block();
    let indexing_ok = last_block > 0;

    let ready = db_ok && indexing_ok;

    let body = ReadinessResponse {
        ready,
        checks: ReadinessChecks {
            db: if db_ok {
                "ok".to_string()
            } else {
                "error".to_string()
            },
            indexing: if indexing_ok {
                "ok".to_string()
            } else {
                "stale".to_string()
            },
        },
        last_indexed_block: last_block,
    };
    if ready {
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
        let client_ip = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        tracing::warn!(
            target: "audit",
            event = "admin_reset_denied",
            client_ip = %client_ip,
            reason = "invalid_or_missing_api_key",
            "Admin reset authentication failed"
        );
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "missing or invalid X-Admin-Key header".to_string(),
            }),
        ));
    }

    state.storage.reset_lock_state();

    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    tracing::info!(
        target: "audit",
        event = "admin_reset",
        client_ip = %client_ip,
        "Admin reset executed successfully"
    );

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
        .route("/ready", get(handle_ready))
        .with_state(state)
        // WebSocket routes (different state type)
        .merge(ws_routes)
        // Request correlation ID middleware
        .layer(middleware::from_fn(request_id_middleware))
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
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.total, 1);
        assert_eq!(page.offset, 0);
        assert!(!page.has_more);
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
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].id, "0x02");
        assert_eq!(page.data[0].status, "challenged");
        assert_eq!(page.data[0].challenger.as_deref(), Some("0xc"));
    }

    #[tokio::test]
    async fn test_list_results_filter_by_submitter() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xaaa111", 1)
            .unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xbbb222", 2)
            .unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?submitter=0xbbb222")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].submitter, "0xbbb222");
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
        s.insert_result("0x01", "0xm", "0xi", "0xa", 100).unwrap();
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
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].id, "0x02"); // block 50 first
        assert_eq!(page.data[1].id, "0x03"); // block 30
        assert_eq!(page.data[2].id, "0x01"); // block 10 last
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
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.total, 10);
        assert_eq!(page.limit, 3);
        assert!(page.has_more);
    }

    #[tokio::test]
    async fn test_list_results_sort_by_block_number_asc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 50).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 30).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=block_number&sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].id, "0x01"); // block 10 first (ascending)
        assert_eq!(page.data[1].id, "0x03"); // block 30
        assert_eq!(page.data[2].id, "0x02"); // block 50 last
    }

    #[tokio::test]
    async fn test_list_results_sort_by_status() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
        s.update_result_status("0x02", "challenged", Some("0xc"))
            .unwrap();
        s.update_result_status("0x03", "finalized", None).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=status&sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        // Alphabetically ascending: challenged < finalized < submitted
        assert_eq!(page.data[0].status, "challenged");
        assert_eq!(page.data[1].status, "finalized");
        assert_eq!(page.data[2].status, "submitted");
    }

    #[tokio::test]
    async fn test_list_results_sort_by_submitted_at() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 20).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 30).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        // submitted_at maps to the timestamp column; all default to 0 in
        // test storage, so the secondary tiebreaker (id ASC) determines order.
        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=submitted_at&sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        // All timestamps are 0, so tiebreak is id ASC
        assert_eq!(page.data[0].id, "0x01");
        assert_eq!(page.data[1].id, "0x02");
        assert_eq!(page.data[2].id, "0x03");
    }

    #[tokio::test]
    async fn test_list_results_invalid_sort_by_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=id")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("invalid sort_by"),
            "expected 'invalid sort_by' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_list_results_invalid_sort_order_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_order=random")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("invalid sort_order"),
            "expected 'invalid sort_order' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_list_results_default_sort_is_block_desc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 50).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 30).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        // No sort_by or sort_order -> defaults to block_number DESC
        let req = axum::http::Request::builder()
            .uri("/api/v1/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data[0].block_number, 50);
        assert_eq!(page.data[1].block_number, 30);
        assert_eq!(page.data[2].block_number, 10);
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
            err.error.contains("limit must be between")
                && err.error.contains("submitter must be a hex"),
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
        assert!(!is_valid_hex("0x")); // no digits after prefix
        assert!(!is_valid_hex("abc")); // missing 0x prefix
        assert!(!is_valid_hex("0xZZZ")); // invalid hex digits
        assert!(!is_valid_hex("")); // empty
        assert!(!is_valid_hex("0xab cd")); // space
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
        let retry_secs: u64 = retry_after.unwrap().to_str().unwrap().parse().unwrap();
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

    // -----------------------------------------------------------------------
    // Admin reset endpoint tests
    // -----------------------------------------------------------------------

    /// All admin reset tests run in a single test to avoid env var races.
    /// Multiple tests that set/remove ADMIN_API_KEY cannot run in parallel
    /// since env vars are process-global.
    #[tokio::test]
    async fn test_admin_reset_auth_scenarios() {
        // --- Scenario 1: no header at all → 401 ---
        std::env::remove_var("ADMIN_API_KEY");
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("missing or invalid"),
            "expected auth error, got: {}",
            err.error
        );

        // --- Scenario 2: wrong key → 401 ---
        std::env::set_var("ADMIN_API_KEY", "correct-secret-key");
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .header("x-admin-key", "wrong-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // --- Scenario 3: valid key → 200 ---
        std::env::set_var("ADMIN_API_KEY", "test-admin-key-123");
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .header("x-admin-key", "test-admin-key-123")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["message"], "Lock state reset");

        // --- Scenario 4: env var unset → 401 ---
        std::env::remove_var("ADMIN_API_KEY");
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .header("x-admin-key", "anything")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // -----------------------------------------------------------------------
    // Rate limit response header tests (X-RateLimit-* on 200 responses)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_rate_limit_headers_present_on_success() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();

        // Use a small limit so we can verify the remaining count decreases.
        let rl_config = RateLimitConfig {
            max_requests: 10,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // X-RateLimit-Limit must be present and equal to max_requests
        let limit_hdr = resp
            .headers()
            .get("x-ratelimit-limit")
            .expect("X-RateLimit-Limit header must be present on 200 responses");
        assert_eq!(limit_hdr.to_str().unwrap(), "10");

        // X-RateLimit-Remaining must be present (10 - 1 = 9)
        let remaining_hdr = resp
            .headers()
            .get("x-ratelimit-remaining")
            .expect("X-RateLimit-Remaining header must be present on 200 responses");
        assert_eq!(remaining_hdr.to_str().unwrap(), "9");

        // X-RateLimit-Reset must be present and > 0
        let reset_hdr = resp
            .headers()
            .get("x-ratelimit-reset")
            .expect("X-RateLimit-Reset header must be present on 200 responses");
        let reset_secs: u64 = reset_hdr.to_str().unwrap().parse().unwrap();
        assert!(
            reset_secs > 0 && reset_secs <= 61,
            "reset should be between 1 and 61 seconds, got {}",
            reset_secs
        );
    }

    #[tokio::test]
    async fn test_rate_limit_remaining_decreases() {
        let s = test_storage();

        let rl_config = RateLimitConfig {
            max_requests: 5,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        // Make 3 requests and verify remaining decreases each time.
        for i in 0u32..3 {
            let req = axum::http::Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let remaining: u32 = resp
                .headers()
                .get("x-ratelimit-remaining")
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                remaining,
                5 - i - 1,
                "after request {}, remaining should be {}",
                i + 1,
                5 - i - 1
            );
        }
    }

    #[tokio::test]
    async fn test_rate_limit_headers_on_429() {
        let s = test_storage();

        // Allow only 1 request per minute so the 2nd is rate limited.
        let rl_config = RateLimitConfig {
            max_requests: 1,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        // First request succeeds.
        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second request is rate limited.
        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // 429 responses must also carry the rate-limit headers.
        let limit_hdr = resp
            .headers()
            .get("x-ratelimit-limit")
            .expect("X-RateLimit-Limit must be present on 429 responses");
        assert_eq!(limit_hdr.to_str().unwrap(), "1");

        let remaining_hdr = resp
            .headers()
            .get("x-ratelimit-remaining")
            .expect("X-RateLimit-Remaining must be present on 429 responses");
        assert_eq!(remaining_hdr.to_str().unwrap(), "0");

        let reset_hdr = resp
            .headers()
            .get("x-ratelimit-reset")
            .expect("X-RateLimit-Reset must be present on 429 responses");
        let reset_secs: u64 = reset_hdr.to_str().unwrap().parse().unwrap();
        assert!(
            reset_secs > 0 && reset_secs <= 61,
            "reset should be between 1 and 61 seconds, got {}",
            reset_secs
        );
    }

    #[tokio::test]
    async fn test_rate_limit_headers_on_api_endpoint() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();

        let rl_config = RateLimitConfig {
            max_requests: 50,
            window: std::time::Duration::from_secs(60),
        };
        let app = build_app(s, test_broadcaster(), rl_config);

        // Test on a versioned API endpoint (not just /health)
        let req = axum::http::Request::builder()
            .uri("/api/v1/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        assert!(
            resp.headers().get("x-ratelimit-limit").is_some(),
            "X-RateLimit-Limit must be present on /api/v1/results"
        );
        assert!(
            resp.headers().get("x-ratelimit-remaining").is_some(),
            "X-RateLimit-Remaining must be present on /api/v1/results"
        );
        assert!(
            resp.headers().get("x-ratelimit-reset").is_some(),
            "X-RateLimit-Reset must be present on /api/v1/results"
        );

        assert_eq!(
            resp.headers()
                .get("x-ratelimit-limit")
                .unwrap()
                .to_str()
                .unwrap(),
            "50"
        );
        assert_eq!(
            resp.headers()
                .get("x-ratelimit-remaining")
                .unwrap()
                .to_str()
                .unwrap(),
            "49"
        );
    }

    // -----------------------------------------------------------------------
    // Readiness probe tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_ready_returns_503_when_no_blocks_indexed() {
        let s = test_storage();
        // Do not set last_indexed_block -- defaults to 0
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let readiness: super::ReadinessResponse = serde_json::from_slice(&body).unwrap();
        assert!(!readiness.ready);
        assert_eq!(readiness.last_indexed_block, 0);
        assert_eq!(
            readiness.checks.indexing, "stale",
            "indexing check should be stale when no blocks indexed"
        );
    }

    #[tokio::test]
    async fn test_ready_returns_200_when_blocks_indexed() {
        let s = test_storage();
        s.set_last_indexed_block(42).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let readiness: super::ReadinessResponse = serde_json::from_slice(&body).unwrap();
        assert!(readiness.ready);
        assert_eq!(readiness.last_indexed_block, 42);
        assert_eq!(readiness.checks.db, "ok");
        assert_eq!(readiness.checks.indexing, "ok");
    }

    #[tokio::test]
    async fn test_ready_checks_json_structure() {
        // Verify the response includes the expected fields and omits
        // "reason" when the service is ready (skip_serializing_if).
        let s = test_storage();
        s.set_last_indexed_block(10).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ready"], true);
        assert!(
            v.get("last_indexed_block").is_some(),
            "missing 'last_indexed_block' field"
        );
        // "reason" field was removed; verify it is not present
        assert!(v.get("reason").is_none(), "reason field should not exist");
        // "checks" should be present
        assert!(v.get("checks").is_some(), "missing 'checks' field");
    }

    #[tokio::test]
    async fn test_ready_returns_503_when_storage_unhealthy() {
        let sqlite = super::super::SqliteStorage::open_in_memory().unwrap();
        sqlite
            .lock_poisoned
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let s: Arc<dyn super::super::Storage> = Arc::new(sqlite);
        s.set_last_indexed_block(100).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let readiness: super::ReadinessResponse = serde_json::from_slice(&body).unwrap();
        assert!(!readiness.ready);
        assert_eq!(
            readiness.checks.db, "error",
            "db check should be error when storage is unhealthy"
        );
    }

    #[tokio::test]
    async fn test_ready_db_connectivity_check_passes() {
        let s = test_storage();
        s.set_last_indexed_block(5).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let readiness: super::ReadinessResponse = serde_json::from_slice(&body).unwrap();
        assert!(readiness.ready);
        assert_eq!(readiness.checks.db, "ok");
        assert_eq!(readiness.checks.indexing, "ok");
    }

    #[tokio::test]
    async fn test_ready_response_has_required_fields() {
        let s = test_storage();
        s.set_last_indexed_block(1).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v.get("ready").is_some(), "missing 'ready' field");
        assert!(
            v.get("last_indexed_block").is_some(),
            "missing 'last_indexed_block' field"
        );
    }

    // -----------------------------------------------------------------------
    // Sorting tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_sort_by_block_number_asc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 30).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 20).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=block_number&sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].block_number, 10);
        assert_eq!(page.data[1].block_number, 20);
        assert_eq!(page.data[2].block_number, 30);
    }

    #[tokio::test]
    async fn test_sort_by_block_number_desc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 30).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 20).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=block_number&sort_order=desc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].block_number, 30);
        assert_eq!(page.data[1].block_number, 20);
        assert_eq!(page.data[2].block_number, 10);
    }

    #[tokio::test]
    async fn test_sort_by_status_asc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
        s.update_result_status("0x02", "finalized", None).unwrap();
        s.update_result_status("0x03", "challenged", None).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=status&sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].status, "challenged");
        assert_eq!(page.data[1].status, "finalized");
        assert_eq!(page.data[2].status, "submitted");
    }

    #[tokio::test]
    async fn test_sort_by_submitter_asc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xcharlie", 1)
            .unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xalice", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xbob", 3).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=submitter&sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].submitter, "0xalice");
        assert_eq!(page.data[1].submitter, "0xbob");
        assert_eq!(page.data[2].submitter, "0xcharlie");
    }

    #[tokio::test]
    async fn test_sort_by_submitter_desc() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xcharlie", 1)
            .unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xalice", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xbob", 3).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=submitter&sort_order=desc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].submitter, "0xcharlie");
        assert_eq!(page.data[1].submitter, "0xbob");
        assert_eq!(page.data[2].submitter, "0xalice");
    }

    #[tokio::test]
    async fn test_default_sort_is_block_number_desc() {
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
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].block_number, 50);
        assert_eq!(page.data[1].block_number, 30);
        assert_eq!(page.data[2].block_number, 10);
    }

    #[tokio::test]
    async fn test_invalid_sort_by_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_by=nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("invalid sort_by"),
            "expected 'invalid sort_by' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_invalid_sort_order_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_order=updown")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("invalid sort_order"),
            "expected 'invalid sort_order' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_sort_order_without_sort_by_uses_default_column() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 10).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 50).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 30).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?sort_order=asc")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);
        assert_eq!(page.data[0].block_number, 10);
        assert_eq!(page.data[1].block_number, 30);
        assert_eq!(page.data[2].block_number, 50);
    }

    // -----------------------------------------------------------------------
    // Pagination tests
    // -----------------------------------------------------------------------

    /// Helper: send GET and return (status, headers, body bytes).
    async fn get_with_headers(
        app: axum::Router,
        uri: &str,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let req = axum::http::Request::builder()
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    #[tokio::test]
    async fn test_pagination_headers_present_on_list() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, _body) = get_with_headers(app, "/api/v1/results").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers.get("x-total-count").is_some(),
            "missing x-total-count header"
        );
        assert!(
            headers.get("x-has-more").is_some(),
            "missing x-has-more header"
        );
    }

    #[tokio::test]
    async fn test_pagination_total_count_value() {
        let s = test_storage();
        for i in 0..5u64 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, _body) = get_with_headers(app, "/api/v1/results?limit=2").await;
        assert_eq!(status, StatusCode::OK);

        let total: u64 = headers
            .get("x-total-count")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            total, 5,
            "x-total-count should reflect all matching results"
        );
    }

    #[tokio::test]
    async fn test_pagination_has_more_true() {
        let s = test_storage();
        for i in 0..10u64 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, _body) = get_with_headers(app, "/api/v1/results?limit=3").await;
        assert_eq!(status, StatusCode::OK);

        let has_more = headers.get("x-has-more").unwrap().to_str().unwrap();
        assert_eq!(has_more, "true", "should have more results");
    }

    #[tokio::test]
    async fn test_pagination_has_more_false() {
        let s = test_storage();
        for i in 0..3u64 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, body) = get_with_headers(app, "/api/v1/results").await;
        assert_eq!(status, StatusCode::OK);

        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 3);

        let has_more = headers.get("x-has-more").unwrap().to_str().unwrap();
        assert_eq!(has_more, "false", "should NOT have more results");
    }

    #[tokio::test]
    async fn test_offset_pagination() {
        let s = test_storage();
        for i in 0..5u64 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i * 10)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, body) =
            get_with_headers(app.clone(), "/api/v1/results?limit=2&offset=0").await;
        assert_eq!(status, StatusCode::OK);
        let page1: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page1.data.len(), 2);
        assert_eq!(headers.get("x-has-more").unwrap().to_str().unwrap(), "true");

        let (status, _headers, body) =
            get_with_headers(app.clone(), "/api/v1/results?limit=2&offset=2").await;
        assert_eq!(status, StatusCode::OK);
        let page2: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page2.data.len(), 2);

        let (status, headers, body) =
            get_with_headers(app, "/api/v1/results?limit=2&offset=4").await;
        assert_eq!(status, StatusCode::OK);
        let page3: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page3.data.len(), 1);
        assert_eq!(
            headers.get("x-has-more").unwrap().to_str().unwrap(),
            "false"
        );

        let all_ids: Vec<String> = page1
            .data
            .iter()
            .chain(page2.data.iter())
            .chain(page3.data.iter())
            .map(|r| r.id.clone())
            .collect();
        let unique: std::collections::HashSet<&String> = all_ids.iter().collect();
        assert_eq!(unique.len(), 5, "all 5 results should appear across pages");
    }

    #[tokio::test]
    async fn test_after_id_cursor_pagination() {
        let s = test_storage();
        for i in 0..5u64 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i * 10)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, _headers, body) =
            get_with_headers(app.clone(), "/api/v1/results?limit=2").await;
        assert_eq!(status, StatusCode::OK);
        let page1: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page1.data.len(), 2);

        let last_id = &page1.data.last().unwrap().id;
        let uri = format!("/api/v1/results?limit=2&after_id={}", last_id);
        let (status, _headers, body) = get_with_headers(app.clone(), &uri).await;
        assert_eq!(status, StatusCode::OK);
        let page2: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page2.data.len(), 2);

        for row in &page2.data {
            assert!(
                !page1.data.iter().any(|r| r.id == row.id),
                "after_id page should not overlap with previous page"
            );
        }

        let last_id = &page2.data.last().unwrap().id;
        let uri = format!("/api/v1/results?limit=2&after_id={}", last_id);
        let (status, headers, body) = get_with_headers(app, &uri).await;
        assert_eq!(status, StatusCode::OK);
        let page3: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page3.data.len(), 1);
        assert_eq!(
            headers.get("x-has-more").unwrap().to_str().unwrap(),
            "false"
        );
    }

    #[tokio::test]
    async fn test_offset_and_after_id_mutually_exclusive() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?offset=5&after_id=0x01")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("mutually exclusive"),
            "expected mutual exclusion error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_deep_pagination_rejected() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?offset=9500&limit=600")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("offset + limit must not exceed 10000"),
            "expected deep pagination error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_offset_within_limit_accepted() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?offset=100&limit=50")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_backward_compat_no_offset_no_after_id() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, body) = get_with_headers(app, "/api/v1/results").await;
        assert_eq!(status, StatusCode::OK);

        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 2);
        assert_eq!(headers.get("x-total-count").unwrap().to_str().unwrap(), "2");
        assert_eq!(
            headers.get("x-has-more").unwrap().to_str().unwrap(),
            "false"
        );
    }

    #[tokio::test]
    async fn test_total_count_with_filter() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.insert_result("0x03", "0xm", "0xi", "0xa", 3).unwrap();
        s.update_result_status("0x01", "finalized", None).unwrap();
        s.update_result_status("0x02", "finalized", None).unwrap();
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, headers, body) =
            get_with_headers(app, "/api/v1/results?status=finalized&limit=1").await;
        assert_eq!(status, StatusCode::OK);

        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 1, "should return 1 row (limit=1)");

        let total: u64 = headers
            .get("x-total-count")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            total, 2,
            "x-total-count should be 2 (all matching finalized)"
        );

        let has_more = headers.get("x-has-more").unwrap().to_str().unwrap();
        assert_eq!(has_more, "true", "should have more results");
    }

    #[tokio::test]
    async fn test_after_id_too_long_returns_400() {
        let app = build_app(test_storage(), test_broadcaster(), test_rate_limit_config());

        let long_id = "x".repeat(200);
        let req = axum::http::Request::builder()
            .uri(&format!("/api/v1/results?after_id={}", long_id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("after_id too long"),
            "expected 'after_id too long' in error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_paginated_response_fields() {
        let s = test_storage();
        for i in 0..5u64 {
            s.insert_result(&format!("0x{:02x}", i), "0xm", "0xi", "0xa", i * 10)
                .unwrap();
        }
        let app = build_app(s, test_broadcaster(), test_rate_limit_config());

        let (status, _headers, body) =
            get_with_headers(app, "/api/v1/results?limit=2&offset=1").await;
        assert_eq!(status, StatusCode::OK);

        let page: PaginatedResponse<ResultRow> = serde_json::from_slice(&body).unwrap();
        assert_eq!(page.data.len(), 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.limit, 2);
        assert_eq!(page.offset, 1);
        assert!(page.has_more);
    }
}
