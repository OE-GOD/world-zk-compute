//! Bonsai cloud proving integration
//!
//! Bonsai is RISC Zero's cloud proving service that provides:
//! - GPU-accelerated proof generation
//! - Parallel proving across multiple machines
//! - 10-100x faster proofs compared to local CPU proving
//!
//! Use Bonsai for production workloads where proof generation speed matters.

use anyhow::{Context, Result};
use bonsai_sdk::blocking::Client;
use risc0_zkvm::Receipt;
use std::time::Duration;
use tracing::{info, debug, warn};

/// Bonsai proving configuration
#[derive(Clone, Debug)]
pub struct BonsaiConfig {
    /// Bonsai API key (from https://bonsai.xyz)
    pub api_key: String,
    /// Bonsai API URL (default: https://api.bonsai.xyz)
    pub api_url: String,
    /// Maximum time to wait for proof (seconds)
    pub timeout_secs: u64,
    /// Polling interval when waiting for proof (seconds)
    pub poll_interval_secs: u64,
}

impl Default for BonsaiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            api_url: "https://api.bonsai.xyz".to_string(),
            timeout_secs: 3600, // 1 hour max
            poll_interval_secs: 5,
        }
    }
}

impl BonsaiConfig {
    /// Create from environment variables
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("BONSAI_API_KEY")
            .context("BONSAI_API_KEY environment variable not set")?;

        let api_url = std::env::var("BONSAI_API_URL")
            .unwrap_or_else(|_| "https://api.bonsai.xyz".to_string());

        let timeout_secs = std::env::var("BONSAI_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);

        let poll_interval_secs = std::env::var("BONSAI_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        Ok(Self {
            api_key,
            api_url,
            timeout_secs,
            poll_interval_secs,
        })
    }

    /// Check if Bonsai is configured
    #[allow(dead_code)]
    pub fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// Bonsai prover client
pub struct BonsaiProver {
    config: BonsaiConfig,
    client: Client,
}

impl BonsaiProver {
    /// Create a new Bonsai prover
    pub fn new(config: BonsaiConfig) -> Result<Self> {
        let client = Client::from_parts(
            config.api_url.clone(),
            config.api_key.clone(),
            risc0_zkvm::VERSION,
        )?;

        Ok(Self { config, client })
    }

    /// Create from environment variables
    pub fn from_env() -> Result<Self> {
        let config = BonsaiConfig::from_env()?;
        Self::new(config)
    }

    /// Upload program ELF to Bonsai (if not already uploaded)
    pub fn upload_program(&self, elf: &[u8]) -> Result<String> {
        let image_id = risc0_zkvm::compute_image_id(elf)?;
        let image_id_hex = hex::encode(image_id);

        // Check if already uploaded using has_img
        if self.client.has_img(&image_id_hex)? {
            debug!("Program already uploaded: {}", image_id_hex);
            return Ok(image_id_hex);
        }

        info!("Uploading program to Bonsai: {}", image_id_hex);
        self.client.upload_img(&image_id_hex, elf.to_vec())?;

        Ok(image_id_hex)
    }

    /// Execute program and generate proof via Bonsai
    pub fn prove(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        info!("Starting Bonsai proving session...");
        let start = std::time::Instant::now();

        // 1. Upload program if needed
        let image_id_hex = self.upload_program(elf)?;

        // 2. Upload input
        debug!("Uploading input ({} bytes)...", input.len());
        let input_id = self.client.upload_input(input.to_vec())?;

        // 3. Start proving session
        info!("Starting proof generation...");
        let session = self.client.create_session(
            image_id_hex.clone(),
            input_id,
            vec![], // assumptions
            false,  // execute_only
        )?;

        info!("Session created: {}", session.uuid);

        // 4. Poll for completion
        let receipt = self.wait_for_proof(&session)?;

        let elapsed = start.elapsed();
        info!("Bonsai proof generated in {:.2?}", elapsed);

        // 5. Extract seal and journal
        let seal = self.extract_seal(&receipt)?;
        let journal = receipt.journal.bytes.clone();

        info!(
            "Proof ready: seal={} bytes, journal={} bytes",
            seal.len(),
            journal.len()
        );

        Ok((seal, journal))
    }

    /// Wait for proof to complete
    fn wait_for_proof(&self, session: &bonsai_sdk::blocking::SessionId) -> Result<Receipt> {
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let start = std::time::Instant::now();

        loop {
            // Check timeout
            if start.elapsed() > timeout {
                anyhow::bail!("Bonsai proof timed out after {:?}", timeout);
            }

            // Get session status
            let status = session.status(&self.client)?;

            match status.status.as_str() {
                "RUNNING" => {
                    debug!("Proof in progress... ({})", status.state.unwrap_or_default());
                }
                "SUCCEEDED" => {
                    info!("Proof succeeded!");
                    let receipt_url = status.receipt_url
                        .context("No receipt URL in successful session")?;
                    let receipt_bytes = self.client.download(&receipt_url)?;
                    let receipt: Receipt = bincode::deserialize(&receipt_bytes)?;
                    return Ok(receipt);
                }
                "FAILED" => {
                    let error = status.error_msg.unwrap_or_else(|| "Unknown error".to_string());
                    anyhow::bail!("Bonsai proof failed: {}", error);
                }
                "TIMED_OUT" => {
                    anyhow::bail!("Bonsai session timed out");
                }
                "ABORTED" => {
                    anyhow::bail!("Bonsai session was aborted");
                }
                other => {
                    warn!("Unknown session status: {}", other);
                }
            }

            std::thread::sleep(poll_interval);
        }
    }

    /// Extract seal from receipt
    fn extract_seal(&self, receipt: &Receipt) -> Result<Vec<u8>> {
        let seal = bincode::serialize(&receipt.inner)?;
        Ok(seal)
    }
}

/// Proving mode selection
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProvingMode {
    /// Local CPU proving (slow but free)
    #[default]
    Local,
    /// Bonsai cloud proving (fast but requires API key)
    Bonsai,
    /// Try Bonsai first, fall back to local
    BonsaiWithFallback,
}

impl ProvingMode {
    /// Parse from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "bonsai" => Self::Bonsai,
            "bonsai-fallback" | "bonsai_fallback" => Self::BonsaiWithFallback,
            _ => Self::Local,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proving_mode_parse() {
        assert_eq!(ProvingMode::from_str("local"), ProvingMode::Local);
        assert_eq!(ProvingMode::from_str("bonsai"), ProvingMode::Bonsai);
        assert_eq!(ProvingMode::from_str("bonsai-fallback"), ProvingMode::BonsaiWithFallback);
    }

    #[test]
    fn test_bonsai_config_default() {
        let config = BonsaiConfig::default();
        assert!(!config.is_configured());
        assert_eq!(config.timeout_secs, 3600);
    }
}
