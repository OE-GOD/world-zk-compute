use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::header;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// Middleware that adds `X-API-Version: v1` header to all responses.
async fn version_header_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("X-API-Version", header::HeaderValue::from_static("v1"));
    response
}

/// Shared metrics state, safe for concurrent access via atomics.
pub struct MetricsState {
    pub total_submissions: AtomicU64,
    pub total_challenges: AtomicU64,
    pub total_disputes_resolved: AtomicU64,
    pub total_disputes_failed: AtomicU64,
    pub total_finalizations: AtomicU64,
    pub total_errors: AtomicU64,
    pub active_disputes: AtomicU64,
    pub last_block_polled: AtomicU64,
    pub start_time: Instant,
}

impl Default for MetricsState {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsState {
    pub fn new() -> Self {
        Self {
            total_submissions: AtomicU64::new(0),
            total_challenges: AtomicU64::new(0),
            total_disputes_resolved: AtomicU64::new(0),
            total_disputes_failed: AtomicU64::new(0),
            total_finalizations: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            active_disputes: AtomicU64::new(0),
            last_block_polled: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Record a new result submission.
    pub fn record_submission(&self) {
        self.total_submissions.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a new challenge event.
    pub fn record_challenge(&self) {
        self.total_challenges.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a dispute resolution.
    pub fn record_dispute_resolved(&self) {
        self.total_disputes_resolved.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed dispute (proof submission failed or timed out).
    pub fn record_dispute_failed(&self) {
        self.total_disputes_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a finalization.
    pub fn record_finalization(&self) {
        self.total_finalizations.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an error.
    pub fn record_error(&self) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Set the current number of active disputes.
    pub fn set_active_disputes(&self, count: u64) {
        self.active_disputes.store(count, Ordering::Relaxed);
    }

    /// Update the last polled block number.
    pub fn set_last_block(&self, block: u64) {
        self.last_block_polled.store(block, Ordering::Relaxed);
    }

    /// Get uptime in seconds.
    #[allow(dead_code)]
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render_prometheus(&self) -> String {
        let uptime = self.start_time.elapsed().as_secs();
        let challenges = self.total_challenges.load(Ordering::Relaxed);
        let submissions = self.total_submissions.load(Ordering::Relaxed);
        let disputes_resolved = self.total_disputes_resolved.load(Ordering::Relaxed);
        let disputes_failed = self.total_disputes_failed.load(Ordering::Relaxed);
        let finalizations = self.total_finalizations.load(Ordering::Relaxed);
        let errors = self.total_errors.load(Ordering::Relaxed);
        let active = self.active_disputes.load(Ordering::Relaxed);
        let last_block = self.last_block_polled.load(Ordering::Relaxed);

        format!(
            "\
# HELP operator_challenges_detected Total challenges detected\n\
# TYPE operator_challenges_detected counter\n\
operator_challenges_detected {challenges}\n\
# HELP operator_proofs_submitted Total proofs submitted to resolve disputes\n\
# TYPE operator_proofs_submitted counter\n\
operator_proofs_submitted {submissions}\n\
# HELP operator_disputes_resolved Total disputes resolved successfully\n\
# TYPE operator_disputes_resolved counter\n\
operator_disputes_resolved {disputes_resolved}\n\
# HELP operator_disputes_failed Total disputes that failed to resolve\n\
# TYPE operator_disputes_failed counter\n\
operator_disputes_failed {disputes_failed}\n\
# HELP operator_errors_total Total errors encountered\n\
# TYPE operator_errors_total counter\n\
operator_errors_total {errors}\n\
# HELP operator_finalizations_total Total result finalizations\n\
# TYPE operator_finalizations_total counter\n\
operator_finalizations_total {finalizations}\n\
# HELP operator_active_disputes Current number of active disputes\n\
# TYPE operator_active_disputes gauge\n\
operator_active_disputes {active}\n\
# HELP operator_uptime_seconds Operator uptime in seconds\n\
# TYPE operator_uptime_seconds gauge\n\
operator_uptime_seconds {uptime}\n\
# HELP operator_last_block_polled Last block number polled\n\
# TYPE operator_last_block_polled gauge\n\
operator_last_block_polled {last_block}\n"
        )
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    uptime_secs: u64,
    last_block_polled: u64,
}

#[derive(Serialize)]
struct MetricsJsonResponse {
    uptime_secs: u64,
    last_block_polled: u64,
    total_submissions: u64,
    total_challenges: u64,
    total_disputes_resolved: u64,
    total_disputes_failed: u64,
    total_finalizations: u64,
    total_errors: u64,
    active_disputes: u64,
}

async fn health_handler(State(state): State<Arc<MetricsState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        uptime_secs: state.start_time.elapsed().as_secs(),
        last_block_polled: state.last_block_polled.load(Ordering::Relaxed),
    })
}

/// Prometheus text format metrics endpoint.
async fn prometheus_metrics_handler(State(state): State<Arc<MetricsState>>) -> impl IntoResponse {
    let body = state.render_prometheus();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

/// JSON metrics endpoint (backward compatibility).
async fn json_metrics_handler(State(state): State<Arc<MetricsState>>) -> Json<MetricsJsonResponse> {
    Json(MetricsJsonResponse {
        uptime_secs: state.start_time.elapsed().as_secs(),
        last_block_polled: state.last_block_polled.load(Ordering::Relaxed),
        total_submissions: state.total_submissions.load(Ordering::Relaxed),
        total_challenges: state.total_challenges.load(Ordering::Relaxed),
        total_disputes_resolved: state.total_disputes_resolved.load(Ordering::Relaxed),
        total_disputes_failed: state.total_disputes_failed.load(Ordering::Relaxed),
        total_finalizations: state.total_finalizations.load(Ordering::Relaxed),
        total_errors: state.total_errors.load(Ordering::Relaxed),
        active_disputes: state.active_disputes.load(Ordering::Relaxed),
    })
}

/// Spawn the metrics HTTP server on the given port.
///
/// This runs forever serving the following endpoints:
/// - `/health` — JSON health check
/// - `/metrics` — Prometheus text exposition format
/// - `/metrics/json` — JSON metrics (backward compatibility)
///
/// It should be spawned as a background task via `tokio::spawn`.
pub async fn serve_metrics(state: Arc<MetricsState>, port: u16) {
    // Versioned API routes under /api/v1/ with X-API-Version header
    let versioned = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(prometheus_metrics_handler))
        .route("/metrics/json", get(json_metrics_handler))
        .layer(middleware::from_fn(version_header_middleware));

    let app = Router::new()
        // Backward-compatible unversioned routes
        .route("/health", get(health_handler))
        .route("/metrics", get(prometheus_metrics_handler))
        .route("/metrics/json", get(json_metrics_handler))
        // Versioned routes
        .nest("/api/v1", versioned)
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
        assert_eq!(state.total_disputes_failed.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_finalizations.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_errors.load(Ordering::Relaxed), 0);
        assert_eq!(state.active_disputes.load(Ordering::Relaxed), 0);
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
        state.total_disputes_failed.fetch_add(1, Ordering::Relaxed);
        state.total_finalizations.fetch_add(5, Ordering::Relaxed);
        state.total_errors.fetch_add(4, Ordering::Relaxed);
        state.active_disputes.store(7, Ordering::Relaxed);
        state.last_block_polled.store(42, Ordering::Relaxed);

        assert_eq!(state.total_submissions.load(Ordering::Relaxed), 1);
        assert_eq!(state.total_challenges.load(Ordering::Relaxed), 3);
        assert_eq!(state.total_disputes_resolved.load(Ordering::Relaxed), 2);
        assert_eq!(state.total_disputes_failed.load(Ordering::Relaxed), 1);
        assert_eq!(state.total_finalizations.load(Ordering::Relaxed), 5);
        assert_eq!(state.total_errors.load(Ordering::Relaxed), 4);
        assert_eq!(state.active_disputes.load(Ordering::Relaxed), 7);
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

    #[test]
    fn test_metrics_state_default() {
        let state = MetricsState::default();
        assert_eq!(state.total_submissions.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_challenges.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_disputes_failed.load(Ordering::Relaxed), 0);
        assert_eq!(state.total_errors.load(Ordering::Relaxed), 0);
        assert_eq!(state.active_disputes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_convenience_methods() {
        let state = MetricsState::new();
        state.record_submission();
        state.record_submission();
        state.record_challenge();
        state.record_dispute_resolved();
        state.record_dispute_failed();
        state.record_finalization();
        state.record_finalization();
        state.record_finalization();
        state.record_error();
        state.record_error();
        state.set_active_disputes(3);
        state.set_last_block(999);

        assert_eq!(state.total_submissions.load(Ordering::Relaxed), 2);
        assert_eq!(state.total_challenges.load(Ordering::Relaxed), 1);
        assert_eq!(state.total_disputes_resolved.load(Ordering::Relaxed), 1);
        assert_eq!(state.total_disputes_failed.load(Ordering::Relaxed), 1);
        assert_eq!(state.total_finalizations.load(Ordering::Relaxed), 3);
        assert_eq!(state.total_errors.load(Ordering::Relaxed), 2);
        assert_eq!(state.active_disputes.load(Ordering::Relaxed), 3);
        assert_eq!(state.last_block_polled.load(Ordering::Relaxed), 999);
    }

    #[test]
    fn test_uptime_secs() {
        let state = MetricsState::new();
        // Uptime should be very small (< 1s) right after creation
        assert!(state.uptime_secs() < 2);
    }

    #[tokio::test]
    async fn test_metrics_endpoint_prometheus_format() {
        let state = Arc::new(MetricsState::new());
        state.total_submissions.store(5, Ordering::Relaxed);
        state.total_challenges.store(2, Ordering::Relaxed);
        state.total_disputes_resolved.store(1, Ordering::Relaxed);
        state.total_disputes_failed.store(3, Ordering::Relaxed);
        state.total_finalizations.store(4, Ordering::Relaxed);
        state.total_errors.store(7, Ordering::Relaxed);
        state.active_disputes.store(2, Ordering::Relaxed);
        state.last_block_polled.store(200, Ordering::Relaxed);

        let app = Router::new()
            .route("/metrics", get(prometheus_metrics_handler))
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

        // Verify Content-Type header
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(content_type, "text/plain; version=0.0.4; charset=utf-8");

        let body = resp.text().await.unwrap();

        // Verify Prometheus text format structure
        assert!(
            body.contains("# HELP operator_challenges_detected Total challenges detected"),
            "Missing HELP for challenges_detected"
        );
        assert!(
            body.contains("# TYPE operator_challenges_detected counter"),
            "Missing TYPE for challenges_detected"
        );
        assert!(
            body.contains("operator_challenges_detected 2"),
            "Wrong value for challenges_detected"
        );

        assert!(
            body.contains(
                "# HELP operator_proofs_submitted Total proofs submitted to resolve disputes"
            ),
            "Missing HELP for proofs_submitted"
        );
        assert!(
            body.contains("# TYPE operator_proofs_submitted counter"),
            "Missing TYPE for proofs_submitted"
        );
        assert!(
            body.contains("operator_proofs_submitted 5"),
            "Wrong value for proofs_submitted"
        );

        assert!(
            body.contains("# TYPE operator_disputes_resolved counter"),
            "Missing TYPE for disputes_resolved"
        );
        assert!(
            body.contains("operator_disputes_resolved 1"),
            "Wrong value for disputes_resolved"
        );

        assert!(
            body.contains("# TYPE operator_disputes_failed counter"),
            "Missing TYPE for disputes_failed"
        );
        assert!(
            body.contains("operator_disputes_failed 3"),
            "Wrong value for disputes_failed"
        );

        assert!(
            body.contains("# TYPE operator_errors_total counter"),
            "Missing TYPE for errors_total"
        );
        assert!(
            body.contains("operator_errors_total 7"),
            "Wrong value for errors_total"
        );

        assert!(
            body.contains("# TYPE operator_active_disputes gauge"),
            "Missing TYPE for active_disputes"
        );
        assert!(
            body.contains("operator_active_disputes 2"),
            "Wrong value for active_disputes"
        );

        assert!(
            body.contains("# TYPE operator_uptime_seconds gauge"),
            "Missing TYPE for uptime_seconds"
        );
        // Uptime is dynamic, just check it exists
        assert!(
            body.contains("operator_uptime_seconds "),
            "Missing value for uptime_seconds"
        );

        assert!(
            body.contains("# TYPE operator_last_block_polled gauge"),
            "Missing TYPE for last_block_polled"
        );
        assert!(
            body.contains("operator_last_block_polled 200"),
            "Wrong value for last_block_polled"
        );
    }

    #[tokio::test]
    async fn test_metrics_json_endpoint() {
        let state = Arc::new(MetricsState::new());
        state.total_submissions.store(5, Ordering::Relaxed);
        state.total_challenges.store(2, Ordering::Relaxed);
        state.total_disputes_resolved.store(1, Ordering::Relaxed);
        state.total_disputes_failed.store(3, Ordering::Relaxed);
        state.total_finalizations.store(4, Ordering::Relaxed);
        state.total_errors.store(7, Ordering::Relaxed);
        state.active_disputes.store(2, Ordering::Relaxed);
        state.last_block_polled.store(200, Ordering::Relaxed);

        let app = Router::new()
            .route("/metrics/json", get(json_metrics_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let resp = reqwest::get(format!("http://{}/metrics/json", addr))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["total_submissions"], 5);
        assert_eq!(body["total_challenges"], 2);
        assert_eq!(body["total_disputes_resolved"], 1);
        assert_eq!(body["total_disputes_failed"], 3);
        assert_eq!(body["total_finalizations"], 4);
        assert_eq!(body["total_errors"], 7);
        assert_eq!(body["active_disputes"], 2);
        assert_eq!(body["last_block_polled"], 200);
    }

    #[test]
    fn test_render_prometheus_format() {
        let state = MetricsState::new();
        state.total_challenges.store(5, Ordering::Relaxed);
        state.total_submissions.store(10, Ordering::Relaxed);
        state.total_disputes_resolved.store(3, Ordering::Relaxed);
        state.total_disputes_failed.store(1, Ordering::Relaxed);
        state.total_errors.store(2, Ordering::Relaxed);
        state.total_finalizations.store(8, Ordering::Relaxed);
        state.active_disputes.store(4, Ordering::Relaxed);
        state.last_block_polled.store(12345, Ordering::Relaxed);

        let output = state.render_prometheus();

        // Each metric must have HELP, TYPE, and value lines
        let lines: Vec<&str> = output.lines().collect();

        // Verify all counter metrics have correct format
        assert!(lines.contains(&"# HELP operator_challenges_detected Total challenges detected"));
        assert!(lines.contains(&"# TYPE operator_challenges_detected counter"));
        assert!(lines.contains(&"operator_challenges_detected 5"));

        assert!(lines.contains(
            &"# HELP operator_proofs_submitted Total proofs submitted to resolve disputes"
        ));
        assert!(lines.contains(&"# TYPE operator_proofs_submitted counter"));
        assert!(lines.contains(&"operator_proofs_submitted 10"));

        assert!(lines
            .contains(&"# HELP operator_disputes_resolved Total disputes resolved successfully"));
        assert!(lines.contains(&"# TYPE operator_disputes_resolved counter"));
        assert!(lines.contains(&"operator_disputes_resolved 3"));

        assert!(lines
            .contains(&"# HELP operator_disputes_failed Total disputes that failed to resolve"));
        assert!(lines.contains(&"# TYPE operator_disputes_failed counter"));
        assert!(lines.contains(&"operator_disputes_failed 1"));

        assert!(lines.contains(&"# HELP operator_errors_total Total errors encountered"));
        assert!(lines.contains(&"# TYPE operator_errors_total counter"));
        assert!(lines.contains(&"operator_errors_total 2"));

        assert!(lines.contains(&"# HELP operator_finalizations_total Total result finalizations"));
        assert!(lines.contains(&"# TYPE operator_finalizations_total counter"));
        assert!(lines.contains(&"operator_finalizations_total 8"));

        // Verify all gauge metrics have correct format
        assert!(
            lines.contains(&"# HELP operator_active_disputes Current number of active disputes")
        );
        assert!(lines.contains(&"# TYPE operator_active_disputes gauge"));
        assert!(lines.contains(&"operator_active_disputes 4"));

        assert!(lines.contains(&"# HELP operator_uptime_seconds Operator uptime in seconds"));
        assert!(lines.contains(&"# TYPE operator_uptime_seconds gauge"));
        // Uptime is dynamic, check it exists with some value
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("operator_uptime_seconds ")),
            "Missing operator_uptime_seconds value line"
        );

        assert!(lines.contains(&"# HELP operator_last_block_polled Last block number polled"));
        assert!(lines.contains(&"# TYPE operator_last_block_polled gauge"));
        assert!(lines.contains(&"operator_last_block_polled 12345"));
    }

    #[test]
    fn test_render_prometheus_zero_values() {
        let state = MetricsState::new();
        let output = state.render_prometheus();

        // All counters should be 0
        assert!(output.contains("operator_challenges_detected 0"));
        assert!(output.contains("operator_proofs_submitted 0"));
        assert!(output.contains("operator_disputes_resolved 0"));
        assert!(output.contains("operator_disputes_failed 0"));
        assert!(output.contains("operator_errors_total 0"));
        assert!(output.contains("operator_finalizations_total 0"));
        assert!(output.contains("operator_active_disputes 0"));
        assert!(output.contains("operator_last_block_polled 0"));
    }

    #[test]
    fn test_render_prometheus_each_line_ends_with_newline() {
        let state = MetricsState::new();
        let output = state.render_prometheus();

        // Prometheus format requires each line to end with \n
        assert!(output.ends_with('\n'), "Output must end with newline");

        // No empty lines (no double newlines)
        assert!(
            !output.contains("\n\n"),
            "Output should not contain empty lines"
        );
    }
}
