//! Health Check and Monitoring API
//!
//! Provides HTTP endpoints for monitoring prover health and metrics.
//!
//! ## Endpoints
//!
//! - `GET /health` - Basic health check
//! - `GET /metrics` - Prometheus-style metrics
//! - `GET /status` - Detailed prover status
//!
//! ## Usage
//!
//! ```rust
//! let server = HealthServer::new(8081, metrics.clone());
//! server.start().await;
//! ```

#![allow(dead_code)]

use crate::metrics::{metrics, MetricsSnapshot};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error};

/// Prover status information
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProverStatus {
    /// Whether the prover is running
    pub running: bool,
    /// Current state
    pub state: ProverState,
    /// Wallet address
    pub wallet: String,
    /// Engine contract address
    pub engine: String,
    /// Last block processed
    pub last_block: u64,
    /// Jobs in queue
    pub queue_size: usize,
    /// Active proofs
    pub active_proofs: u64,
    /// Version
    pub version: &'static str,
}

/// Prover operational state
#[derive(Clone, Debug, serde::Serialize, PartialEq)]
pub enum ProverState {
    Starting,
    Running,
    Paused,
    Error(String),
}

/// Shared prover state for the health server
#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<ProverStatus>>,
}

impl SharedState {
    pub fn new(wallet: String, engine: String) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ProverStatus {
                running: false,
                state: ProverState::Starting,
                wallet,
                engine,
                last_block: 0,
                queue_size: 0,
                active_proofs: 0,
                version: env!("CARGO_PKG_VERSION"),
            })),
        }
    }

    pub async fn set_running(&self, running: bool) {
        let mut state = self.inner.write().await;
        state.running = running;
        state.state = if running { ProverState::Running } else { ProverState::Paused };
    }

    pub async fn set_error(&self, error: String) {
        let mut state = self.inner.write().await;
        state.state = ProverState::Error(error);
    }

    pub async fn update_block(&self, block: u64) {
        let mut state = self.inner.write().await;
        state.last_block = block;
    }

    pub async fn update_queue(&self, size: usize) {
        let mut state = self.inner.write().await;
        state.queue_size = size;
    }

    pub async fn update_active_proofs(&self, count: u64) {
        let mut state = self.inner.write().await;
        state.active_proofs = count;
    }

    pub async fn get_status(&self) -> ProverStatus {
        self.inner.read().await.clone()
    }
}

/// Health check server
pub struct HealthServer {
    port: u16,
    state: SharedState,
}

impl HealthServer {
    /// Create a new health server
    pub fn new(port: u16, state: SharedState) -> Self {
        Self { port, state }
    }

    /// Start the health server (non-blocking)
    pub async fn start(self) -> tokio::task::JoinHandle<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        info!("Starting health server on http://{}", addr);

        let state = self.state;

        tokio::spawn(async move {
            if let Err(e) = Self::run_server(addr, state).await {
                error!("Health server error: {}", e);
            }
        })
    }

    async fn run_server(addr: SocketAddr, state: SharedState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!("Health server listening on {}", addr);

        loop {
            let (stream, _) = listener.accept().await?;
            let state = state.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream, state).await {
                    error!("Connection error: {}", e);
                }
            });
        }
    }

    async fn handle_connection(
        mut stream: tokio::net::TcpStream,
        state: SharedState,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await?;

        // Parse the request path
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/");

        let response = match path {
            "/health" => Self::health_response(&state).await,
            "/metrics" => Self::metrics_response().await,
            "/status" => Self::status_response(&state).await,
            "/" => Self::root_response(),
            _ => Self::not_found_response(),
        };

        // Read remaining headers (required to properly handle the request)
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            if line == "\r\n" || line == "\n" || line.is_empty() {
                break;
            }
        }

        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;

        Ok(())
    }

    async fn health_response(state: &SharedState) -> String {
        let status = state.get_status().await;
        let (status_code, body) = if status.running && status.state == ProverState::Running {
            (200, r#"{"status":"healthy"}"#)
        } else {
            (503, r#"{"status":"unhealthy"}"#)
        };

        format!(
            "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            status_code,
            body.len(),
            body
        )
    }

    async fn metrics_response() -> String {
        let snapshot = metrics().snapshot();
        let body = Self::format_prometheus_metrics(&snapshot);

        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn format_prometheus_metrics(snapshot: &MetricsSnapshot) -> String {
        let mut output = String::new();

        // Counter metrics
        output.push_str("# HELP prover_proofs_total Total number of proofs generated\n");
        output.push_str("# TYPE prover_proofs_total counter\n");
        output.push_str(&format!("prover_proofs_total{{status=\"success\"}} {}\n", snapshot.proofs_generated));
        output.push_str(&format!("prover_proofs_total{{status=\"failed\"}} {}\n", snapshot.proofs_failed));

        // Gauge metrics
        output.push_str("# HELP prover_active_proofs Number of proofs currently in progress\n");
        output.push_str("# TYPE prover_active_proofs gauge\n");
        output.push_str(&format!("prover_active_proofs {}\n", snapshot.active_proofs));

        // Summary metrics
        output.push_str("# HELP prover_proof_duration_seconds Time to generate a proof\n");
        output.push_str("# TYPE prover_proof_duration_seconds summary\n");
        output.push_str(&format!("prover_proof_duration_seconds{{quantile=\"0.5\"}} {:.3}\n", snapshot.avg_proof_time.as_secs_f64()));
        output.push_str(&format!("prover_proof_duration_seconds{{quantile=\"0.99\"}} {:.3}\n", snapshot.p99_proof_time.as_secs_f64()));

        // Additional metrics
        output.push_str("# HELP prover_cycles_total Total zkVM cycles executed\n");
        output.push_str("# TYPE prover_cycles_total counter\n");
        output.push_str(&format!("prover_cycles_total {}\n", snapshot.total_cycles));

        output.push_str("# HELP prover_bytes_processed_total Total bytes of input processed\n");
        output.push_str("# TYPE prover_bytes_processed_total counter\n");
        output.push_str(&format!("prover_bytes_processed_total {}\n", snapshot.total_bytes_processed));

        output.push_str("# HELP prover_uptime_seconds Prover uptime in seconds\n");
        output.push_str("# TYPE prover_uptime_seconds gauge\n");
        output.push_str(&format!("prover_uptime_seconds {:.0}\n", snapshot.uptime.as_secs_f64()));

        output.push_str("# HELP prover_success_rate Proof success rate percentage\n");
        output.push_str("# TYPE prover_success_rate gauge\n");
        output.push_str(&format!("prover_success_rate {:.2}\n", snapshot.success_rate()));

        output.push_str("# HELP prover_throughput_per_hour Proofs completed per hour\n");
        output.push_str("# TYPE prover_throughput_per_hour gauge\n");
        output.push_str(&format!("prover_throughput_per_hour {:.2}\n", snapshot.proofs_per_hour()));

        output
    }

    async fn status_response(state: &SharedState) -> String {
        let status = state.get_status().await;
        let snapshot = metrics().snapshot();

        let body = serde_json::json!({
            "prover": status,
            "metrics": {
                "proofs_generated": snapshot.proofs_generated,
                "proofs_failed": snapshot.proofs_failed,
                "success_rate": format!("{:.1}%", snapshot.success_rate()),
                "throughput": format!("{:.1} proofs/hour", snapshot.proofs_per_hour()),
                "avg_proof_time_ms": snapshot.avg_proof_time.as_millis(),
                "p99_proof_time_ms": snapshot.p99_proof_time.as_millis(),
                "total_cycles": snapshot.total_cycles,
                "uptime_secs": snapshot.uptime.as_secs(),
            }
        }).to_string();

        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn root_response() -> String {
        let body = r#"{"name":"World ZK Prover","endpoints":["/health","/metrics","/status"]}"#;
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn not_found_response() -> String {
        let body = r#"{"error":"Not Found"}"#;
        format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shared_state() {
        let state = SharedState::new("0x123".to_string(), "0x456".to_string());

        state.set_running(true).await;
        let status = state.get_status().await;
        assert!(status.running);
        assert_eq!(status.state, ProverState::Running);

        state.update_block(100).await;
        let status = state.get_status().await;
        assert_eq!(status.last_block, 100);
    }

    #[test]
    fn test_prometheus_format() {
        let snapshot = MetricsSnapshot {
            uptime: std::time::Duration::from_secs(3600),
            proofs_generated: 100,
            proofs_failed: 5,
            total_cycles: 1_000_000_000,
            total_bytes_processed: 1024 * 1024 * 100,
            active_proofs: 2,
            avg_proof_time: std::time::Duration::from_secs(30),
            p99_proof_time: std::time::Duration::from_secs(120),
            avg_fetch_time: std::time::Duration::from_millis(500),
            avg_submit_time: std::time::Duration::from_millis(200),
        };

        let output = HealthServer::format_prometheus_metrics(&snapshot);
        assert!(output.contains("prover_proofs_total{status=\"success\"} 100"));
        assert!(output.contains("prover_active_proofs 2"));
    }
}
