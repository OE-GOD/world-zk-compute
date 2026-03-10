//! HTTP server for warm prover mode.
//!
//! Pre-builds the GKR circuit and Pedersen generators on startup, then
//! serves proof requests via POST /prove and POST /prove/batch endpoints.
//! This avoids the expensive circuit construction on every request.
//!
//! Supports optional API key authentication via `Authorization: Bearer <key>`
//! on POST routes. GET /health is always unauthenticated.

use crate::circuit::CachedProver;
use crate::model::{self, XgboostModel};
use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Maximum number of requests in a single batch.
const MAX_BATCH_SIZE: usize = 10;

/// Shared application state for the warm prover server.
struct AppState {
    prover: CachedProver,
    /// Optional API key. When set, POST routes require `Authorization: Bearer <key>`.
    api_key: Option<String>,
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

/// Start the warm prover HTTP server.
///
/// When `api_key` is `Some(key)`, POST routes require `Authorization: Bearer <key>`.
/// GET /health is always unauthenticated.
///
/// This function blocks until the server is shut down.
pub async fn run_server(
    model: XgboostModel,
    host: &str,
    port: u16,
    api_key: Option<String>,
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

    if api_key.is_some() {
        println!("API key authentication enabled for POST routes");
    }

    let state = Arc::new(AppState { prover, api_key });

    // POST routes are protected by the auth middleware
    let auth_routes = Router::new()
        .route("/prove", post(prove_handler))
        .route("/prove/batch", post(batch_prove_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // GET /health is unauthenticated (for load balancer health checks)
    let app = Router::new()
        .route("/health", get(health_handler))
        .merge(auth_routes)
        .with_state(state);

    let bind_addr = format!("{}:{}", host, port);
    println!("Listening on {}", bind_addr);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
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
async fn prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProveRequest>,
) -> Result<Json<ProveResponse>, (StatusCode, Json<ErrorResponse>)> {
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

    // Generate proof (this is the expensive part, but circuit is cached)
    let start = Instant::now();
    let (proof_bytes, circuit_hash, public_inputs) = state
        .prover
        .prove(&req.features, predicted_class)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Proof generation failed: {}", e),
                }),
            )
        })?;
    let prove_time = start.elapsed();

    Ok(Json(ProveResponse {
        predicted_class,
        proof_hex: hex::encode(&proof_bytes),
        circuit_hash: hex::encode(&circuit_hash),
        public_inputs_hex: hex::encode(&public_inputs),
        proof_size_bytes: proof_bytes.len(),
        prove_time_ms: prove_time.as_millis() as u64,
    }))
}

/// POST /prove/batch -- generate proofs for multiple feature vectors.
///
/// Processes each request sequentially (proving is CPU-bound, parallelism won't help).
/// Returns all results in order. Fails fast on any invalid request.
async fn batch_prove_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchProveRequest>,
) -> Result<Json<BatchProveResponse>, (StatusCode, Json<ErrorResponse>)> {
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

    let batch_start = Instant::now();
    let mut results = Vec::with_capacity(req.requests.len());

    for single_req in &req.requests {
        // Validate feature count for each request
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

        let predicted_class = model::predict(&state.prover.model, &single_req.features);

        let start = Instant::now();
        let (proof_bytes, circuit_hash, public_inputs) = state
            .prover
            .prove(&single_req.features, predicted_class)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Proof failed: {}", e),
                    }),
                )
            })?;

        results.push(ProveResponse {
            predicted_class,
            proof_hex: hex::encode(&proof_bytes),
            circuit_hash: hex::encode(&circuit_hash),
            public_inputs_hex: hex::encode(&public_inputs),
            proof_size_bytes: proof_bytes.len(),
            prove_time_ms: start.elapsed().as_millis() as u64,
        });
    }

    Ok(Json(BatchProveResponse {
        results,
        total_time_ms: batch_start.elapsed().as_millis() as u64,
    }))
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
}
