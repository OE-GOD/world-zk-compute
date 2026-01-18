//! zkVM execution and proof generation
//!
//! Supports multiple proving modes:
//! - **Local**: CPU-based proving (slow but free)
//! - **LocalGpu**: GPU-accelerated local proving (CUDA/Metal)
//! - **GpuWithCpuFallback**: Try GPU first, fall back to CPU
//! - **Bonsai**: Cloud proving via RISC Zero's Bonsai service (fast, GPU-accelerated)
//! - **BonsaiWithFallback**: Try Bonsai first, fall back to local CPU
//! - **BonsaiWithGpuFallback**: Try Bonsai first, fall back to GPU, then CPU

use alloy::primitives::B256;
use risc0_zkvm::{default_prover, ExecutorEnv, Receipt};
use std::path::PathBuf;
use tracing::{info, debug, warn};

use crate::bonsai::{BonsaiProver, ProvingMode};
use crate::gpu_optimize::{GpuBackend, LocalGpuProver};

/// Unified prover that supports local, GPU, and Bonsai proving
pub struct UnifiedProver {
    mode: ProvingMode,
    bonsai_prover: Option<BonsaiProver>,
    gpu_prover: LocalGpuProver,
    gpu_backend: GpuBackend,
}

impl UnifiedProver {
    /// Create a new unified prover
    pub fn new(mode: ProvingMode) -> anyhow::Result<Self> {
        // Detect GPU backend
        let gpu_backend = GpuBackend::detect();
        let gpu_prover = LocalGpuProver::with_backend(gpu_backend);

        // Initialize Bonsai if needed
        let bonsai_prover = match &mode {
            ProvingMode::Bonsai | ProvingMode::BonsaiWithFallback | ProvingMode::BonsaiWithGpuFallback => {
                match BonsaiProver::from_env() {
                    Ok(prover) => {
                        info!("Bonsai prover initialized");
                        Some(prover)
                    }
                    Err(e) => {
                        if mode == ProvingMode::Bonsai {
                            return Err(e);
                        }
                        warn!("Bonsai not configured, will use local proving: {}", e);
                        None
                    }
                }
            }
            _ => None,
        };

        // Log GPU status
        if mode.uses_gpu() {
            if gpu_backend.is_gpu() {
                info!("GPU proving enabled: {}", gpu_backend);
            } else {
                warn!("GPU mode requested but no GPU detected, will use CPU");
            }
        }

        Ok(Self { mode, bonsai_prover, gpu_prover, gpu_backend })
    }

    /// Get the current GPU backend
    pub fn gpu_backend(&self) -> GpuBackend {
        self.gpu_backend
    }

    /// Check if GPU is available
    pub fn has_gpu(&self) -> bool {
        self.gpu_backend.is_gpu()
    }

    /// Execute and prove with the configured mode
    pub async fn prove(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        match (&self.mode, &self.bonsai_prover) {
            // Bonsai mode with prover available
            (ProvingMode::Bonsai, Some(prover)) => {
                info!("Using Bonsai cloud proving");
                prover.prove(elf, input).await
            }

            // Bonsai with CPU fallback
            (ProvingMode::BonsaiWithFallback, Some(prover)) => {
                info!("Trying Bonsai cloud proving...");
                match prover.prove(elf, input).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        warn!("Bonsai proving failed, falling back to local CPU: {}", e);
                        Self::prove_local_cpu(elf, input).await
                    }
                }
            }

            // Bonsai with GPU fallback (then CPU)
            (ProvingMode::BonsaiWithGpuFallback, Some(prover)) => {
                info!("Trying Bonsai cloud proving...");
                match prover.prove(elf, input).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        warn!("Bonsai proving failed, falling back to GPU: {}", e);
                        self.prove_with_gpu_fallback(elf, input).await
                    }
                }
            }

            // GPU mode (direct)
            (ProvingMode::LocalGpu, _) => {
                info!("Using local {} proving", self.gpu_backend);
                self.gpu_prover.prove_async(elf, input).await
            }

            // GPU with CPU fallback
            (ProvingMode::GpuWithCpuFallback, _) => {
                info!("Trying local {} proving...", self.gpu_backend);
                self.prove_with_gpu_fallback(elf, input).await
            }

            // Local CPU mode or fallback
            _ => {
                info!("Using local CPU proving");
                Self::prove_local_cpu(elf, input).await
            }
        }
    }

    /// Prove with GPU, falling back to CPU on failure
    async fn prove_with_gpu_fallback(&self, elf: &[u8], input: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        if self.gpu_backend.is_gpu() {
            match self.gpu_prover.prove_async(elf, input).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!("{} proving failed, falling back to CPU: {}", self.gpu_backend, e);
                }
            }
        }
        Self::prove_local_cpu(elf, input).await
    }

    /// Local CPU-based proving (async wrapper around blocking prover)
    ///
    /// Runs the blocking RISC Zero prover in a dedicated thread pool
    /// to avoid blocking the async runtime.
    async fn prove_local_cpu(elf: &[u8], input: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let elf = elf.to_vec();
        let input = input.to_vec();

        // Run blocking prover in spawn_blocking to avoid blocking the runtime
        tokio::task::spawn_blocking(move || {
            let start = std::time::Instant::now();

            // Build executor environment
            let env = ExecutorEnv::builder()
                .write_slice(&input)
                .build()?;

            // Run prover (uses CPU when no GPU features enabled)
            let prover = default_prover();
            let prove_info = prover.prove(env, &elf)?;

            let elapsed = start.elapsed();
            info!("Local CPU proof generated in {:.2?}", elapsed);

            let receipt = prove_info.receipt;

            // Note: Receipt verification happens on-chain via the RISC Zero verifier contract
            info!("Local proof generated, ready for on-chain verification");

            // Extract seal and journal
            let seal = extract_seal(&receipt)?;
            let journal = receipt.journal.bytes.clone();

            Ok((seal, journal))
        })
        .await?
    }
}

/// Execute a zkVM program and generate a proof
///
/// # Arguments
/// * `image_id` - The program's image ID
/// * `input_url` - URL to fetch inputs from
/// * `input_digest` - Expected hash of inputs
/// * `proving_mode` - Local or Bonsai proving
///
/// # Returns
/// * `(seal, journal)` - The proof seal and public outputs
pub async fn execute_and_prove(
    image_id: &B256,
    input_url: &str,
    input_digest: &B256,
    proving_mode: ProvingMode,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    info!("Fetching inputs from: {}", input_url);

    // Fetch inputs
    let input_bytes = fetch_inputs(input_url).await?;

    // Verify input digest
    let computed_digest = compute_digest(&input_bytes);
    if computed_digest != *input_digest {
        anyhow::bail!(
            "Input digest mismatch: expected {}, got {}",
            input_digest,
            computed_digest
        );
    }

    info!("Input digest verified");

    // Fetch the program ELF
    let elf = fetch_program_elf(image_id).await?;

    // Create prover and generate proof
    let prover = UnifiedProver::new(proving_mode)?;
    let (seal, journal) = prover.prove(&elf, &input_bytes).await?;

    info!(
        "Proof ready: seal={} bytes, journal={} bytes",
        seal.len(),
        journal.len()
    );

    Ok((seal, journal))
}

/// Legacy function for backwards compatibility (uses local proving)
#[allow(dead_code)]
pub async fn execute_and_prove_local(
    image_id: &B256,
    input_url: &str,
    input_digest: &B256,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    execute_and_prove(image_id, input_url, input_digest, ProvingMode::Local).await
}

/// Fetch inputs from URL
pub async fn fetch_inputs(url: &str) -> anyhow::Result<Vec<u8>> {
    // Handle different URL schemes
    if url.starts_with("ipfs://") {
        fetch_from_ipfs(url).await
    } else if url.starts_with("http://") || url.starts_with("https://") {
        fetch_from_http(url).await
    } else if url.starts_with("data:") {
        // Data URL (base64 encoded)
        parse_data_url(url)
    } else {
        anyhow::bail!("Unsupported URL scheme: {}", url)
    }
}

async fn fetch_from_http(url: &str) -> anyhow::Result<Vec<u8>> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}

async fn fetch_from_ipfs(url: &str) -> anyhow::Result<Vec<u8>> {
    // Convert ipfs:// to HTTP gateway URL
    let cid = url.trim_start_matches("ipfs://");
    let gateway_url = format!("https://ipfs.io/ipfs/{}", cid);
    fetch_from_http(&gateway_url).await
}

fn parse_data_url(url: &str) -> anyhow::Result<Vec<u8>> {
    // Parse data:application/octet-stream;base64,XXXX
    let parts: Vec<&str> = url.splitn(2, ',').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid data URL format");
    }

    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD.decode(parts[1])?;
    Ok(decoded)
}

/// Compute SHA256 digest of data
pub fn compute_digest(data: &[u8]) -> B256 {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    B256::from_slice(&result)
}

/// Fetch program ELF from registry or cache
pub async fn fetch_program_elf(image_id: &B256) -> anyhow::Result<Vec<u8>> {
    // In production, this would:
    // 1. Check local cache
    // 2. Query program registry for URL
    // 3. Download and verify ELF matches image ID

    // For now, look in local programs directory
    let programs_dir = PathBuf::from("./programs");
    let elf_path = programs_dir.join(format!("{}.elf", hex::encode(image_id)));

    if elf_path.exists() {
        debug!("Loading ELF from cache: {:?}", elf_path);
        let elf = std::fs::read(&elf_path)?;
        return Ok(elf);
    }

    anyhow::bail!(
        "Program ELF not found for image ID {}. \
        Please download and place in ./programs/",
        image_id
    )
}

/// Extract the proof seal from receipt
fn extract_seal(receipt: &Receipt) -> anyhow::Result<Vec<u8>> {
    // Serialize the receipt's inner proof
    let seal = bincode::serialize(&receipt.inner)?;
    Ok(seal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_digest() {
        let data = b"hello world";
        let digest = compute_digest(data);
        assert!(!digest.is_zero());
    }

    #[test]
    fn test_parse_data_url() {
        let url = "data:application/octet-stream;base64,SGVsbG8gV29ybGQ=";
        let decoded = parse_data_url(url).unwrap();
        assert_eq!(decoded, b"Hello World");
    }
}
