//! Boundless decentralized proving marketplace integration
//!
//! Boundless is RISC Zero's decentralized proof marketplace running on Base chain.
//! GPU operators compete to fulfill proof requests, returning standard risc0 Groth16
//! proofs compatible with on-chain verifiers.
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

use std::time::Duration;

use anyhow::{Context, Result};
use boundless_market::storage::StorageUploaderType;
use boundless_market::{Client, StorageUploaderConfig};
use secrecy::{ExposeSecret, SecretString};
use tracing::info;
use url::Url;

/// Storage provider type for uploading program/input data
#[derive(Clone, Debug)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub struct BoundlessConfig {
    /// Base chain RPC URL
    pub rpc_url: String,
    /// Private key for signing market transactions.
    /// Wrapped in `SecretString` to prevent accidental logging and ensure
    /// the value is zeroized on drop.
    pub private_key: SecretString,
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

        let private_key: SecretString = std::env::var("BOUNDLESS_PRIVATE_KEY")
            .or_else(|_| std::env::var("PRIVATE_KEY"))
            .context("Neither BOUNDLESS_PRIVATE_KEY nor PRIVATE_KEY environment variable set")?
            .into();

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
        !self.rpc_url.is_empty() && !self.private_key.expose_secret().is_empty()
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
        elf: &[u8],
        input: &[u8],
        _use_snark: bool,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        // Build storage uploader config from our StorageProvider enum
        let storage_config = match &self.config.storage_provider {
            StorageProvider::Pinata { jwt } => StorageUploaderConfig::builder()
                .storage_uploader(StorageUploaderType::Pinata)
                .pinata_jwt(jwt.clone())
                .build()?,
            StorageProvider::S3 { bucket } => StorageUploaderConfig::builder()
                .storage_uploader(StorageUploaderType::S3)
                .s3_bucket(bucket.clone())
                .build()?,
            StorageProvider::Gcs { bucket } => StorageUploaderConfig::builder()
                .storage_uploader(StorageUploaderType::Gcs)
                .gcs_bucket(bucket.clone())
                .build()?,
        };

        // Build the Boundless client
        let client = Client::builder()
            .with_rpc_url(Url::parse(&self.config.rpc_url)?)
            .with_private_key_str(self.config.private_key.expose_secret())?
            .with_uploader_config(&storage_config)
            .await?
            .build()
            .await?;

        // Submit proof request
        info!("Submitting proof request to Boundless marketplace");
        let params = client
            .new_request()
            .with_program(elf.to_vec())
            .with_stdin(input.to_vec());
        let (request_id, expires_at) = client.submit(params).await?;
        info!(%request_id, expires_at, "Proof request submitted, waiting for fulfillment");

        // Wait for a prover to fulfill the request
        let fulfillment = client
            .wait_for_request_fulfillment(
                request_id,
                Duration::from_secs(self.config.poll_interval_secs),
                expires_at,
            )
            .await?;

        // Extract seal and journal from fulfillment
        let seal = fulfillment.seal.to_vec();
        let journal = fulfillment
            .data()?
            .journal()
            .context("no journal in fulfillment")?
            .to_vec();

        info!(
            seal_len = seal.len(),
            journal_len = journal.len(),
            "Boundless proof received"
        );
        Ok((seal, journal))
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
