//! HTTP server for warm prover mode.
//!
//! Pre-builds the GKR circuit and Pedersen generators on startup, then
//! serves proof requests via POST /prove and POST /prove/batch endpoints.
//! This avoids the expensive circuit construction on every request.
//!
//! Supports optional API key authentication via `Authorization: Bearer <key>`
//! on POST routes. GET /health is always unauthenticated.
//!
//! Features:
//! - **Rate limiting**: Per-IP token bucket via `rate_limit` / `rate_limit_burst`.
//! - **CORS**: Optional cross-origin support via `enable_cors` / `cors_origins`.
//! - **Request timeout**: Per-request timeout on proof generation via `request_timeout_secs`.

use crate::circuit::CachedProver;
use crate::model::{self, XgboostModel};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderValue, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Maximum number of requests in a single batch.
const MAX_BATCH_SIZE: usize = 10;

/// Per-client token bucket state for rate limiting.
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

/// Token-bucket rate limiter keyed by client IP address.
///
/// Each IP gets `max_per_minute + burst` tokens initially. Tokens refill at
/// `max_per_minute / 60` tokens per second. A request consumes one token.
/// When `max_per_minute` is 0, rate limiting is disabled (all requests pass).
pub struct RateLimiter {
    max_per_minute: u32,
    burst: u32,
    clients: Mutex<HashMap<IpAddr, TokenBucket>>,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// - `max_per_minute`: sustained request rate (0 = unlimited).
    /// - `burst`: extra tokens above the per-minute rate for bursts.
    pub fn new(max_per_minute: u32, burst: u32) -> Self {
        Self {
            max_per_minute,
            burst,
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` if the request from `ip` should be allowed.
    ///
    /// When `max_per_minute` is 0, always returns `true` (unlimited).
    pub fn check(&self, ip: IpAddr) -> bool {
        if self.max_per_minute == 0 {
            return true; // unlimited
        }

        let mut clients = self.clients.lock().unwrap_or_else(|poisoned| {
            // Recover from a poisoned mutex (another thread panicked while holding the lock).
            // Rate limiting should not crash the server, so we recover the inner data.
            poisoned.into_inner()
        });
        let now = Instant::now();
        let capacity = (self.max_per_minute + self.burst) as f64;

        let entry = clients.entry(ip).or_insert(TokenBucket {
            tokens: capacity,
            last_refill: now,
        });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(entry.last_refill).as_secs_f64();
        let refill = elapsed * (self.max_per_minute as f64 / 60.0);
        entry.tokens = (entry.tokens + refill).min(capacity);
        entry.last_refill = now;

        // Consume a token
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Configuration for the warm prover HTTP server.
///
/// Use `Default::default()` for sensible defaults, or construct manually.
/// Pass to `run_server_with_config()` for full control, or use `run_server()`
/// for backward compatibility.
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    pub rate_limit: u32,
    pub rate_limit_burst: u32,
    /// Request timeout in seconds (0 = no timeout). Default: 120.
    pub request_timeout_secs: u64,
    /// Enable CORS headers on all responses. Default: false.
    pub enable_cors: bool,
    /// Allowed CORS origins. When `None` and CORS enabled, allows all origins.
    pub cors_origins: Option<Vec<String>>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
            api_key: None,
            rate_limit: 60,
            rate_limit_burst: 10,
            request_timeout_secs: 120,
            enable_cors: false,
            cors_origins: None,
        }
    }
}

/// Shared application state for the warm prover server.
struct AppState {
    prover: CachedProver,
    /// Optional API key. When set, POST routes require `Authorization: Bearer <key>`.
    api_key: Option<String>,
    /// Rate limiter applied to POST routes.
    rate_limiter: RateLimiter,
    /// Per-request timeout for proof generation (0 = no timeout).
    request_timeout: Duration,
}

/// Request body for the /prove endpoint.
#[derive(Debug, Deserialize)]
pub struct ProveRequest {
    /// Feature vector (floating point values).
    pub features: Vec<f64>,
}

/// Response body from the /prove endpoint.
#[derive(Debug, Serialize)]
pub struct ProveResponse {
    /// Predicted class from model inference.
    pub predicted_class: u32,
    /// Hex-encoded ABI proof bytes.
    pub proof_hex: String,
    /// Hex-encoded circuit hash.
    pub circuit_hash: String,
    /// Hex-encoded public inputs.
    pub public_inputs_hex: String,
    /// Proof size in bytes.
    pub proof_size_bytes: usize,
    /// Proving time in milliseconds.
    pub prove_time_ms: u64,
}

/// Request body for the /prove/batch endpoint.
#[derive(Debug, Deserialize)]
pub struct BatchProveRequest {
    /// List of individual prove requests to process sequentially.
    pub requests: Vec<ProveRequest>,
}

/// Response body from the /prove/batch endpoint.
#[derive(Debug, Serialize)]
pub struct BatchProveResponse {
    /// Results for each request in the batch, in order.
    pub results: Vec<ProveResponse>,
    /// Total wall-clock time for the entire batch in milliseconds.
    pub total_time_ms: u64,
}

/// Response body for the /health endpoint.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    num_trees: usize,
    num_features: usize,
    max_depth: usize,
    circuit_hash: String,
}

/// Error response body.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// Middleware that checks `Authorization: Bearer <key>` on incoming requests.
///
/// When `AppState.api_key` is `None`, all requests pass through.
/// When set, the bearer token must match exactly.
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    if let Some(ref expected_key) = state.api_key {
        let auth_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok());

        match auth_header {
            Some(header) if header.starts_with("Bearer ") => {
                let token = &header[7..];
                if token != expected_key {
                    return Err((
                        StatusCode::UNAUTHORIZED,
                        Json(ErrorResponse {
                            error: "Invalid API key".into(),
                        }),
                    ));
                }
            }
            _ => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "Missing Authorization: Bearer <key> header".into(),
                    }),
                ));
            }
        }
    }
    Ok(next.run(req).await)
}

/// Middleware that enforces per-IP rate limiting via a token bucket.
///
/// Returns 429 Too Many Requests when the client has exceeded their rate.
/// Only applied to POST routes (GET /health is exempt).
async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    if !state.rate_limiter.check(addr.ip()) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "Rate limit exceeded. Try again later.".into(),
            }),
        ));
    }
    Ok(next.run(req).await)
}

/// Start the warm prover HTTP server with full configuration.
///
/// Features:
/// - **Auth**: When `config.api_key` is set, POST routes require `Authorization: Bearer <key>`.
/// - **Rate limiting**: Token bucket per IP when `config.rate_limit > 0`.
/// - **CORS**: Cross-origin headers when `config.enable_cors` is true.
/// - **Request timeout**: Proof generation aborts after `config.request_timeout_secs` (0 = unlimited).
///
/// GET /health is always unauthenticated and exempt from rate limiting.
pub async fn run_server_with_config(
    model: XgboostModel,
    config: ServerConfig,
) -> anyhow::Result<()> {
    println!("Building circuit and generators (this may take a moment)...");
    let start = Instant::now();

    let num_trees = model.trees.len();
    let num_features = model.num_features;
    let max_depth = model.max_depth;

    let prover = CachedProver::new(model);
    let circuit_hash_hex = hex::encode(prover.circuit_hash);

    println!(
        "Warm prover ready in {:.2}s: {} trees, {} features, depth {}, circuit_hash={}",
        start.elapsed().as_secs_f64(),
        num_trees,
        num_features,
        max_depth,
        &circuit_hash_hex[..16],
    );

    if config.api_key.is_some() {
        println!("API key authentication enabled for POST routes");
    }

    if config.rate_limit > 0 {
        println!(
            "Rate limiting enabled: {} req/min per IP, burst={}",
            config.rate_limit, config.rate_limit_burst
        );
    }

    if config.request_timeout_secs > 0 {
        println!(
            "Request timeout: {}s per proof request",
            config.request_timeout_secs
        );
    }

    if config.enable_cors {
        match &config.cors_origins {
            Some(origins) => println!("CORS enabled for origins: {}", origins.join(", ")),
            None => println!("CORS enabled for all origins"),
        }
    }

    let request_timeout = Duration::from_secs(config.request_timeout_secs);
    let rate_limiter = RateLimiter::new(config.rate_limit, config.rate_limit_burst);
    let state = Arc::new(AppState {
        prover,
        api_key: config.api_key,
        rate_limiter,
        request_timeout,
    });

    // POST routes are protected by auth middleware and rate limiting.
    // Layer order: auth (outermost, runs first) -> rate limit -> handler.
    let auth_routes = Router::new()
        .route("/prove", post(prove_handler))
        .route("/prove/batch", post(batch_prove_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // GET /health is unauthenticated (for load balancer health checks)
    let mut app = Router::new()
        .route("/health", get(health_handler))
        .merge(auth_routes)
        .with_state(state);

    // Apply CORS layer if enabled
    if config.enable_cors {
        let cors = build_cors_layer(config.cors_origins.as_deref());
        app = app.layer(cors);
    }

    let bind_addr = format!("{}:{}", config.host, config.port);
    println!("Listening on {}", bind_addr);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Build a CORS layer from optional allowed origins.
///
/// When `origins` is `None`, allows all origins (permissive).
/// When `origins` is `Some(list)`, only those origins are allowed.
fn build_cors_layer(origins: Option<&[String]>) -> CorsLayer {
    let allow_origin = match origins {
        Some(list) => {
            let parsed: Vec<HeaderValue> = list.iter().filter_map(|o| o.parse().ok()).collect();
            AllowOrigin::list(parsed)
        }
        None => AllowOrigin::any(),
    };

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ])
}

/// GET /health -- returns server status and model info.
async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let resp = HealthResponse {
        status: "ok".to_string(),
        num_trees: state.prover.model.trees.len(),
        num_features: state.prover.model.num_features,
        max_depth: state.prover.max_depth,
        circuit_hash: hex::encode(state.prover.circuit_hash),
    };
    Json(resp)
}

/// POST /prove -- generate a proof for the given features.
///
/// Returns `X-Request-Duration` header with elapsed time in milliseconds.
/// Returns 504 if the proof exceeds the configured request timeout.
async fn prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProveRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    let request_start = Instant::now();

    // Validate feature count
    if req.features.len() != state.prover.model.num_features {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "Expected {} features, got {}",
                    state.prover.model.num_features,
                    req.features.len()
                ),
            }),
        ));
    }

    // Run inference
    let predicted_class = model::predict(&state.prover.model, &req.features);

    // Generate proof with optional timeout
    let start = Instant::now();
    let prove_result: anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> =
        if state.request_timeout.is_zero() {
            // No timeout — run directly
            state.prover.prove(&req.features, predicted_class)
        } else {
            // Wrap in timeout. Proving is CPU-bound but we can't cancel it,
            // so if timeout fires the blocking task keeps running (result is dropped).
            let timeout_dur = state.request_timeout;
            match tokio::time::timeout(timeout_dur, async {
                state.prover.prove(&req.features, predicted_class)
            })
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => {
                    return Err((
                        StatusCode::GATEWAY_TIMEOUT,
                        Json(ErrorResponse {
                            error: format!(
                                "Proof generation timed out after {}s",
                                state.request_timeout.as_secs()
                            ),
                        }),
                    ));
                }
            }
        };

    let (proof_bytes, circuit_hash, public_inputs) = prove_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Proof generation failed: {}", e),
            }),
        )
    })?;
    let prove_time = start.elapsed();
    let duration_ms = request_start.elapsed().as_millis();

    let response = Json(ProveResponse {
        predicted_class,
        proof_hex: hex::encode(&proof_bytes),
        circuit_hash: hex::encode(&circuit_hash),
        public_inputs_hex: hex::encode(&public_inputs),
        proof_size_bytes: proof_bytes.len(),
        prove_time_ms: prove_time.as_millis() as u64,
    });

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "X-Request-Duration",
        HeaderValue::from_str(&format!("{}ms", duration_ms))
            .expect("numeric duration string is always valid ASCII"),
    );

    Ok((headers, response))
}

/// POST /prove/batch -- generate proofs for multiple feature vectors.
///
/// Processes each request sequentially (proving is CPU-bound, parallelism won't help).
/// Returns all results in order. Fails fast on any invalid request.
/// The timeout applies to the entire batch, not individual proofs.
/// Returns `X-Request-Duration` header with elapsed time in milliseconds.
async fn batch_prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchProveRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    if req.requests.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Empty batch".into(),
            }),
        ));
    }
    if req.requests.len() > MAX_BATCH_SIZE {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "Batch size {} exceeds maximum {}",
                    req.requests.len(),
                    MAX_BATCH_SIZE
                ),
            }),
        ));
    }

    let request_start = Instant::now();

    // Validate all feature counts upfront before expensive proving
    for single_req in &req.requests {
        if single_req.features.len() != state.prover.model.num_features {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "Expected {} features, got {}",
                        state.prover.model.num_features,
                        single_req.features.len()
                    ),
                }),
            ));
        }
    }

    // Run batch proving (potentially with timeout)
    let batch_result = if state.request_timeout.is_zero() {
        run_batch_proofs(&state, &req.requests)
    } else {
        let state_clone = state.clone();
        let requests: Vec<ProveRequest> = req
            .requests
            .into_iter()
            .map(|r| ProveRequest {
                features: r.features,
            })
            .collect();
        let timeout_dur = state.request_timeout;

        match tokio::time::timeout(timeout_dur, async move {
            let result: anyhow::Result<_> =
                tokio::task::spawn_blocking(move || run_batch_proofs(&state_clone, &requests))
                    .await
                    .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?;
            result
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(ErrorResponse {
                        error: format!(
                            "Batch proof generation timed out after {}s",
                            state.request_timeout.as_secs()
                        ),
                    }),
                ));
            }
        }
    };

    let (results, batch_elapsed) = batch_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Proof failed: {}", e),
            }),
        )
    })?;

    let duration_ms = request_start.elapsed().as_millis();
    let response = Json(BatchProveResponse {
        results,
        total_time_ms: batch_elapsed.as_millis() as u64,
    });

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "X-Request-Duration",
        HeaderValue::from_str(&format!("{}ms", duration_ms))
            .expect("numeric duration string is always valid ASCII"),
    );

    Ok((headers, response))
}

/// Run batch proofs synchronously. Extracted so it can be used with or without
/// `spawn_blocking` + timeout.
fn run_batch_proofs(
    state: &AppState,
    requests: &[ProveRequest],
) -> anyhow::Result<(Vec<ProveResponse>, Duration)> {
    let batch_start = Instant::now();
    let mut results = Vec::with_capacity(requests.len());

    for single_req in requests {
        let predicted_class = model::predict(&state.prover.model, &single_req.features);

        let start = Instant::now();
        let (proof_bytes, circuit_hash, public_inputs) =
            state.prover.prove(&single_req.features, predicted_class)?;

        results.push(ProveResponse {
            predicted_class,
            proof_hex: hex::encode(&proof_bytes),
            circuit_hash: hex::encode(&circuit_hash),
            public_inputs_hex: hex::encode(&public_inputs),
            proof_size_bytes: proof_bytes.len(),
            prove_time_ms: start.elapsed().as_millis() as u64,
        });
    }

    Ok((results, batch_start.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_response_serialization() {
        let resp = ErrorResponse {
            error: "test error".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("test error"));
    }

    #[test]
    fn test_prove_request_deserialization() {
        let json = r#"{"features": [1.0, 2.0, 3.0]}"#;
        let req: ProveRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.features, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_batch_prove_request_deserialization() {
        let json = r#"{"requests": [{"features": [1.0, 2.0]}, {"features": [3.0, 4.0]}]}"#;
        let req: BatchProveRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.requests.len(), 2);
        assert_eq!(req.requests[0].features, vec![1.0, 2.0]);
        assert_eq!(req.requests[1].features, vec![3.0, 4.0]);
    }

    #[test]
    fn test_prove_response_serialization() {
        let resp = ProveResponse {
            predicted_class: 1,
            proof_hex: "deadbeef".into(),
            circuit_hash: "abcd1234".into(),
            public_inputs_hex: "1234".into(),
            proof_size_bytes: 4,
            prove_time_ms: 100,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("predicted_class"));
        assert!(json.contains("deadbeef"));
    }

    #[test]
    fn test_batch_prove_response_serialization() {
        let resp = BatchProveResponse {
            results: vec![ProveResponse {
                predicted_class: 0,
                proof_hex: "aa".into(),
                circuit_hash: "bb".into(),
                public_inputs_hex: "cc".into(),
                proof_size_bytes: 1,
                prove_time_ms: 50,
            }],
            total_time_ms: 200,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("total_time_ms"));
        assert!(json.contains("results"));
    }

    #[test]
    fn test_health_response_serialization() {
        let resp = HealthResponse {
            status: "ok".into(),
            num_trees: 5,
            num_features: 10,
            max_depth: 3,
            circuit_hash: "abcdef".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"num_trees\":5"));
    }

    #[test]
    fn test_max_batch_size_constant() {
        assert_eq!(MAX_BATCH_SIZE, 10);
    }

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(60, 10);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Should allow up to max_per_minute + burst = 70 requests immediately
        for i in 0..70 {
            assert!(
                limiter.check(ip),
                "Request {} should be allowed within capacity",
                i
            );
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new(10, 0);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Exhaust all 10 tokens
        for _ in 0..10 {
            assert!(limiter.check(ip));
        }

        // Next request should be blocked
        assert!(!limiter.check(ip), "Should block after exceeding limit");
    }

    #[test]
    fn test_rate_limiter_unlimited() {
        let limiter = RateLimiter::new(0, 0);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // When max_per_minute is 0, all requests should pass
        for _ in 0..1000 {
            assert!(limiter.check(ip));
        }
    }

    #[test]
    fn test_rate_limiter_separate_ips() {
        let limiter = RateLimiter::new(2, 0);
        let ip1: IpAddr = "10.0.0.1".parse().unwrap();
        let ip2: IpAddr = "10.0.0.2".parse().unwrap();

        // Exhaust ip1's tokens
        assert!(limiter.check(ip1));
        assert!(limiter.check(ip1));
        assert!(!limiter.check(ip1), "ip1 should be blocked");

        // ip2 should still have its own bucket
        assert!(limiter.check(ip2), "ip2 should be allowed independently");
        assert!(limiter.check(ip2));
        assert!(
            !limiter.check(ip2),
            "ip2 should be blocked after its own limit"
        );
    }

    #[test]
    fn test_rate_limiter_burst_allowance() {
        let limiter = RateLimiter::new(10, 5);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Should allow 10 + 5 = 15 requests before blocking
        for i in 0..15 {
            assert!(
                limiter.check(ip),
                "Request {} should be allowed (10 base + 5 burst)",
                i
            );
        }
        assert!(
            !limiter.check(ip),
            "Request 16 should be blocked after exhausting burst"
        );
    }

    #[test]
    fn test_server_config_defaults() {
        let config = ServerConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 3000);
        assert!(config.api_key.is_none());
        assert_eq!(config.rate_limit, 60);
        assert_eq!(config.rate_limit_burst, 10);
        assert_eq!(config.request_timeout_secs, 120);
        assert!(!config.enable_cors);
        assert!(config.cors_origins.is_none());
    }

    #[test]
    fn test_build_cors_layer_any_origin() {
        // Should not panic when building with no origin restrictions
        let _layer = build_cors_layer(None);
    }

    #[test]
    fn test_build_cors_layer_specific_origins() {
        let origins = vec![
            "http://localhost:3000".to_string(),
            "https://example.com".to_string(),
        ];
        // Should not panic when building with specific origins
        let _layer = build_cors_layer(Some(&origins));
    }

    #[test]
    fn test_build_cors_layer_empty_origins() {
        let origins: Vec<String> = vec![];
        // Empty list should not panic
        let _layer = build_cors_layer(Some(&origins));
    }

    #[test]
    fn test_rate_limiter_refill() {
        // Use a very high rate so tokens refill fast in wall-clock terms.
        // 6000 per minute = 100 per second. With burst=0, capacity=6000.
        let limiter = RateLimiter::new(6000, 0);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Consume all tokens
        for _ in 0..6000 {
            limiter.check(ip);
        }
        assert!(
            !limiter.check(ip),
            "Should be blocked after exhausting all tokens"
        );

        // Wait a short time for some tokens to refill.
        // At 100 tokens/sec, sleeping 50ms should refill ~5 tokens.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Should have refilled enough for at least 1 request
        assert!(
            limiter.check(ip),
            "Should be allowed after tokens have refilled"
        );
    }
}
