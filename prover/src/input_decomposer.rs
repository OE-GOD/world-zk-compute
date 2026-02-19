//! Input Decomposition for Parallel Sub-Proofs
//!
//! Splits large proving inputs into independent sub-jobs that can be proved
//! in parallel, then merges results. This is application-level decomposition
//! — complementary to risc0's segment-level parallelism.
//!
//! ## Use Cases
//!
//! - **XGBoost inference**: Split N samples into K batches, prove each batch
//!   independently, merge prediction results.
//! - **Rule engine**: Split N records into K chunks, prove each chunk,
//!   aggregate compliance results.
//! - **Data validation**: Split large datasets into partitions for parallel
//!   verification.
//!
//! ## How It Works
//!
//! ```text
//! [Large Input (N items)] → decompose() → [K sub-inputs]
//!                                              ↓
//!                                    prove each in parallel
//!                                              ↓
//!                                   [K sub-proofs + journals]
//!                                              ↓
//!                                     merge_results()
//!                                              ↓
//!                                [Combined journal + K seals]
//! ```
//!
//! The decomposer doesn't know the guest program's internal format. Instead,
//! it works with a `DecompositionStrategy` that defines how to split and
//! merge for a specific program type.


use anyhow::{anyhow, Result};
use risc0_zkvm::Receipt;
use std::time::{Duration, Instant};
use tracing::info;

use crate::bonsai::ProvingMode;
use crate::segment_prover::{SegmentProver, SegmentProverConfig};

// ═══════════════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for input decomposition.
#[derive(Clone, Debug)]
pub struct DecomposerConfig {
    /// Minimum input size (bytes) before decomposition kicks in.
    pub min_input_size: usize,
    /// Maximum number of sub-jobs to create.
    pub max_sub_jobs: usize,
    /// Target cycles per sub-job (used to estimate split count).
    pub target_cycles_per_job: u64,
    /// Maximum concurrent sub-proofs.
    pub max_concurrent_proofs: usize,
}

impl Default for DecomposerConfig {
    fn default() -> Self {
        Self {
            min_input_size: 4096,           // 4 KB
            max_sub_jobs: 16,
            target_cycles_per_job: 20_000_000, // 20M cycles
            max_concurrent_proofs: 4,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Sub-Job and Results
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of a single sub-proof that preserves the Receipt.
#[derive(Debug)]
pub struct SubProofWithReceipt {
    /// Sub-job index.
    pub index: usize,
    /// The full Receipt (needed for recursive verification).
    pub receipt: Receipt,
    /// Proof seal bytes.
    pub seal: Vec<u8>,
    /// Journal (public outputs) from this sub-proof.
    pub journal: Vec<u8>,
    /// Proving time for this sub-job.
    pub prove_time: Duration,
    /// Cycles used.
    pub cycles: u64,
}

/// Decomposed proof result that preserves Receipts for recursive wrapping.
#[derive(Debug)]
#[allow(dead_code)]
pub struct DecomposedReceiptResult {
    /// All sub-proof Receipts (for recursive verification).
    pub receipts: Vec<Receipt>,
    /// All sub-proof seals.
    pub seals: Vec<Vec<u8>>,
    /// All sub-proof journals (in order).
    pub journals: Vec<Vec<u8>>,
    /// Merged journal from all sub-proofs.
    pub merged_journal: Vec<u8>,
    /// Total proving time (wall clock).
    pub total_time: Duration,
    /// Sum of individual sub-proof times.
    pub sum_prove_time: Duration,
    /// Total cycles across all sub-proofs.
    pub total_cycles: u64,
    /// Number of sub-jobs.
    pub sub_job_count: usize,
    /// Speedup factor.
    pub speedup: f64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Decomposition Strategy
// ═══════════════════════════════════════════════════════════════════════════════

/// Defines how to split and merge inputs for a specific program type.
///
/// Implement this trait for each guest program that supports decomposition.
pub trait DecompositionStrategy: Send + Sync {
    /// Name of this strategy (for logging).
    fn name(&self) -> &str;

    /// Whether this input can be decomposed.
    fn can_decompose(&self, input: &[u8]) -> bool;

    /// How many items are in this input (e.g., sample count).
    fn item_count(&self, input: &[u8]) -> Result<usize>;

    /// Split the input into `n` sub-inputs.
    ///
    /// Each sub-input must be a valid standalone input for the guest program.
    fn split(&self, input: &[u8], n: usize) -> Result<Vec<Vec<u8>>>;

    /// Merge journals from sub-proofs into a combined journal.
    ///
    /// The merged journal should represent the combined result of all sub-jobs.
    fn merge_journals(&self, journals: &[Vec<u8>]) -> Result<Vec<u8>>;

    /// Estimate total cycles for this input (optional, for split count planning).
    fn estimate_cycles(&self, input: &[u8]) -> Option<u64> {
        let _ = input;
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Strategy Registry
// ═══════════════════════════════════════════════════════════════════════════════

/// Maps program image IDs to their decomposition strategies.
///
/// The monitor checks this registry after preflight to decide whether
/// to use decomposed proving for a given program.
pub struct StrategyRegistry {
    strategies: std::collections::HashMap<alloy::primitives::B256, std::sync::Arc<dyn DecompositionStrategy>>,
}

impl StrategyRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            strategies: std::collections::HashMap::new(),
        }
    }

    /// Register a decomposition strategy for a program image ID.
    pub fn register(
        &mut self,
        image_id: alloy::primitives::B256,
        strategy: std::sync::Arc<dyn DecompositionStrategy>,
    ) {
        tracing::info!(
            "Registered decomposition strategy '{}' for image {}",
            strategy.name(),
            image_id
        );
        self.strategies.insert(image_id, strategy);
    }

    /// Look up the strategy for a given image ID.
    pub fn get(&self, image_id: &alloy::primitives::B256) -> Option<&std::sync::Arc<dyn DecompositionStrategy>> {
        self.strategies.get(image_id)
    }

}

impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input Decomposer
// ═══════════════════════════════════════════════════════════════════════════════

/// Decomposes large inputs and orchestrates parallel sub-proofs.
pub struct InputDecomposer {
    config: DecomposerConfig,
    proving_mode: ProvingMode,
    use_snark: bool,
}

impl InputDecomposer {
    /// Create a new input decomposer.
    pub fn new(
        config: DecomposerConfig,
        proving_mode: ProvingMode,
        use_snark: bool,
    ) -> Self {
        Self {
            config,
            proving_mode,
            use_snark,
        }
    }

    /// Determine the optimal number of sub-jobs for this input.
    pub fn plan_split(
        &self,
        strategy: &dyn DecompositionStrategy,
        input: &[u8],
    ) -> Result<usize> {
        let item_count = strategy.item_count(input)?;

        // Start with item-count-based split
        let mut n = if let Some(est_cycles) = strategy.estimate_cycles(input) {
            // Split based on target cycles per job
            let jobs = (est_cycles / self.config.target_cycles_per_job).max(1);
            jobs as usize
        } else {
            // Heuristic: ~sqrt(item_count), clamped
            let sqrt = (item_count as f64).sqrt().ceil() as usize;
            sqrt.max(2)
        };

        // Clamp to config limits
        n = n.min(self.config.max_sub_jobs).min(item_count);
        n = n.max(1);

        info!(
            "Decomposition plan: {} items → {} sub-jobs ({} strategy)",
            item_count,
            n,
            strategy.name()
        );

        Ok(n)
    }

    /// Decompose input and prove all sub-jobs, preserving Receipts.
    ///
    /// Same as `prove_decomposed()` but returns the full Receipt for each
    /// sub-proof so they can be passed to the recursive wrapper's
    /// `env::verify()` via `add_assumption()`.
    pub async fn prove_decomposed_with_receipts(
        &self,
        elf: &[u8],
        strategy: &dyn DecompositionStrategy,
        input: &[u8],
    ) -> Result<DecomposedReceiptResult> {
        let wall_start = Instant::now();

        if input.len() < self.config.min_input_size || !strategy.can_decompose(input) {
            return Err(anyhow!(
                "Input not suitable for decomposition (size={}, can_decompose={})",
                input.len(),
                strategy.can_decompose(input)
            ));
        }

        let n = self.plan_split(strategy, input)?;
        if n <= 1 {
            return Err(anyhow!("Decomposition produced only 1 sub-job, not worthwhile"));
        }

        let sub_inputs = strategy.split(input, n)?;
        info!(
            "Split into {} sub-jobs (with receipts): {:?} bytes each",
            sub_inputs.len(),
            sub_inputs.iter().map(|s| s.len()).collect::<Vec<_>>()
        );

        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(
            self.config.max_concurrent_proofs,
        ));

        let mut handles = Vec::new();

        for (i, sub_input) in sub_inputs.into_iter().enumerate() {
            let elf_owned = elf.to_vec();
            let sem = semaphore.clone();
            let proving_mode = self.proving_mode.clone();
            let use_snark = self.use_snark;
            let total = n;

            let handle = tokio::spawn(async move {
                let _permit = sem
                    .acquire()
                    .await
                    .map_err(|e| anyhow!("Semaphore error: {}", e))?;

                info!("Proving sub-job {}/{} (with receipt)", i + 1, total);
                let start = Instant::now();

                let config = SegmentProverConfig::default();
                let prover = SegmentProver::new(config, proving_mode, use_snark);
                let (result, receipt) = prover
                    .prove_with_receipt(&elf_owned, &sub_input)
                    .await?;

                let prove_time = start.elapsed();
                info!(
                    "Sub-job {}/{} complete (with receipt): {} cycles, {:?}",
                    i + 1,
                    total,
                    result.cycles,
                    prove_time
                );

                Ok::<_, anyhow::Error>(SubProofWithReceipt {
                    index: i,
                    receipt,
                    seal: result.seal,
                    journal: result.journal,
                    prove_time,
                    cycles: result.cycles,
                })
            });

            handles.push(handle);
        }

        let mut sub_results: Vec<SubProofWithReceipt> = Vec::with_capacity(n);
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| anyhow!("Sub-job panicked: {}", e))??;
            sub_results.push(result);
        }

        sub_results.sort_by_key(|r| r.index);

        // Capture timing stats before destructuring
        let sum_prove_time: Duration = sub_results.iter().map(|r| r.prove_time).sum();
        let total_cycles: u64 = sub_results.iter().map(|r| r.cycles).sum();

        let journals: Vec<Vec<u8>> = sub_results.iter().map(|r| r.journal.clone()).collect();
        let merged_journal = strategy.merge_journals(&journals)?;
        let seals: Vec<Vec<u8>> = sub_results.iter().map(|r| r.seal.clone()).collect();
        let receipts: Vec<Receipt> = sub_results.into_iter().map(|r| r.receipt).collect();

        let total_time = wall_start.elapsed();
        let speedup = if total_time.as_secs_f64() > 0.0 {
            sum_prove_time.as_secs_f64() / total_time.as_secs_f64()
        } else {
            1.0
        };

        info!(
            "Decomposed proving (with receipts) complete: {} sub-jobs, {} total cycles, \
             {:.1}x speedup ({:?} wall vs {:?} sum)",
            n, total_cycles, speedup, total_time, sum_prove_time
        );

        Ok(DecomposedReceiptResult {
            receipts,
            seals,
            journals,
            merged_journal,
            total_time,
            sum_prove_time,
            total_cycles,
            sub_job_count: n,
            speedup,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decomposer_config_defaults() {
        let config = DecomposerConfig::default();
        assert_eq!(config.min_input_size, 4096);
        assert_eq!(config.max_sub_jobs, 16);
        assert_eq!(config.max_concurrent_proofs, 4);
    }

    #[test]
    fn test_strategy_registry() {
        let registry = StrategyRegistry::new();
        let image_id = alloy::primitives::B256::ZERO;
        assert!(registry.get(&image_id).is_none());
    }
}
