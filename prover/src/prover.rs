//! zkVM execution and proof generation
//!
//! Supports multiple proving modes:
//! - **Local**: CPU-based proving (slow but free)
//! - **LocalGpu**: GPU-accelerated local proving (CUDA/Metal)
//! - **GpuWithCpuFallback**: Try GPU first, fall back to CPU
//! - **Bonsai**: Cloud proving via RISC Zero's Bonsai service (fast, GPU-accelerated)
//! - **BonsaiWithFallback**: Try Bonsai first, fall back to local CPU
//! - **BonsaiWithGpuFallback**: Try Bonsai first, fall back to GPU, then CPU

use risc0_zkvm::{default_prover, ExecutorEnv, InnerReceipt, ProverOpts, Receipt};
use tracing::{info, warn};

use crate::bonsai::{BonsaiProver, ProvingMode};
use crate::gpu_optimize::{GpuBackend, LocalGpuProver};

/// Unified prover that supports local, GPU, Bonsai, and Boundless proving
pub struct UnifiedProver {
    mode: ProvingMode,
    bonsai_prover: Option<BonsaiProver>,
    #[cfg(feature = "boundless")]
    boundless_prover: Option<crate::boundless::BoundlessProver>,
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
            ProvingMode::Bonsai
            | ProvingMode::BonsaiWithFallback
            | ProvingMode::BonsaiWithGpuFallback => match BonsaiProver::from_env() {
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
            },
            _ => None,
        };

        // Initialize Boundless if needed
        #[cfg(feature = "boundless")]
        let boundless_prover = match &mode {
            ProvingMode::Boundless
            | ProvingMode::BoundlessWithFallback
            | ProvingMode::BoundlessWithGpuFallback => {
                match crate::boundless::BoundlessProver::from_env() {
                    Ok(prover) => {
                        info!("Boundless prover initialized");
                        Some(prover)
                    }
                    Err(e) => {
                        if mode == ProvingMode::Boundless {
                            return Err(e);
                        }
                        warn!("Boundless not configured, will use local proving: {}", e);
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

        Ok(Self {
            mode,
            bonsai_prover,
            #[cfg(feature = "boundless")]
            boundless_prover,
            gpu_prover,
            gpu_backend,
        })
    }

    /// Execute and prove with the configured mode
    pub async fn prove(&self, elf: &[u8], input: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        self.prove_inner(elf, input, false).await
    }

    /// Execute and prove, optionally generating a Groth16 SNARK for on-chain verification
    pub async fn prove_with_snark(
        &self,
        elf: &[u8],
        input: &[u8],
        use_snark: bool,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        self.prove_inner(elf, input, use_snark).await
    }

    async fn prove_inner(
        &self,
        elf: &[u8],
        input: &[u8],
        use_snark: bool,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        // Boundless dispatch (feature-gated)
        #[cfg(feature = "boundless")]
        if let Some(ref prover) = self.boundless_prover {
            match &self.mode {
                ProvingMode::Boundless => {
                    info!("Using Boundless decentralized proving");
                    return prover.prove(elf, input, use_snark).await;
                }
                ProvingMode::BoundlessWithFallback => {
                    info!("Trying Boundless decentralized proving...");
                    match prover.prove(elf, input, use_snark).await {
                        Ok(result) => return Ok(result),
                        Err(e) => {
                            warn!("Boundless proving failed, falling back to local CPU: {}", e);
                        }
                    }
                }
                ProvingMode::BoundlessWithGpuFallback => {
                    info!("Trying Boundless decentralized proving...");
                    match prover.prove(elf, input, use_snark).await {
                        Ok(result) => return Ok(result),
                        Err(e) => {
                            warn!("Boundless proving failed, falling back to GPU: {}", e);
                            return self.prove_with_gpu_fallback(elf, input, use_snark).await;
                        }
                    }
                }
                _ => {}
            }
        }

        match (&self.mode, &self.bonsai_prover) {
            // Bonsai mode with prover available
            (ProvingMode::Bonsai, Some(prover)) => {
                info!("Using Bonsai cloud proving");
                prover.prove(elf, input, use_snark).await
            }

            // Bonsai with CPU fallback
            (ProvingMode::BonsaiWithFallback, Some(prover)) => {
                info!("Trying Bonsai cloud proving...");
                match prover.prove(elf, input, use_snark).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        warn!("Bonsai proving failed, falling back to local CPU: {}", e);
                        Self::prove_local_cpu(elf, input, use_snark).await
                    }
                }
            }

            // Bonsai with GPU fallback (then CPU)
            (ProvingMode::BonsaiWithGpuFallback, Some(prover)) => {
                info!("Trying Bonsai cloud proving...");
                match prover.prove(elf, input, use_snark).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        warn!("Bonsai proving failed, falling back to GPU: {}", e);
                        self.prove_with_gpu_fallback(elf, input, use_snark).await
                    }
                }
            }

            // GPU mode (direct)
            (ProvingMode::LocalGpu, _) => {
                info!("Using local {} proving", self.gpu_backend);
                self.gpu_prover.prove_async(elf, input, use_snark).await
            }

            // GPU with CPU fallback
            (ProvingMode::GpuWithCpuFallback, _) => {
                info!("Trying local {} proving...", self.gpu_backend);
                self.prove_with_gpu_fallback(elf, input, use_snark).await
            }

            // Local CPU mode or fallback
            _ => {
                info!("Using local CPU proving");
                Self::prove_local_cpu(elf, input, use_snark).await
            }
        }
    }

    /// Prove with GPU, falling back to CPU on failure
    async fn prove_with_gpu_fallback(
        &self,
        elf: &[u8],
        input: &[u8],
        use_snark: bool,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        if self.gpu_backend.is_gpu() {
            match self.gpu_prover.prove_async(elf, input, use_snark).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!(
                        "{} proving failed, falling back to CPU: {}",
                        self.gpu_backend, e
                    );
                }
            }
        }
        Self::prove_local_cpu(elf, input, use_snark).await
    }

    /// Local CPU-based proving (async wrapper around blocking prover)
    ///
    /// Runs the blocking RISC Zero prover in a dedicated thread pool
    /// to avoid blocking the async runtime.
    ///
    /// When `use_snark` is true, generates a Groth16 proof suitable for
    /// on-chain verification. Requires Docker on x86_64 Linux (or Docker Desktop on macOS).
    async fn prove_local_cpu(
        elf: &[u8],
        input: &[u8],
        use_snark: bool,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let elf = elf.to_vec();
        let input = input.to_vec();

        // Run blocking prover in spawn_blocking to avoid blocking the runtime
        tokio::task::spawn_blocking(move || {
            let start = std::time::Instant::now();

            // Build executor environment
            let env = ExecutorEnv::builder().write_slice(&input).build()?;

            let prover = default_prover();

            let receipt = if use_snark {
                // Groth16 local proving requires x86_64 architecture + Docker
                if cfg!(not(target_arch = "x86_64")) {
                    anyhow::bail!(
                        "Groth16 SNARK proving requires x86_64 architecture. \
                         On Apple Silicon, use Bonsai cloud proving (--proving-mode bonsai) \
                         or run on an x86_64 Linux machine."
                    );
                }
                info!("Generating Groth16 SNARK proof (requires Docker)...");
                let opts = ProverOpts::groth16();
                let prove_info = prover.prove_with_opts(env, &elf, &opts)?;
                let elapsed = start.elapsed();
                info!("Groth16 proof generated in {:.2?}", elapsed);
                prove_info.receipt
            } else {
                let prove_info = prover.prove(env, &elf)?;
                let elapsed = start.elapsed();
                info!("Local CPU proof generated in {:.2?}", elapsed);
                prove_info.receipt
            };

            info!("Local proof generated, ready for on-chain verification");

            // Extract seal and journal
            let seal = extract_seal(&receipt)?;
            let journal = receipt.journal.bytes.clone();

            Ok((seal, journal))
        })
        .await?
    }
}

/// Extract the proof seal from receipt
///
/// For Groth16 receipts, encodes as `selector || seal` where selector is the
/// first 4 bytes of verifier_parameters — this is the format expected by the
/// on-chain RiscZeroVerifierRouter.
///
/// For other receipt types (STARK), falls back to bincode serialization.
pub fn extract_seal(receipt: &Receipt) -> anyhow::Result<Vec<u8>> {
    match &receipt.inner {
        InnerReceipt::Groth16(groth16_receipt) => {
            let selector = &groth16_receipt.verifier_parameters.as_bytes()[..4];
            let mut encoded = Vec::with_capacity(4 + groth16_receipt.seal.len());
            encoded.extend_from_slice(selector);
            encoded.extend_from_slice(&groth16_receipt.seal);
            info!(
                "Groth16 seal encoded: {} bytes (selector: {:02x}{:02x}{:02x}{:02x})",
                encoded.len(),
                selector[0],
                selector[1],
                selector[2],
                selector[3]
            );
            Ok(encoded)
        }
        _ => {
            // STARK receipts: bincode serialize for local/dev usage
            let seal = bincode::serialize(&receipt.inner)?;
            Ok(seal)
        }
    }
}

// Off x86_64 a Groth16 request is refused before the prover runs, which makes
// the `use_snark` flag observable without proving. There is no such signal on
// x86_64, so these tests are not built there.
#[cfg(all(test, not(target_arch = "x86_64")))]
mod tests {
    use super::*;

    /// In GPU mode `use_snark` is passed on to the GPU prover.
    #[tokio::test]
    async fn test_gpu_mode_passes_use_snark() {
        let prover = UnifiedProver::new(ProvingMode::LocalGpu).unwrap();
        let err = prover
            .prove_with_snark(b"not an elf", &[], true)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Groth16 SNARK proving requires"), "{err}");
    }
}
