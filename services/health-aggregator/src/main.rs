//! Health check aggregator for World ZK Compute services.
//!
//! Polls health endpoints of all configured services and provides
//! a unified status page at GET /status.
//!
//! Usage:
//!   health-aggregator
//!
//! Environment:
//!   SERVICES: comma-separated list of "name=url" pairs
//!   PORT: listen port (default 8090)
//!   POLL_INTERVAL_SECS: check interval (default 30)

use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceStatus {
    name: String,
    url: String,
    healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    last_checked: String,
}

#[derive(Debug, Clone, Serialize)]
struct AggregatedStatus {
    overall: String,
    services: Vec<ServiceStatus>,
    healthy_count: usize,
    total_count: usize,
}

type StatusStore = Arc<RwLock<HashMap<String, ServiceStatus>>>;

async fn check_service(client: &reqwest::Client, name: &str, url: &str) -> ServiceStatus {
    let start = std::time::Instant::now();
    let health_url = format!("{}/health", url.trim_end_matches('/'));

    let result = client.get(&health_url).send().await;

    let now = chrono_now();
    match result {
        Ok(resp) if resp.status().is_success() => ServiceStatus {
            name: name.to_string(),
            url: url.to_string(),
            healthy: true,
            response_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
            last_checked: now,
        },
        Ok(resp) => ServiceStatus {
            name: name.to_string(),
            url: url.to_string(),
            healthy: false,
            response_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(format!("HTTP {}", resp.status())),
            last_checked: now,
        },
        Err(e) => ServiceStatus {
            name: name.to_string(),
            url: url.to_string(),
            healthy: false,
            response_ms: None,
            error: Some(e.to_string()),
            last_checked: now,
        },
    }
}

fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}Z", secs)
}

async fn status_handler(State(store): State<StatusStore>) -> Json<AggregatedStatus> {
    let statuses = store.read().await;
    let services: Vec<ServiceStatus> = statuses.values().cloned().collect();
    let healthy_count = services.iter().filter(|s| s.healthy).count();
    let total_count = services.len();
    let overall = if healthy_count == total_count {
        "healthy"
    } else if healthy_count > 0 {
        "degraded"
    } else {
        "unhealthy"
    };

    Json(AggregatedStatus {
        overall: overall.to_string(),
        services,
        healthy_count,
        total_count,
    })
}

async fn health_handler(State(store): State<StatusStore>) -> Json<serde_json::Value> {
    let statuses = store.read().await;
    let all_healthy = statuses.values().all(|s| s.healthy);
    Json(serde_json::json!({
        "status": if all_healthy || statuses.is_empty() { "ok" } else { "degraded" },
        "services": statuses.len(),
    }))
}

fn parse_services(env_val: &str) -> Vec<(String, String)> {
    env_val
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            let (name, url) = s.split_once('=')?;
            Some((name.trim().to_string(), url.trim().to_string()))
        })
        .collect()
}

#[tokio::main]
async fn main() {
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "json".into());
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    match log_format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .init();
        }
    }

    let port = std::env::var("PORT").unwrap_or_else(|_| "8090".to_string());
    let poll_interval: u64 = std::env::var("POLL_INTERVAL_SECS")
        .unwrap_or_else(|_| "30".to_string())
        .parse()
        .unwrap_or(30);

    let services_env = std::env::var("SERVICES").unwrap_or_else(|_| {
        "verifier=http://localhost:3000,enclave=http://localhost:8080,operator=http://localhost:9090".to_string()
    });
    let services = parse_services(&services_env);

    let store: StatusStore = Arc::new(RwLock::new(HashMap::new()));

    // Spawn polling task
    let poll_store = store.clone();
    let poll_services = services.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        loop {
            for (name, url) in &poll_services {
                let status = check_service(&client, name, url).await;
                poll_store.write().await.insert(name.clone(), status);
            }
            tokio::time::sleep(Duration::from_secs(poll_interval)).await;
        }
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/status", get(status_handler))
        .with_state(store);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("Health aggregator on {addr}, polling {} services every {poll_interval}s", services.len());

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_services() {
        let services = parse_services("api=http://localhost:3000, enclave=http://localhost:8080");
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].0, "api");
        assert_eq!(services[0].1, "http://localhost:3000");
        assert_eq!(services[1].0, "enclave");
    }

    #[test]
    fn test_parse_empty() {
        let services = parse_services("");
        assert!(services.is_empty());
    }

    #[test]
    fn test_parse_single() {
        let services = parse_services("verifier=http://localhost:3000");
        assert_eq!(services.len(), 1);
    }
}
