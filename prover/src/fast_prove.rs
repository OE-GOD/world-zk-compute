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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info};

use crate::bonsai::ProvingMode;
use crate::multi_vm::MultiVmProver;
use crate::prover::UnifiedProver;
use crate::segment_prover::{
    optimal_po2, recommended_thread_count, estimate_segments_for_po2,
    SegmentProver, SegmentProverConfig,
};
use crate::zkvm_backend::{detect_vm_type, ZkVmType};

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
    /// Maximum time for proof generation before timeout
    pub proof_timeout: Duration,
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
            proof_timeout: Duration::from_secs(30 * 60), // 30 minutes
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
    /// Enable SNARK (Groth16) proof generation
    use_snark: bool,
    /// Multi-VM router for risc0/SP1 dispatch
    multi_vm: Arc<MultiVmProver>,
}

impl FastProver {
    /// Create a new fast prover
    pub fn new(config: FastProveConfig) -> Self {
        Self::with_mode(config, ProvingMode::Local)
    }

    /// Create a new fast prover with specified proving mode
    pub fn with_mode(config: FastProveConfig, proving_mode: ProvingMode) -> Self {
        Self::with_mode_and_snark(config, proving_mode, false)
    }

    /// Create a new fast prover with proving mode and SNARK option
    pub fn with_mode_and_snark(config: FastProveConfig, proving_mode: ProvingMode, use_snark: bool) -> Self {
        let gpu_slots = if config.gpu_memory_pool { 4 } else { 1 };
        let multi_vm = Arc::new(MultiVmProver::new(proving_mode.clone()));

        Self {
            config: config.clone(),
            session_pool: Arc::new(RwLock::new(SessionPool::new(4))),
            trace_cache: Arc::new(RwLock::new(HashMap::new())),
            gpu_semaphore: Arc::new(Semaphore::new(gpu_slots)),
            proving_mode,
            use_snark,
            multi_vm,
        }
    }

    /// Create a new fast prover with an explicit multi-VM router.
    pub fn with_multi_vm(
        config: FastProveConfig,
        proving_mode: ProvingMode,
        use_snark: bool,
        multi_vm: Arc<MultiVmProver>,
    ) -> Self {
        let gpu_slots = if config.gpu_memory_pool { 4 } else { 1 };

        Self {
            config: config.clone(),
            session_pool: Arc::new(RwLock::new(SessionPool::new(4))),
            trace_cache: Arc::new(RwLock::new(HashMap::new())),
            gpu_semaphore: Arc::new(Semaphore::new(gpu_slots)),
            proving_mode,
            use_snark,
            multi_vm,
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

        // Determine strategy (VM-type aware)
        let vm_type = detect_vm_type(elf);
        let strategy = self.select_strategy_for_vm(cycles, memory_bytes, vm_type);

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

    /// Execute without generating proof (for preflight).
    ///
    /// Uses the multi-VM router to automatically dispatch to the correct
    /// backend (risc0 or SP1) based on the ELF binary.
    async fn execute_only(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<(u64, usize, Option<Vec<u8>>)> {
        let result = self.multi_vm.execute(elf, input).await?;
        Ok((
            result.cycles,
            result.memory_estimate_bytes,
            Some(result.journal),
        ))
    }

    /// Select optimal proving strategy.
    ///
    /// SP1 programs always use Direct — SP1 manages segments internally.
    /// For risc0 programs, selects based on cycle count and memory.
    fn select_strategy(&self, cycles: u64, memory_bytes: usize) -> ProvingStrategy {
        self.select_strategy_for_vm(cycles, memory_bytes, ZkVmType::Risc0)
    }

    /// Select strategy with explicit VM type awareness.
    fn select_strategy_for_vm(
        &self,
        cycles: u64,
        memory_bytes: usize,
        vm_type: ZkVmType,
    ) -> ProvingStrategy {
        // SP1 manages its own segmentation — always use Direct
        if vm_type == ZkVmType::Sp1 {
            return ProvingStrategy::Direct;
        }

        let memory_mb = memory_bytes / (1024 * 1024);

        match (cycles, memory_mb) {
            // Fast path - simple proofs
            (c, m) if c < 20_000_000 && m < 128 => ProvingStrategy::Direct,

            // Medium complexity - segment for parallelism
            (c, m) if c < 100_000_000 && m < 256 => {
                let num_segments = ((c / self.config.segment_threshold) as usize).max(2);
                ProvingStrategy::Segmented { num_segments }
            }

            // Large programs - use continuations (raised to 1B cycles)
            (c, m) if c < 1_000_000_000 && m < 512 => ProvingStrategy::Continuation,

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

        // Step 4: Prove based on strategy using real RISC Zero prover (with timeout)
        let timeout_duration = self.config.proof_timeout;
        let (proof, journal) = tokio::time::timeout(timeout_duration, async {
            match preflight.strategy {
                ProvingStrategy::Direct => {
                    self.prove_direct(elf, input).await
                }
                ProvingStrategy::Segmented { num_segments } => {
                    self.prove_segmented(elf, input, num_segments).await
                }
                ProvingStrategy::Continuation => {
                    self.prove_continuation(elf, input).await
                }
                ProvingStrategy::TooComplex => unreachable!(),
            }
        })
        .await
        .map_err(|_| anyhow!("Proof generation timed out after {:?}", timeout_duration))??;

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

    /// Direct proving (fastest for small programs).
    ///
    /// Uses the multi-VM router which auto-detects risc0 vs SP1 and
    /// dispatches to the correct backend.
    async fn prove_direct(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        debug!("Using direct proving strategy with {:?}", self.proving_mode);

        // For SP1 programs or when multi-VM handles it
        let vm_type = detect_vm_type(elf);
        if vm_type == ZkVmType::Sp1 {
            let result = if self.use_snark {
                self.multi_vm.prove_with_snark(elf, input).await?
            } else {
                self.multi_vm.prove(elf, input).await?
            };
            return Ok((result.seal, result.journal));
        }

        // For risc0, use UnifiedProver (preserves existing behavior)
        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove_with_snark(elf, input, self.use_snark).await?;

        Ok((seal, journal))
    }

    /// Segmented proving for medium-complexity programs (20M–100M cycles).
    ///
    /// Tunes `segment_limit_po2` to create more parallel segments and
    /// configures thread count to match. For SP1 programs, falls back to
    /// direct proving since SP1 manages segments internally.
    async fn prove_segmented(
        &self,
        elf: &[u8],
        input: &[u8],
        num_segments: usize,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        // SP1 manages its own segments — fall back to direct
        if detect_vm_type(elf) == ZkVmType::Sp1 {
            return self.prove_direct(elf, input).await;
        }

        // Calculate optimal po2 for this program's cycle count.
        // We use the preflight segment count to estimate total cycles,
        // then choose a po2 that gives roughly num_segments segments.
        let estimated_cycles = num_segments as u64 * self.config.segment_threshold;
        let po2 = optimal_po2(estimated_cycles);
        let est_segments = estimate_segments_for_po2(estimated_cycles, po2);
        let threads = recommended_thread_count(est_segments);

        info!(
            "Segmented proving: est_cycles={}, po2={}, est_segments={}, threads={}",
            estimated_cycles, po2, est_segments, threads
        );

        let config = SegmentProverConfig {
            segment_limit_po2: Some(po2),
            proving_threads: threads,
            cache_executions: false, // preflight already ran
            ..Default::default()
        };

        let segment_prover = SegmentProver::new(
            config,
            self.proving_mode.clone(),
            self.use_snark,
        );
        let result = segment_prover.prove_optimized(elf, input).await?;

        info!(
            "Segmented proof complete: {} segments, po2={}, {:?}",
            result.segment_count, result.po2_used, result.prove_time
        );

        Ok((result.seal, result.journal))
    }

    /// Continuation proving for very large programs (100M–1B cycles).
    ///
    /// Uses smaller segment size (po2=18–19) for maximum parallelism.
    /// For Bonsai-capable configurations, offloads to cloud proving since
    /// large programs benefit most from GPU acceleration.
    /// SP1 programs fall back to direct proving.
    async fn prove_continuation(&self, elf: &[u8], input: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        // SP1 manages its own segments — fall back to direct
        if detect_vm_type(elf) == ZkVmType::Sp1 {
            return self.prove_direct(elf, input).await;
        }

        info!("Using continuation proving strategy (large program)");

        // For Bonsai modes, offload to cloud — large programs benefit most
        // from Bonsai's GPU cluster.
        if matches!(
            self.proving_mode,
            ProvingMode::Bonsai
                | ProvingMode::BonsaiWithFallback
                | ProvingMode::BonsaiWithGpuFallback
        ) {
            info!("Offloading large program to Bonsai cloud proving");
            let prover = UnifiedProver::new(self.proving_mode.clone())?;
            return prover.prove_with_snark(elf, input, self.use_snark).await;
        }

        // Local proving: use aggressive segment tuning for max parallelism
        let config = SegmentProverConfig {
            segment_limit_po2: Some(18), // ~256K cycles/segment → many parallel segments
            proving_threads: 0,           // auto-detect
            cache_executions: false,
            max_cycles: 1_000_000_000,    // 1B cycles (raised from 500M)
            max_segments: 4000,           // allow more segments for large programs
            bonsai_threshold_cycles: u64::MAX, // don't redirect to bonsai (already checked)
        };

        let segment_prover = SegmentProver::new(
            config,
            self.proving_mode.clone(),
            self.use_snark,
        );
        let result = segment_prover.prove_optimized(elf, input).await?;

        info!(
            "Continuation proof complete: {} segments, po2={}, {:?}",
            result.segment_count, result.po2_used, result.prove_time
        );

        Ok((result.seal, result.journal))
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

        // Large program -> Continuation (threshold raised to 1B)
        assert_eq!(
            prover.select_strategy(200_000_000, 256 * 1024 * 1024),
            ProvingStrategy::Continuation
        );

        // Very large but under 1B -> still Continuation
        assert_eq!(
            prover.select_strategy(800_000_000, 256 * 1024 * 1024),
            ProvingStrategy::Continuation
        );

        // SP1 programs always get Direct
        assert_eq!(
            prover.select_strategy_for_vm(200_000_000, 256 * 1024 * 1024, ZkVmType::Sp1),
            ProvingStrategy::Direct
        );
    }

    #[tokio::test]
    async fn test_preflight() {
        let prover = FastProver::new(FastProveConfig::default());
        // Empty ELF will fail to parse - this is expected
        let result = prover.preflight(&[], &[]).await;
        // Preflight with empty ELF should fail with parse error
        assert!(result.is_err() || !result.unwrap().success);
    }
}
