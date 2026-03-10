use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// Shared metrics state, safe for concurrent access via atomics.
pub struct MetricsState {
    pub total_submissions: AtomicU64,
    pub total_challenges: AtomicU64,
    pub total_disputes_resolved: AtomicU64,
    pub total_finalizations: AtomicU64,
    pub last_block_polled: AtomicU64,
    pub start_time: Instant,
}

impl MetricsState {
    pub fn new() -> Self {
        Self {
            total_submissions: AtomicU64::new(0),
            total_challenges: AtomicU64::new(0),
            total_disputes_resolved: AtomicU64::new(0),
            total_finalizations: AtomicU64::new(0),
            last_block_polled: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    uptime_secs: u64,
    last_block_polled: u64,
}

#[derive(Serialize)]
struct MetricsResponse {
    uptime_secs: u64,
    last_block_polled: u64,
    total_submissions: u64,
    total_challenges: u64,
    total_disputes_resolved: u64,
    total_finalizations: u64,
}

async fn health_handler(State(state): State<Arc<MetricsState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        last_block_polled: state.last_block_polled.load(Ordering::Relaxed),
    })
}

async fn metrics_handler(State(state): State<Arc<MetricsState>>) -> Json<MetricsResponse> {
    Json(MetricsResponse {
        uptime_secs: state.start_time.elapsed().as_secs(),
        last_block_polled: state.last_block_polled.load(Ordering::Relaxed),
        total_submissions: state.total_submissions.load(Ordering::Relaxed),
        total_challenges: state.total_challenges.load(Ordering::Relaxed),
        total_disputes_resolved: state.total_disputes_resolved.load(Ordering::Relaxed),
        total_finalizations: state.total_finalizations.load(Ordering::Relaxed),
    })
}

/// Spawn the metrics HTTP server on the given port.
///
/// This runs forever serving `/health` and `/metrics` endpoints.
/// It should be spawned as a background task via `tokio::spawn`.
pub async fn serve_metrics(state: Arc<MetricsState>, port: u16) {
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("Metrics server listening on {}", addr);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind metrics server on {}: {}", addr, e);
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("Metrics server error: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_state_new() {
        let state = MetricsState::new();
        assert_eq!(state.total_submissions.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_challenges.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_disputes_resolved.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_finalizations.load(Ordering::Relaxed), 0);
        assert_eq!(state.last_block_polled.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_metrics_state_increment() {
        let state = MetricsState::new();
        state.total_submissions.fetch_add(1, Ordering::Relaxed);
        state.total_challenges.fetch_add(3, Ordering::Relaxed);
        state
            .total_disputes_resolved
            .fetch_add(2, Ordering::Relaxed);
        state.total_finalizations.fetch_add(5, Ordering::Relaxed);
        state.last_block_polled.store(42, Ordering::Relaxed);

        assert_eq!(state.total_submissions.load(Ordering::Relaxed), 1);
        assert_eq!(state.total_challenges.load(Ordering::Relaxed), 3);
        assert_eq!(state.total_disputes_resolved.load(Ordering::Relaxed), 2);
        assert_eq!(state.total_finalizations.load(Ordering::Relaxed), 5);
        assert_eq!(state.last_block_polled.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn test_metrics_state_concurrent_increments() {
        let state = Arc::new(MetricsState::new());
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let s = state.clone();
                std::thread::spawn(move || {
                    s.total_submissions.fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(state.total_submissions.load(Ordering::Relaxed), 10);
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let state = Arc::new(MetricsState::new());
        state.last_block_polled.store(100, Ordering::Relaxed);

        let app = Router::new()
            .route("/health", get(health_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let resp = reqwest::get(format!("http://{}/health", addr))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["last_block_polled"], 100);
    }

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let state = Arc::new(MetricsState::new());
        state.total_submissions.store(5, Ordering::Relaxed);
        state.total_challenges.store(2, Ordering::Relaxed);
        state.total_disputes_resolved.store(1, Ordering::Relaxed);
        state.total_finalizations.store(3, Ordering::Relaxed);
        state.last_block_polled.store(200, Ordering::Relaxed);

        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let resp = reqwest::get(format!("http://{}/metrics", addr))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["total_submissions"], 5);
        assert_eq!(body["total_challenges"], 2);
        assert_eq!(body["total_disputes_resolved"], 1);
        assert_eq!(body["total_finalizations"], 3);
        assert_eq!(body["last_block_polled"], 200);
    }
}
