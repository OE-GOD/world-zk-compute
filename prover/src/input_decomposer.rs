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

#![allow(dead_code)]

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

/// A sub-job created by decomposition.
#[derive(Debug, Clone)]
pub struct SubJob {
    /// Index of this sub-job (0-based).
    pub index: usize,
    /// Total number of sub-jobs.
    pub total: usize,
    /// The sub-input bytes for this chunk.
    pub input: Vec<u8>,
    /// Number of items in this chunk (for logging).
    pub item_count: usize,
}

/// Result of proving a single sub-job.
#[derive(Debug, Clone)]
pub struct SubProofResult {
    /// Sub-job index.
    pub index: usize,
    /// Proof seal bytes.
    pub seal: Vec<u8>,
    /// Journal (public outputs) from this sub-proof.
    pub journal: Vec<u8>,
    /// Proving time for this sub-job.
    pub prove_time: Duration,
    /// Cycles used.
    pub cycles: u64,
}

/// Merged result from all sub-proofs.
#[derive(Debug)]
pub struct DecomposedProofResult {
    /// All sub-proof seals (one per sub-job).
    pub seals: Vec<Vec<u8>>,
    /// Merged journal from all sub-proofs.
    pub merged_journal: Vec<u8>,
    /// Total proving time (wall clock, including parallelism).
    pub total_time: Duration,
    /// Sum of individual sub-proof times.
    pub sum_prove_time: Duration,
    /// Total cycles across all sub-proofs.
    pub total_cycles: u64,
    /// Number of sub-jobs.
    pub sub_job_count: usize,
    /// Speedup factor (sum_time / wall_time).
    pub speedup: f64,
}

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
// Chunk-Based Strategy (generic)
// ═══════════════════════════════════════════════════════════════════════════════

/// Generic chunk-based decomposition.
///
/// Splits input by a fixed item size. Items are assumed to be contiguous
/// in the input after a header. This works for simple array-of-structs inputs.
///
/// Input format assumed:
/// ```text
/// [header: header_size bytes][item_0][item_1]...[item_N-1]
/// ```
#[derive(Debug, Clone)]
pub struct ChunkStrategy {
    /// Name for logging.
    pub name: String,
    /// Size of the header (bytes) that precedes the items.
    pub header_size: usize,
    /// Size of each item (bytes). 0 = variable-length (uses item_count_fn).
    pub item_size: usize,
    /// Function to read item count from header (offset in header).
    /// The count is read as a little-endian u32 at this offset.
    pub count_offset: usize,
}

impl ChunkStrategy {
    /// Read item count from the header.
    fn read_count(&self, input: &[u8]) -> Result<usize> {
        if input.len() < self.count_offset + 4 {
            return Err(anyhow!("Input too short to read item count"));
        }
        let bytes: [u8; 4] = input[self.count_offset..self.count_offset + 4]
            .try_into()
            .map_err(|_| anyhow!("Failed to read count bytes"))?;
        Ok(u32::from_le_bytes(bytes) as usize)
    }

    /// Build a sub-input from header + subset of items.
    fn build_sub_input(
        &self,
        header: &[u8],
        items: &[u8],
        new_count: usize,
        count_offset: usize,
    ) -> Vec<u8> {
        let mut sub = Vec::with_capacity(header.len() + items.len());
        sub.extend_from_slice(header);
        // Patch the item count in the header
        let count_bytes = (new_count as u32).to_le_bytes();
        sub[count_offset..count_offset + 4].copy_from_slice(&count_bytes);
        sub.extend_from_slice(items);
        sub
    }
}

impl DecompositionStrategy for ChunkStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn can_decompose(&self, input: &[u8]) -> bool {
        if input.len() < self.header_size {
            return false;
        }
        self.read_count(input).map_or(false, |c| c > 1)
    }

    fn item_count(&self, input: &[u8]) -> Result<usize> {
        self.read_count(input)
    }

    fn split(&self, input: &[u8], n: usize) -> Result<Vec<Vec<u8>>> {
        let total_items = self.read_count(input)?;
        if total_items == 0 {
            return Err(anyhow!("No items to split"));
        }

        let header = &input[..self.header_size];
        let items_data = &input[self.header_size..];

        if self.item_size == 0 {
            return Err(anyhow!("Variable-length items not supported by ChunkStrategy"));
        }

        let expected_len = total_items * self.item_size;
        if items_data.len() < expected_len {
            return Err(anyhow!(
                "Items data too short: expected {} bytes for {} items, got {}",
                expected_len,
                total_items,
                items_data.len()
            ));
        }

        let chunk_count = n.min(total_items);
        let base_size = total_items / chunk_count;
        let remainder = total_items % chunk_count;

        let mut sub_inputs = Vec::with_capacity(chunk_count);
        let mut offset = 0;

        for i in 0..chunk_count {
            let this_chunk = base_size + if i < remainder { 1 } else { 0 };
            let start = offset * self.item_size;
            let end = (offset + this_chunk) * self.item_size;

            let sub = self.build_sub_input(
                header,
                &items_data[start..end],
                this_chunk,
                self.count_offset,
            );
            sub_inputs.push(sub);
            offset += this_chunk;
        }

        Ok(sub_inputs)
    }

    fn merge_journals(&self, journals: &[Vec<u8>]) -> Result<Vec<u8>> {
        // Default merge: concatenate with length prefixes
        let mut merged = Vec::new();
        // Header: number of sub-journals
        merged.extend_from_slice(&(journals.len() as u32).to_le_bytes());
        // Length of each journal
        for j in journals {
            merged.extend_from_slice(&(j.len() as u32).to_le_bytes());
        }
        // Journal data
        for j in journals {
            merged.extend_from_slice(j);
        }
        Ok(merged)
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

    /// Check if a strategy is registered for this image ID.
    pub fn has_strategy(&self, image_id: &alloy::primitives::B256) -> bool {
        self.strategies.contains_key(image_id)
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

    /// Decompose input and prove all sub-jobs in parallel.
    pub async fn prove_decomposed(
        &self,
        elf: &[u8],
        strategy: &dyn DecompositionStrategy,
        input: &[u8],
    ) -> Result<DecomposedProofResult> {
        let wall_start = Instant::now();

        // Check if decomposition is worthwhile
        if input.len() < self.config.min_input_size || !strategy.can_decompose(input) {
            return Err(anyhow!(
                "Input not suitable for decomposition (size={}, can_decompose={})",
                input.len(),
                strategy.can_decompose(input)
            ));
        }

        // Plan the split
        let n = self.plan_split(strategy, input)?;
        if n <= 1 {
            return Err(anyhow!("Decomposition produced only 1 sub-job, not worthwhile"));
        }

        // Split input
        let sub_inputs = strategy.split(input, n)?;
        info!(
            "Split into {} sub-jobs: {:?} bytes each",
            sub_inputs.len(),
            sub_inputs.iter().map(|s| s.len()).collect::<Vec<_>>()
        );

        // Prove all sub-jobs in parallel using tokio::JoinSet
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
                let _permit = sem.acquire().await.map_err(|e| anyhow!("Semaphore error: {}", e))?;

                info!("Proving sub-job {}/{}", i + 1, total);
                let start = Instant::now();

                let config = SegmentProverConfig::default();
                let prover = SegmentProver::new(config, proving_mode, use_snark);
                let result = prover.prove_optimized(&elf_owned, &sub_input).await?;

                let prove_time = start.elapsed();
                info!(
                    "Sub-job {}/{} complete: {} cycles, {:?}",
                    i + 1,
                    total,
                    result.cycles,
                    prove_time
                );

                Ok::<_, anyhow::Error>(SubProofResult {
                    index: i,
                    seal: result.seal,
                    journal: result.journal,
                    prove_time,
                    cycles: result.cycles,
                })
            });

            handles.push(handle);
        }

        // Collect results
        let mut sub_results: Vec<SubProofResult> = Vec::with_capacity(n);
        for handle in handles {
            let result = handle.await.map_err(|e| anyhow!("Sub-job panicked: {}", e))??;
            sub_results.push(result);
        }

        // Sort by index to maintain order
        sub_results.sort_by_key(|r| r.index);

        // Merge journals
        let journals: Vec<Vec<u8>> = sub_results.iter().map(|r| r.journal.clone()).collect();
        let merged_journal = strategy.merge_journals(&journals)?;

        // Collect seals
        let seals: Vec<Vec<u8>> = sub_results.iter().map(|r| r.seal.clone()).collect();

        // Compute timing stats
        let total_time = wall_start.elapsed();
        let sum_prove_time: Duration = sub_results.iter().map(|r| r.prove_time).sum();
        let total_cycles: u64 = sub_results.iter().map(|r| r.cycles).sum();
        let speedup = if total_time.as_secs_f64() > 0.0 {
            sum_prove_time.as_secs_f64() / total_time.as_secs_f64()
        } else {
            1.0
        };

        info!(
            "Decomposed proving complete: {} sub-jobs, {} total cycles, \
             {:.1}x speedup ({:?} wall vs {:?} sum)",
            n, total_cycles, speedup, total_time, sum_prove_time
        );

        Ok(DecomposedProofResult {
            seals,
            merged_journal,
            total_time,
            sum_prove_time,
            total_cycles,
            sub_job_count: n,
            speedup,
        })
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

    /// Test strategy that splits a simple format:
    /// [u32 count][item_0: 8 bytes][item_1: 8 bytes]...
    fn test_strategy() -> ChunkStrategy {
        ChunkStrategy {
            name: "test".to_string(),
            header_size: 4, // just the count
            item_size: 8,
            count_offset: 0,
        }
    }

    fn make_test_input(item_count: usize) -> Vec<u8> {
        let mut input = Vec::new();
        input.extend_from_slice(&(item_count as u32).to_le_bytes());
        for i in 0..item_count {
            input.extend_from_slice(&(i as u64).to_le_bytes());
        }
        input
    }

    #[test]
    fn test_chunk_strategy_can_decompose() {
        let strategy = test_strategy();

        // Empty input
        assert!(!strategy.can_decompose(&[]));

        // Single item
        let single = make_test_input(1);
        assert!(!strategy.can_decompose(&single));

        // Multiple items
        let multi = make_test_input(10);
        assert!(strategy.can_decompose(&multi));
    }

    #[test]
    fn test_chunk_strategy_item_count() {
        let strategy = test_strategy();
        let input = make_test_input(42);
        assert_eq!(strategy.item_count(&input).unwrap(), 42);
    }

    #[test]
    fn test_chunk_strategy_split() {
        let strategy = test_strategy();
        let input = make_test_input(10);

        // Split into 3 chunks: 4, 3, 3
        let chunks = strategy.split(&input, 3).unwrap();
        assert_eq!(chunks.len(), 3);

        // Verify item counts in each chunk
        assert_eq!(strategy.read_count(&chunks[0]).unwrap(), 4); // 10/3 + 1 remainder
        assert_eq!(strategy.read_count(&chunks[1]).unwrap(), 3);
        assert_eq!(strategy.read_count(&chunks[2]).unwrap(), 3);

        // Verify chunk sizes
        assert_eq!(chunks[0].len(), 4 + 4 * 8); // header + 4 items
        assert_eq!(chunks[1].len(), 4 + 3 * 8);
        assert_eq!(chunks[2].len(), 4 + 3 * 8);
    }

    #[test]
    fn test_chunk_strategy_split_even() {
        let strategy = test_strategy();
        let input = make_test_input(8);

        let chunks = strategy.split(&input, 4).unwrap();
        assert_eq!(chunks.len(), 4);

        for chunk in &chunks {
            assert_eq!(strategy.read_count(chunk).unwrap(), 2);
        }
    }

    #[test]
    fn test_chunk_strategy_split_more_chunks_than_items() {
        let strategy = test_strategy();
        let input = make_test_input(3);

        // Request 10 chunks but only 3 items → 3 chunks of 1 item each
        let chunks = strategy.split(&input, 10).unwrap();
        assert_eq!(chunks.len(), 3);

        for chunk in &chunks {
            assert_eq!(strategy.read_count(chunk).unwrap(), 1);
        }
    }

    #[test]
    fn test_merge_journals() {
        let strategy = test_strategy();
        let journals = vec![
            b"journal_a".to_vec(),
            b"journal_b".to_vec(),
        ];

        let merged = strategy.merge_journals(&journals).unwrap();

        // Header: 2 journals, then lengths, then data
        let count = u32::from_le_bytes(merged[0..4].try_into().unwrap());
        assert_eq!(count, 2);

        let len0 = u32::from_le_bytes(merged[4..8].try_into().unwrap()) as usize;
        let len1 = u32::from_le_bytes(merged[8..12].try_into().unwrap()) as usize;
        assert_eq!(len0, 9); // "journal_a"
        assert_eq!(len1, 9); // "journal_b"

        assert_eq!(&merged[12..12 + len0], b"journal_a");
        assert_eq!(&merged[12 + len0..12 + len0 + len1], b"journal_b");
    }

    #[test]
    fn test_plan_split() {
        let decomposer = InputDecomposer::new(
            DecomposerConfig {
                max_sub_jobs: 8,
                ..Default::default()
            },
            ProvingMode::Local,
            false,
        );

        let strategy = test_strategy();
        let input = make_test_input(100);

        let n = decomposer.plan_split(&strategy, &input).unwrap();
        assert!(n >= 2);
        assert!(n <= 8);
    }

    #[test]
    fn test_plan_split_small_input() {
        let decomposer = InputDecomposer::new(
            DecomposerConfig::default(),
            ProvingMode::Local,
            false,
        );

        let strategy = test_strategy();
        let input = make_test_input(2);

        let n = decomposer.plan_split(&strategy, &input).unwrap();
        assert_eq!(n, 2); // min 2 for decomposition
    }

    #[test]
    fn test_decomposer_config_defaults() {
        let config = DecomposerConfig::default();
        assert_eq!(config.min_input_size, 4096);
        assert_eq!(config.max_sub_jobs, 16);
        assert_eq!(config.max_concurrent_proofs, 4);
    }

    #[test]
    fn test_sub_job_structure() {
        let sub = SubJob {
            index: 0,
            total: 4,
            input: vec![1, 2, 3],
            item_count: 10,
        };

        assert_eq!(sub.index, 0);
        assert_eq!(sub.total, 4);
        assert_eq!(sub.item_count, 10);
    }

    #[test]
    fn test_chunk_strategy_data_preservation() {
        let strategy = test_strategy();
        let input = make_test_input(6);

        let chunks = strategy.split(&input, 2).unwrap();

        // Reconstruct all items from chunks
        let mut all_items = Vec::new();
        for chunk in &chunks {
            let count = strategy.read_count(chunk).unwrap();
            let items_data = &chunk[4..]; // skip header
            for j in 0..count {
                let start = j * 8;
                let val = u64::from_le_bytes(items_data[start..start + 8].try_into().unwrap());
                all_items.push(val);
            }
        }

        // Should have all original items 0..6
        assert_eq!(all_items, vec![0, 1, 2, 3, 4, 5]);
    }
}
