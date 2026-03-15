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

    #[tokio::test]
    async fn test_list_results_filter_by_status() {
        let s = test_storage();
        s.insert_result("0x01", "0xm", "0xi", "0xa", 1).unwrap();
        s.insert_result("0x02", "0xm", "0xi", "0xa", 2).unwrap();
        s.update_result_status("0x02", "challenged", Some("0xc"))
            .unwrap();
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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

        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
        let app = build_app(s, test_broadcaster());

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
}
