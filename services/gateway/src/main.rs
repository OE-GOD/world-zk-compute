//! Unified API gateway for World ZK Compute services.
//!
//! Provides a single entry point that reverse-proxies requests to the
//! appropriate backend service. Deployers expose one URL instead of three+.
//!
//! # Routes
//!
//! | Prefix      | Backend            | Default URL                  |
//! |-------------|--------------------|------------------------------|
//! | `/prove`    | proof-generator    | `http://localhost:3002`      |
//! | `/models`   | proof-generator    | `http://localhost:3002`      |
//! | `/proofs`   | proof-registry     | `http://localhost:3001`      |
//! | `/stats`    | proof-registry     | `http://localhost:3001`      |
//! | `/verify`   | verifier-api       | `http://localhost:3000`      |
//! | `/circuits` | verifier-api       | `http://localhost:3000`      |
//! | `/health`   | aggregated health  | (built-in)                   |
//! | `/health/{svc}` | specific svc   | (built-in)                   |
//!
//! # Environment Variables
//!
//! | Variable           | Description                              | Default                  |
//! |--------------------|------------------------------------------|--------------------------|
//! | `VERIFIER_URL`     | Base URL of the verifier service         | `http://localhost:3000`  |
//! | `REGISTRY_URL`     | Base URL of the proof registry           | `http://localhost:3001`  |
//! | `GENERATOR_URL`    | Base URL of the proof generator          | `http://localhost:3002`  |
//! | `GATEWAY_PORT`     | Port the gateway listens on              | `8080`                   |
//! | `GATEWAY_API_KEYS` | Comma-separated API keys (optional)      | (none = no auth)         |

mod logging;

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Backend service descriptor.
#[derive(Debug, Clone)]
struct BackendService {
    name: String,
    base_url: String,
}

/// Gateway configuration parsed from environment variables.
#[derive(Debug, Clone)]
struct GatewayConfig {
    backends: HashMap<String, BackendService>,
    api_keys: Vec<String>,
}

impl GatewayConfig {
    fn from_env() -> Self {
        let verifier_url =
            env::var("VERIFIER_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
        let registry_url =
            env::var("REGISTRY_URL").unwrap_or_else(|_| "http://localhost:3001".to_string());
        let generator_url =
            env::var("GENERATOR_URL").unwrap_or_else(|_| "http://localhost:3002".to_string());

        let api_keys: Vec<String> = env::var("GATEWAY_API_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut backends = HashMap::new();
        backends.insert(
            "verifier".to_string(),
            BackendService {
                name: "verifier".to_string(),
                base_url: verifier_url,
            },
        );
        backends.insert(
            "registry".to_string(),
            BackendService {
                name: "registry".to_string(),
                base_url: registry_url,
            },
        );
        backends.insert(
            "generator".to_string(),
            BackendService {
                name: "generator".to_string(),
                base_url: generator_url,
            },
        );

        GatewayConfig { backends, api_keys }
    }
}

/// Shared gateway state accessible from handlers.
#[derive(Clone)]
struct GatewayState {
    config: Arc<GatewayConfig>,
    client: reqwest::Client,
}

// ---------------------------------------------------------------------------
// Route prefix -> backend mapping
// ---------------------------------------------------------------------------

/// Determines which backend a request path should be routed to.
fn route_to_backend(path: &str) -> Option<&'static str> {
    let trimmed = path.trim_start_matches('/');
    let segment = trimmed.split('/').next().unwrap_or("");

    match segment {
        "prove" => Some("generator"),
        "models" => Some("generator"),
        "proofs" => Some("registry"),
        "stats" => Some("registry"),
        "verify" => Some("verifier"),
        "circuits" => Some("verifier"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Proxy logic
// ---------------------------------------------------------------------------

/// Forward a request to the appropriate backend, preserving method, headers,
/// path, query string, and body. Returns the backend response as-is.
async fn proxy_request(
    state: &GatewayState,
    backend_name: &str,
    original_req: Request,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let backend = state
        .config
        .backends
        .get(backend_name)
        .ok_or_else(|| err_response(StatusCode::BAD_GATEWAY, "unknown backend"))?;

    let method = original_req.method().clone();
    let uri = original_req.uri().clone();
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(uri.path());

    let target_url = format!(
        "{}{}",
        backend.base_url.trim_end_matches('/'),
        path_and_query
    );

    // Copy headers, excluding hop-by-hop and Host.
    let mut forwarded_headers = HeaderMap::new();
    for (key, value) in original_req.headers() {
        let name = key.as_str().to_lowercase();
        if name == "host" || name == "transfer-encoding" || name == "connection" {
            continue;
        }
        forwarded_headers.insert(key.clone(), value.clone());
    }

    let body_bytes = match axum::body::to_bytes(original_req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return Err(err_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read request body: {e}"),
            ));
        }
    };

    let upstream_resp = state
        .client
        .request(method, &target_url)
        .headers(forwarded_headers)
        .body(body_bytes)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("proxy error to {target_url}: {e}");
            err_response(StatusCode::BAD_GATEWAY, &format!("upstream error: {e}"))
        })?;

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    let resp_headers = upstream_resp.headers().clone();
    let resp_body = upstream_resp.bytes().await.map_err(|e| {
        err_response(
            StatusCode::BAD_GATEWAY,
            &format!("failed to read upstream response: {e}"),
        )
    })?;

    let mut response = Response::builder().status(status);
    for (key, value) in &resp_headers {
        let name = key.as_str().to_lowercase();
        if name == "transfer-encoding" || name == "connection" {
            continue;
        }
        response = response.header(key, value);
    }

    response.body(Body::from(resp_body)).map_err(|e| {
        err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("response build error: {e}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Catch-all proxy handler
// ---------------------------------------------------------------------------

async fn proxy_handler(
    State(state): State<GatewayState>,
    req: Request,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let path = req.uri().path().to_string();
    let backend = route_to_backend(&path).ok_or_else(|| {
        err_response(
            StatusCode::NOT_FOUND,
            &format!("no backend route for path: {path}"),
        )
    })?;

    proxy_request(&state, backend, req).await
}

// ---------------------------------------------------------------------------
// Health endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct ServiceHealth {
    name: String,
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AggregateHealthResponse {
    status: String,
    services: Vec<ServiceHealth>,
    healthy_count: usize,
    total_count: usize,
}

#[derive(Serialize, Deserialize)]
struct SingleServiceHealthResponse {
    name: String,
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Check health of a single backend by GETting its /health endpoint.
async fn check_backend_health(client: &reqwest::Client, svc: &BackendService) -> ServiceHealth {
    let url = format!("{}/health", svc.base_url.trim_end_matches('/'));
    let start = std::time::Instant::now();

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => ServiceHealth {
            name: svc.name.clone(),
            healthy: true,
            response_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(resp) => ServiceHealth {
            name: svc.name.clone(),
            healthy: false,
            response_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => ServiceHealth {
            name: svc.name.clone(),
            healthy: false,
            response_ms: None,
            error: Some(e.to_string()),
        },
    }
}

/// `GET /health` -- aggregate health of all backends.
async fn health_aggregate(State(state): State<GatewayState>) -> Json<AggregateHealthResponse> {
    let mut services = Vec::new();

    for svc in state.config.backends.values() {
        let status = check_backend_health(&state.client, svc).await;
        services.push(status);
    }

    // Sort by name for deterministic output.
    services.sort_by(|a, b| a.name.cmp(&b.name));

    let healthy_count = services.iter().filter(|s| s.healthy).count();
    let total_count = services.len();

    let status = if healthy_count == total_count {
        "healthy"
    } else if healthy_count > 0 {
        "degraded"
    } else {
        "unhealthy"
    };

    Json(AggregateHealthResponse {
        status: status.to_string(),
        services,
        healthy_count,
        total_count,
    })
}

/// `GET /health/:service` -- health of a specific backend.
async fn health_single(
    State(state): State<GatewayState>,
    Path(service): Path<String>,
) -> Result<Json<SingleServiceHealthResponse>, (StatusCode, Json<ErrorResponse>)> {
    let svc = state.config.backends.get(&service).ok_or_else(|| {
        err_response(
            StatusCode::NOT_FOUND,
            &format!("unknown service: {service}. Valid: verifier, registry, generator"),
        )
    })?;

    let status = check_backend_health(&state.client, svc).await;

    Ok(Json(SingleServiceHealthResponse {
        name: status.name,
        healthy: status.healthy,
        response_ms: status.response_ms,
        error: status.error,
    }))
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

/// Optional API-key authentication middleware.
///
/// If `GATEWAY_API_KEYS` is set, every request must include a valid key in
/// the `Authorization: Bearer <key>` header. Health endpoints are exempt.
async fn auth_middleware(
    State(state): State<GatewayState>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    // No keys configured = auth disabled.
    if state.config.api_keys.is_empty() {
        return Ok(next.run(req).await);
    }

    // Health endpoints are always public.
    let path = req.uri().path();
    if path == "/health" || path.starts_with("/health/") {
        return Ok(next.run(req).await);
    }

    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

    if state.config.api_keys.iter().any(|k| k == token) {
        Ok(next.run(req).await)
    } else {
        Err(err_response(
            StatusCode::UNAUTHORIZED,
            "invalid or missing API key",
        ))
    }
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

fn err_response(status: StatusCode, msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

// ---------------------------------------------------------------------------
// App construction
// ---------------------------------------------------------------------------

fn build_app(state: GatewayState) -> Router {
    // Health routes take priority over the fallback proxy.
    // Auth middleware is applied as a layer (not route_layer) so it covers
    // the fallback handler too. The middleware itself exempts /health paths.
    //
    // Layer ordering (bottom-up — last `.layer()` call runs first):
    //   1. request_logger  — outermost: captures total latency including auth
    //   2. TraceLayer       — tower-http span tracing
    //   3. CorsLayer        — CORS headers
    //   4. auth_middleware   — API-key gating
    Router::new()
        .route("/health", get(health_aggregate))
        .route("/health/:service", get(health_single))
        .fallback(proxy_handler)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(logging::request_logger))
        .with_state(state)
}

fn build_state(config: GatewayConfig) -> GatewayState {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("failed to build HTTP client");

    GatewayState {
        config: Arc::new(config),
        client,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    logging::init_tracing();

    let port: u16 = env::var("GATEWAY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);

    let config = GatewayConfig::from_env();

    if config.api_keys.is_empty() {
        tracing::info!("API key auth disabled (no GATEWAY_API_KEYS set)");
    } else {
        tracing::info!(
            "API key auth enabled ({} key(s) configured)",
            config.api_keys.len()
        );
    }

    for (name, svc) in &config.backends {
        tracing::info!("Backend '{name}' -> {}", svc.base_url);
    }

    let state = build_state(config);
    let app = build_app(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("Gateway listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{self, Request as HttpRequest};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Helper: create a gateway state pointing backends at the given mock URLs.
    fn test_state(
        verifier_url: &str,
        registry_url: &str,
        generator_url: &str,
        api_keys: Vec<String>,
    ) -> GatewayState {
        let mut backends = HashMap::new();
        backends.insert(
            "verifier".to_string(),
            BackendService {
                name: "verifier".to_string(),
                base_url: verifier_url.to_string(),
            },
        );
        backends.insert(
            "registry".to_string(),
            BackendService {
                name: "registry".to_string(),
                base_url: registry_url.to_string(),
            },
        );
        backends.insert(
            "generator".to_string(),
            BackendService {
                name: "generator".to_string(),
                base_url: generator_url.to_string(),
            },
        );

        let config = GatewayConfig { backends, api_keys };
        build_state(config)
    }

    // -- Route matching tests --

    #[test]
    fn test_route_to_backend_prove() {
        assert_eq!(route_to_backend("/prove"), Some("generator"));
        assert_eq!(route_to_backend("/prove/some-model"), Some("generator"));
    }

    #[test]
    fn test_route_to_backend_models() {
        assert_eq!(route_to_backend("/models"), Some("generator"));
        assert_eq!(route_to_backend("/models/abc123"), Some("generator"));
        assert_eq!(
            route_to_backend("/models/abc123/versions"),
            Some("generator")
        );
    }

    #[test]
    fn test_route_to_backend_proofs() {
        assert_eq!(route_to_backend("/proofs"), Some("registry"));
        assert_eq!(route_to_backend("/proofs/xyz"), Some("registry"));
        assert_eq!(route_to_backend("/proofs/xyz/verify"), Some("registry"));
    }

    #[test]
    fn test_route_to_backend_stats() {
        assert_eq!(route_to_backend("/stats"), Some("registry"));
    }

    #[test]
    fn test_route_to_backend_verify() {
        assert_eq!(route_to_backend("/verify"), Some("verifier"));
        assert_eq!(route_to_backend("/verify/hybrid"), Some("verifier"));
    }

    #[test]
    fn test_route_to_backend_circuits() {
        assert_eq!(route_to_backend("/circuits"), Some("verifier"));
        assert_eq!(route_to_backend("/circuits/abc"), Some("verifier"));
    }

    #[test]
    fn test_route_to_backend_unknown() {
        assert_eq!(route_to_backend("/unknown"), None);
        assert_eq!(route_to_backend("/"), None);
        assert_eq!(route_to_backend("/admin"), None);
    }

    // -- Health aggregation tests --

    #[tokio::test]
    async fn test_health_all_down() {
        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: AggregateHealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.status, "unhealthy");
        assert_eq!(json.healthy_count, 0);
        assert_eq!(json.total_count, 3);
    }

    #[tokio::test]
    async fn test_health_with_live_backend() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "ok"
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            &mock_server.uri(),
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: AggregateHealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.status, "degraded");
        assert_eq!(json.healthy_count, 1);
    }

    #[tokio::test]
    async fn test_health_single_known_service() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "ok"
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            &mock_server.uri(),
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/health/verifier")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: SingleServiceHealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.name, "verifier");
        assert!(json.healthy);
    }

    #[tokio::test]
    async fn test_health_single_unknown_service() {
        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/health/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Proxy routing tests --

    #[tokio::test]
    async fn test_proxy_prove_routes_to_generator() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prove"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "proof_id": "test-123"
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            &mock_server.uri(),
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(http::Method::POST)
                    .uri("/prove")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"model_id":"m1","features":[1.0]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["proof_id"], "test-123");
    }

    #[tokio::test]
    async fn test_proxy_proofs_routes_to_registry() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proofs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "proofs": []
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            "http://127.0.0.1:19990",
            &mock_server.uri(),
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/proofs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["proofs"].is_array());
    }

    #[tokio::test]
    async fn test_proxy_verify_routes_to_verifier() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "verified": true,
                "circuit_hash": "0xabc"
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            &mock_server.uri(),
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(http::Method::POST)
                    .uri("/verify")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_proxy_models_routes_to_generator() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": []
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            &mock_server.uri(),
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -- Auth tests --

    #[tokio::test]
    async fn test_auth_required_when_keys_configured() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proofs"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let state = test_state(
            "http://127.0.0.1:19990",
            &mock_server.uri(),
            "http://127.0.0.1:19992",
            vec!["secret-key-1".to_string()],
        );
        let app = build_app(state);

        // No auth header -> 401
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/proofs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_passes_with_valid_key() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proofs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let state = test_state(
            "http://127.0.0.1:19990",
            &mock_server.uri(),
            "http://127.0.0.1:19992",
            vec!["secret-key-1".to_string()],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/proofs")
                    .header("Authorization", "Bearer secret-key-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_health_bypasses_auth() {
        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec!["secret-key-1".to_string()],
        );
        let app = build_app(state);

        // Health endpoints should work without auth even when keys are configured.
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -- 404 test --

    #[tokio::test]
    async fn test_unknown_route_returns_404() {
        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/unknown/path")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- Upstream error pass-through test --

    #[tokio::test]
    async fn test_upstream_error_status_passed_through() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "bad proof data"
            })))
            .mount(&mock_server)
            .await;

        let state = test_state(
            &mock_server.uri(),
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(http::Method::POST)
                    .uri("/verify")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "bad proof data");
    }

    // -- Backend unreachable returns 502 --

    #[tokio::test]
    async fn test_unreachable_backend_returns_502() {
        let state = test_state(
            "http://127.0.0.1:19990",
            "http://127.0.0.1:19991",
            "http://127.0.0.1:19992",
            vec![],
        );
        let app = build_app(state);

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method(http::Method::POST)
                    .uri("/prove")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
