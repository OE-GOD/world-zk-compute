//! Health-check aggregator for all world-zk-compute services.
//!
//! Polls the health endpoints of verifier, operator, enclave, and indexer
//! services, then returns an aggregate status.
//!
//! # Configuration (environment variables)
//!
//! | Variable | Default |
//! |---|---|
//! | `VERIFIER_URL` | `http://localhost:3000` |
//! | `OPERATOR_URL` | `http://localhost:9090` |
//! | `ENCLAVE_URL` | `http://localhost:8080` |
//! | `INDEXER_URL` | `http://localhost:8081` |
//! | `HEALTH_PORT` | `8888` |
//! | `CHECK_TIMEOUT_MS` | `5000` |
//!
//! # Endpoints
//!
//! - `GET /health` -- aggregate status of all services
//! - `GET /health/{service}` -- status of a single service (`verifier`, `operator`, `enclave`, `indexer`)

use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Describes a backend service to health-check.
#[derive(Clone, Debug)]
struct ServiceTarget {
    name: String,
    base_url: String,
    health_path: String,
}

/// Shared application state.
#[derive(Clone, Debug)]
struct AppState {
    services: Arc<Vec<ServiceTarget>>,
    client: reqwest::Client,
    timeout: Duration,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
struct AggregateHealth {
    /// `"healthy"` -- all services up.
    /// `"degraded"` -- some services down.
    /// `"down"` -- all services down.
    status: String,
    services: Vec<ServiceHealth>,
    checked_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ServiceHealth {
    name: String,
    url: String,
    /// `"up"`, `"down"`, or `"timeout"`.
    status: String,
    latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// Health-check logic
// ---------------------------------------------------------------------------

async fn check_one(client: &reqwest::Client, target: &ServiceTarget, timeout: Duration) -> ServiceHealth {
    let url = format!("{}{}", target.base_url, target.health_path);
    let start = Instant::now();

    let result = tokio::time::timeout(timeout, client.get(&url).send()).await;
    let latency = start.elapsed();

    match result {
        Ok(Ok(resp)) => {
            if resp.status().is_success() {
                ServiceHealth {
                    name: target.name.clone(),
                    url: url.clone(),
                    status: "up".to_string(),
                    latency_ms: latency.as_millis() as u64,
                    error: None,
                }
            } else {
                ServiceHealth {
                    name: target.name.clone(),
                    url: url.clone(),
                    status: "down".to_string(),
                    latency_ms: latency.as_millis() as u64,
                    error: Some(format!("HTTP {}", resp.status())),
                }
            }
        }
        Ok(Err(e)) => ServiceHealth {
            name: target.name.clone(),
            url: url.clone(),
            status: "down".to_string(),
            latency_ms: latency.as_millis() as u64,
            error: Some(e.to_string()),
        },
        Err(_) => ServiceHealth {
            name: target.name.clone(),
            url: url.clone(),
            status: "timeout".to_string(),
            latency_ms: timeout.as_millis() as u64,
            error: Some(format!("timed out after {}ms", timeout.as_millis())),
        },
    }
}

async fn check_all(state: &AppState) -> AggregateHealth {
    // Fire all checks concurrently.
    let handles: Vec<_> = state
        .services
        .iter()
        .map(|svc| {
            let client = state.client.clone();
            let target = svc.clone();
            let timeout = state.timeout;
            tokio::spawn(async move { check_one(&client, &target, timeout).await })
        })
        .collect();

    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        match h.await {
            Ok(r) => results.push(r),
            Err(e) => results.push(ServiceHealth {
                name: "unknown".to_string(),
                url: String::new(),
                status: "down".to_string(),
                latency_ms: 0,
                error: Some(format!("join error: {e}")),
            }),
        }
    }

    let up_count = results.iter().filter(|r| r.status == "up").count();
    let total = results.len();

    let status = if up_count == total {
        "healthy"
    } else if up_count == 0 {
        "down"
    } else {
        "degraded"
    }
    .to_string();

    AggregateHealth {
        status,
        services: results,
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn aggregate_handler(State(state): State<AppState>) -> Json<AggregateHealth> {
    Json(check_all(&state).await)
}

async fn single_handler(
    State(state): State<AppState>,
    Path(service): Path<String>,
) -> Result<Json<ServiceHealth>, impl IntoResponse> {
    let target = state.services.iter().find(|s| s.name == service);
    match target {
        Some(t) => Ok(Json(check_one(&state.client, t, state.timeout).await)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("unknown service: {service}"),
                "known": state.services.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            })),
        )),
    }
}

// ---------------------------------------------------------------------------
// App construction (public for tests)
// ---------------------------------------------------------------------------

fn build_app() -> Router {
    let services = vec![
        ServiceTarget {
            name: "verifier".to_string(),
            base_url: env::var("VERIFIER_URL").unwrap_or_else(|_| "http://localhost:3000".to_string()),
            health_path: "/health".to_string(),
        },
        ServiceTarget {
            name: "operator".to_string(),
            base_url: env::var("OPERATOR_URL").unwrap_or_else(|_| "http://localhost:9090".to_string()),
            health_path: "/health".to_string(),
        },
        ServiceTarget {
            name: "enclave".to_string(),
            base_url: env::var("ENCLAVE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string()),
            health_path: "/health".to_string(),
        },
        ServiceTarget {
            name: "indexer".to_string(),
            base_url: env::var("INDEXER_URL").unwrap_or_else(|_| "http://localhost:8081".to_string()),
            health_path: "/health".to_string(),
        },
    ];

    let timeout_ms: u64 = env::var("CHECK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);

    let state = AppState {
        services: Arc::new(services),
        client: reqwest::Client::new(),
        timeout: Duration::from_millis(timeout_ms),
    };

    Router::new()
        .route("/health", get(aggregate_handler))
        .route("/health/:service", get(single_handler))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "health_aggregator=info".parse().unwrap()),
        )
        .init();

    let port: u16 = env::var("HEALTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8888);

    let app = build_app();

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("health-aggregator listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");
    axum::serve(listener, app).await.expect("server error");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    /// Build an app that points at a local mock server.
    fn test_app(mock_url: &str) -> Router {
        let services = vec![
            ServiceTarget {
                name: "verifier".to_string(),
                base_url: mock_url.to_string(),
                health_path: "/health".to_string(),
            },
            ServiceTarget {
                name: "operator".to_string(),
                base_url: mock_url.to_string(),
                health_path: "/health".to_string(),
            },
        ];

        let state = AppState {
            services: Arc::new(services),
            client: reqwest::Client::new(),
            timeout: Duration::from_secs(2),
        };

        Router::new()
            .route("/health", get(aggregate_handler))
            .route("/health/:service", get(single_handler))
            .with_state(state)
    }

    /// Spin up a tiny axum server that always returns 200 with `{"status":"ok"}`.
    async fn start_mock_healthy() -> String {
        let app = Router::new().route(
            "/health",
            get(|| async { Json(serde_json::json!({"status": "ok"})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// Spin up a mock that returns 503.
    async fn start_mock_unhealthy() -> String {
        let app = Router::new().route(
            "/health",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "down") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn test_aggregate_all_healthy() {
        let mock_url = start_mock_healthy().await;
        let app = test_app(&mock_url);

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: AggregateHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "healthy");
        assert_eq!(health.services.len(), 2);
        for svc in &health.services {
            assert_eq!(svc.status, "up");
            assert!(svc.error.is_none());
        }
    }

    #[tokio::test]
    async fn test_aggregate_all_down() {
        // Point at a port that is not listening.
        let app = test_app("http://127.0.0.1:1");

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: AggregateHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "down");
        for svc in &health.services {
            assert_ne!(svc.status, "up");
            assert!(svc.error.is_some());
        }
    }

    #[tokio::test]
    async fn test_aggregate_degraded() {
        let healthy_url = start_mock_healthy().await;

        // Mix: one points at real mock, other at unreachable port.
        let services = vec![
            ServiceTarget {
                name: "verifier".to_string(),
                base_url: healthy_url,
                health_path: "/health".to_string(),
            },
            ServiceTarget {
                name: "operator".to_string(),
                base_url: "http://127.0.0.1:1".to_string(),
                health_path: "/health".to_string(),
            },
        ];

        let state = AppState {
            services: Arc::new(services),
            client: reqwest::Client::new(),
            timeout: Duration::from_secs(2),
        };

        let app = Router::new()
            .route("/health", get(aggregate_handler))
            .with_state(state);

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: AggregateHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "degraded");
    }

    #[tokio::test]
    async fn test_single_service_healthy() {
        let mock_url = start_mock_healthy().await;
        let app = test_app(&mock_url);

        let req = Request::get("/health/verifier")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let svc: ServiceHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(svc.name, "verifier");
        assert_eq!(svc.status, "up");
    }

    #[tokio::test]
    async fn test_single_service_unknown() {
        let mock_url = start_mock_healthy().await;
        let app = test_app(&mock_url);

        let req = Request::get("/health/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_single_service_down() {
        let mock_url = start_mock_unhealthy().await;
        let app = test_app(&mock_url);

        let req = Request::get("/health/verifier")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let svc: ServiceHealth = serde_json::from_slice(&body).unwrap();
        assert_eq!(svc.name, "verifier");
        assert_eq!(svc.status, "down");
        assert!(svc.error.is_some());
    }

    #[tokio::test]
    async fn test_checked_at_present() {
        let mock_url = start_mock_healthy().await;
        let app = test_app(&mock_url);

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: AggregateHealth = serde_json::from_slice(&body).unwrap();
        // checked_at should be a valid RFC3339 timestamp.
        assert!(chrono::DateTime::parse_from_rfc3339(&health.checked_at).is_ok());
    }

    #[tokio::test]
    async fn test_latency_recorded() {
        let mock_url = start_mock_healthy().await;
        let app = test_app(&mock_url);

        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let health: AggregateHealth = serde_json::from_slice(&body).unwrap();
        for svc in &health.services {
            // Latency should be non-negative (it will be > 0 in practice but
            // the mock is local so it can be 0ms).
            assert!(svc.latency_ms < 5000);
        }
    }
}
