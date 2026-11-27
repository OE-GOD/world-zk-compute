//! API route definitions for the TEE indexer.
//!
//! All data endpoints live under `/api/v1/`. Infrastructure endpoints
//! (`/health`, `/ws/events`) remain at the root (unversioned).
//! Old unversioned paths (`/results`, `/stats`) are preserved as
//! backward-compatible aliases.

use axum::extract::{ConnectInfo, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

use super::rate_limit::{client_ip, RateLimitLayer};
#[cfg(test)]
use super::rate_limit::{RateLimitConfig, TrustedProxyConfig};
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

/// Looser hex-prefix check: starts with `0x` followed by at least one
/// character. Used for fields like `model_hash` and `after_id` which may
/// contain non-strict-hex identifiers (e.g. `0xmodelA`).
fn has_hex_prefix(s: &str) -> bool {
    s.starts_with("0x") && s.len() > 2
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
        } else if !has_hex_prefix(model_hash) {
            violations.push(format!(
                "model_hash must be a hex string starting with 0x, got '{model_hash}'"
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

    // --- after_id format ---
    if let Some(ref after_id) = filter.after_id {
        if after_id.len() > MAX_ID_LENGTH {
            violations.push(format!(
                "after_id too long ({} chars); maximum is {MAX_ID_LENGTH}",
                after_id.len()
            ));
        } else if !has_hex_prefix(after_id) {
            violations.push(format!(
                "after_id must be a hex string starting with 0x, got '{after_id}'"
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
// API-key middleware
// ---------------------------------------------------------------------------

/// Optional API key authentication middleware.
///
/// When the captured `api_key` is `Some(key)`, requires
/// `Authorization: Bearer <key>` on all requests routed through this
/// middleware. Returns 401 if missing/wrong.
/// When `api_key` is `None`, this is a no-op pass-through.
///
/// The key is captured once at router-construction time (from `API_KEY`
/// env var) so that tests mutating the env do not race with concurrent
/// non-serial tests.
async fn api_key_middleware(
    axum::extract::State(api_key): axum::extract::State<Option<String>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(ref expected) = api_key else {
        return next.run(req).await;
    };

    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.starts_with("Bearer ") && &value[7..] == expected.as_str() => {
            next.run(req).await
        }
        _ => {
            let body = serde_json::json!({
                "error": "Unauthorized",
                "message": "Valid API key required. Set Authorization: Bearer <key> header."
            });
            (
                StatusCode::UNAUTHORIZED,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                serde_json::to_string(&body).unwrap_or_default(),
            )
                .into_response()
        }
    }
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
    // Clamp limit to [1, 1000] with default 100 (must match storage layer).
    let limit = filter.limit.unwrap_or(100).clamp(1, 1000);
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
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // The audit log names the client the way the rate limiter counts it:
    // forwarding headers are read only from a trusted proxy, and from the
    // right, so a client cannot write the address that gets logged.
    let client_ip = client_ip(
        connect_info.map(|ci| ci.0.ip()),
        &headers,
        &state.trusted_proxies,
    );

    // Authenticate via X-Admin-Key header
    let expected_key = std::env::var("ADMIN_API_KEY").unwrap_or_default();
    let provided_key = headers
        .get("x-admin-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if expected_key.is_empty() || provided_key != expected_key {
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
/// Reads `API_KEY` from the environment at call time. When set (non-empty),
/// all `/api/v1/*` and backward-compat data routes require
/// `Authorization: Bearer <key>`. Health, readiness, and WebSocket endpoints
/// remain public.
///
/// `rate_layer` limits the requests per client within a sliding time window
/// (`RATE_LIMIT_PER_IP`, default 100 req/min). Its trusted proxies also decide
/// which address the per-IP WebSocket limit counts a connection under.
pub fn build_app(
    storage: Arc<dyn Storage>,
    broadcaster: Arc<EventBroadcaster>,
    rate_layer: RateLimitLayer,
) -> Router {
    let api_key: Option<String> = std::env::var("API_KEY").ok().filter(|k| !k.is_empty());
    build_app_with_layer(storage, broadcaster, rate_layer, api_key)
}

/// Build the application router with an explicit API key value and a rate
/// limiter without background cleanup. Tests call this directly to avoid
/// env-var mutation races.
#[cfg(test)]
pub fn build_app_with_key(
    storage: Arc<dyn Storage>,
    broadcaster: Arc<EventBroadcaster>,
    rate_limit_config: RateLimitConfig,
    trusted_proxies: TrustedProxyConfig,
    api_key: Option<String>,
) -> Router {
    build_app_with_layer(
        storage,
        broadcaster,
        RateLimitLayer::new_without_cleanup(rate_limit_config, trusted_proxies),
        api_key,
    )
}

fn build_app_with_layer(
    storage: Arc<dyn Storage>,
    broadcaster: Arc<EventBroadcaster>,
    rate_layer: RateLimitLayer,
    api_key: Option<String>,
) -> Router {
    let state = AppState {
        storage,
        broadcaster: broadcaster.clone(),
        trusted_proxies: rate_layer.trusted_proxies().clone(),
    };

    // Versioned API routes under /api/v1
    let mut v1 = Router::new()
        .route("/results", get(handle_list_results))
        .route("/results/:id", get(handle_get_result))
        .route("/stats", get(handle_stats));

    // Only register admin routes when ADMIN_API_KEY is configured
    let admin_key_set = std::env::var("ADMIN_API_KEY").is_ok_and(|k| !k.is_empty());
    if admin_key_set {
        v1 = v1.route("/admin/reset", post(handle_admin_reset));
    }

    // WebSocket route uses its own state type
    let ws_routes = Router::new()
        .route("/ws/events", get(websocket::ws_handler))
        .with_state(websocket::WsState {
            broadcaster,
            trusted_proxies: rate_layer.trusted_proxies().clone(),
        });

    // Group API routes behind the optional API-key middleware so that
    // /health, /ready, and /ws/events remain publicly accessible.
    let api_routes = Router::new()
        .nest("/api/v1", v1.clone())
        // Backward compat: keep unversioned routes working
        .route("/results", get(handle_list_results))
        .route("/results/:id", get(handle_get_result))
        .route("/stats", get(handle_stats))
        .layer(middleware::from_fn_with_state(api_key, api_key_middleware));

    Router::new()
        // API routes (protected by optional API-key auth)
        .merge(api_routes)
        // Unversioned infrastructure endpoints (always public)
        .route("/health", get(handle_health))
        .route("/ready", get(handle_ready))
        .with_state(state)
        // WebSocket routes (different state type, always public)
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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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

        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let long_id = "x".repeat(200);
        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/results/{}", long_id))
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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?submitter=0xabcdef1234567890")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_submitter_0x_only_returns_400() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let long_status = "a".repeat(300);
        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/results?status={}", long_status))
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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let long_submitter = format!("0x{}", "a".repeat(300));
        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/results?submitter={}", long_submitter))
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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let long_hash = "h".repeat(300);
        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/results?model_hash={}", long_hash))
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
    async fn test_model_hash_invalid_hex_returns_400() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?model_hash=not_hex_value")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("model_hash must be a hex string"),
            "expected prefix validation error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_model_hash_valid_hex_accepted() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?model_hash=0xabcdef1234")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_after_id_invalid_hex_returns_400() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?after_id=not_a_hex_id")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("after_id must be a hex string"),
            "expected prefix validation error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_after_id_valid_hex_accepted() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?after_id=0xabc123")
            .body(Body::empty())
            .unwrap();

        // Should not get a 400 for format -- may get 200 with empty results
        // since 0xabc123 doesn't exist, but the validation should pass.
        let resp = app.oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "valid hex after_id should not be rejected by validation"
        );
    }

    #[tokio::test]
    async fn test_multiple_violations_returned_together() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            rl_config,
            TrustedProxyConfig::default(),
            None,
        );

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

        // Allow only 1 request per minute. Trust 192.168.0.0/16 as proxy.
        let rl_config = RateLimitConfig {
            max_requests: 1,
            window: std::time::Duration::from_secs(60),
        };
        let trusted = TrustedProxyConfig::from_env_str("192.168.0.0/16");
        let app = build_app_with_key(s, test_broadcaster(), rl_config, trusted, None);

        // First request from IP 10.0.0.1 (via trusted proxy) should succeed
        let mut req = axum::http::Request::builder()
            .uri("/health")
            .header("x-forwarded-for", "10.0.0.1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1)),
                12345,
            )));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second request from same IP should be blocked
        let mut req = axum::http::Request::builder()
            .uri("/health")
            .header("x-forwarded-for", "10.0.0.1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1)),
                12345,
            )));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Request from different IP should still succeed
        let mut req = axum::http::Request::builder()
            .uri("/health")
            .header("x-forwarded-for", "10.0.0.2")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 1)),
                12345,
            )));
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_not_bypassed_by_client_written_xff_entries() {
        // Behind a trusted proxy on 127.0.0.1, one client sends a different
        // made-up X-Forwarded-For value with every request. The proxy appends
        // the real address, 198.51.100.7, which is what gets counted.
        let rl_config = RateLimitConfig {
            max_requests: 3,
            window: std::time::Duration::from_secs(60),
        };
        let trusted = TrustedProxyConfig::from_env_str("127.0.0.0/8");
        let app = build_app_with_key(test_storage(), test_broadcaster(), rl_config, trusted, None);

        let mut statuses = Vec::new();
        for i in 0..10u8 {
            let mut req = axum::http::Request::builder()
                .uri("/health")
                .header("x-forwarded-for", format!("203.0.113.{i}, 198.51.100.7"))
                .body(Body::empty())
                .unwrap();
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    12345,
                )));
            statuses.push(app.clone().oneshot(req).await.unwrap().status());
        }
        let allowed = statuses.iter().filter(|s| **s == StatusCode::OK).count();
        assert_eq!(allowed, 3, "{statuses:?}");
    }

    // -----------------------------------------------------------------------
    // WebSocket limits
    // -----------------------------------------------------------------------

    /// Serve `app` on a loopback port, with the socket peer available to the
    /// handlers as in production.
    async fn serve_with_peer(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        addr
    }

    type ClientWs = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// Open `/ws/events` as a proxy would for the client `xff`.
    async fn ws_connect_via_proxy(
        addr: std::net::SocketAddr,
        xff: &str,
    ) -> Result<ClientWs, tokio_tungstenite::tungstenite::Error> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = format!("ws://{addr}/ws/events")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("x-forwarded-for", xff.parse().unwrap());
        tokio_tungstenite::connect_async(req)
            .await
            .map(|(ws, _)| ws)
    }

    #[tokio::test]
    async fn test_ws_per_ip_limit_counts_clients_behind_trusted_proxy() {
        let broadcaster = test_broadcaster();
        let per_ip = broadcaster.max_per_ip();
        let app = build_app_with_key(
            test_storage(),
            broadcaster.clone(),
            test_rate_limit_config(),
            TrustedProxyConfig::from_env_str("127.0.0.0/8"),
            None,
        );
        let addr = serve_with_peer(app).await;

        // More clients than the per-IP limit, all arriving through the proxy.
        let mut open = Vec::new();
        for i in 0..=per_ip {
            let ws = ws_connect_via_proxy(addr, &format!("203.0.113.{i}"))
                .await
                .unwrap_or_else(|e| panic!("client {i} refused: {e}"));
            open.push(ws);
        }
        let first: std::net::IpAddr = "203.0.113.0".parse().unwrap();
        let proxy: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(broadcaster.connections_for_ip(first), 1);
        assert_eq!(broadcaster.connections_for_ip(proxy), 0);

        // One client is still held to the per-IP limit.
        for _ in 1..per_ip {
            open.push(ws_connect_via_proxy(addr, "203.0.113.0").await.unwrap());
        }
        assert!(ws_connect_via_proxy(addr, "203.0.113.0").await.is_err());
    }

    #[tokio::test]
    async fn test_ws_oversized_client_message_closes_connection() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );
        let addr = serve_with_peer(app).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
            .await
            .unwrap();

        let big = "x".repeat(2 * websocket::MAX_CLIENT_MESSAGE_BYTES);
        ws.send(Message::Text(big)).await.unwrap();

        // The server drops the connection instead of buffering the message.
        let next = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("connection still open 5 s after an oversized message");
        assert!(
            !matches!(next, Some(Ok(Message::Text(_)))),
            "unexpected message: {next:?}"
        );
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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            rl_config,
            TrustedProxyConfig::default(),
            None,
        );

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

    /// Held by every test that sets or removes ADMIN_API_KEY: env vars are
    /// process-global, and the tests run in parallel.
    static ADMIN_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn test_admin_reset_auth_scenarios() {
        let _env = ADMIN_ENV_LOCK.lock().await;
        // --- Scenario 1: no ADMIN_API_KEY → route not registered → 404 ---
        std::env::remove_var("ADMIN_API_KEY");
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // --- Scenario 2: wrong key → 401 ---
        std::env::set_var("ADMIN_API_KEY", "correct-secret-key");
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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

        // --- Scenario 4: env var unset → route not registered → 404 ---
        std::env::remove_var("ADMIN_API_KEY");
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .header("x-admin-key", "anything")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// Collects what the `tracing` macros print on this thread until the
    /// returned guard is dropped.
    fn capture_logs() -> (
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        tracing::subscriber::DefaultGuard,
    ) {
        #[derive(Clone)]
        struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Sink(buf.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        (buf, tracing::subscriber::set_default(subscriber))
    }

    /// An admin reset request from `peer` with the given key and
    /// X-Forwarded-For header.
    fn admin_reset_request(peer: [u8; 4], key: &str, xff: &str) -> Request<Body> {
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/admin/reset")
            .header("x-admin-key", key)
            .header("x-forwarded-for", xff)
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                std::net::IpAddr::from(peer),
                12345,
            )));
        req
    }

    // The test runs on one thread (the default tokio test runtime), so the
    // thread-local log capture sees the handler's events.
    #[tokio::test]
    async fn test_admin_reset_audit_log_client_ip_cannot_be_chosen_by_the_client() {
        let _env = ADMIN_ENV_LOCK.lock().await;
        std::env::set_var("ADMIN_API_KEY", "audit-test-key");
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::from_env_str("10.0.0.0/8"),
            None,
        );
        let (logs, guard) = capture_logs();

        // Through the trusted proxy 10.0.0.1, which appended 203.0.113.50;
        // the client wrote 6.6.6.6 itself.
        let req = admin_reset_request([10, 0, 0, 1], "wrong-key", "6.6.6.6, 203.0.113.50");
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Straight from an untrusted peer, whose header is ignored.
        let req = admin_reset_request([198, 51, 100, 7], "audit-test-key", "6.6.6.6");
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        drop(guard);
        std::env::remove_var("ADMIN_API_KEY");
        let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        let line = |needle: &str| {
            logs.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no log line with {needle:?} in:\n{logs}"))
                .to_string()
        };
        assert!(
            line("admin_reset_denied").contains("client_ip=203.0.113.50"),
            "{logs}"
        );
        assert!(
            line("Admin reset executed successfully").contains("client_ip=198.51.100.7"),
            "{logs}"
        );
        assert!(!logs.contains("6.6.6.6"), "{logs}");
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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            rl_config,
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            rl_config,
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            rl_config,
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            rl_config,
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let long_id = "x".repeat(200);
        let req = axum::http::Request::builder()
            .uri(format!("/api/v1/results?after_id={}", long_id))
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
        let app = build_app_with_key(
            s,
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

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

    // -----------------------------------------------------------------------
    // T416: model_hash hex validation and after_id format tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_model_hash_not_hex_returns_400() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?model_hash=not_a_hex_value")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error
                .contains("model_hash must be a hex string starting with 0x"),
            "expected model_hash hex error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_model_hash_valid_long_hex_accepted() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?model_hash=0xabcdef1234567890")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_model_hash_bare_0x_returns_400() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?model_hash=0x")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error
                .contains("model_hash must be a hex string starting with 0x"),
            "expected model_hash hex error, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn test_after_id_bare_0x_returns_400() {
        let app = build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            None,
        );

        let req = axum::http::Request::builder()
            .uri("/api/v1/results?after_id=0x")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let err: ErrorResponse = serde_json::from_slice(&body).unwrap();
        assert!(
            err.error.contains("after_id must be a hex string"),
            "expected after_id validation error, got: {}",
            err.error
        );
    }

    // -----------------------------------------------------------------------
    // API key authentication tests
    // -----------------------------------------------------------------------

    /// Helper: build an app with the given API key baked in.
    fn build_app_with_test_key(api_key: Option<&str>) -> Router {
        build_app_with_key(
            test_storage(),
            test_broadcaster(),
            test_rate_limit_config(),
            TrustedProxyConfig::default(),
            api_key.map(|s| s.to_string()),
        )
    }

    #[tokio::test]
    async fn test_api_key_no_env_passes_through() {
        // When no API key is configured, requests pass through
        let app = build_app_with_test_key(None);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_key_rejects_wrong_key() {
        let app = build_app_with_test_key(Some("test-secret-key"));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats")
                    .header("Authorization", "Bearer wrong-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_api_key_accepts_correct_key() {
        let app = build_app_with_test_key(Some("test-secret-key"));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats")
                    .header("Authorization", "Bearer test-secret-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_key_health_always_public() {
        // /health should be accessible even when API key is configured
        let app = build_app_with_test_key(Some("test-secret-key"));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
