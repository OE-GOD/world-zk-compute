//! GPU Optimization for Proof Generation
//!
//! Provides GPU-accelerated proving via RISC Zero's CUDA and Metal backends.
//!
//! ## Features
//!
//! - **Local GPU Proving**: CUDA (NVIDIA) and Metal (Apple) support
//! - **GPU Detection**: Runtime detection of available GPU backends
//! - **Fallback Support**: Automatic fallback to CPU when GPU unavailable

use anyhow::Result;
use risc0_zkvm::{ExecutorEnv, Receipt};
use std::time::Instant;
use tracing::{info, warn};

/// Available GPU backends for proof generation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Cuda/Metal variants are conditionally constructed at runtime via feature flags
pub enum GpuBackend {
    /// NVIDIA CUDA (Linux/Windows)
    Cuda,
    /// Apple Metal (macOS)
    Metal,
    /// No GPU available, use CPU
    Cpu,
}

impl GpuBackend {
    /// Detect available GPU backend at runtime
    ///
    /// Checks for CUDA and Metal support based on compile-time features
    /// and runtime availability.
    pub fn detect() -> Self {
        // Check CUDA first (Linux/Windows with NVIDIA GPU)
        #[cfg(feature = "cuda")]
        {
            if Self::is_cuda_available() {
                info!("CUDA GPU detected - using GPU acceleration");
                return Self::Cuda;
            }
        }

        // Check Metal (macOS with Apple GPU)
        #[cfg(feature = "metal")]
        {
            if Self::is_metal_available() {
                info!("Metal GPU detected - using GPU acceleration");
                return Self::Metal;
            }
        }

        // Check environment variable override
        if let Ok(backend) = std::env::var("RISC0_GPU_BACKEND") {
            match backend.to_lowercase().as_str() {
                "cuda" => {
                    #[cfg(feature = "cuda")]
                    {
                        info!("Using CUDA backend (via RISC0_GPU_BACKEND)");
                        return Self::Cuda;
                    }
                    #[cfg(not(feature = "cuda"))]
                    {
                        warn!("CUDA requested but not compiled in, using CPU");
                    }
                }
                "metal" => {
                    #[cfg(feature = "metal")]
                    {
                        info!("Using Metal backend (via RISC0_GPU_BACKEND)");
                        return Self::Metal;
                    }
                    #[cfg(not(feature = "metal"))]
                    {
                        warn!("Metal requested but not compiled in, using CPU");
                    }
                }
                "cpu" => {
                    info!("Using CPU backend (via RISC0_GPU_BACKEND)");
                    return Self::Cpu;
                }
                _ => {
                    warn!("Unknown GPU backend '{}', using CPU", backend);
                }
            }
        }

        info!("No GPU detected - using CPU proving");
        Self::Cpu
    }

    /// Check if CUDA is available at runtime
    #[cfg(feature = "cuda")]
    fn is_cuda_available() -> bool {
        // RISC Zero automatically detects CUDA when the feature is enabled
        // We can also check for the CUDA library
        std::env::var("CUDA_PATH").is_ok()
            || std::path::Path::new("/usr/local/cuda").exists()
            || std::path::Path::new("/opt/cuda").exists()
    }

    /// Check if Metal is available at runtime
    #[cfg(feature = "metal")]
    fn is_metal_available() -> bool {
        // Metal is always available on macOS with Apple Silicon or compatible GPU
        #[cfg(target_os = "macos")]
        {
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Check if this is a GPU backend
    pub fn is_gpu(&self) -> bool {
        matches!(self, Self::Cuda | Self::Metal)
    }

    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cuda => "CUDA",
            Self::Metal => "Metal",
            Self::Cpu => "CPU",
        }
    }
}

impl std::fmt::Display for GpuBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Local GPU prover
///
/// Wraps RISC Zero's prover with GPU-aware configuration
pub struct LocalGpuProver {
    backend: GpuBackend,
    /// When set, `prove_sync` sets `CUDA_VISIBLE_DEVICES` to this device
    /// before calling the risc0 prover. Used by multi-GPU device manager.
    gpu_device_id: Option<usize>,
}

impl LocalGpuProver {
    /// Create with specific backend
    pub fn with_backend(backend: GpuBackend) -> Self {
        Self {
            backend,
            gpu_device_id: None,
        }
    }

    /// Create with specific backend and GPU device ID.
    ///
    /// When `gpu_device_id` is set, `prove_sync` will set
    /// `CUDA_VISIBLE_DEVICES` to route to the assigned GPU.
    #[allow(dead_code)]
    pub fn with_backend_and_device(backend: GpuBackend, device_id: usize) -> Self {
        Self {
            backend,
            gpu_device_id: Some(device_id),
        }
    }

    /// Execute and prove using GPU (or CPU fallback)
    ///
    /// This method runs in a blocking context - use `prove_async` for async code.
    pub fn prove_sync(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let start = Instant::now();

        // Set CUDA_VISIBLE_DEVICES if a specific device is assigned
        if let Some(device_id) = self.gpu_device_id {
            std::env::set_var("CUDA_VISIBLE_DEVICES", device_id.to_string());
            info!(
                "Starting {} proof generation on GPU device {}...",
                self.backend, device_id
            );
        } else {
            info!("Starting {} proof generation...", self.backend);
        }

        // Build executor environment
        let env = ExecutorEnv::builder()
            .write_slice(input)
            .build()?;

        // Get the prover - RISC Zero automatically uses GPU when available
        // The `default_prover()` respects the cuda/metal feature flags
        let prover = risc0_zkvm::default_prover();

        // Generate proof
        let prove_info = prover.prove(env, elf)?;

        let elapsed = start.elapsed();
        info!("{} proof generated in {:.2?}", self.backend, elapsed);

        // Extract seal and journal
        let receipt = prove_info.receipt;
        let seal = extract_seal_local(&receipt)?;
        let journal = receipt.journal.bytes.clone();

        Ok((seal, journal))
    }

    /// Execute and prove asynchronously
    ///
    /// Runs the blocking prover in a dedicated thread pool to avoid
    /// blocking the async runtime.
    pub async fn prove_async(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let elf = elf.to_vec();
        let input = input.to_vec();
        let backend = self.backend;

        tokio::task::spawn_blocking(move || {
            let prover = LocalGpuProver::with_backend(backend);
            prover.prove_sync(&elf, &input)
        })
        .await?
    }
}

/// Extract seal from receipt (local helper to avoid module dependency)
fn extract_seal_local(receipt: &Receipt) -> Result<Vec<u8>> {
    let seal = bincode::serialize(&receipt.inner)?;
    Ok(seal)
}
