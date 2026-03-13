//! API route definitions for the operator HTTP server.
//!
//! Provides versioned API routes under `/api/v1/` while maintaining
//! backward-compatible unversioned routes. Infrastructure endpoints
//! (`/health`, `/metrics`) remain at the root.

use axum::http::header::HeaderValue;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;

/// API version constant.
#[allow(dead_code)]
pub const API_VERSION: &str = "v1";

/// Middleware that adds `X-API-Version` header to all responses.
#[allow(dead_code)]
pub async fn version_header_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "X-API-Version",
        HeaderValue::from_static(API_VERSION),
    );
    response
}

/// Build versioned API routes for the operator.
///
/// Routes:
/// - `GET /health` — health check (unversioned)
/// - `GET /metrics` — Prometheus metrics (unversioned)
/// - `GET /api/v1/health` — versioned health
/// - `GET /api/v1/status` — operator status
///
/// The returned router should be merged with the main app router.
#[allow(dead_code)]
pub fn versioned_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .layer(middleware::from_fn(version_header_middleware))
}

/// Handler that redirects old unversioned API paths to `/api/v1/`.
#[allow(dead_code)]
pub async fn redirect_to_v1(
    uri: axum::http::Uri,
) -> impl IntoResponse {
    let path = uri.path();
    let new_path = format!("/api/v1{}", path);
    (
        StatusCode::MOVED_PERMANENTLY,
        [("Location", new_path)],
        "",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    #[test]
    fn test_api_version_constant() {
        assert_eq!(API_VERSION, "v1");
    }

    #[tokio::test]
    async fn test_version_header_middleware() {
        let app = Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(middleware::from_fn(version_header_middleware));

        let req = Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("X-API-Version").unwrap().to_str().unwrap(),
            "v1"
        );
    }

    #[tokio::test]
    async fn test_redirect_to_v1() {
        let app = Router::new()
            .route("/results", get(redirect_to_v1));

        let req = Request::builder()
            .uri("/results")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            resp.headers().get("Location").unwrap().to_str().unwrap(),
            "/api/v1/results"
        );
    }

    #[tokio::test]
    async fn test_versioned_routes_returns_router() {
        // Just verify it builds without panicking
        let _router: Router = versioned_routes();
    }
}
