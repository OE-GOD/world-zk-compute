//! Boundless decentralized proving marketplace integration
//!
//! Boundless is RISC Zero's decentralized proof marketplace running on Base chain.
//! GPU operators compete to fulfill proof requests, returning standard risc0 Groth16
//! proofs compatible with on-chain verifiers.
//!
//! ## Status
//!
//! The `boundless-market` SDK crate (v1.1) requires `alloy` v1.0, but this project
//! currently uses `alloy` v0.8. Due to the `c-kzg` native library link conflict,
//! both versions cannot coexist. Once `alloy` is upgraded to v1.0 across this
//! project, enable the `boundless-market` dependency in `Cargo.toml` and replace
//! the placeholder `prove()` implementation with the real SDK calls (see comments).
//!
//! ## Configuration
//!
//! Required environment variables:
//! - `BOUNDLESS_RPC_URL` — Base chain RPC endpoint
//! - `BOUNDLESS_PRIVATE_KEY` (or `PRIVATE_KEY`) — Signer for market transactions
//! - One storage provider: `PINATA_JWT`, `S3_BUCKET`, or `GCS_BUCKET`
//!
//! Optional:
//! - `BOUNDLESS_TIMEOUT_SECS` — Max wait for fulfillment (default: 3600)
//! - `BOUNDLESS_POLL_INTERVAL_SECS` — Polling interval (default: 5)

use anyhow::{Context, Result};
use tracing::info;

/// Storage provider type for uploading program/input data
#[derive(Clone, Debug)]
pub enum StorageProvider {
    /// Pinata IPFS gateway
    Pinata { jwt: String },
    /// S3-compatible storage
    S3 { bucket: String },
    /// Google Cloud Storage
    Gcs { bucket: String },
}

/// Boundless proving configuration
#[derive(Clone, Debug)]
pub struct BoundlessConfig {
    /// Base chain RPC URL
    pub rpc_url: String,
    /// Private key for signing market transactions
    pub private_key: String,
    /// Storage provider for uploading program/input data
    pub storage_provider: StorageProvider,
    /// Maximum time to wait for proof fulfillment (seconds)
    pub timeout_secs: u64,
    /// Polling interval when waiting for fulfillment (seconds)
    pub poll_interval_secs: u64,
}

impl BoundlessConfig {
    /// Create from environment variables
    pub fn from_env() -> Result<Self> {
        let rpc_url = std::env::var("BOUNDLESS_RPC_URL")
            .context("BOUNDLESS_RPC_URL environment variable not set")?;

        let private_key = std::env::var("BOUNDLESS_PRIVATE_KEY")
            .or_else(|_| std::env::var("PRIVATE_KEY"))
            .context("Neither BOUNDLESS_PRIVATE_KEY nor PRIVATE_KEY environment variable set")?;

        let timeout_secs = std::env::var("BOUNDLESS_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);

        let poll_interval_secs = std::env::var("BOUNDLESS_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        let storage_provider = Self::storage_provider_from_env()?;

        Ok(Self {
            rpc_url,
            private_key,
            storage_provider,
            timeout_secs,
            poll_interval_secs,
        })
    }

    /// Check if Boundless is configured
    pub fn is_configured(&self) -> bool {
        !self.rpc_url.is_empty() && !self.private_key.is_empty()
    }

    /// Detect storage provider from environment variables
    fn storage_provider_from_env() -> Result<StorageProvider> {
        if let Ok(jwt) = std::env::var("PINATA_JWT") {
            return Ok(StorageProvider::Pinata { jwt });
        }

        if let Ok(bucket) = std::env::var("S3_BUCKET") {
            return Ok(StorageProvider::S3 { bucket });
        }

        if let Ok(bucket) = std::env::var("GCS_BUCKET") {
            return Ok(StorageProvider::Gcs { bucket });
        }

        anyhow::bail!(
            "No storage provider configured for Boundless. \
             Set one of: PINATA_JWT, S3_BUCKET, or GCS_BUCKET"
        );
    }
}

/// Async Boundless prover client
///
/// Submits proof requests to the Boundless decentralized marketplace and
/// waits for GPU operators to fulfill them.
///
/// ## Integration Path
///
/// This module is structured to mirror `bonsai.rs`. Once `alloy` is upgraded
/// to v1.0, the `boundless-market` crate can be added as a dependency and
/// `prove()` can be implemented using:
///
/// ```ignore
/// let client = boundless_market::Client::builder()
///     .with_rpc_url(rpc_url)
///     .with_deployment(None)
///     .with_uploader_config(&storage_config).await?
///     .with_private_key(signer)
///     .build().await?;
///
/// let request = client.new_request()
///     .with_program(elf)
///     .with_stdin(input);
///
/// let (request_id, expires_at) = client.submit_onchain(request).await?;
/// let fulfillment = client.wait_for_request_fulfillment(
///     request_id, poll_interval, expires_at
/// ).await?;
///
/// let seal = fulfillment.seal.to_vec();
/// let journal = fulfillment.data()?.journal()?.to_vec();
/// ```
pub struct BoundlessProver {
    config: BoundlessConfig,
}

impl BoundlessProver {
    /// Create from environment variables
    pub fn from_env() -> Result<Self> {
        let config = BoundlessConfig::from_env()?;
        info!(
            "Boundless prover configured (RPC: {}, storage: {:?})",
            config.rpc_url,
            match &config.storage_provider {
                StorageProvider::Pinata { .. } => "Pinata/IPFS",
                StorageProvider::S3 { .. } => "S3",
                StorageProvider::Gcs { .. } => "GCS",
            }
        );
        Ok(Self { config })
    }

    /// Execute program and generate proof via Boundless marketplace
    ///
    /// Submits a proof request to the Boundless marketplace and waits for
    /// a prover to fulfill it. Returns the Groth16 seal and journal.
    ///
    /// Note: The `_use_snark` parameter is accepted for API compatibility
    /// but Boundless always returns Groth16 proofs.
    pub async fn prove(
        &self,
        _elf: &[u8],
        _input: &[u8],
        _use_snark: bool,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        // TODO: Implement using boundless-market SDK once alloy is upgraded to v1.0.
        //
        // The boundless-market crate (v1.1) requires alloy v1.0, but this project
        // currently uses alloy v0.8. The c-kzg native library link conflict prevents
        // both versions from coexisting.
        //
        // To enable:
        // 1. Upgrade alloy to v1.0 in Cargo.toml
        // 2. Uncomment `boundless-market` dependency in Cargo.toml
        // 3. Update `boundless = ["boundless-market"]` in [features]
        // 4. Replace this method body with the SDK calls shown in the struct docs
        anyhow::bail!(
            "Boundless proving is not yet available: the boundless-market SDK requires \
             alloy v1.0, but this project uses alloy v0.8. Upgrade alloy to enable \
             Boundless integration. See prover/src/boundless.rs for details."
        )
    }

    /// Get a reference to the config
    #[allow(dead_code)]
    pub fn config(&self) -> &BoundlessConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boundless_config_from_env_missing() {
        // Without BOUNDLESS_RPC_URL, should fail
        std::env::remove_var("BOUNDLESS_RPC_URL");
        let result = BoundlessConfig::from_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_boundless_config_is_configured() {
        let config = BoundlessConfig {
            rpc_url: "https://rpc.example.com".to_string(),
            private_key: "0xdeadbeef".to_string(),
            storage_provider: StorageProvider::Pinata {
                jwt: "test".to_string(),
            },
            timeout_secs: 3600,
            poll_interval_secs: 5,
        };
        assert!(config.is_configured());

        let empty_config = BoundlessConfig {
            rpc_url: String::new(),
            private_key: String::new(),
            storage_provider: StorageProvider::Pinata { jwt: String::new() },
            timeout_secs: 3600,
            poll_interval_secs: 5,
        };
        assert!(!empty_config.is_configured());
    }

    #[test]
    fn test_boundless_prover_creation_without_env() {
        // Without env vars, from_env should fail
        std::env::remove_var("BOUNDLESS_RPC_URL");
        std::env::remove_var("BOUNDLESS_PRIVATE_KEY");
        let result = BoundlessProver::from_env();
        assert!(result.is_err());
    }
}
