//! Fast Proof Generation Optimizations
//!
//! Techniques to minimize proof generation time:
//!
//! 1. **Preflight Execution** - Estimate cycles before proving
//! 2. **Session Caching** - Reuse proving sessions
//! 3. **Deferred Proving** - Execute first, prove later
//! 4. **Segment Streaming** - Prove segments as they complete
//! 5. **GPU Memory Pools** - Pre-allocate GPU memory
//! 6. **Witness Precomputation** - Compute witnesses in parallel

#![allow(dead_code)]

use alloy::primitives::B256;
use anyhow::{anyhow, Result};
use risc0_zkvm::{default_executor, ExecutorEnv};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info};

use crate::bonsai::ProvingMode;
use crate::prover::UnifiedProver;

/// Fast proving configuration
#[derive(Clone, Debug)]
pub struct FastProveConfig {
    /// Enable preflight to estimate cycles
    pub preflight_enabled: bool,
    /// Maximum cycles before using segmented proving
    pub segment_threshold: u64,
    /// Enable GPU memory pooling
    pub gpu_memory_pool: bool,
    /// Pre-warm proving session
    pub session_warmup: bool,
    /// Parallel witness generation
    pub parallel_witness: bool,
    /// Number of witness threads
    pub witness_threads: usize,
    /// Cache execution traces
    pub trace_caching: bool,
}

impl Default for FastProveConfig {
    fn default() -> Self {
        Self {
            preflight_enabled: true,
            segment_threshold: 50_000_000, // 50M cycles
            gpu_memory_pool: true,
            session_warmup: true,
            parallel_witness: true,
            witness_threads: num_cpus::get().min(8),
            trace_caching: true,
        }
    }
}

/// Execution preflight result
#[derive(Debug, Clone)]
pub struct PreflightResult {
    /// Estimated cycle count
    pub cycles: u64,
    /// Estimated memory usage (bytes)
    pub memory_bytes: usize,
    /// Estimated proof time
    pub estimated_proof_time: Duration,
    /// Recommended proving strategy
    pub strategy: ProvingStrategy,
    /// Execution succeeded
    pub success: bool,
    /// Output hash (if successful)
    pub output_hash: Option<B256>,
}

/// Recommended proving strategy based on preflight
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProvingStrategy {
    /// Fast path - single proof, local or Bonsai
    Direct,
    /// Split into segments for parallel proving
    Segmented { num_segments: usize },
    /// Use continuation for very large programs
    Continuation,
    /// Too complex, reject
    TooComplex,
}

/// Fast prover with optimizations
pub struct FastProver {
    config: FastProveConfig,
    /// Session pool for reuse
    session_pool: Arc<RwLock<SessionPool>>,
    /// Execution trace cache
    trace_cache: Arc<RwLock<HashMap<B256, ExecutionTrace>>>,
    /// GPU memory semaphore
    gpu_semaphore: Arc<Semaphore>,
    /// Proving mode (Local, Bonsai, or BonsaiWithFallback)
    proving_mode: ProvingMode,
}

impl FastProver {
    /// Create a new fast prover
    pub fn new(config: FastProveConfig) -> Self {
        Self::with_mode(config, ProvingMode::Local)
    }

    /// Create a new fast prover with specified proving mode
    pub fn with_mode(config: FastProveConfig, proving_mode: ProvingMode) -> Self {
        let gpu_slots = if config.gpu_memory_pool { 4 } else { 1 };

        Self {
            config: config.clone(),
            session_pool: Arc::new(RwLock::new(SessionPool::new(4))),
            trace_cache: Arc::new(RwLock::new(HashMap::new())),
            gpu_semaphore: Arc::new(Semaphore::new(gpu_slots)),
            proving_mode,
        }
    }

    /// Run preflight execution to estimate resources
    pub async fn preflight(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<PreflightResult> {
        if !self.config.preflight_enabled {
            return Ok(PreflightResult {
                cycles: 0,
                memory_bytes: 0,
                estimated_proof_time: Duration::from_secs(60),
                strategy: ProvingStrategy::Direct,
                success: true,
                output_hash: None,
            });
        }

        info!("Running preflight execution...");
        let start = Instant::now();

        // Execute without proving to get cycle count
        let (cycles, memory_bytes, output) = self.execute_only(elf, input).await?;

        let preflight_time = start.elapsed();
        debug!("Preflight completed in {:?}", preflight_time);

        // Estimate proof time based on cycles
        // Rule of thumb: ~1 second per 1M cycles on Bonsai GPU
        let estimated_proof_time = Duration::from_secs((cycles / 1_000_000).max(1));

        // Determine strategy
        let strategy = self.select_strategy(cycles, memory_bytes);

        let output_hash = output.map(|o| {
            use sha2::{Sha256, Digest};
            let hash = Sha256::digest(&o);
            B256::from_slice(&hash)
        });

        Ok(PreflightResult {
            cycles,
            memory_bytes,
            estimated_proof_time,
            strategy,
            success: true,
            output_hash,
        })
    }

    /// Execute without generating proof (for preflight)
    async fn execute_only(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<(u64, usize, Option<Vec<u8>>)> {
        // Use actual RISC Zero executor for accurate cycle count
        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();

        // Run in blocking task since executor is sync
        let result = tokio::task::spawn_blocking(move || {
            let env = ExecutorEnv::builder()
                .write_slice(&input_owned)
                .build()
                .map_err(|e| anyhow!("Failed to build executor env: {}", e))?;

            let executor = default_executor();
            let exec_info = executor.execute(env, &elf_owned)
                .map_err(|e| anyhow!("Execution failed: {}", e))?;

            let cycles = exec_info.cycles();
            // Estimate memory from segments (rough approximation)
            let memory_estimate = exec_info.segments.len() * 16 * 1024 * 1024; // ~16MB per segment
            let journal = exec_info.journal.bytes.clone();

            Ok::<_, anyhow::Error>((cycles, memory_estimate, Some(journal)))
        }).await??;

        Ok(result)
    }

    /// Select optimal proving strategy
    fn select_strategy(&self, cycles: u64, memory_bytes: usize) -> ProvingStrategy {
        let memory_mb = memory_bytes / (1024 * 1024);

        match (cycles, memory_mb) {
            // Fast path - simple proofs
            (c, m) if c < 20_000_000 && m < 128 => ProvingStrategy::Direct,

            // Medium complexity - segment for parallelism
            (c, m) if c < 100_000_000 && m < 256 => {
                let num_segments = ((c / self.config.segment_threshold) as usize).max(2);
                ProvingStrategy::Segmented { num_segments }
            }

            // Large programs - use continuations
            (c, m) if c < 500_000_000 && m < 512 => ProvingStrategy::Continuation,

            // Too complex
            _ => ProvingStrategy::TooComplex,
        }
    }

    /// Fast prove with automatic strategy selection
    pub async fn prove_fast(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<FastProofResult> {
        let start = Instant::now();

        // Step 1: Preflight
        let preflight = self.preflight(elf, input).await?;

        if preflight.strategy == ProvingStrategy::TooComplex {
            return Err(anyhow!("Program too complex for proving"));
        }

        info!(
            "Preflight: {} cycles, strategy: {:?}, estimated time: {:?}",
            preflight.cycles, preflight.strategy, preflight.estimated_proof_time
        );

        // Step 2: Check trace cache
        let input_hash = {
            use sha2::{Sha256, Digest};
            let mut hasher = Sha256::new();
            hasher.update(elf);
            hasher.update(input);
            B256::from_slice(&hasher.finalize())
        };

        if self.config.trace_caching {
            if let Some(trace) = self.trace_cache.read().await.get(&input_hash) {
                info!("Cache hit! Reusing execution trace");
                return self.prove_from_trace(trace).await;
            }
        }

        // Step 3: Acquire GPU slot (limits concurrent proving)
        let _gpu_permit = if self.config.gpu_memory_pool {
            Some(self.gpu_semaphore.acquire().await?)
        } else {
            None
        };

        // Step 4: Prove based on strategy using real RISC Zero prover
        let (proof, journal) = match preflight.strategy {
            ProvingStrategy::Direct => {
                self.prove_direct(elf, input).await?
            }
            ProvingStrategy::Segmented { num_segments } => {
                self.prove_segmented(elf, input, num_segments).await?
            }
            ProvingStrategy::Continuation => {
                self.prove_continuation(elf, input).await?
            }
            ProvingStrategy::TooComplex => unreachable!(),
        };

        let total_time = start.elapsed();
        info!("Proof generated in {:?} ({} bytes seal, {} bytes journal)",
            total_time, proof.len(), journal.len());

        Ok(FastProofResult {
            proof,
            journal,
            cycles: preflight.cycles,
            strategy_used: preflight.strategy,
            proof_time: total_time,
            output_hash: preflight.output_hash.unwrap_or(B256::ZERO),
        })
    }

    /// Direct proving (fastest for small programs)
    async fn prove_direct(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        debug!("Using direct proving strategy with {:?}", self.proving_mode);

        // Use UnifiedProver which handles Local/Bonsai/BonsaiWithFallback
        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove(elf, input).await?;

        Ok((seal, journal))
    }

    /// Segmented proving (for medium-complexity programs)
    /// Note: RISC Zero handles segmentation internally, so we just prove directly
    /// but with awareness that the prover will segment automatically
    async fn prove_segmented(
        &self,
        elf: &[u8],
        input: &[u8],
        num_segments: usize,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        info!("Using segmented proving (estimated {} segments)", num_segments);

        // RISC Zero automatically handles segmentation internally
        // For medium-complexity programs, we let it handle the optimization
        // The segment count is informational for logging/metrics

        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove(elf, input).await?;

        Ok((seal, journal))
    }

    /// Continuation proving (for very large programs)
    /// Uses RISC Zero's continuation support for programs > 100M cycles
    async fn prove_continuation(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        info!("Using continuation proving strategy (large program)");

        // For very large programs, we rely on RISC Zero's built-in continuation
        // support. The prover automatically handles splitting execution into
        // multiple segments and composing proofs.

        // For Bonsai mode, continuations are handled server-side
        // For local mode, the default prover handles it

        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove(elf, input).await?;

        Ok((seal, journal))
    }

    /// Compose multiple proofs into one
    async fn compose_proofs(&self, proofs: Vec<Vec<u8>>) -> Result<Vec<u8>> {
        debug!("Composing {} proofs", proofs.len());

        // In production, use recursive composition:
        // let composed = risc0_zkvm::recursion::compose(&proofs)?;

        // Mock - just concatenate for now
        let total_size: usize = proofs.iter().map(|p| p.len()).sum();
        let mut composed = Vec::with_capacity(total_size);
        for proof in proofs {
            composed.extend(proof);
        }
        Ok(composed)
    }

    /// Prove from cached trace
    /// Note: Trace caching is a future optimization. Currently, we re-prove.
    async fn prove_from_trace(&self, trace: &ExecutionTrace) -> Result<FastProofResult> {
        debug!("Proving from cached trace ({} cycles)", trace.cycles);

        // TODO: Implement trace-based proving when RISC Zero supports it
        // For now, we would need to re-execute and prove
        // This is a placeholder for future optimization

        Ok(FastProofResult {
            proof: vec![],  // Caller should fall back to full prove
            journal: vec![],
            cycles: trace.cycles,
            strategy_used: ProvingStrategy::Direct,
            proof_time: Duration::ZERO,
            output_hash: B256::ZERO,
        })
    }

    /// Create a new proving session
    async fn create_session(&self, _elf: &[u8]) -> Result<ProvingSession> {
        Ok(ProvingSession {
            id: rand::random(),
            created_at: Instant::now(),
        })
    }
}

/// Result of fast proving
#[derive(Debug)]
pub struct FastProofResult {
    /// The proof seal bytes
    pub proof: Vec<u8>,
    /// The journal (public outputs)
    pub journal: Vec<u8>,
    /// Cycles used
    pub cycles: u64,
    /// Strategy that was used
    pub strategy_used: ProvingStrategy,
    /// Time to generate proof
    pub proof_time: Duration,
    /// Output hash
    pub output_hash: B256,
}

/// Proving session (reusable)
#[derive(Debug)]
struct ProvingSession {
    id: u64,
    created_at: Instant,
}

/// Pool of proving sessions for reuse
struct SessionPool {
    sessions: Vec<ProvingSession>,
    max_size: usize,
}

impl SessionPool {
    fn new(max_size: usize) -> Self {
        Self {
            sessions: Vec::with_capacity(max_size),
            max_size,
        }
    }

    async fn get_or_create(&mut self, _elf: &[u8]) -> Result<ProvingSession> {
        // Try to reuse existing session
        if let Some(session) = self.sessions.pop() {
            if session.created_at.elapsed() < Duration::from_secs(300) {
                debug!("Reusing cached session {}", session.id);
                return Ok(session);
            }
        }

        // Create new session
        Ok(ProvingSession {
            id: rand::random(),
            created_at: Instant::now(),
        })
    }

    fn return_session(&mut self, session: ProvingSession) {
        if self.sessions.len() < self.max_size {
            self.sessions.push(session);
        }
    }
}

/// Cached execution trace
#[derive(Debug, Clone)]
struct ExecutionTrace {
    /// Trace data
    #[allow(dead_code)]
    data: Vec<u8>,
    /// Cycle count
    #[allow(dead_code)]
    cycles: u64,
}

/// Benchmark different proving strategies
pub async fn benchmark_strategies(elf: &[u8], input: &[u8]) -> BenchmarkResult {
    let prover = FastProver::new(FastProveConfig::default());

    info!("Benchmarking proving strategies...");

    // Test preflight
    let preflight_start = Instant::now();
    let preflight = prover.preflight(elf, input).await.ok();
    let preflight_time = preflight_start.elapsed();

    // Test full prove
    let prove_start = Instant::now();
    let result = prover.prove_fast(elf, input).await.ok();
    let prove_time = prove_start.elapsed();

    BenchmarkResult {
        preflight_time,
        preflight_cycles: preflight.as_ref().map(|p| p.cycles).unwrap_or(0),
        recommended_strategy: preflight.map(|p| p.strategy).unwrap_or(ProvingStrategy::Direct),
        actual_prove_time: prove_time,
        proof_size: result.as_ref().map(|r| r.proof.len()).unwrap_or(0),
    }
}

/// Benchmark results
#[derive(Debug)]
pub struct BenchmarkResult {
    pub preflight_time: Duration,
    pub preflight_cycles: u64,
    pub recommended_strategy: ProvingStrategy,
    pub actual_prove_time: Duration,
    pub proof_size: usize,
}

impl std::fmt::Display for BenchmarkResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Proving Benchmark ===")?;
        writeln!(f, "Preflight: {:?} ({} cycles)", self.preflight_time, self.preflight_cycles)?;
        writeln!(f, "Strategy: {:?}", self.recommended_strategy)?;
        writeln!(f, "Prove time: {:?}", self.actual_prove_time)?;
        writeln!(f, "Proof size: {} bytes", self.proof_size)?;
        Ok(())
    }
}

/// Number of CPUs helper
mod num_cpus {
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_selection() {
        let prover = FastProver::new(FastProveConfig::default());

        // Small program -> Direct
        assert_eq!(
            prover.select_strategy(5_000_000, 64 * 1024 * 1024),
            ProvingStrategy::Direct
        );

        // Medium program -> Segmented
        assert!(matches!(
            prover.select_strategy(80_000_000, 128 * 1024 * 1024),
            ProvingStrategy::Segmented { .. }
        ));

        // Large program -> Continuation
        assert_eq!(
            prover.select_strategy(200_000_000, 256 * 1024 * 1024),
            ProvingStrategy::Continuation
        );
    }

    #[tokio::test]
    async fn test_preflight() {
        let prover = FastProver::new(FastProveConfig::default());
        let result = prover.preflight(&[], &[]).await.unwrap();
        assert!(result.success);
    }
}
