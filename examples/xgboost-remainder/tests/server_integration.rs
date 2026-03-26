//! HTTP integration tests for the warm prover server.
//!
//! These tests start an actual HTTP server and send real HTTP requests
//! using `reqwest`. Marked `#[ignore]` because `CachedProver::new()` takes
//! several seconds to build the circuit.
//!
//! Run with: cargo test --test server_integration -- --ignored --nocapture

use std::net::TcpListener;
use std::time::Duration;
use tokio::time::sleep;

use xgboost_remainder::model;
use xgboost_remainder::server::{run_server_with_config, ServerConfig};

/// Find a free TCP port by binding to port 0 and extracting the assigned port.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Start the server in the background and return the base URL.
async fn start_server(config: ServerConfig) -> String {
    let port = config.port;
    let base_url = format!("http://127.0.0.1:{}", port);

    tokio::spawn(async move {
        let model = model::sample_model();
        run_server_with_config(model, config).await.unwrap();
    });

    // Wait for server to be ready
    let client = reqwest::Client::new();
    for _ in 0..50 {
        if client
            .get(format!("{}/health", &base_url))
            .send()
            .await
            .is_ok()
        {
            return base_url;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("Server did not start within 5 seconds");
}

#[tokio::test]
#[ignore]
async fn test_health_endpoint() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["num_trees"], 2);
    assert_eq!(body["num_features"], 5);
    assert!(body["circuit_hash"].as_str().unwrap().len() > 0);
}

#[tokio::test]
#[ignore]
async fn test_prove_valid_features() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        request_timeout_secs: 300,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove", base_url))
        .json(&serde_json::json!({"features": [0.6, 0.2, 0.8, 0.5, 0.3]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["predicted_class"], 1);
    assert!(body["proof_hex"].as_str().unwrap().len() > 0);
    assert!(body["circuit_hash"].as_str().unwrap().len() > 0);
    assert!(body["public_inputs_hex"].as_str().unwrap().len() > 0);
    assert!(body["proof_size_bytes"].as_u64().unwrap() > 0);
    assert!(body["prove_time_ms"].as_u64().unwrap() > 0);
}

#[tokio::test]
#[ignore]
async fn test_prove_wrong_feature_count() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove", base_url))
        .json(&serde_json::json!({"features": [1.0, 2.0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Expected 5 features"));
}

#[tokio::test]
#[ignore]
async fn test_prove_batch_valid() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        request_timeout_secs: 600,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove/batch", base_url))
        .json(&serde_json::json!({
            "requests": [
                {"features": [0.6, 0.2, 0.8, 0.5, 0.3]},
                {"features": [0.1, 0.9, 0.3, 0.7, 0.4]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"].as_array().unwrap().len(), 2);
    assert!(body["total_time_ms"].as_u64().unwrap() > 0);
}

#[tokio::test]
#[ignore]
async fn test_prove_batch_empty() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove/batch", base_url))
        .json(&serde_json::json!({"requests": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("Empty batch"));
}

#[tokio::test]
#[ignore]
async fn test_prove_batch_exceeds_max() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    // Create 11 requests (max is 10)
    let requests: Vec<serde_json::Value> = (0..11)
        .map(|_| serde_json::json!({"features": [0.1, 0.2, 0.3, 0.4, 0.5]}))
        .collect();

    let resp = client
        .post(format!("{}/prove/batch", base_url))
        .json(&serde_json::json!({"requests": requests}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("exceeds maximum"));
}

#[tokio::test]
#[ignore]
async fn test_auth_required_without_key() {
    let port = free_port();
    let config = ServerConfig {
        port,
        api_key: Some("test-secret-key".to_string()),
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    // POST /prove without auth header should return 401
    let resp = client
        .post(format!("{}/prove", base_url))
        .json(&serde_json::json!({"features": [0.6, 0.2, 0.8, 0.5, 0.3]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
#[ignore]
async fn test_auth_required_wrong_key() {
    let port = free_port();
    let config = ServerConfig {
        port,
        api_key: Some("test-secret-key".to_string()),
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove", base_url))
        .header("Authorization", "Bearer wrong-key")
        .json(&serde_json::json!({"features": [0.6, 0.2, 0.8, 0.5, 0.3]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
#[ignore]
async fn test_auth_valid_key_allowed() {
    let port = free_port();
    let config = ServerConfig {
        port,
        api_key: Some("test-secret-key".to_string()),
        rate_limit: 0,
        request_timeout_secs: 300,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove", base_url))
        .header("Authorization", "Bearer test-secret-key")
        .json(&serde_json::json!({"features": [0.6, 0.2, 0.8, 0.5, 0.3]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore]
async fn test_health_bypasses_auth() {
    let port = free_port();
    let config = ServerConfig {
        port,
        api_key: Some("test-secret-key".to_string()),
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    // GET /health should work without any auth
    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore]
async fn test_prove_returns_duration_header() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        request_timeout_secs: 300,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/prove", base_url))
        .json(&serde_json::json!({"features": [0.6, 0.2, 0.8, 0.5, 0.3]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let duration = resp.headers().get("x-request-duration");
    assert!(duration.is_some());
    let val = duration.unwrap().to_str().unwrap();
    assert!(val.ends_with("ms"));
}

#[tokio::test]
#[ignore]
async fn test_rate_limiting_blocks_excess_requests() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 2,       // 2 per minute
        rate_limit_burst: 0, // no burst
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    // First 2 requests should succeed (use wrong features to get fast 400, not slow proof)
    for _ in 0..2 {
        let resp = client
            .post(format!("{}/prove", base_url))
            .json(&serde_json::json!({"features": [1.0]}))
            .send()
            .await
            .unwrap();
        // 400 = passed rate limit but failed validation (expected)
        assert_eq!(resp.status(), 400);
    }

    // Third request should hit rate limit (429)
    let resp = client
        .post(format!("{}/prove", base_url))
        .json(&serde_json::json!({"features": [1.0]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("Rate limit"));
}

#[tokio::test]
#[ignore]
async fn test_batch_with_mixed_feature_counts() {
    let port = free_port();
    let config = ServerConfig {
        port,
        rate_limit: 0,
        ..ServerConfig::default()
    };
    let base_url = start_server(config).await;
    let client = reqwest::Client::new();

    // One valid, one invalid feature count — should reject the whole batch
    let resp = client
        .post(format!("{}/prove/batch", base_url))
        .json(&serde_json::json!({
            "requests": [
                {"features": [0.1, 0.2, 0.3, 0.4, 0.5]},
                {"features": [0.1, 0.2]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Expected 5 features"));
}
