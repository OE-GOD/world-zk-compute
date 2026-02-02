//! Private Input Server
//!
//! A server that stores encrypted inputs and releases them only to authorized provers.
//! Similar to Bonsol's Private Input Server approach.
//!
//! ## Flow
//!
//! 1. User uploads input → gets inputId
//! 2. User submits job on-chain with inputId
//! 3. Prover claims job on-chain
//! 4. Prover requests input with signed auth
//! 5. Server verifies claim on-chain → returns encrypted data + key
//!
//! ## API
//!
//! - POST /inputs - Upload input (returns inputId)
//! - GET /inputs/:id - Fetch input (requires auth headers)
//! - DELETE /inputs/:id - Delete input (admin only)
//! - GET /health - Health check

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use alloy::primitives::{Address, FixedBytes};
use alloy::signers::Signature;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use clap::Parser;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

/// Private Input Server CLI
#[derive(Parser, Debug)]
#[command(name = "private-input-server")]
#[command(about = "Private Input Server for World ZK Compute")]
struct Args {
    /// Server host
    #[arg(long, default_value = "0.0.0.0", env = "HOST")]
    host: String,

    /// Server port
    #[arg(long, default_value = "3000", env = "PORT")]
    port: u16,

    /// RPC URL for on-chain verification
    #[arg(long, env = "RPC_URL")]
    rpc_url: String,

    /// ExecutionEngine contract address
    #[arg(long, env = "ENGINE_ADDRESS")]
    engine_address: String,

    /// Master encryption key (32 bytes hex)
    #[arg(long, env = "MASTER_KEY")]
    master_key: Option<String>,

    /// Input TTL in hours
    #[arg(long, default_value = "24", env = "INPUT_TTL_HOURS")]
    input_ttl_hours: u64,

    /// Max input size in MB
    #[arg(long, default_value = "100", env = "MAX_INPUT_SIZE_MB")]
    max_input_size_mb: usize,

    /// Skip on-chain verification (for testing only!)
    #[arg(long, default_value = "false", env = "SKIP_CHAIN_VERIFICATION")]
    skip_chain_verification: bool,
}

/// Stored input entry
#[derive(Clone)]
struct StoredInput {
    /// Input ID (hash of encrypted data)
    id: [u8; 32],
    /// Encrypted input data
    encrypted_data: Vec<u8>,
    /// Encryption key (encrypted with master key)
    encrypted_key: Vec<u8>,
    /// Hash of plaintext for verification
    input_digest: String,
    /// Associated request ID (set when job is submitted)
    request_id: Option<u64>,
    /// Creation time
    created_at: DateTime<Utc>,
    /// Expiration time
    expires_at: DateTime<Utc>,
    /// Access log
    access_log: Vec<AccessEntry>,
}

/// Access log entry
#[derive(Clone, Serialize)]
struct AccessEntry {
    prover: String,
    timestamp: DateTime<Utc>,
    granted: bool,
}

/// Application state
struct AppState {
    /// Input storage (in-memory, use Redis/DB for production)
    inputs: DashMap<[u8; 32], StoredInput>,
    /// Master encryption key
    master_key: [u8; 32],
    /// Configuration
    config: AppConfig,
    /// HTTP client for RPC calls
    http_client: reqwest::Client,
}

/// Application configuration
#[derive(Clone)]
struct AppConfig {
    rpc_url: String,
    engine_address: Address,
    input_ttl: Duration,
    max_input_size: usize,
    skip_chain_verification: bool,
}

// === Request/Response Types ===

/// Upload input request
#[derive(Deserialize)]
struct UploadRequest {
    /// Raw input data (base64 encoded)
    data: String,
    /// TTL in hours (optional, uses default)
    ttl_hours: Option<u64>,
}

/// Upload input response
#[derive(Serialize)]
struct UploadResponse {
    /// Input ID to use in execution request
    input_id: String,
    /// Hash of plaintext input
    input_digest: String,
    /// Expiration time
    expires_at: String,
}

/// Fetch input response
#[derive(Serialize)]
struct FetchResponse {
    /// Encrypted input data (base64)
    encrypted_data: String,
    /// Decryption key (base64)
    decryption_key: String,
    /// Hash of plaintext for verification
    input_digest: String,
}

/// Error response
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    reason: Option<String>,
}

/// Health response
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    inputs_stored: usize,
}

// === Main ===

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present
    let _ = dotenvy::dotenv();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("private_input_server=info".parse()?)
                .add_directive("tower_http=debug".parse()?),
        )
        .init();

    // Parse CLI args
    let args = Args::parse();

    // Parse master key or generate random one
    let master_key: [u8; 32] = match args.master_key {
        Some(key_hex) => {
            let bytes = hex::decode(&key_hex)?;
            if bytes.len() != 32 {
                anyhow::bail!("Master key must be 32 bytes (64 hex chars)");
            }
            bytes.try_into().unwrap()
        }
        None => {
            warn!("No master key provided, generating random key (data won't persist across restarts!)");
            let mut key = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
            key
        }
    };

    // Parse engine address
    let engine_address: Address = args.engine_address.parse()?;

    // Create app state
    let state = Arc::new(AppState {
        inputs: DashMap::new(),
        master_key,
        config: AppConfig {
            rpc_url: args.rpc_url,
            engine_address,
            input_ttl: Duration::from_secs(args.input_ttl_hours * 3600),
            max_input_size: args.max_input_size_mb * 1024 * 1024,
            skip_chain_verification: args.skip_chain_verification,
        },
        http_client: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?,
    });

    // Start cleanup task
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await; // Every 5 minutes
            cleanup_expired_inputs(&cleanup_state);
        }
    });

    // Build router
    let app = Router::new()
        .route("/inputs", post(upload_input))
        .route("/inputs/{id}", get(fetch_input))
        .route("/inputs/{id}", delete(delete_input))
        .route("/health", get(health_check))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Start server
    let addr = format!("{}:{}", args.host, args.port);
    info!("Starting Private Input Server on {}", addr);
    info!("Engine address: {}", engine_address);

    if args.skip_chain_verification {
        warn!("⚠️  Chain verification is DISABLED - for testing only!");
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// === Handlers ===

/// Upload input - POST /inputs
async fn upload_input(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UploadRequest>,
) -> Result<Json<UploadResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Decode input
    let plaintext = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &req.data,
    )
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "INVALID_DATA".to_string(),
                reason: Some(format!("Failed to decode base64: {}", e)),
            }),
        )
    })?;

    // Check size
    if plaintext.len() > state.config.max_input_size {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "INPUT_TOO_LARGE".to_string(),
                reason: Some(format!(
                    "Input size {} exceeds max {}",
                    plaintext.len(),
                    state.config.max_input_size
                )),
            }),
        ));
    }

    // Calculate input digest
    let input_digest = sha256_hex(&plaintext);

    // Generate encryption key for this input
    let mut input_key = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut input_key);

    // Encrypt input with its key
    let encrypted_data = encrypt_aes_gcm(&plaintext, &input_key).map_err(|e| {
        error!("Encryption failed: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "ENCRYPTION_FAILED".to_string(),
                reason: None,
            }),
        )
    })?;

    // Encrypt the input key with master key
    let encrypted_key = encrypt_aes_gcm(&input_key, &state.master_key).map_err(|e| {
        error!("Key encryption failed: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "ENCRYPTION_FAILED".to_string(),
                reason: None,
            }),
        )
    })?;

    // Calculate input ID (hash of encrypted data)
    let input_id: [u8; 32] = sha256_bytes(&encrypted_data);

    // Calculate expiration
    let ttl = req
        .ttl_hours
        .map(|h| Duration::from_secs(h * 3600))
        .unwrap_or(state.config.input_ttl);
    let now = Utc::now();
    let expires_at = now + chrono::Duration::from_std(ttl).unwrap();

    // Store
    let stored = StoredInput {
        id: input_id,
        encrypted_data,
        encrypted_key,
        input_digest: input_digest.clone(),
        request_id: None,
        created_at: now,
        expires_at,
        access_log: vec![],
    };

    state.inputs.insert(input_id, stored);

    info!(
        "Input stored: id={}, size={}, expires={}",
        hex::encode(input_id),
        plaintext.len(),
        expires_at
    );

    Ok(Json(UploadResponse {
        input_id: hex::encode(input_id),
        input_digest,
        expires_at: expires_at.to_rfc3339(),
    }))
}

/// Fetch input - GET /inputs/:id
async fn fetch_input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<FetchResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Parse input ID
    let input_id: [u8; 32] = hex::decode(&id)
        .map_err(|_| bad_request("INVALID_ID", "Invalid hex input ID"))?
        .try_into()
        .map_err(|_| bad_request("INVALID_ID", "Input ID must be 32 bytes"))?;

    // Get stored input
    let mut stored = state.inputs.get_mut(&input_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "NOT_FOUND".to_string(),
                reason: Some("Input not found or expired".to_string()),
            }),
        )
    })?;

    // Check expiration
    if stored.expires_at < Utc::now() {
        drop(stored);
        state.inputs.remove(&input_id);
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "EXPIRED".to_string(),
                reason: Some("Input has expired".to_string()),
            }),
        ));
    }

    // Extract auth headers
    let request_id: u64 = headers
        .get("X-Request-Id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad_request("MISSING_AUTH", "Missing X-Request-Id header"))?;

    let prover_address = headers
        .get("X-Prover-Address")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad_request("MISSING_AUTH", "Missing X-Prover-Address header"))?;

    let signature = headers
        .get("X-Signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad_request("MISSING_AUTH", "Missing X-Signature header"))?;

    let timestamp: u64 = headers
        .get("X-Timestamp")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad_request("MISSING_AUTH", "Missing X-Timestamp header"))?;

    // Verify timestamp freshness (5 minute window)
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    if now_ms.saturating_sub(timestamp) > 5 * 60 * 1000 {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "EXPIRED_SIGNATURE".to_string(),
                reason: Some("Signature timestamp too old".to_string()),
            }),
        ));
    }

    // Verify signature
    let message = format!("{}:{}", request_id, timestamp);
    let recovered_address = recover_signer(&message, signature).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "INVALID_SIGNATURE".to_string(),
                reason: Some(format!("Signature verification failed: {}", e)),
            }),
        )
    })?;

    let claimed_address: Address = prover_address.parse().map_err(|_| {
        bad_request("INVALID_ADDRESS", "Invalid prover address format")
    })?;

    if recovered_address != claimed_address {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "SIGNATURE_MISMATCH".to_string(),
                reason: Some("Recovered address doesn't match claimed address".to_string()),
            }),
        ));
    }

    // Verify on-chain claim (unless disabled for testing)
    if !state.config.skip_chain_verification {
        verify_on_chain_claim(&state, request_id, &claimed_address)
            .await
            .map_err(|e| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "NOT_AUTHORIZED".to_string(),
                        reason: Some(e.to_string()),
                    }),
                )
            })?;
    }

    // Log access
    stored.access_log.push(AccessEntry {
        prover: prover_address.to_string(),
        timestamp: Utc::now(),
        granted: true,
    });

    // Decrypt the input key with master key
    let input_key = decrypt_aes_gcm(&stored.encrypted_key, &state.master_key).map_err(|e| {
        error!("Key decryption failed: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "DECRYPTION_FAILED".to_string(),
                reason: None,
            }),
        )
    })?;

    info!(
        "Input fetched: id={}, prover={}, request_id={}",
        hex::encode(input_id),
        prover_address,
        request_id
    );

    Ok(Json(FetchResponse {
        encrypted_data: base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &stored.encrypted_data,
        ),
        decryption_key: base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &input_key,
        ),
        input_digest: stored.input_digest.clone(),
    }))
}

/// Delete input - DELETE /inputs/:id
async fn delete_input(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let input_id: [u8; 32] = hex::decode(&id)
        .map_err(|_| bad_request("INVALID_ID", "Invalid hex input ID"))?
        .try_into()
        .map_err(|_| bad_request("INVALID_ID", "Input ID must be 32 bytes"))?;

    if state.inputs.remove(&input_id).is_some() {
        info!("Input deleted: {}", id);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "NOT_FOUND".to_string(),
                reason: None,
            }),
        ))
    }
}

/// Health check - GET /health
async fn health_check(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        inputs_stored: state.inputs.len(),
    })
}

// === Helpers ===

fn bad_request(error: &str, reason: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: error.to_string(),
            reason: Some(reason.to_string()),
        }),
    )
}

/// Verify on-chain that the prover has claimed this job
async fn verify_on_chain_claim(
    state: &AppState,
    request_id: u64,
    prover: &Address,
) -> anyhow::Result<()> {
    // Make JSON-RPC call to get request info
    let call_data = encode_get_request_call(request_id);

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{
            "to": format!("{:?}", state.config.engine_address),
            "data": format!("0x{}", hex::encode(&call_data))
        }, "latest"],
        "id": 1
    });

    let response = state
        .http_client
        .post(&state.config.rpc_url)
        .json(&payload)
        .send()
        .await?;

    let result: serde_json::Value = response.json().await?;

    if let Some(error) = result.get("error") {
        anyhow::bail!("RPC error: {}", error);
    }

    let data = result
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid RPC response"))?;

    // Decode response - check status and claimer
    let bytes = hex::decode(data.trim_start_matches("0x"))?;

    // The ExecutionRequest struct has status at a specific offset and claimedBy at another
    // This is simplified - in production you'd use alloy's ABI decoding
    if bytes.len() < 320 {
        anyhow::bail!("Response too short");
    }

    // Status is at offset 288 (9th field * 32 bytes)
    let status = bytes[288 + 31]; // Last byte of the status word

    // ClaimedBy is at offset 320 (11th field * 32 bytes)
    let claimer_bytes = &bytes[352..352 + 20];
    let claimer = Address::from_slice(claimer_bytes);

    // Status 1 = Claimed
    if status != 1 {
        anyhow::bail!("Job is not in Claimed status (status={})", status);
    }

    if claimer != *prover {
        anyhow::bail!(
            "Prover {} is not the claimer (claimer={})",
            prover,
            claimer
        );
    }

    Ok(())
}

/// Encode getRequest(uint256) function call
fn encode_get_request_call(request_id: u64) -> Vec<u8> {
    // Function selector for getRequest(uint256)
    let selector = &sha256_bytes(b"getRequest(uint256)")[..4];
    let mut data = selector.to_vec();

    // Encode request_id as uint256 (32 bytes, big endian)
    let mut id_bytes = [0u8; 32];
    id_bytes[24..].copy_from_slice(&request_id.to_be_bytes());
    data.extend_from_slice(&id_bytes);

    data
}

/// Recover signer address from message and signature
fn recover_signer(message: &str, signature: &str) -> anyhow::Result<Address> {
    // Ethereum signed message format
    let prefixed = format!(
        "\x19Ethereum Signed Message:\n{}{}",
        message.len(),
        message
    );
    let hash = sha256_bytes(prefixed.as_bytes());

    // Parse signature
    let sig_bytes = hex::decode(signature.trim_start_matches("0x"))?;
    if sig_bytes.len() != 65 {
        anyhow::bail!("Invalid signature length");
    }

    let signature = Signature::try_from(sig_bytes.as_slice())?;
    let hash_fixed: FixedBytes<32> = hash.into();

    let recovered = signature.recover_address_from_prehash(&hash_fixed)?;

    Ok(recovered)
}

/// Cleanup expired inputs
fn cleanup_expired_inputs(state: &AppState) {
    let now = Utc::now();
    let mut removed = 0;

    state.inputs.retain(|_, v| {
        let keep = v.expires_at > now;
        if !keep {
            removed += 1;
        }
        keep
    });

    if removed > 0 {
        info!("Cleaned up {} expired inputs", removed);
    }
}

/// SHA256 hash returning bytes
fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// SHA256 hash returning hex string
fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha256_bytes(data))
}

/// Encrypt with AES-256-GCM
fn encrypt_aes_gcm(plaintext: &[u8], key: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    let mut result = nonce.to_vec();
    result.extend(ciphertext);
    Ok(result)
}

/// Decrypt with AES-256-GCM
fn decrypt_aes_gcm(encrypted: &[u8], key: &[u8]) -> anyhow::Result<Vec<u8>> {
    if encrypted.len() < 28 {
        anyhow::bail!("Encrypted data too short");
    }

    let cipher = Aes256Gcm::new_from_slice(key)?;
    let nonce = Nonce::from_slice(&encrypted[..12]);
    let ciphertext = &encrypted[12..];

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

    Ok(plaintext)
}
