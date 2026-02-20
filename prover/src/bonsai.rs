//! Bonsai cloud proving integration
//!
//! Bonsai is RISC Zero's cloud proving service that provides:
//! - GPU-accelerated proof generation
//! - Parallel proving across multiple machines
//! - 10-100x faster proofs compared to local CPU proving
//!
//! Use Bonsai for production workloads where proof generation speed matters.
//!
//! ## Async Design
//!
//! This module uses async/await patterns to avoid blocking the Tokio runtime:
//! - Blocking SDK calls are wrapped in `spawn_blocking`
//! - Polling uses `tokio::time::sleep` instead of `std::thread::sleep`
//! - All public methods are async

use anyhow::{Context, Result};
use bonsai_sdk::blocking::Client;
use risc0_zkvm::Receipt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

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

/// Async Bonsai prover client
///
/// Wraps the blocking bonsai-sdk client with async methods using `spawn_blocking`
/// to avoid blocking the Tokio runtime.
pub struct BonsaiProver {
    config: BonsaiConfig,
    /// Arc-wrapped client for sharing across spawn_blocking tasks
    client: Arc<Client>,
}

impl BonsaiProver {
    /// Create a new Bonsai prover
    pub fn new(config: BonsaiConfig) -> Result<Self> {
        let client = Client::from_parts(
            config.api_url.clone(),
            config.api_key.clone(),
            risc0_zkvm::VERSION,
        )?;

        Ok(Self {
            config,
            client: Arc::new(client),
        })
    }

    /// Create from environment variables
    pub fn from_env() -> Result<Self> {
        let config = BonsaiConfig::from_env()?;
        Self::new(config)
    }

    /// Upload program ELF to Bonsai (if not already uploaded)
    ///
    /// This is an async method that runs the blocking upload in a dedicated thread.
    pub async fn upload_program(&self, elf: &[u8]) -> Result<String> {
        let elf = elf.to_vec();
        let client = self.client.clone();

        // Run blocking operation in spawn_blocking to avoid blocking the runtime
        tokio::task::spawn_blocking(move || {
            let image_id = risc0_zkvm::compute_image_id(&elf)?;
            let image_id_hex = hex::encode(image_id);

            // Check if already uploaded
            if client.has_img(&image_id_hex)? {
                debug!("Program already uploaded: {}", image_id_hex);
                return Ok(image_id_hex);
            }

            info!("Uploading program to Bonsai: {}", image_id_hex);
            client.upload_img(&image_id_hex, elf)?;

            Ok(image_id_hex)
        })
        .await?
    }

    /// Execute program and generate proof via Bonsai (async)
    ///
    /// This is the main proving method. It:
    /// 1. Uploads the program (if needed)
    /// 2. Uploads the input
    /// 3. Creates a STARK proving session
    /// 4. Polls for completion using async sleep
    /// 5. If `use_snark`, converts STARK → Groth16 SNARK via Bonsai
    /// 6. Downloads and returns the proof
    pub async fn prove(
        &self,
        elf: &[u8],
        input: &[u8],
        use_snark: bool,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        info!("Starting Bonsai proving session (snark: {})...", use_snark);
        let start = std::time::Instant::now();

        // 1. Upload program if needed
        let image_id_hex = self.upload_program(elf).await?;

        // 2. Upload input (blocking, wrapped in spawn_blocking)
        let input = input.to_vec();
        let client = self.client.clone();
        let input_id = tokio::task::spawn_blocking(move || {
            debug!("Uploading input ({} bytes)...", input.len());
            client.upload_input(input)
        })
        .await??;

        // 3. Start STARK proving session
        let client = self.client.clone();
        let image_id_for_session = image_id_hex.clone();
        let session = tokio::task::spawn_blocking(move || {
            info!("Starting STARK proof generation...");
            client.create_session(
                image_id_for_session,
                input_id,
                vec![], // assumptions
                false,  // execute_only
            )
        })
        .await??;

        info!("STARK session created: {}", session.uuid);

        // 4. Poll for STARK completion (async loop with tokio::time::sleep)
        let receipt = self.wait_for_proof_async(&session).await?;

        let elapsed = start.elapsed();
        info!("Bonsai STARK proof generated in {:.2?}", elapsed);

        // 5. If SNARK requested, convert STARK → Groth16 via Bonsai
        let receipt = if use_snark {
            info!("Converting STARK → Groth16 SNARK via Bonsai...");
            let snark_receipt = self.convert_to_snark(&session.uuid).await?;
            info!(
                "SNARK conversion completed in {:.2?} (total)",
                start.elapsed()
            );
            snark_receipt
        } else {
            receipt
        };

        // 6. Extract seal and journal
        let seal = crate::prover::extract_seal(&receipt)?;
        let journal = receipt.journal.bytes.clone();

        info!(
            "Proof ready: seal={} bytes, journal={} bytes, type={}",
            seal.len(),
            journal.len(),
            if use_snark { "Groth16" } else { "STARK" }
        );

        Ok((seal, journal))
    }

    /// Convert a completed STARK session to Groth16 SNARK via Bonsai (async)
    async fn convert_to_snark(&self, stark_session_uuid: &str) -> Result<Receipt> {
        // Create SNARK session from STARK session
        let client = self.client.clone();
        let session_id = stark_session_uuid.to_string();
        let snark_session = tokio::task::spawn_blocking(move || {
            info!("Creating SNARK conversion session...");
            client.create_snark(session_id)
        })
        .await??;

        info!("SNARK session created: {}", snark_session.uuid);

        // Poll for SNARK completion
        let timeout = Duration::from_secs(600); // 10 minutes for SNARK
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("SNARK conversion timed out after {:?}", timeout);
            }

            let client = self.client.clone();
            let snark_uuid = snark_session.uuid.clone();

            let status = tokio::task::spawn_blocking(move || {
                let snark_id = bonsai_sdk::blocking::SnarkId { uuid: snark_uuid };
                snark_id.status(&client)
            })
            .await??;

            match status.status.as_str() {
                "RUNNING" => {
                    debug!("SNARK conversion in progress...");
                }
                "SUCCEEDED" => {
                    info!("SNARK conversion succeeded!");
                    let output_url = status
                        .output
                        .context("No output URL in successful SNARK session")?;

                    let client = self.client.clone();
                    let receipt = tokio::task::spawn_blocking(move || {
                        let receipt_bytes = client.download(&output_url)?;
                        let receipt: Receipt = bincode::deserialize(&receipt_bytes)?;
                        Ok::<_, anyhow::Error>(receipt)
                    })
                    .await??;

                    return Ok(receipt);
                }
                "FAILED" => {
                    let error = status
                        .error_msg
                        .unwrap_or_else(|| "Unknown error".to_string());
                    anyhow::bail!("SNARK conversion failed: {}", error);
                }
                "TIMED_OUT" => {
                    anyhow::bail!("SNARK session timed out on Bonsai");
                }
                "ABORTED" => {
                    anyhow::bail!("SNARK session was aborted");
                }
                other => {
                    warn!("Unknown SNARK status: {}", other);
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Wait for proof to complete using async polling
    ///
    /// Uses `tokio::time::sleep` instead of `std::thread::sleep` to avoid
    /// blocking the async runtime while waiting.
    async fn wait_for_proof_async(
        &self,
        session: &bonsai_sdk::blocking::SessionId,
    ) -> Result<Receipt> {
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let start = std::time::Instant::now();

        loop {
            // Check timeout
            if start.elapsed() > timeout {
                anyhow::bail!("Bonsai proof timed out after {:?}", timeout);
            }

            // Get session status (blocking call wrapped in spawn_blocking)
            let client = self.client.clone();
            let session_uuid = session.uuid.clone();

            let status = tokio::task::spawn_blocking(move || {
                // Recreate session ID for status check
                let session = bonsai_sdk::blocking::SessionId { uuid: session_uuid };
                session.status(&client)
            })
            .await??;

            match status.status.as_str() {
                "RUNNING" => {
                    debug!(
                        "Proof in progress... ({})",
                        status.state.unwrap_or_default()
                    );
                }
                "SUCCEEDED" => {
                    info!("Proof succeeded!");
                    let receipt_url = status
                        .receipt_url
                        .context("No receipt URL in successful session")?;

                    // Download receipt (blocking)
                    let client = self.client.clone();
                    let receipt = tokio::task::spawn_blocking(move || {
                        let receipt_bytes = client.download(&receipt_url)?;
                        let receipt: Receipt = bincode::deserialize(&receipt_bytes)?;
                        Ok::<_, anyhow::Error>(receipt)
                    })
                    .await??;

                    return Ok(receipt);
                }
                "FAILED" => {
                    let error = status
                        .error_msg
                        .unwrap_or_else(|| "Unknown error".to_string());
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

            // Use async sleep instead of blocking sleep!
            tokio::time::sleep(poll_interval).await;
        }
    }
}

/// Proving mode selection
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProvingMode {
    /// Local CPU proving (slow but free)
    #[default]
    Local,
    /// Local GPU proving (fast, requires CUDA or Metal)
    LocalGpu,
    /// Try GPU first, fall back to CPU
    GpuWithCpuFallback,
    /// Bonsai cloud proving (fast but requires API key)
    Bonsai,
    /// Try Bonsai first, fall back to local CPU
    BonsaiWithFallback,
    /// Try Bonsai first, fall back to GPU, then CPU
    BonsaiWithGpuFallback,
    /// Boundless decentralized proving marketplace
    Boundless,
    /// Try Boundless first, fall back to local CPU
    BoundlessWithFallback,
    /// Try Boundless first, fall back to GPU, then CPU
    BoundlessWithGpuFallback,
}

impl ProvingMode {
    /// Parse from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "gpu" | "local-gpu" | "local_gpu" => Self::LocalGpu,
            "gpu-fallback" | "gpu_fallback" => Self::GpuWithCpuFallback,
            "bonsai" => Self::Bonsai,
            "bonsai-fallback" | "bonsai_fallback" => Self::BonsaiWithFallback,
            "bonsai-gpu" | "bonsai_gpu" | "bonsai-gpu-fallback" => Self::BonsaiWithGpuFallback,
            "boundless" => Self::Boundless,
            "boundless-fallback" | "boundless_fallback" => Self::BoundlessWithFallback,
            "boundless-gpu" | "boundless_gpu" | "boundless-gpu-fallback" => {
                Self::BoundlessWithGpuFallback
            }
            _ => Self::Local,
        }
    }

    /// Check if this mode uses GPU
    pub fn uses_gpu(&self) -> bool {
        matches!(
            self,
            Self::LocalGpu
                | Self::GpuWithCpuFallback
                | Self::BonsaiWithGpuFallback
                | Self::BoundlessWithGpuFallback
        )
    }

    /// Check if this mode uses Bonsai
    pub fn uses_bonsai(&self) -> bool {
        matches!(
            self,
            Self::Bonsai | Self::BonsaiWithFallback | Self::BonsaiWithGpuFallback
        )
    }

    /// Check if this mode uses Boundless
    #[allow(dead_code)]
    pub fn uses_boundless(&self) -> bool {
        matches!(
            self,
            Self::Boundless | Self::BoundlessWithFallback | Self::BoundlessWithGpuFallback
        )
    }

    /// Check if this mode uses any remote proving service (Bonsai or Boundless)
    #[allow(dead_code)]
    pub fn uses_remote_proving(&self) -> bool {
        self.uses_bonsai() || self.uses_boundless()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proving_mode_parse() {
        assert_eq!(ProvingMode::from_str("local"), ProvingMode::Local);
        assert_eq!(ProvingMode::from_str("gpu"), ProvingMode::LocalGpu);
        assert_eq!(ProvingMode::from_str("local-gpu"), ProvingMode::LocalGpu);
        assert_eq!(
            ProvingMode::from_str("gpu-fallback"),
            ProvingMode::GpuWithCpuFallback
        );
        assert_eq!(ProvingMode::from_str("bonsai"), ProvingMode::Bonsai);
        assert_eq!(
            ProvingMode::from_str("bonsai-fallback"),
            ProvingMode::BonsaiWithFallback
        );
        assert_eq!(
            ProvingMode::from_str("bonsai-gpu"),
            ProvingMode::BonsaiWithGpuFallback
        );
        assert_eq!(ProvingMode::from_str("boundless"), ProvingMode::Boundless);
        assert_eq!(
            ProvingMode::from_str("boundless-fallback"),
            ProvingMode::BoundlessWithFallback
        );
        assert_eq!(
            ProvingMode::from_str("boundless-gpu"),
            ProvingMode::BoundlessWithGpuFallback
        );
    }

    #[test]
    fn test_proving_mode_flags() {
        assert!(ProvingMode::LocalGpu.uses_gpu());
        assert!(ProvingMode::GpuWithCpuFallback.uses_gpu());
        assert!(ProvingMode::BonsaiWithGpuFallback.uses_gpu());
        assert!(ProvingMode::BoundlessWithGpuFallback.uses_gpu());
        assert!(!ProvingMode::Local.uses_gpu());
        assert!(!ProvingMode::Bonsai.uses_gpu());
        assert!(!ProvingMode::Boundless.uses_gpu());

        assert!(ProvingMode::Bonsai.uses_bonsai());
        assert!(ProvingMode::BonsaiWithFallback.uses_bonsai());
        assert!(ProvingMode::BonsaiWithGpuFallback.uses_bonsai());
        assert!(!ProvingMode::Local.uses_bonsai());
        assert!(!ProvingMode::LocalGpu.uses_bonsai());
        assert!(!ProvingMode::Boundless.uses_bonsai());

        assert!(ProvingMode::Boundless.uses_boundless());
        assert!(ProvingMode::BoundlessWithFallback.uses_boundless());
        assert!(ProvingMode::BoundlessWithGpuFallback.uses_boundless());
        assert!(!ProvingMode::Local.uses_boundless());
        assert!(!ProvingMode::Bonsai.uses_boundless());

        // Remote proving covers both Bonsai and Boundless
        assert!(ProvingMode::Bonsai.uses_remote_proving());
        assert!(ProvingMode::Boundless.uses_remote_proving());
        assert!(ProvingMode::BoundlessWithFallback.uses_remote_proving());
        assert!(!ProvingMode::Local.uses_remote_proving());
        assert!(!ProvingMode::LocalGpu.uses_remote_proving());
    }

    #[test]
    fn test_bonsai_config_default() {
        let config = BonsaiConfig::default();
        assert!(!config.is_configured());
        assert_eq!(config.timeout_secs, 3600);
    }

    #[test]
    fn test_bonsai_prover_creation_without_api_key() {
        // Creating with empty API key should still succeed (fails on use)
        let config = BonsaiConfig::default();
        // Note: BonsaiProver::new may succeed or fail depending on bonsai-sdk validation
        let _result = BonsaiProver::new(config);
        // We don't assert anything - just verify it doesn't panic
    }
}
