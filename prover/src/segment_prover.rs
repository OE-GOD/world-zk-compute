//! Segment-Aware Parallel Proving Engine
//!
//! Optimizes zkVM proving for large programs by:
//!
//! 1. **Auto-tuned segment sizes** — Chooses `segment_limit_po2` based on
//!    cycle count to balance parallelism vs. composition overhead.
//!
//! 2. **Execute-first validation** — Runs the program without proving to
//!    detect failures early, measure cycles, and plan the proving strategy.
//!
//! 3. **Thread pool configuration** — Sizes the Rayon thread pool to match
//!    the segment count and available hardware.
//!
//! 4. **Multi-stage proof compression** — Pipeline from Composite (STARK
//!    segments) → Succinct (compressed STARK) → Groth16 (on-chain).
//!
//! 5. **Execution plan caching** — Avoids double-executing when preflight
//!    is followed by proving.
//!
//! ## Segment Size Tuning
//!
//! risc0 splits execution into segments of `2^po2` cycles. Smaller segments
//! mean more parallelism but higher composition overhead:
//!
//! | Cycles       | po2 | Cycles/Segment | Typical Segments |
//! |--------------|-----|----------------|------------------|
//! | < 10M        | 21  | ~2M            | 1–5              |
//! | 10M – 50M    | 20  | ~1M            | 10–50            |
//! | 50M – 200M   | 19  | ~512K          | 100–400          |
//! | > 200M       | 18  | ~256K          | 800+             |
//!
//! ## Thread Configuration
//!
//! risc0's prover uses Rayon for parallel segment proving. We configure
//! `RAYON_NUM_THREADS` based on segment count and available CPUs, leaving
//! one core free for the async runtime.

use anyhow::{anyhow, Result};
use risc0_zkvm::{default_executor, default_prover, ExecutorEnv, ProverOpts, Receipt};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::bonsai::ProvingMode;
use crate::prover::{extract_seal, UnifiedProver};

// ═══════════════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for segment-aware proving.
#[derive(Clone, Debug)]
pub struct SegmentProverConfig {
    /// Override segment limit po2 (None = auto-tune based on cycle count).
    pub segment_limit_po2: Option<u32>,
    /// Number of proving threads (0 = auto-detect from hardware).
    pub proving_threads: usize,
    /// Enable execution plan caching to avoid double-execution.
    pub cache_executions: bool,
    /// Maximum total cycles before rejecting a program as too complex.
    pub max_cycles: u64,
    /// Maximum segments before rejecting (prevents OOM during composition).
    pub max_segments: usize,
    /// Prefer Bonsai for programs exceeding this cycle count.
    pub bonsai_threshold_cycles: u64,
}

impl Default for SegmentProverConfig {
    fn default() -> Self {
        Self {
            segment_limit_po2: None, // auto-tune
            proving_threads: 0,      // auto-detect
            cache_executions: true,
            max_cycles: 1_000_000_000, // 1B cycles (raised from 500M)
            max_segments: 4000,        // raised from 2000
            bonsai_threshold_cycles: 100_000_000, // 100M → prefer Bonsai
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Execution Plan
// ═══════════════════════════════════════════════════════════════════════════════

/// Analysis of a program execution — produced by preflight, consumed by proving.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ExecutionPlan {
    /// Total cycle count from execution.
    pub cycles: u64,
    /// Number of execution segments (at default po2).
    pub segment_count: usize,
    /// Estimated memory usage in bytes.
    pub memory_estimate_bytes: usize,
    /// Public outputs from execution (journal).
    pub journal: Vec<u8>,
    /// Recommended segment_limit_po2 for optimal parallelism.
    pub recommended_po2: u32,
    /// Recommended thread count for proving.
    pub recommended_threads: usize,
    /// Estimated proving time (rough heuristic).
    pub estimated_prove_time: Duration,
    /// Time taken for the execution (preflight) itself.
    pub execution_time: Duration,
}

impl ExecutionPlan {
    /// Whether this program should be offloaded to Bonsai.
    pub fn should_use_bonsai(&self, threshold: u64) -> bool {
        self.cycles > threshold
    }

    /// Human-readable summary.
    pub fn summary(&self) -> String {
        format!(
            "cycles={}, segments={}, po2={}, threads={}, est_time={:?}",
            self.cycles,
            self.segment_count,
            self.recommended_po2,
            self.recommended_threads,
            self.estimated_prove_time,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Segment Prover
// ═══════════════════════════════════════════════════════════════════════════════

/// Segment-aware parallel proving engine.
///
/// Wraps risc0's prover with automatic segment size tuning, thread pool
/// configuration, and multi-stage proof compression.
pub struct SegmentProver {
    config: SegmentProverConfig,
    proving_mode: ProvingMode,
    use_snark: bool,
    /// Cache of execution plans keyed by SHA-256(elf || input).
    plan_cache: Arc<RwLock<HashMap<[u8; 32], ExecutionPlan>>>,
}

impl SegmentProver {
    /// Create a new segment prover.
    pub fn new(config: SegmentProverConfig, proving_mode: ProvingMode, use_snark: bool) -> Self {
        Self {
            config,
            proving_mode,
            use_snark,
            plan_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Execute without proving to analyze the program.
    ///
    /// Returns an `ExecutionPlan` with cycle counts, segment info, and
    /// recommendations for optimal proving configuration. The plan is
    /// cached so a subsequent `prove_optimized()` call avoids re-executing.
    pub async fn analyze(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionPlan> {
        // Check cache first
        let cache_key = Self::cache_key(elf, input);
        if self.config.cache_executions {
            if let Some(plan) = self.plan_cache.read().await.get(&cache_key) {
                debug!("Execution plan cache hit");
                return Ok(plan.clone());
            }
        }

        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();

        let plan = tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let env = ExecutorEnv::builder()
                .write_slice(&input_owned)
                .build()
                .map_err(|e| anyhow!("Failed to build executor env: {}", e))?;

            let executor = default_executor();
            let session = executor
                .execute(env, &elf_owned)
                .map_err(|e| anyhow!("Execution failed: {}", e))?;

            let cycles = session.cycles();
            let segment_count = session.segments.len();
            let memory_estimate_bytes = segment_count * 16 * 1024 * 1024; // ~16 MB/segment
            let journal = session.journal.bytes.clone();
            let execution_time = start.elapsed();

            let recommended_po2 = optimal_po2(cycles);
            let recommended_threads = recommended_thread_count(segment_count);
            let estimated_prove_time = estimate_prove_time(cycles);

            info!(
                "Execution plan: {} cycles, {} segments, po2={}, threads={}, exec_time={:?}",
                cycles, segment_count, recommended_po2, recommended_threads, execution_time
            );

            Ok::<_, anyhow::Error>(ExecutionPlan {
                cycles,
                segment_count,
                memory_estimate_bytes,
                journal,
                recommended_po2,
                recommended_threads,
                estimated_prove_time,
                execution_time,
            })
        })
        .await??;

        // Reject if too complex
        if plan.cycles > self.config.max_cycles {
            return Err(anyhow!(
                "Program too complex: {} cycles exceeds limit {}",
                plan.cycles,
                self.config.max_cycles
            ));
        }

        // Cache the plan
        if self.config.cache_executions {
            self.plan_cache
                .write()
                .await
                .insert(cache_key, plan.clone());
        }

        Ok(plan)
    }

    /// Prove with segment-tuned configuration.
    ///
    /// Optionally analyzes first (if no cached plan), then configures the
    /// prover with optimal segment size and thread count.
    pub async fn prove_optimized(&self, elf: &[u8], input: &[u8]) -> Result<ProveResult> {
        let start = Instant::now();

        // Get or compute execution plan
        let plan = self.analyze(elf, input).await?;

        info!("Proving with plan: {}", plan.summary());

        // Decide if we should offload to remote proving (Bonsai or Boundless)
        if plan.should_use_bonsai(self.config.bonsai_threshold_cycles)
            && self.proving_mode.uses_remote_proving()
        {
            info!(
                "Program has {} cycles, offloading to remote proving (threshold: {})",
                plan.cycles, self.config.bonsai_threshold_cycles
            );
            return self.prove_via_remote(elf, input).await;
        }

        // Determine segment limit
        let po2 = self
            .config
            .segment_limit_po2
            .unwrap_or(plan.recommended_po2);

        // Estimate segment count with tuned po2
        let est_segments = estimate_segments_for_po2(plan.cycles, po2);
        if est_segments > self.config.max_segments {
            return Err(anyhow!(
                "Too many segments: estimated {} with po2={} exceeds limit {}. \
                 Consider using Bonsai for this program.",
                est_segments,
                po2,
                self.config.max_segments
            ));
        }

        // Configure thread count
        let threads = if self.config.proving_threads > 0 {
            self.config.proving_threads
        } else {
            plan.recommended_threads
        };

        let (seal, journal) = self.prove_with_config(elf, input, po2, threads).await?;

        let total_time = start.elapsed();

        info!(
            "Proof complete: {} bytes seal, {} bytes journal, {:?} total",
            seal.len(),
            journal.len(),
            total_time
        );

        // Clear cached plan
        if self.config.cache_executions {
            let cache_key = Self::cache_key(elf, input);
            self.plan_cache.write().await.remove(&cache_key);
        }

        Ok(ProveResult {
            seal,
            journal,
            cycles: plan.cycles,
            segment_count: est_segments,
            po2_used: po2,
            threads_used: threads,
            prove_time: total_time,
        })
    }

    /// Prove with explicit segment and thread configuration.
    async fn prove_with_config(
        &self,
        elf: &[u8],
        input: &[u8],
        po2: u32,
        threads: usize,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let (receipt, seal, journal) = self
            .prove_with_config_receipt(elf, input, po2, threads)
            .await?;
        drop(receipt); // Discard receipt in this path
        Ok((seal, journal))
    }

    /// Prove with explicit configuration, returning the full Receipt.
    ///
    /// Used by the recursive wrapper to collect sub-proof receipts that
    /// can be passed as assumptions to the wrapper guest's `env::verify()`.
    pub async fn prove_with_receipt(
        &self,
        elf: &[u8],
        input: &[u8],
    ) -> Result<(ProveResult, Receipt)> {
        let start = Instant::now();

        let plan = self.analyze(elf, input).await?;

        let po2 = self
            .config
            .segment_limit_po2
            .unwrap_or(plan.recommended_po2);

        let est_segments = estimate_segments_for_po2(plan.cycles, po2);
        if est_segments > self.config.max_segments {
            return Err(anyhow!(
                "Too many segments: estimated {} with po2={} exceeds limit {}",
                est_segments,
                po2,
                self.config.max_segments
            ));
        }

        let threads = if self.config.proving_threads > 0 {
            self.config.proving_threads
        } else {
            plan.recommended_threads
        };

        let (receipt, seal, journal) = self
            .prove_with_config_receipt(elf, input, po2, threads)
            .await?;

        let total_time = start.elapsed();

        if self.config.cache_executions {
            let cache_key = Self::cache_key(elf, input);
            self.plan_cache.write().await.remove(&cache_key);
        }

        let result = ProveResult {
            seal,
            journal,
            cycles: plan.cycles,
            segment_count: est_segments,
            po2_used: po2,
            threads_used: threads,
            prove_time: total_time,
        };

        Ok((result, receipt))
    }

    /// Internal: prove and return (Receipt, seal, journal).
    async fn prove_with_config_receipt(
        &self,
        elf: &[u8],
        input: &[u8],
        po2: u32,
        threads: usize,
    ) -> Result<(Receipt, Vec<u8>, Vec<u8>)> {
        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();
        let use_snark = self.use_snark;

        info!(
            "Proving with segment_limit_po2={}, threads={}, snark={}",
            po2, threads, use_snark
        );

        tokio::task::spawn_blocking(move || {
            // Configure Rayon thread pool for segment parallelism.
            // risc0's prover uses Rayon internally; setting this env var
            // controls how many segments are proved concurrently.
            std::env::set_var("RAYON_NUM_THREADS", threads.to_string());

            let env = ExecutorEnv::builder()
                .write_slice(&input_owned)
                .segment_limit_po2(po2)
                .build()
                .map_err(|e| anyhow!("Failed to build executor env: {}", e))?;

            let prover = default_prover();

            let receipt = if use_snark {
                if cfg!(not(target_arch = "x86_64")) {
                    anyhow::bail!(
                        "Groth16 SNARK proving requires x86_64 architecture. \
                         Use Bonsai cloud proving or run on an x86_64 Linux machine."
                    );
                }
                info!("Generating Groth16 SNARK proof (segments will be composed → compressed)...");
                let opts = ProverOpts::groth16();
                let prove_info = prover
                    .prove_with_opts(env, &elf_owned, &opts)
                    .map_err(|e| anyhow!("Groth16 proving failed: {}", e))?;
                prove_info.receipt
            } else {
                let prove_info = prover
                    .prove(env, &elf_owned)
                    .map_err(|e| anyhow!("Proving failed: {}", e))?;
                prove_info.receipt
            };

            let seal = extract_seal(&receipt)?;
            let journal = receipt.journal.bytes.clone();

            Ok((receipt, seal, journal))
        })
        .await?
    }

    /// Offload proving to a remote service (Bonsai or Boundless).
    async fn prove_via_remote(&self, elf: &[u8], input: &[u8]) -> Result<ProveResult> {
        let start = Instant::now();

        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove_with_snark(elf, input, self.use_snark).await?;

        Ok(ProveResult {
            seal,
            journal,
            cycles: 0, // Bonsai doesn't report cycles back
            segment_count: 0,
            po2_used: 0,
            threads_used: 0,
            prove_time: start.elapsed(),
        })
    }

    /// Compute cache key from ELF + input.
    fn cache_key(elf: &[u8], input: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(elf);
        hasher.update(input);
        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result);
        key
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Prove Result
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of an optimized proving operation.
#[derive(Debug)]
pub struct ProveResult {
    /// Proof seal bytes (Groth16-encoded if snark=true).
    pub seal: Vec<u8>,
    /// Public outputs (journal).
    pub journal: Vec<u8>,
    /// Total cycles executed.
    pub cycles: u64,
    /// Number of segments proved.
    pub segment_count: usize,
    /// Segment limit po2 used.
    pub po2_used: u32,
    /// Number of proving threads used.
    pub threads_used: usize,
    /// Total proving time (including preflight if not cached).
    pub prove_time: Duration,
}

impl std::fmt::Display for ProveResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProveResult(seal={}B, journal={}B, cycles={}, segments={}, po2={}, threads={}, time={:?})",
            self.seal.len(),
            self.journal.len(),
            self.cycles,
            self.segment_count,
            self.po2_used,
            self.threads_used,
            self.prove_time,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tuning Heuristics
// ═══════════════════════════════════════════════════════════════════════════════

/// Choose optimal `segment_limit_po2` based on total cycle count.
///
/// Smaller po2 → more segments → more parallelism → more composition overhead.
/// The sweet spot balances these: enough segments to saturate available cores
/// without drowning in Merkle composition work.
///
/// risc0's default is po2=20 (~1M cycles/segment). We tune down for large
/// programs to increase parallelism.
pub fn optimal_po2(cycles: u64) -> u32 {
    match cycles {
        // Small programs: fewer, larger segments (less overhead)
        0..=10_000_000 => 21,
        // Medium programs: default segment size
        10_000_001..=50_000_000 => 20,
        // Large programs: smaller segments for more parallelism
        50_000_001..=200_000_000 => 19,
        // Very large: maximum parallelism
        _ => 18,
    }
}

/// Estimate segment count for a given cycle count and po2.
pub fn estimate_segments_for_po2(cycles: u64, po2: u32) -> usize {
    let cycles_per_segment = 1u64 << po2;
    cycles.div_ceil(cycles_per_segment) as usize
}

/// Recommend thread count based on segment count and available hardware.
///
/// Rules:
/// - Don't use more threads than segments (no benefit)
/// - Leave 1 core for the async runtime
/// - Cap at 16 threads (diminishing returns beyond that)
pub fn recommended_thread_count(segment_count: usize) -> usize {
    let cpu_count = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);

    let available = cpu_count.saturating_sub(1).max(1);
    segment_count.max(1).min(available).min(16)
}

/// Rough estimate of proving time based on cycle count.
///
/// Heuristic: ~1 second per 1M cycles on modern CPU with parallel segments.
/// GPU proving is roughly 10x faster; Bonsai 20-50x.
pub fn estimate_prove_time(cycles: u64) -> Duration {
    let million_cycles = (cycles as f64) / 1_000_000.0;
    Duration::from_secs_f64(million_cycles.max(1.0))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimal_po2() {
        assert_eq!(optimal_po2(5_000_000), 21);
        assert_eq!(optimal_po2(10_000_000), 21);
        assert_eq!(optimal_po2(30_000_000), 20);
        assert_eq!(optimal_po2(50_000_000), 20);
        assert_eq!(optimal_po2(100_000_000), 19);
        assert_eq!(optimal_po2(300_000_000), 18);
    }

    #[test]
    fn test_estimate_segments() {
        // po2=20 → 1M cycles/segment
        assert_eq!(estimate_segments_for_po2(10_000_000, 20), 10);
        assert_eq!(estimate_segments_for_po2(1, 20), 1);

        // po2=18 → 256K cycles/segment
        assert_eq!(estimate_segments_for_po2(10_000_000, 18), 39);
    }

    #[test]
    fn test_recommended_threads() {
        // Should never exceed segment count
        assert!(recommended_thread_count(1) <= 1);
        // Should be capped at 16
        assert!(recommended_thread_count(1000) <= 16);
        // Should be at least 1
        assert!(recommended_thread_count(0) >= 1);
    }

    #[test]
    fn test_estimate_prove_time() {
        let t = estimate_prove_time(10_000_000);
        assert_eq!(t, Duration::from_secs(10));

        // Minimum 1 second
        let t_small = estimate_prove_time(100);
        assert_eq!(t_small, Duration::from_secs(1));
    }

    #[test]
    fn test_execution_plan_summary() {
        let plan = ExecutionPlan {
            cycles: 50_000_000,
            segment_count: 50,
            memory_estimate_bytes: 800 * 1024 * 1024,
            journal: vec![0; 32],
            recommended_po2: 20,
            recommended_threads: 8,
            estimated_prove_time: Duration::from_secs(50),
            execution_time: Duration::from_millis(500),
        };

        let summary = plan.summary();
        assert!(summary.contains("50000000"));
        assert!(summary.contains("po2=20"));
    }

    #[test]
    fn test_should_use_bonsai() {
        let plan = ExecutionPlan {
            cycles: 200_000_000,
            segment_count: 200,
            memory_estimate_bytes: 0,
            journal: vec![],
            recommended_po2: 19,
            recommended_threads: 8,
            estimated_prove_time: Duration::from_secs(200),
            execution_time: Duration::from_secs(1),
        };

        assert!(plan.should_use_bonsai(100_000_000));
        assert!(!plan.should_use_bonsai(500_000_000));
    }

    #[test]
    fn test_config_defaults() {
        let config = SegmentProverConfig::default();
        assert!(config.segment_limit_po2.is_none());
        assert_eq!(config.proving_threads, 0);
        assert!(config.cache_executions);
        assert_eq!(config.max_cycles, 1_000_000_000);
        assert_eq!(config.max_segments, 4000);
    }
}
