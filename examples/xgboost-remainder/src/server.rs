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
//!   Proofs run on the blocking thread pool, at most as many at once as the
//!   async runtime has worker threads. A request waiting for its turn counts
//!   toward the timeout. A proof cannot be cancelled: after a 504 it keeps
//!   running to the end and its result is dropped.

use crate::abi_encode::encode_pedersen_gens;
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
use tokio::sync::Semaphore;
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
    /// Pre-computed hex-encoded Pedersen generators (same for all proofs from this model).
    gens_hex: String,
    /// One permit per proof job allowed to run at once (see `proof_permit_count`).
    proof_permits: Arc<Semaphore>,
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
    /// Hex-encoded Pedersen generators for on-chain verification.
    pub gens_hex: String,
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
/// - **Request timeout**: The response is a 504 after `config.request_timeout_secs` (0 = unlimited).
///   The proof itself cannot be cancelled and keeps running to the end.
/// - **Proof concurrency**: At most as many proof jobs run at once as the
///   current runtime has worker threads; the others wait, inside their timeout.
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

    let prover = CachedProver::new(model)?;
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

    // Pre-compute ABI-encoded Pedersen generators once (deterministic, same for all proofs)
    let gens_hex = hex::encode(
        encode_pedersen_gens(&prover.pedersen_committer)
            .expect("Failed to encode Pedersen generators"),
    );
    println!("Pedersen generators encoded: {} bytes", gens_hex.len() / 2);

    let request_timeout = Duration::from_secs(config.request_timeout_secs);
    let rate_limiter = RateLimiter::new(config.rate_limit, config.rate_limit_burst);
    let max_proofs = proof_permit_count();
    println!("At most {} proofs run at once", max_proofs);
    let state = Arc::new(AppState {
        prover,
        api_key: config.api_key,
        rate_limiter,
        request_timeout,
        gens_hex,
        proof_permits: Arc::new(Semaphore::new(max_proofs)),
    });

    // POST routes are protected by auth middleware and rate limiting.
    // Layer order: auth (outermost, runs first) -> rate limit -> handler.
    let auth_routes = Router::new()
        .route("/prove", post(prove_handler))
        .route("/prove/bundle", post(prove_bundle_handler))
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

/// How many proof jobs may run at once: the number of worker threads of the
/// current runtime (tokio's default is one per CPU).
///
/// Before proofs moved to the blocking pool, each one held an async worker, so
/// no more ran at once than there were workers. The blocking pool has up to 512
/// threads, and a timed-out proof keeps running, so without this bound retries
/// after a 504 would pile up proofs that all share the CPU and all run slower.
fn proof_permit_count() -> usize {
    tokio::runtime::Handle::current().metrics().num_workers()
}

/// Message sent to the client when a proof job panics. The panic message stays
/// in the server log: it can name internal values the client has no use for.
const PROOF_PANIC_MESSAGE: &str = "proof generation panicked";

/// Why a proof job gave no result.
enum ProofJobError {
    /// The request timeout passed first, while the job waited for a permit or
    /// ran. A job that had started is still running.
    TimedOut,
    /// The job returned an error, its blocking task was cancelled (only at
    /// runtime shutdown), or the permits were closed.
    Failed(anyhow::Error),
    /// The job panicked. The details were logged, not kept here.
    Panicked,
}

/// Run a proof job on the blocking thread pool, under `timeout` unless it is zero.
///
/// The job first waits for a permit from `permits`; the wait counts toward the
/// timeout. Proving is CPU-bound and takes seconds. Run on an async worker it
/// would stall every other request on that worker, and a timeout around it
/// would never be checked, since the proof finishes before the timer is first
/// polled. A blocking task cannot be cancelled: after a timeout it keeps
/// running to the end, still holding its permit, and its result is dropped.
async fn run_proof_job<T, F>(
    permits: Arc<Semaphore>,
    timeout: Duration,
    job: F,
) -> Result<T, ProofJobError>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    let run = async move {
        let permit = permits.acquire_owned().await.map_err(|_closed| {
            ProofJobError::Failed(anyhow::anyhow!("proof permits are closed"))
        })?;
        tokio::task::spawn_blocking(move || {
            // Released when the closure ends, that is when the proof ends,
            // also after the request has timed out.
            let _permit = permit;
            job()
        })
        .await
        .map_err(join_error_to_job_error)
    };
    let joined = if timeout.is_zero() {
        run.await?
    } else {
        tokio::time::timeout(timeout, run)
            .await
            .map_err(|_elapsed| ProofJobError::TimedOut)??
    };
    joined.map_err(ProofJobError::Failed)
}

/// Turn a failed blocking task into an error. A panic is logged on the server,
/// and its details are kept out of the error.
fn join_error_to_job_error(err: tokio::task::JoinError) -> ProofJobError {
    if !err.is_panic() {
        // A blocking task is only cancelled when the runtime shuts down.
        return ProofJobError::Failed(anyhow::anyhow!("proof task was cancelled"));
    }
    let payload = err.into_panic();
    let detail = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(panic payload is not a string)".to_string());
    tracing::error!(panic = %detail, "proof job panicked");
    ProofJobError::Panicked
}

/// The answer for a proof job of /prove or /prove/bundle that gave no result.
fn single_proof_error(err: ProofJobError, timeout: Duration) -> (StatusCode, Json<ErrorResponse>) {
    let (status, error) = match err {
        ProofJobError::TimedOut => (
            StatusCode::GATEWAY_TIMEOUT,
            format!("Proof generation timed out after {}s", timeout.as_secs()),
        ),
        ProofJobError::Failed(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Proof generation failed: {}", e),
        ),
        ProofJobError::Panicked => (
            StatusCode::INTERNAL_SERVER_ERROR,
            PROOF_PANIC_MESSAGE.to_string(),
        ),
    };
    (status, Json(ErrorResponse { error }))
}

/// The answer for a /prove/batch job that gave no result.
fn batch_proof_error(err: ProofJobError, timeout: Duration) -> (StatusCode, Json<ErrorResponse>) {
    let (status, error) = match err {
        ProofJobError::TimedOut => (
            StatusCode::GATEWAY_TIMEOUT,
            format!(
                "Batch proof generation timed out after {}s",
                timeout.as_secs()
            ),
        ),
        ProofJobError::Failed(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Proof failed: {}", e),
        ),
        ProofJobError::Panicked => (
            StatusCode::INTERNAL_SERVER_ERROR,
            PROOF_PANIC_MESSAGE.to_string(),
        ),
    };
    (status, Json(ErrorResponse { error }))
}

/// POST /prove -- generate a proof for the given features.
///
/// Returns `X-Request-Duration` header with elapsed time in milliseconds.
/// Returns 504 if waiting for a proof slot plus the proof take longer than the
/// configured request timeout.
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

    let (proof_bytes, circuit_hash, public_inputs, prove_time) =
        prove_single(&state, req.features, predicted_class).await?;
    let duration_ms = request_start.elapsed().as_millis();

    let response = Json(ProveResponse {
        predicted_class,
        proof_hex: hex::encode(&proof_bytes),
        circuit_hash: hex::encode(&circuit_hash),
        public_inputs_hex: hex::encode(&public_inputs),
        gens_hex: state.gens_hex.clone(),
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

/// Proof bytes, circuit hash, public inputs and the time the proof itself took.
type SingleProof = (Vec<u8>, Vec<u8>, Vec<u8>, Duration);

/// Prove one feature vector on the blocking pool, under the request timeout.
///
/// The proving time is measured inside the job, so it leaves out the wait for
/// a permit.
async fn prove_single(
    state: &Arc<AppState>,
    features: Vec<f64>,
    predicted_class: u32,
) -> Result<SingleProof, (StatusCode, Json<ErrorResponse>)> {
    let job_state = state.clone();
    run_proof_job(
        state.proof_permits.clone(),
        state.request_timeout,
        move || {
            let start = Instant::now();
            let (proof_bytes, circuit_hash, public_inputs) =
                job_state.prover.prove(&features, predicted_class)?;
            Ok((proof_bytes, circuit_hash, public_inputs, start.elapsed()))
        },
    )
    .await
    .map_err(|e| single_proof_error(e, state.request_timeout))
}

/// POST /prove/bundle -- generate a self-contained ProofBundle JSON.
///
/// Returns 504 if waiting for a proof slot plus the proof take longer than the
/// configured request timeout.
async fn prove_bundle_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProveRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
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

    let predicted_class = model::predict(&state.prover.model, &req.features);
    let (proof_bytes, circuit_hash, public_inputs, prove_time) =
        prove_single(&state, req.features, predicted_class).await?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let bundle = serde_json::json!({
        "proof_hex": format!("0x{}", hex::encode(&proof_bytes)),
        "gens_hex": format!("0x{}", &state.gens_hex),
        "public_inputs_hex": format!("0x{}", hex::encode(&public_inputs)),
        "dag_circuit_description": hex::encode(&state.prover.circuit_desc),
        "circuit_hash": format!("0x{}", hex::encode(&circuit_hash)),
        "timestamp": timestamp,
        "prover_version": env!("CARGO_PKG_VERSION"),
        "predicted_class": predicted_class,
        "prove_time_ms": prove_time.as_millis() as u64,
    });

    Ok(Json(bundle))
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

    // Run batch proving on the blocking pool (with the timeout, when one is set)
    let job_state = state.clone();
    let requests = req.requests;
    // The whole batch is one job: its proofs run one after another on one
    // thread, so it takes one permit.
    let (results, batch_elapsed) = run_proof_job(
        state.proof_permits.clone(),
        state.request_timeout,
        move || run_batch_proofs(&job_state, &requests),
    )
    .await
    .map_err(|e| batch_proof_error(e, state.request_timeout))?;

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

/// Run batch proofs synchronously, inside one proof job (see `run_proof_job`).
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
            gens_hex: state.gens_hex.clone(),
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
            gens_hex: "5678".into(),
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
                gens_hex: "dd".into(),
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

    fn permits(n: usize) -> Arc<Semaphore> {
        Arc::new(Semaphore::new(n))
    }

    #[tokio::test]
    async fn test_proof_job_times_out_while_the_job_still_runs() {
        // A single-threaded runtime: a job run on it instead of the blocking
        // pool would hold the thread, and the timer could not fire.
        let start = Instant::now();
        let result = run_proof_job(permits(1), Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(1500));
            Ok(())
        })
        .await;
        assert!(matches!(result, Err(ProofJobError::TimedOut)));
        assert!(
            start.elapsed() < Duration::from_millis(1000),
            "the timeout fired only after the job ended: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn test_proof_job_returns_result_and_error_without_timeout() {
        let ok = run_proof_job(permits(1), Duration::ZERO, || Ok(7u32)).await;
        assert!(matches!(ok, Ok(7)));
        let err = run_proof_job(
            permits(1),
            Duration::from_secs(30),
            || -> anyhow::Result<u32> { anyhow::bail!("witness failed") },
        )
        .await;
        match err {
            Err(ProofJobError::Failed(e)) => assert_eq!(e.to_string(), "witness failed"),
            _ => panic!("expected the job's error"),
        }
    }

    #[tokio::test]
    async fn test_proof_jobs_never_run_more_at_once_than_permits() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let shared = permits(2);
        let running = Arc::new(AtomicUsize::new(0));
        let most_at_once = Arc::new(AtomicUsize::new(0));
        let mut jobs = tokio::task::JoinSet::new();
        for _ in 0..6 {
            let (shared, running, most_at_once) =
                (shared.clone(), running.clone(), most_at_once.clone());
            jobs.spawn(run_proof_job(shared, Duration::from_secs(30), move || {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most_at_once.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                running.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            }));
        }
        while let Some(joined) = jobs.join_next().await {
            assert!(matches!(joined.unwrap(), Ok(())));
        }
        assert_eq!(most_at_once.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_timed_out_job_keeps_its_permit_and_waiting_job_times_out() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let shared = permits(1);
        let first = run_proof_job(shared.clone(), Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(800));
            Ok(())
        })
        .await;
        assert!(matches!(first, Err(ProofJobError::TimedOut)));
        assert_eq!(
            shared.available_permits(),
            0,
            "the timed-out job still runs and must still hold its permit"
        );

        // A second job cannot start while the first runs; its timeout expires
        // while it waits for the permit.
        let second_ran = Arc::new(AtomicBool::new(false));
        let flag = second_ran.clone();
        let start = Instant::now();
        let second = run_proof_job(shared.clone(), Duration::from_millis(100), move || {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(matches!(second, Err(ProofJobError::TimedOut)));
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(!second_ran.load(Ordering::SeqCst));

        // Once the first job ends, its permit is free again.
        let free = shared.clone().acquire_owned();
        let _permit = tokio::time::timeout(Duration::from_secs(5), free)
            .await
            .expect("the permit was not released when the job ended")
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn test_proof_permit_count_is_the_runtime_worker_count() {
        assert_eq!(proof_permit_count(), 3);
    }

    #[tokio::test]
    async fn test_panicking_job_answers_500_with_a_fixed_message() {
        let result = run_proof_job(
            permits(1),
            Duration::from_secs(30),
            || -> anyhow::Result<()> { panic!("secret witness value 1234") },
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(()) => panic!("the job panicked, it cannot succeed"),
        };
        assert!(matches!(err, ProofJobError::Panicked));
        let (status, Json(body)) = single_proof_error(err, Duration::from_secs(30));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.error, "proof generation panicked");
        let (status, Json(body)) =
            batch_proof_error(ProofJobError::Panicked, Duration::from_secs(30));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.error, "proof generation panicked");
    }

    #[tokio::test]
    async fn test_panicking_job_releases_its_permit() {
        let shared = permits(1);
        let result = run_proof_job(shared.clone(), Duration::ZERO, || -> anyhow::Result<()> {
            panic!("boom")
        })
        .await;
        assert!(matches!(result, Err(ProofJobError::Panicked)));
        assert_eq!(shared.available_permits(), 1);
    }

    #[tokio::test]
    async fn test_server_rejects_unprovable_model() {
        let mut model = model::sample_model();
        model.trees.clear();
        let config = ServerConfig {
            port: 0,
            ..ServerConfig::default()
        };
        let err = run_server_with_config(model, config).await.unwrap_err();
        assert_eq!(err.to_string(), "Model has no trees");
    }
}
