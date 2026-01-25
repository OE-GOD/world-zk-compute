//! HTTP API Server for Prover Metrics and Health
//!
//! Exposes endpoints:
//! - GET /health - Health check
//! - GET /metrics - Prometheus metrics
//! - GET /stats - JSON stats
//! - GET /status - Current prover status

use crate::metrics::ProverMetrics;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use tracing::{info, error};

/// Health check state
#[derive(Debug, Clone)]
pub struct HealthCheck {
    pub rpc_connected: bool,
    pub last_block_time: u64,
    pub jobs_processed: u64,
    pub consecutive_failures: u32,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            rpc_connected: false,
            last_block_time: 0,
            jobs_processed: 0,
            consecutive_failures: 0,
        }
    }
}

impl HealthCheck {
    pub fn status(&self) -> HealthStatus {
        if !self.rpc_connected {
            return HealthStatus::Unhealthy;
        }
        if self.consecutive_failures > 5 {
            return HealthStatus::Degraded;
        }
        HealthStatus::Healthy
    }
}

/// Health status levels
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Prover status information
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProverStatus {
    pub is_running: bool,
    pub current_job: Option<u64>,
    pub jobs_in_queue: usize,
    pub uptime_seconds: u64,
}

/// API server state
pub struct ApiState {
    pub health: RwLock<HealthCheck>,
    pub metrics: Arc<ProverMetrics>,
    pub status: RwLock<ProverStatus>,
    start_time: std::time::Instant,
}

impl ApiState {
    pub fn new(metrics: Arc<ProverMetrics>) -> Self {
        Self {
            health: RwLock::new(HealthCheck::default()),
            metrics,
            status: RwLock::new(ProverStatus {
                is_running: true,
                current_job: None,
                jobs_in_queue: 0,
                uptime_seconds: 0,
            }),
            start_time: std::time::Instant::now(),
        }
    }

    pub async fn update_health(&self, rpc_connected: bool, last_block: u64) {
        let mut health = self.health.write().await;
        health.rpc_connected = rpc_connected;
        health.last_block_time = last_block;
    }

    pub async fn record_job_processed(&self) {
        let mut health = self.health.write().await;
        health.jobs_processed += 1;
        health.consecutive_failures = 0;
    }

    pub async fn record_failure(&self) {
        let mut health = self.health.write().await;
        health.consecutive_failures += 1;
    }

    pub async fn update_status(&self, current_job: Option<u64>, jobs_in_queue: usize) {
        let mut status = self.status.write().await;
        status.current_job = current_job;
        status.jobs_in_queue = jobs_in_queue;
        status.uptime_seconds = self.start_time.elapsed().as_secs();
    }
}

/// API server configuration
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub host: String,
    pub port: u16,
    pub enabled: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 9090,
            enabled: true,
        }
    }
}

/// Start the API server
pub async fn start_api_server(
    config: ApiConfig,
    state: Arc<ApiState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !config.enabled {
        info!("API server disabled");
        return Ok(());
    }

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;

    info!("Starting API server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    loop {
        let (socket, _) = listener.accept().await?;
        let state = state.clone();

        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut socket = socket;
            let mut buf = [0u8; 4096];

            match socket.read(&mut buf).await {
                Ok(0) => return,
                Ok(n) => {
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let response = handle_request(&request, &state).await;

                    if let Err(e) = socket.write_all(response.as_bytes()).await {
                        error!("Failed to write response: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to read request: {}", e);
                }
            }
        });
    }
}

async fn handle_request(request: &str, state: &ApiState) -> String {
    let path = extract_path(request);

    match path {
        "/health" => handle_health(state).await,
        "/metrics" => handle_metrics(state).await,
        "/stats" => handle_stats(state).await,
        "/status" => handle_status(state).await,
        "/" => handle_index(),
        _ => handle_not_found(),
    }
}

fn extract_path(request: &str) -> &str {
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
}

async fn handle_health(state: &ApiState) -> String {
    let health = state.health.read().await;
    let status = health.status();

    let (status_code, status_text) = match status {
        HealthStatus::Healthy => (200, "healthy"),
        HealthStatus::Degraded => (200, "degraded"),
        HealthStatus::Unhealthy => (503, "unhealthy"),
    };

    let body = serde_json::json!({
        "status": status_text,
        "checks": {
            "rpc_connected": health.rpc_connected,
            "last_block_time": health.last_block_time,
            "jobs_processed": health.jobs_processed,
            "consecutive_failures": health.consecutive_failures,
        }
    });

    http_response(status_code, "application/json", &body.to_string())
}

async fn handle_metrics(state: &ApiState) -> String {
    let metrics = state.metrics.to_prometheus();
    http_response(200, "text/plain; charset=utf-8", &metrics)
}

async fn handle_stats(state: &ApiState) -> String {
    let stats = state.metrics.get_stats();
    let body = serde_json::json!({
        "jobs_processed": stats.jobs_processed,
        "jobs_failed": stats.jobs_failed,
        "total_proofs_generated": stats.total_proofs,
        "total_earnings_wei": stats.total_earnings.to_string(),
        "cache_hits": stats.cache_hits,
        "cache_misses": stats.cache_misses,
        "cache_hit_rate": stats.cache_hit_rate(),
        "avg_proof_time_ms": stats.avg_proof_time_ms,
        "success_rate": stats.success_rate(),
    });

    http_response(200, "application/json", &body.to_string())
}

async fn handle_status(state: &ApiState) -> String {
    let mut status = state.status.write().await;
    status.uptime_seconds = state.start_time.elapsed().as_secs();

    let body = serde_json::to_string(&*status).unwrap();
    http_response(200, "application/json", &body)
}

fn handle_index() -> String {
    let body = r#"<!DOCTYPE html>
<html>
<head><title>World ZK Compute Prover</title></head>
<body>
<h1>World ZK Compute Prover API</h1>
<ul>
<li><a href="/health">Health Check</a></li>
<li><a href="/metrics">Prometheus Metrics</a></li>
<li><a href="/stats">JSON Stats</a></li>
<li><a href="/status">Prover Status</a></li>
</ul>
</body>
</html>"#;

    http_response(200, "text/html", body)
}

fn handle_not_found() -> String {
    http_response(404, "text/plain", "Not Found")
}

fn http_response(status_code: u16, content_type: &str, body: &str) -> String {
    let status_text = match status_code {
        200 => "OK",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Unknown",
    };

    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status_code,
        status_text,
        content_type,
        body.len(),
        body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_path() {
        assert_eq!(extract_path("GET /health HTTP/1.1\r\n"), "/health");
        assert_eq!(extract_path("GET /metrics HTTP/1.1\r\n"), "/metrics");
        assert_eq!(extract_path("GET / HTTP/1.1\r\n"), "/");
        assert_eq!(extract_path("invalid"), "/");
    }

    #[test]
    fn test_http_response() {
        let response = http_response(200, "text/plain", "OK");
        assert!(response.contains("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Type: text/plain"));
        assert!(response.contains("Content-Length: 2"));
        assert!(response.ends_with("OK"));
    }

    #[test]
    fn test_health_status() {
        let mut health = HealthCheck::default();
        assert_eq!(health.status(), HealthStatus::Unhealthy);

        health.rpc_connected = true;
        assert_eq!(health.status(), HealthStatus::Healthy);

        health.consecutive_failures = 10;
        assert_eq!(health.status(), HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn test_handle_index() {
        let response = handle_index();
        assert!(response.contains("200 OK"));
        assert!(response.contains("World ZK Compute Prover API"));
    }

    #[tokio::test]
    async fn test_handle_not_found() {
        let response = handle_not_found();
        assert!(response.contains("404 Not Found"));
    }
}
