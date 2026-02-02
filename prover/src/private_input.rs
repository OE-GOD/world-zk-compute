//! Private Input Server Client
//!
//! Fetches private inputs from a Private Input Server with authentication.
//! Similar to Bonsol's approach: provers must prove they claimed the job
//! on-chain before receiving the input data.
//!
//! ## How It Works
//!
//! 1. User uploads input to Private Input Server, gets an `inputId`
//! 2. User submits job on-chain with `inputId` reference
//! 3. Prover claims the job on-chain
//! 4. Prover requests input from server, signing with their wallet
//! 5. Server verifies on-chain that prover is the claimer
//! 6. Server returns encrypted input + decryption key
//!
//! ## Usage
//!
//! ```rust,ignore
//! let client = PrivateInputClient::new(config, wallet);
//! let input = client.fetch_input(request_id, input_id).await?;
//! ```

use alloy::primitives::{Address, B256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Private Input Server configuration
#[derive(Clone, Debug)]
pub struct PrivateInputConfig {
    /// Server URL (e.g., "https://inputs.example.com")
    pub server_url: String,
    /// Request timeout
    pub timeout: Duration,
    /// Maximum input size (bytes)
    pub max_size: usize,
    /// Enable encryption (recommended)
    pub encryption_enabled: bool,
}

impl Default for PrivateInputConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:3000".to_string(),
            timeout: Duration::from_secs(30),
            max_size: 100 * 1024 * 1024, // 100 MB
            encryption_enabled: true,
        }
    }
}

impl PrivateInputConfig {
    /// Create config from environment variables
    pub fn from_env() -> Result<Self> {
        let server_url = std::env::var("PRIVATE_INPUT_SERVER_URL")
            .unwrap_or_else(|_| "http://localhost:3000".to_string());

        let timeout_secs: u64 = std::env::var("PRIVATE_INPUT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);

        let max_size: usize = std::env::var("PRIVATE_INPUT_MAX_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100 * 1024 * 1024);

        Ok(Self {
            server_url,
            timeout: Duration::from_secs(timeout_secs),
            max_size,
            encryption_enabled: true,
        })
    }
}

/// Authentication request sent to the server
#[derive(Debug, Serialize)]
pub struct AuthRequest {
    /// The job request ID on-chain
    pub request_id: u64,
    /// Prover's wallet address
    pub prover_address: String,
    /// Signature of (request_id + timestamp)
    pub signature: String,
    /// Current timestamp (milliseconds)
    pub timestamp: u64,
}

/// Response from the server when fetching input
#[derive(Debug, Deserialize)]
pub struct InputResponse {
    /// Encrypted input data (base64 encoded)
    pub encrypted_data: String,
    /// Decryption key (base64 encoded) - only sent if prover is authorized
    pub decryption_key: String,
    /// Hash of plaintext input for verification
    pub input_digest: String,
}

/// Response when input is not encrypted
#[derive(Debug, Deserialize)]
pub struct PlainInputResponse {
    /// Raw input data (base64 encoded)
    pub data: String,
    /// Hash of input for verification
    pub input_digest: String,
}

/// Error response from server
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub reason: Option<String>,
}

/// Private Input Server client
///
/// Authenticates with the server by signing requests with the prover's wallet.
pub struct PrivateInputClient {
    config: PrivateInputConfig,
    http: Client,
    signer: PrivateKeySigner,
}

impl PrivateInputClient {
    /// Create a new Private Input client
    pub fn new(config: PrivateInputConfig, signer: PrivateKeySigner) -> Result<Self> {
        let http = Client::builder()
            .timeout(config.timeout)
            .pool_max_idle_per_host(4)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            config,
            http,
            signer,
        })
    }

    /// Get the prover's address
    pub fn prover_address(&self) -> Address {
        self.signer.address()
    }

    /// Fetch private input for a claimed job
    ///
    /// The prover must have already claimed this job on-chain.
    /// The server will verify the claim before returning the input.
    pub async fn fetch_input(&self, request_id: u64, input_id: B256) -> Result<Vec<u8>> {
        info!(
            "Fetching private input: request_id={}, input_id={}",
            request_id,
            hex::encode(input_id)
        );

        // 1. Create authentication
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u64;

        let message = format!("{}:{}", request_id, timestamp);
        let signature = self.sign_message(&message).await?;

        let auth = AuthRequest {
            request_id,
            prover_address: self.signer.address().to_string(),
            signature,
            timestamp,
        };

        // 2. Make request to server
        let url = format!(
            "{}/inputs/{}",
            self.config.server_url,
            hex::encode(input_id)
        );

        debug!("Requesting input from: {}", url);

        let response = self
            .http
            .get(&url)
            .header("X-Request-Id", request_id.to_string())
            .header("X-Prover-Address", auth.prover_address.clone())
            .header("X-Signature", auth.signature.clone())
            .header("X-Timestamp", auth.timestamp.to_string())
            .send()
            .await
            .context("Failed to connect to Private Input Server")?;

        // 3. Handle response
        if !response.status().is_success() {
            let status = response.status();
            let error: Result<ErrorResponse, _> = response.json().await;

            return match error {
                Ok(err) => Err(anyhow!(
                    "Input fetch failed ({}): {} - {}",
                    status,
                    err.error,
                    err.reason.unwrap_or_default()
                )),
                Err(_) => Err(anyhow!("Input fetch failed: HTTP {}", status)),
            };
        }

        // 4. Decrypt and verify
        let plaintext = if self.config.encryption_enabled {
            let body: InputResponse = response
                .json()
                .await
                .context("Failed to parse response")?;

            let key = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &body.decryption_key,
            )
            .context("Failed to decode decryption key")?;

            let encrypted = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &body.encrypted_data,
            )
            .context("Failed to decode encrypted data")?;

            // Decrypt using AES-256-GCM
            let plaintext = decrypt_aes_gcm(&encrypted, &key)?;

            // Verify digest
            let digest = sha256_hex(&plaintext);
            if digest != body.input_digest {
                return Err(anyhow!(
                    "Input digest mismatch: expected {}, got {}",
                    body.input_digest,
                    digest
                ));
            }

            plaintext
        } else {
            let body: PlainInputResponse = response
                .json()
                .await
                .context("Failed to parse response")?;

            let plaintext = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &body.data,
            )
            .context("Failed to decode input data")?;

            // Verify digest
            let digest = sha256_hex(&plaintext);
            if digest != body.input_digest {
                return Err(anyhow!(
                    "Input digest mismatch: expected {}, got {}",
                    body.input_digest,
                    digest
                ));
            }

            plaintext
        };

        // 5. Check size
        if plaintext.len() > self.config.max_size {
            return Err(anyhow!(
                "Input too large: {} bytes (max: {})",
                plaintext.len(),
                self.config.max_size
            ));
        }

        info!(
            "Successfully fetched private input: {} bytes",
            plaintext.len()
        );

        Ok(plaintext)
    }

    /// Sign a message with the prover's wallet
    async fn sign_message(&self, message: &str) -> Result<String> {
        let signature = self
            .signer
            .sign_message(message.as_bytes())
            .await
            .context("Failed to sign message")?;

        Ok(format!("0x{}", hex::encode(signature.as_bytes())))
    }

    /// Check if a URL is a private input URL
    pub fn is_private_input_url(url: &str) -> bool {
        url.starts_with("private://") || url.starts_with("priv://")
    }

    /// Extract input ID from a private input URL
    pub fn parse_private_url(url: &str) -> Option<B256> {
        let id_str = url
            .trim_start_matches("private://")
            .trim_start_matches("priv://");

        hex::decode(id_str)
            .ok()
            .and_then(|bytes| {
                if bytes.len() == 32 {
                    Some(B256::from_slice(&bytes))
                } else {
                    None
                }
            })
    }
}

/// Calculate SHA256 hash and return as hex string
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Decrypt data using AES-256-GCM
///
/// Format: nonce (12 bytes) || ciphertext || tag (16 bytes)
fn decrypt_aes_gcm(encrypted: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };

    if encrypted.len() < 28 {
        // 12 (nonce) + 16 (tag) minimum
        return Err(anyhow!("Encrypted data too short"));
    }

    if key.len() != 32 {
        return Err(anyhow!("Invalid key length: expected 32, got {}", key.len()));
    }

    let nonce = Nonce::from_slice(&encrypted[..12]);
    let ciphertext = &encrypted[12..];

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| anyhow!("Failed to create cipher: {}", e))?;

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("Decryption failed: {}", e))?;

    Ok(plaintext)
}

/// Encrypt data using AES-256-GCM
///
/// Returns: nonce (12 bytes) || ciphertext || tag (16 bytes)
#[allow(dead_code)]
fn encrypt_aes_gcm(plaintext: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit, OsRng},
        Aes256Gcm, AeadCore,
    };

    if key.len() != 32 {
        return Err(anyhow!("Invalid key length: expected 32, got {}", key.len()));
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| anyhow!("Failed to create cipher: {}", e))?;

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    // Combine nonce + ciphertext
    let mut result = nonce.to_vec();
    result.extend(ciphertext);

    Ok(result)
}

/// Helper to resolve input URLs (supports HTTP, IPFS, and Private Input Server)
pub async fn resolve_input(
    url: &str,
    request_id: u64,
    private_client: Option<&PrivateInputClient>,
    ipfs_client: Option<&crate::ipfs::IpfsClient>,
) -> Result<Vec<u8>> {
    if PrivateInputClient::is_private_input_url(url) {
        // Private Input Server
        let input_id = PrivateInputClient::parse_private_url(url)
            .ok_or_else(|| anyhow!("Invalid private input URL: {}", url))?;

        match private_client {
            Some(client) => client.fetch_input(request_id, input_id).await,
            None => Err(anyhow!("Private Input client not configured")),
        }
    } else if crate::ipfs::IpfsClient::is_ipfs_url(url) {
        // IPFS
        match ipfs_client {
            Some(client) => client.fetch(url).await,
            None => {
                let client = crate::ipfs::IpfsClient::new(crate::ipfs::IpfsConfig::default());
                client.fetch(url).await
            }
        }
    } else {
        // Regular HTTP
        let response = reqwest::get(url).await?;
        if !response.status().is_success() {
            return Err(anyhow!("HTTP error: {}", response.status()));
        }
        Ok(response.bytes().await?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_private_input_url() {
        assert!(PrivateInputClient::is_private_input_url(
            "private://abc123"
        ));
        assert!(PrivateInputClient::is_private_input_url("priv://abc123"));
        assert!(!PrivateInputClient::is_private_input_url(
            "https://example.com"
        ));
        assert!(!PrivateInputClient::is_private_input_url("ipfs://Qm123"));
    }

    #[test]
    fn test_parse_private_url() {
        let url = "private://0000000000000000000000000000000000000000000000000000000000000001";
        let id = PrivateInputClient::parse_private_url(url);
        assert!(id.is_some());
        assert_eq!(id.unwrap()[31], 1);
    }

    #[test]
    fn test_sha256_hex() {
        let data = b"hello world";
        let hash = sha256_hex(data);
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0u8; 32]; // Test key (don't use in production!)
        let plaintext = b"secret data";

        let encrypted = encrypt_aes_gcm(plaintext, &key).unwrap();
        let decrypted = decrypt_aes_gcm(&encrypted, &key).unwrap();

        assert_eq!(plaintext.to_vec(), decrypted);
    }
}
