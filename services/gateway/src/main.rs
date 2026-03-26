//! Unified API Gateway
//!
//! Routes requests to the appropriate backend service based on path prefix.
//!
//! Routing:
//!   /verify/*     → Verifier API (VERIFIER_URL)
//!   /proofs/*     → Proof Registry (REGISTRY_URL)
//!   /prove/*      → Proof Generator (GENERATOR_URL)
//!   /models/*     → Proof Generator (GENERATOR_URL)
//!   /health       → Aggregated health from all backends
//!
//! Environment:
//!   PORT: listen port (default 8080)
//!   VERIFIER_URL: verifier backend (default http://localhost:3000)
//!   REGISTRY_URL: registry backend (default http://localhost:3001)
//!   GENERATOR_URL: generator backend (default http://localhost:3002)

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

struct GatewayState {
    verifier_url: String,
    registry_url: String,
    generator_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    backends: serde_json::Value,
}

async fn health(State(state): State<Arc<GatewayState>>) -> Json<HealthResponse> {
    let mut backends = serde_json::Map::new();

    for (name, url) in [
        ("verifier", &state.verifier_url),
        ("registry", &state.registry_url),
        ("generator", &state.generator_url),
    ] {
        let health_url = format!("{}/health", url);
        let status = match state.client.get(&health_url).send().await {
            Ok(r) if r.status().is_success() => "healthy".to_string(),
            Ok(r) => format!("unhealthy (HTTP {})", r.status()),
            Err(e) => format!("unreachable ({})", e),
        };
        backends.insert(name.to_string(), serde_json::Value::String(status));
    }

    let all_healthy = backends.values().all(|v| v.as_str() == Some("healthy"));
    Json(HealthResponse {
        status: if all_healthy { "ok" } else { "degraded" }.to_string(),
        backends: serde_json::Value::Object(backends),
    })
}

async fn proxy(
    State(state): State<Arc<GatewayState>>,
    req: Request<Body>,
) -> impl IntoResponse {
    let path = req.uri().path();
    let backend_url = if path.starts_with("/verify") {
        &state.verifier_url
    } else if path.starts_with("/circuits") {
        &state.verifier_url
    } else if path.starts_with("/proofs") || path.starts_with("/stats") {
        &state.registry_url
    } else if path.starts_with("/prove") || path.starts_with("/models") {
        &state.generator_url
    } else {
        return (StatusCode::NOT_FOUND, "unknown route").into_response();
    };

    let target_url = format!(
        "{}{}{}",
        backend_url,
        path,
        req.uri()
            .query()
            .map(|q| format!("?{}", q))
            .unwrap_or_default()
    );

    let method = req.method().clone();
    let headers = req.headers().clone();

    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "body too large").into_response(),
    };

    let mut proxy_req = state.client.request(method, &target_url);
    for (key, val) in headers.iter() {
        if key != "host" {
            proxy_req = proxy_req.header(key, val);
        }
    }
    if !body_bytes.is_empty() {
        proxy_req = proxy_req.body(body_bytes.to_vec());
    }

    match proxy_req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = resp.text().await.unwrap_or_default();
            (status, body).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("backend error: {}", e),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = Arc::new(GatewayState {
        verifier_url: std::env::var("VERIFIER_URL")
            .unwrap_or_else(|_| "http://localhost:3000".to_string()),
        registry_url: std::env::var("REGISTRY_URL")
            .unwrap_or_else(|_| "http://localhost:3001".to_string()),
        generator_url: std::env::var("GENERATOR_URL")
            .unwrap_or_else(|_| "http://localhost:3002".to_string()),
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap(),
    });

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{port}");

    let app = Router::new()
        .route("/health", get(health))
        .fallback(proxy)
        .layer(CorsLayer::permissive())
        .with_state(state);

    tracing::info!("API gateway on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    fn test_state() -> Arc<GatewayState> {
        Arc::new(GatewayState {
            verifier_url: "http://localhost:19001".to_string(),
            registry_url: "http://localhost:19002".to_string(),
            generator_url: "http://localhost:19003".to_string(),
            client: reqwest::Client::new(),
        })
    }

    fn test_app() -> Router {
        let state = test_state();
        Router::new()
            .route("/health", get(health))
            .fallback(proxy)
            .layer(CorsLayer::permissive())
            .with_state(state)
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = test_app();
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Backends are unreachable in tests, so status should be "degraded"
        assert_eq!(json["status"], "degraded");
        assert!(json["backends"]["verifier"].is_string());
        assert!(json["backends"]["registry"].is_string());
        assert!(json["backends"]["generator"].is_string());
    }

    #[tokio::test]
    async fn test_unknown_route_returns_404() {
        let app = test_app();
        let req = Request::get("/unknown/path").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_proxy_verify_routes_to_verifier() {
        let app = test_app();
        let req = Request::post("/verify")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Backend is unreachable, so expect 502
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn test_proxy_proofs_routes_to_registry() {
        let app = test_app();
        let req = Request::get("/proofs/test-id").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn test_proxy_prove_routes_to_generator() {
        let app = test_app();
        let req = Request::post("/prove")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn test_proxy_circuits_routes_to_verifier() {
        let app = test_app();
        let req = Request::get("/circuits").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn test_proxy_models_routes_to_generator() {
        let app = test_app();
        let req = Request::get("/models").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn test_proxy_stats_routes_to_registry() {
        let app = test_app();
        let req = Request::get("/stats").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
