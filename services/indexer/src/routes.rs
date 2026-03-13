//! API route definitions for the TEE indexer.
//!
//! All data endpoints live under `/api/v1/`. Infrastructure endpoints
//! (`/health`, `/ws/events`) remain at the root (unversioned).
//! Old unversioned paths (`/results`, `/stats`) are preserved as
//! backward-compatible aliases.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;

use super::websocket::{self, EventBroadcaster};
use super::{AppState, ResultFilter, ResultRow, StatsResponse, Storage};

/// The current API version string.
pub const API_VERSION: &str = "v1";

/// X-API-Version header name (lowercase for HTTP/2 compat).
static X_API_VERSION: HeaderName = HeaderName::from_static("x-api-version");

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub(crate) struct ErrorResponse {
    error: String,
}

pub(crate) async fn handle_list_results(
    State(state): State<AppState>,
    Query(filter): Query<ResultFilter>,
) -> Result<Json<Vec<ResultRow>>, (StatusCode, Json<ErrorResponse>)> {
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
    Json(super::HealthResponse {
        status: "ok".to_string(),
        last_indexed_block: state.storage.get_last_indexed_block(),
        total_results: state.storage.get_total_results(),
    })
}

// ---------------------------------------------------------------------------
// Router construction
// ---------------------------------------------------------------------------

/// Build the full application router with versioned API, backward-compat
/// aliases, infrastructure endpoints, and `X-API-Version` response header.
pub fn build_app(storage: Arc<dyn Storage>, broadcaster: Arc<EventBroadcaster>) -> Router {
    let state = AppState {
        storage,
        broadcaster: broadcaster.clone(),
    };

    // Versioned API routes under /api/v1
    let v1 = Router::new()
        .route("/results", get(handle_list_results))
        .route("/results/:id", get(handle_get_result))
        .route("/stats", get(handle_stats));

    // WebSocket route uses its own state type
    let ws_routes = Router::new()
        .route("/ws/events", get(websocket::ws_handler))
        .with_state(broadcaster);

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

    #[tokio::test]
    async fn test_versioned_results_endpoint() {
        let s = test_storage();
        s.insert_result("0xabc", "0xm", "0xi", "0xa", 1).unwrap();
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
}
