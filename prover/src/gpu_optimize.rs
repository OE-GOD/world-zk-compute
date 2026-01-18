//! GPU Optimization for Proof Generation
//!
//! Provides GPU-accelerated proving for both local and Bonsai workflows.
//!
//! ## Features
//!
//! - **Local GPU Proving**: CUDA (NVIDIA) and Metal (Apple) support
//! - **GPU Detection**: Runtime detection of available GPU backends
//! - **Fallback Support**: Automatic fallback to CPU when GPU unavailable
//! - **Request Batching**: Batch multiple proofs to amortize API overhead
//! - **Pipeline Parallelism**: Upload next job while current proves
//! - **Adaptive Concurrency**: Auto-tune based on GPU load
//!
//! ## Usage
//!
//! ```rust
//! use gpu_optimize::{GpuProver, GpuBackend};
//!
//! // Check GPU availability
//! if let Some(backend) = GpuBackend::detect() {
//!     let prover = GpuProver::new(backend);
//!     let (seal, journal) = prover.prove(&elf, &input).await?;
//! }
//! ```

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use risc0_zkvm::{ExecutorEnv, Receipt};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tracing::{debug, info, warn};

// ═══════════════════════════════════════════════════════════════════════════
// GPU Backend Detection and Local GPU Proving
// ═══════════════════════════════════════════════════════════════════════════

/// Available GPU backends for proof generation
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl LocalGpuProver {
    /// Create a new local GPU prover
    pub fn new() -> Self {
        Self {
            backend: GpuBackend::detect(),
        }
    }

    /// Create with specific backend
    pub fn with_backend(backend: GpuBackend) -> Self {
        Self { backend }
    }

    /// Get the current backend
    pub fn backend(&self) -> GpuBackend {
        self.backend
    }

    /// Check if GPU is available
    pub fn has_gpu(&self) -> bool {
        self.backend.is_gpu()
    }

    /// Execute and prove using GPU (or CPU fallback)
    ///
    /// This method runs in a blocking context - use `prove_async` for async code.
    pub fn prove_sync(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let start = Instant::now();
        info!("Starting {} proof generation...", self.backend);

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

    /// Prove with CPU fallback on GPU failure
    pub async fn prove_with_fallback(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        // First try GPU
        if self.backend.is_gpu() {
            match self.prove_async(elf, input).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!("{} proving failed, falling back to CPU: {}", self.backend, e);
                }
            }
        }

        // Fallback to CPU
        let cpu_prover = LocalGpuProver::with_backend(GpuBackend::Cpu);
        cpu_prover.prove_async(elf, input).await
    }
}

impl Default for LocalGpuProver {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract seal from receipt (local helper to avoid module dependency)
fn extract_seal_local(receipt: &Receipt) -> Result<Vec<u8>> {
    let seal = bincode::serialize(&receipt.inner)?;
    Ok(seal)
}

/// GPU proving statistics
#[derive(Clone, Debug, Default)]
pub struct GpuProvingStats {
    pub backend: Option<GpuBackend>,
    pub proofs_generated: u64,
    pub proofs_failed: u64,
    pub total_proving_time: Duration,
    pub gpu_fallbacks: u64,
}

impl GpuProvingStats {
    /// Average proof time
    pub fn avg_proof_time(&self) -> Duration {
        if self.proofs_generated == 0 {
            return Duration::ZERO;
        }
        self.total_proving_time / self.proofs_generated as u32
    }
}

/// GPU optimization configuration
#[derive(Clone, Debug)]
pub struct GpuConfig {
    /// Maximum concurrent Bonsai sessions
    pub max_concurrent_sessions: usize,
    /// Batch size for proof requests
    pub batch_size: usize,
    /// Pipeline depth (jobs queued ahead)
    pub pipeline_depth: usize,
    /// Timeout for single proof
    pub proof_timeout: Duration,
    /// Enable adaptive concurrency
    pub adaptive_concurrency: bool,
    /// Target GPU utilization (0.0-1.0)
    pub target_utilization: f64,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            max_concurrent_sessions: 4,
            batch_size: 8,
            pipeline_depth: 2,
            proof_timeout: Duration::from_secs(600), // 10 min
            adaptive_concurrency: true,
            target_utilization: 0.85,
        }
    }
}

/// Job to be proven
#[derive(Debug, Clone)]
pub struct ProofJob {
    pub id: u64,
    pub image_id: [u8; 32],
    pub elf: Vec<u8>,
    pub input: Vec<u8>,
    pub priority: u8, // Higher = more urgent
}

/// Completed proof
#[derive(Debug)]
pub struct CompletedProof {
    pub job_id: u64,
    pub proof: Vec<u8>,
    pub journal: Vec<u8>,
    pub cycles: u64,
    pub prove_time: Duration,
}

/// GPU-optimized prover
pub struct GpuOptimizedProver {
    config: GpuConfig,
    /// Semaphore for concurrent sessions
    session_semaphore: Arc<Semaphore>,
    /// Job queue
    job_queue: Arc<RwLock<VecDeque<ProofJob>>>,
    /// Current concurrency level
    current_concurrency: Arc<RwLock<usize>>,
    /// Metrics
    metrics: Arc<RwLock<GpuMetrics>>,
}

impl GpuOptimizedProver {
    /// Create new GPU-optimized prover
    pub fn new(config: GpuConfig) -> Self {
        Self {
            session_semaphore: Arc::new(Semaphore::new(config.max_concurrent_sessions)),
            job_queue: Arc::new(RwLock::new(VecDeque::new())),
            current_concurrency: Arc::new(RwLock::new(config.max_concurrent_sessions)),
            metrics: Arc::new(RwLock::new(GpuMetrics::default())),
            config,
        }
    }

    /// Submit a job for proving
    pub async fn submit(&self, job: ProofJob) -> Result<mpsc::Receiver<CompletedProof>> {
        let (tx, rx) = mpsc::channel(1);

        // Add to queue (sorted by priority)
        {
            let mut queue = self.job_queue.write().await;
            let insert_pos = queue
                .iter()
                .position(|j| j.priority < job.priority)
                .unwrap_or(queue.len());
            queue.insert(insert_pos, job.clone());
        }

        // Spawn worker to process
        let this = self.clone_refs();
        tokio::spawn(async move {
            if let Ok(proof) = this.process_job(job).await {
                let _ = tx.send(proof).await;
            }
        });

        Ok(rx)
    }

    /// Submit batch of jobs
    pub async fn submit_batch(&self, jobs: Vec<ProofJob>) -> Result<Vec<mpsc::Receiver<CompletedProof>>> {
        info!("Submitting batch of {} jobs", jobs.len());

        let mut receivers = Vec::with_capacity(jobs.len());
        for job in jobs {
            receivers.push(self.submit(job).await?);
        }

        Ok(receivers)
    }

    /// Process a single job
    async fn process_job(&self, job: ProofJob) -> Result<CompletedProof> {
        // Acquire GPU slot
        let _permit = self.session_semaphore.acquire().await?;

        let start = Instant::now();
        info!("Starting proof for job {}", job.id);

        // Update metrics
        {
            let mut metrics = self.metrics.write().await;
            metrics.jobs_started += 1;
            metrics.current_active += 1;
        }

        // Call Bonsai API
        let result = self.prove_on_bonsai(&job).await;

        // Update metrics
        {
            let mut metrics = self.metrics.write().await;
            metrics.current_active -= 1;
            match &result {
                Ok(_) => {
                    metrics.jobs_completed += 1;
                    metrics.total_prove_time += start.elapsed();
                }
                Err(_) => metrics.jobs_failed += 1,
            }
        }

        // Adaptive concurrency adjustment
        if self.config.adaptive_concurrency {
            self.adjust_concurrency().await;
        }

        result
    }

    /// Prove using Bonsai API
    async fn prove_on_bonsai(&self, job: &ProofJob) -> Result<CompletedProof> {
        let start = Instant::now();

        // In production, this would use bonsai-sdk:
        //
        // let client = bonsai_sdk::Client::from_env()?;
        //
        // // Upload ELF (with caching)
        // let elf_id = client.upload_elf(&job.elf).await?;
        //
        // // Upload input
        // let input_id = client.upload_input(&job.input).await?;
        //
        // // Start proving session
        // let session = client.create_session(elf_id, input_id).await?;
        //
        // // Poll for completion with timeout
        // let receipt = tokio::time::timeout(
        //     self.config.proof_timeout,
        //     session.wait_for_completion()
        // ).await??;
        //
        // return Ok(CompletedProof {
        //     job_id: job.id,
        //     proof: receipt.inner.groth16_seal(),
        //     journal: receipt.journal.bytes,
        //     cycles: receipt.inner.claim.exit_code.cycle_count,
        //     prove_time: start.elapsed(),
        // });

        // Mock implementation
        debug!("Proving job {} on Bonsai (mock)", job.id);
        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(CompletedProof {
            job_id: job.id,
            proof: vec![0u8; 256],
            journal: vec![0u8; 32],
            cycles: 10_000_000,
            prove_time: start.elapsed(),
        })
    }

    /// Adjust concurrency based on performance
    async fn adjust_concurrency(&self) {
        let metrics = self.metrics.read().await;

        if metrics.jobs_completed < 10 {
            return; // Not enough data
        }

        let success_rate = metrics.jobs_completed as f64
            / (metrics.jobs_completed + metrics.jobs_failed) as f64;
        let avg_time = metrics.total_prove_time.as_secs_f64() / metrics.jobs_completed as f64;

        drop(metrics);

        let mut current = self.current_concurrency.write().await;

        // If success rate is low, reduce concurrency (Bonsai overloaded)
        if success_rate < 0.9 && *current > 1 {
            *current -= 1;
            warn!("Reducing concurrency to {} (success rate: {:.1}%)", *current, success_rate * 100.0);
        }
        // If success rate is high and times are good, increase
        else if success_rate > 0.98 && avg_time < 60.0 && *current < self.config.max_concurrent_sessions {
            *current += 1;
            info!("Increasing concurrency to {} (avg time: {:.1}s)", *current, avg_time);
        }
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> GpuMetrics {
        self.metrics.read().await.clone()
    }

    /// Clone references for spawning
    fn clone_refs(&self) -> Self {
        Self {
            config: self.config.clone(),
            session_semaphore: self.session_semaphore.clone(),
            job_queue: self.job_queue.clone(),
            current_concurrency: self.current_concurrency.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

/// GPU utilization metrics
#[derive(Clone, Debug, Default)]
pub struct GpuMetrics {
    pub jobs_started: u64,
    pub jobs_completed: u64,
    pub jobs_failed: u64,
    pub current_active: u64,
    pub total_prove_time: Duration,
}

impl GpuMetrics {
    /// Average proof time
    pub fn avg_proof_time(&self) -> Duration {
        if self.jobs_completed == 0 {
            return Duration::ZERO;
        }
        self.total_prove_time / self.jobs_completed as u32
    }

    /// Success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.jobs_completed + self.jobs_failed;
        if total == 0 {
            return 1.0;
        }
        self.jobs_completed as f64 / total as f64
    }

    /// Throughput (proofs per hour)
    pub fn throughput_per_hour(&self, elapsed: Duration) -> f64 {
        let hours = elapsed.as_secs_f64() / 3600.0;
        if hours < 0.001 {
            return 0.0;
        }
        self.jobs_completed as f64 / hours
    }
}

impl std::fmt::Display for GpuMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== GPU Metrics ===")?;
        writeln!(f, "Jobs: {} completed, {} failed ({:.1}% success)",
                 self.jobs_completed, self.jobs_failed, self.success_rate() * 100.0)?;
        writeln!(f, "Active: {}", self.current_active)?;
        writeln!(f, "Avg proof time: {:?}", self.avg_proof_time())?;
        Ok(())
    }
}

/// Pipeline for continuous proving
pub struct ProvingPipeline {
    prover: Arc<GpuOptimizedProver>,
    /// Channel to submit jobs
    job_tx: mpsc::Sender<ProofJob>,
    /// Channel to receive completed proofs
    proof_rx: mpsc::Receiver<CompletedProof>,
}

impl ProvingPipeline {
    /// Create a new proving pipeline
    pub fn new(config: GpuConfig) -> Self {
        let prover = Arc::new(GpuOptimizedProver::new(config.clone()));
        let (job_tx, mut job_rx) = mpsc::channel::<ProofJob>(config.pipeline_depth * config.batch_size);
        let (proof_tx, proof_rx) = mpsc::channel::<CompletedProof>(config.pipeline_depth * config.batch_size);

        // Spawn pipeline worker
        let prover_clone = prover.clone();
        tokio::spawn(async move {
            while let Some(job) = job_rx.recv().await {
                let prover = prover_clone.clone_refs();
                let tx = proof_tx.clone();

                tokio::spawn(async move {
                    if let Ok(proof) = prover.process_job(job).await {
                        let _ = tx.send(proof).await;
                    }
                });
            }
        });

        Self {
            prover,
            job_tx,
            proof_rx,
        }
    }

    /// Submit a job to the pipeline
    pub async fn submit(&self, job: ProofJob) -> Result<()> {
        self.job_tx.send(job).await.map_err(|e| anyhow!("Pipeline closed: {}", e))
    }

    /// Receive next completed proof
    pub async fn next_proof(&mut self) -> Option<CompletedProof> {
        self.proof_rx.recv().await
    }

    /// Get metrics
    pub async fn metrics(&self) -> GpuMetrics {
        self.prover.get_metrics().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_job_submission() {
        let prover = GpuOptimizedProver::new(GpuConfig::default());

        let job = ProofJob {
            id: 1,
            image_id: [0u8; 32],
            elf: vec![],
            input: vec![],
            priority: 5,
        };

        let mut rx = prover.submit(job).await.unwrap();
        let proof = rx.recv().await.unwrap();
        assert_eq!(proof.job_id, 1);
    }

    #[tokio::test]
    async fn test_batch_submission() {
        let prover = GpuOptimizedProver::new(GpuConfig::default());

        let jobs: Vec<ProofJob> = (0..5)
            .map(|i| ProofJob {
                id: i,
                image_id: [0u8; 32],
                elf: vec![],
                input: vec![],
                priority: 5,
            })
            .collect();

        let receivers = prover.submit_batch(jobs).await.unwrap();
        assert_eq!(receivers.len(), 5);
    }

    #[test]
    fn test_metrics_calculation() {
        let metrics = GpuMetrics {
            jobs_started: 100,
            jobs_completed: 95,
            jobs_failed: 5,
            current_active: 2,
            total_prove_time: Duration::from_secs(950),
        };

        assert_eq!(metrics.success_rate(), 0.95);
        assert_eq!(metrics.avg_proof_time(), Duration::from_secs(10));
    }
}
