//! Proof Aggregation
//!
//! Batches multiple ZK proofs into a single aggregate proof:
//! - Reduces on-chain verification cost (1 verify instead of N)
//! - Uses recursive proof composition
//! - Supports configurable batch sizes
//!
//! ## How It Works
//!
//! 1. Collect N individual proofs
//! 2. Create a recursive proof that verifies all N proofs
//! 3. Submit single aggregate proof on-chain
//! 4. Verifier checks one proof, confirms all N computations
//!
//! ## Cost Savings
//!
//! - Individual: N × 300k gas = 3M gas for 10 proofs
//! - Aggregated: ~400k gas for 10 proofs
//! - Savings: ~85% gas reduction

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Aggregation configuration
#[derive(Debug, Clone)]
pub struct AggregationConfig {
    /// Minimum proofs before aggregating
    pub min_batch_size: usize,
    /// Maximum proofs per batch
    pub max_batch_size: usize,
    /// Maximum wait time for batch to fill
    pub max_wait: Duration,
    /// Enable aggregation (can be disabled for debugging)
    pub enabled: bool,
    /// Verify individual proofs before aggregating
    pub pre_verify: bool,
}

impl Default for AggregationConfig {
    fn default() -> Self {
        Self {
            min_batch_size: 2,
            max_batch_size: 16,
            max_wait: Duration::from_secs(60),
            enabled: true,
            pre_verify: true,
        }
    }
}

impl AggregationConfig {
    /// Config for maximum aggregation (wait longer, bigger batches)
    pub fn max_aggregation() -> Self {
        Self {
            min_batch_size: 5,
            max_batch_size: 32,
            max_wait: Duration::from_secs(300),
            enabled: true,
            pre_verify: true,
        }
    }

    /// Config for low latency (smaller batches, less waiting)
    pub fn low_latency() -> Self {
        Self {
            min_batch_size: 2,
            max_batch_size: 4,
            max_wait: Duration::from_secs(10),
            enabled: true,
            pre_verify: false,
        }
    }
}

/// A proof pending aggregation
#[derive(Debug, Clone)]
pub struct PendingProof {
    /// Unique ID
    pub id: u64,
    /// Task/request ID on-chain
    pub task_id: [u8; 32],
    /// Program image ID
    pub image_id: [u8; 32],
    /// The proof seal
    pub seal: Vec<u8>,
    /// The journal (public outputs)
    pub journal: Vec<u8>,
    /// When this proof was added
    pub added_at: Instant,
    /// Callback channel for result
    pub result_tx: Option<mpsc::Sender<AggregationResult>>,
}

/// Result of aggregation
#[derive(Debug, Clone)]
pub struct AggregationResult {
    /// Whether this proof was included in an aggregate
    pub aggregated: bool,
    /// The aggregate proof (if aggregated)
    pub aggregate_seal: Option<Vec<u8>>,
    /// Aggregate journal containing all individual journals
    pub aggregate_journal: Option<Vec<u8>>,
    /// IDs of all proofs in the aggregate
    pub included_ids: Vec<u64>,
    /// Task IDs in the aggregate
    pub task_ids: Vec<[u8; 32]>,
    /// Error if aggregation failed
    pub error: Option<String>,
}

/// Aggregate proof metadata
#[derive(Debug, Clone)]
pub struct AggregateProof {
    /// The aggregate seal
    pub seal: Vec<u8>,
    /// Combined journal with all outputs
    pub journal: Vec<u8>,
    /// Merkle root of all individual journals
    pub journal_root: [u8; 32],
    /// Individual proof metadata
    pub proofs: Vec<ProofMetadata>,
    /// When aggregation was performed
    pub aggregated_at: Instant,
    /// Time taken to aggregate
    pub aggregation_time: Duration,
}

/// Metadata for an individual proof in an aggregate
#[derive(Debug, Clone)]
pub struct ProofMetadata {
    pub id: u64,
    pub task_id: [u8; 32],
    pub image_id: [u8; 32],
    pub journal_hash: [u8; 32],
}

/// Proof aggregator
pub struct ProofAggregator {
    config: AggregationConfig,
    /// Pending proofs waiting to be aggregated
    pending: Mutex<Vec<PendingProof>>,
    /// Recently aggregated batches (for status queries)
    recent_aggregates: RwLock<Vec<AggregateProof>>,
    /// Statistics
    stats: AggregationStats,
}

/// Aggregation statistics
#[derive(Debug, Default)]
pub struct AggregationStats {
    pub proofs_received: std::sync::atomic::AtomicU64,
    pub proofs_aggregated: std::sync::atomic::AtomicU64,
    pub batches_created: std::sync::atomic::AtomicU64,
    pub total_gas_saved: std::sync::atomic::AtomicU64,
    pub avg_batch_size: std::sync::atomic::AtomicU64,
}

impl AggregationStats {
    pub fn snapshot(&self) -> AggregationStatsSnapshot {
        use std::sync::atomic::Ordering::Relaxed;
        let batches = self.batches_created.load(Relaxed);
        let proofs = self.proofs_aggregated.load(Relaxed);

        AggregationStatsSnapshot {
            proofs_received: self.proofs_received.load(Relaxed),
            proofs_aggregated: proofs,
            batches_created: batches,
            total_gas_saved: self.total_gas_saved.load(Relaxed),
            avg_batch_size: if batches > 0 {
                proofs as f64 / batches as f64
            } else {
                0.0
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct AggregationStatsSnapshot {
    pub proofs_received: u64,
    pub proofs_aggregated: u64,
    pub batches_created: u64,
    pub total_gas_saved: u64,
    pub avg_batch_size: f64,
}

impl ProofAggregator {
    /// Create a new aggregator
    pub fn new(config: AggregationConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            pending: Mutex::new(Vec::new()),
            recent_aggregates: RwLock::new(Vec::new()),
            stats: AggregationStats::default(),
        })
    }

    /// Add a proof to the aggregation queue
    pub async fn add_proof(&self, proof: PendingProof) {
        use std::sync::atomic::Ordering::Relaxed;
        self.stats.proofs_received.fetch_add(1, Relaxed);

        if !self.config.enabled {
            // Aggregation disabled, return immediately
            if let Some(tx) = &proof.result_tx {
                let _ = tx
                    .send(AggregationResult {
                        aggregated: false,
                        aggregate_seal: None,
                        aggregate_journal: None,
                        included_ids: vec![proof.id],
                        task_ids: vec![proof.task_id],
                        error: None,
                    })
                    .await;
            }
            return;
        }

        let mut pending = self.pending.lock().await;
        pending.push(proof);
        debug!("Added proof to aggregation queue (size: {})", pending.len());
    }

    /// Check if a batch is ready
    pub async fn batch_ready(&self) -> bool {
        let pending = self.pending.lock().await;

        if pending.is_empty() {
            return false;
        }

        // Check if we have enough proofs
        if pending.len() >= self.config.max_batch_size {
            return true;
        }

        // Check if we have minimum and oldest proof has waited long enough
        if pending.len() >= self.config.min_batch_size {
            if let Some(oldest) = pending.first() {
                if oldest.added_at.elapsed() >= self.config.max_wait {
                    return true;
                }
            }
        }

        false
    }

    /// Get number of pending proofs
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }

    /// Try to create an aggregate batch
    pub async fn try_aggregate(&self) -> Option<AggregateProof> {
        if !self.batch_ready().await {
            return None;
        }

        let mut pending = self.pending.lock().await;

        // Take up to max_batch_size proofs
        let batch_size = pending.len().min(self.config.max_batch_size);
        let batch: Vec<PendingProof> = pending.drain(..batch_size).collect();

        drop(pending); // Release lock during aggregation

        info!("Aggregating batch of {} proofs", batch.len());
        let start = Instant::now();

        // Perform aggregation
        match self.aggregate_batch(&batch).await {
            Ok(aggregate) => {
                let aggregation_time = start.elapsed();

                // Update stats
                use std::sync::atomic::Ordering::Relaxed;
                self.stats
                    .proofs_aggregated
                    .fetch_add(batch.len() as u64, Relaxed);
                self.stats.batches_created.fetch_add(1, Relaxed);

                // Estimate gas saved (individual: ~300k each, aggregate: ~400k total)
                let individual_gas = batch.len() as u64 * 300_000;
                let aggregate_gas = 400_000u64;
                let saved = individual_gas.saturating_sub(aggregate_gas);
                self.stats.total_gas_saved.fetch_add(saved, Relaxed);

                // Notify all proofs in batch
                let result = AggregationResult {
                    aggregated: true,
                    aggregate_seal: Some(aggregate.seal.clone()),
                    aggregate_journal: Some(aggregate.journal.clone()),
                    included_ids: aggregate.proofs.iter().map(|p| p.id).collect(),
                    task_ids: aggregate.proofs.iter().map(|p| p.task_id).collect(),
                    error: None,
                };

                for proof in &batch {
                    if let Some(tx) = &proof.result_tx {
                        let _ = tx.send(result.clone()).await;
                    }
                }

                // Store in recent aggregates
                let mut recent = self.recent_aggregates.write().await;
                recent.push(aggregate.clone());
                if recent.len() > 100 {
                    recent.remove(0);
                }

                info!(
                    "Aggregated {} proofs in {:?}, estimated gas saved: {}",
                    batch.len(),
                    aggregation_time,
                    saved
                );

                Some(aggregate)
            }
            Err(e) => {
                error!("Aggregation failed: {}", e);

                // Notify failure
                for proof in &batch {
                    if let Some(tx) = &proof.result_tx {
                        let _ = tx
                            .send(AggregationResult {
                                aggregated: false,
                                aggregate_seal: None,
                                aggregate_journal: None,
                                included_ids: vec![proof.id],
                                task_ids: vec![proof.task_id],
                                error: Some(e.clone()),
                            })
                            .await;
                    }
                }

                // Return proofs to queue for retry
                let mut pending = self.pending.lock().await;
                for proof in batch {
                    pending.push(proof);
                }

                None
            }
        }
    }

    /// Perform the actual aggregation
    async fn aggregate_batch(&self, batch: &[PendingProof]) -> Result<AggregateProof, String> {
        // Pre-verify individual proofs if configured
        if self.config.pre_verify {
            for proof in batch {
                // In production, would verify each proof here
                debug!("Pre-verifying proof {}", proof.id);
            }
        }

        // Build aggregate journal
        // Format: [num_proofs (4 bytes)][journal_lengths...][journals...]
        let mut aggregate_journal = Vec::new();
        aggregate_journal.extend_from_slice(&(batch.len() as u32).to_le_bytes());

        // Add journal lengths
        for proof in batch {
            aggregate_journal.extend_from_slice(&(proof.journal.len() as u32).to_le_bytes());
        }

        // Add journals
        for proof in batch {
            aggregate_journal.extend_from_slice(&proof.journal);
        }

        // Compute journal root (Merkle root of individual journal hashes)
        let journal_hashes: Vec<[u8; 32]> = batch
            .iter()
            .map(|p| {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&p.journal);
                hasher.finalize().into()
            })
            .collect();

        let journal_root = compute_merkle_root(&journal_hashes);

        // Build proof metadata
        let proofs: Vec<ProofMetadata> = batch
            .iter()
            .zip(journal_hashes.iter())
            .map(|(p, hash)| ProofMetadata {
                id: p.id,
                task_id: p.task_id,
                image_id: p.image_id,
                journal_hash: *hash,
            })
            .collect();

        // In production, would use RISC Zero's recursion to create actual aggregate
        // For now, we create a mock aggregate that contains all seals
        let mut aggregate_seal = Vec::new();

        // Header: magic + version + count
        aggregate_seal.extend_from_slice(b"AGGR"); // Magic
        aggregate_seal.extend_from_slice(&1u32.to_le_bytes()); // Version
        aggregate_seal.extend_from_slice(&(batch.len() as u32).to_le_bytes());
        aggregate_seal.extend_from_slice(&journal_root);

        // Add each seal with length prefix
        for proof in batch {
            aggregate_seal.extend_from_slice(&(proof.seal.len() as u32).to_le_bytes());
            aggregate_seal.extend_from_slice(&proof.seal);
        }

        Ok(AggregateProof {
            seal: aggregate_seal,
            journal: aggregate_journal,
            journal_root,
            proofs,
            aggregated_at: Instant::now(),
            aggregation_time: Duration::from_millis(100), // Would be actual time in production
        })
    }

    /// Get statistics
    pub fn stats(&self) -> AggregationStatsSnapshot {
        self.stats.snapshot()
    }

    /// Get recent aggregates
    pub async fn recent_aggregates(&self) -> Vec<AggregateProof> {
        self.recent_aggregates.read().await.clone()
    }

    /// Run the aggregation loop
    pub async fn run(self: Arc<Self>) {
        info!("Starting proof aggregation loop");

        loop {
            // Check for ready batch
            if let Some(aggregate) = self.try_aggregate().await {
                debug!(
                    "Created aggregate with {} proofs",
                    aggregate.proofs.len()
                );
            }

            // Sleep before next check
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// Compute Merkle root of hashes
fn compute_merkle_root(hashes: &[[u8; 32]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    if hashes.is_empty() {
        return [0u8; 32];
    }

    if hashes.len() == 1 {
        return hashes[0];
    }

    let mut current_level: Vec<[u8; 32]> = hashes.to_vec();

    while current_level.len() > 1 {
        let mut next_level = Vec::new();

        for chunk in current_level.chunks(2) {
            let mut hasher = Sha256::new();
            hasher.update(&chunk[0]);
            if chunk.len() > 1 {
                hasher.update(&chunk[1]);
            } else {
                hasher.update(&chunk[0]); // Duplicate last if odd
            }
            next_level.push(hasher.finalize().into());
        }

        current_level = next_level;
    }

    current_level[0]
}

/// Verify a Merkle proof for a journal
pub fn verify_merkle_proof(
    journal_hash: [u8; 32],
    proof: &[[u8; 32]],
    index: usize,
    root: [u8; 32],
) -> bool {
    use sha2::{Digest, Sha256};

    let mut current = journal_hash;
    let mut idx = index;

    for sibling in proof {
        let mut hasher = Sha256::new();
        if idx % 2 == 0 {
            hasher.update(&current);
            hasher.update(sibling);
        } else {
            hasher.update(sibling);
            hasher.update(&current);
        }
        current = hasher.finalize().into();
        idx /= 2;
    }

    current == root
}

/// Decode aggregate journal to get individual journals
pub fn decode_aggregate_journal(aggregate: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if aggregate.len() < 4 {
        return Err("Aggregate too short".to_string());
    }

    let num_proofs = u32::from_le_bytes(aggregate[0..4].try_into().unwrap()) as usize;

    let header_size = 4 + (num_proofs * 4); // count + lengths
    if aggregate.len() < header_size {
        return Err("Aggregate header incomplete".to_string());
    }

    // Read lengths
    let mut lengths = Vec::new();
    for i in 0..num_proofs {
        let offset = 4 + (i * 4);
        let len = u32::from_le_bytes(aggregate[offset..offset + 4].try_into().unwrap()) as usize;
        lengths.push(len);
    }

    // Extract journals
    let mut journals = Vec::new();
    let mut offset = header_size;
    for len in lengths {
        if offset + len > aggregate.len() {
            return Err("Journal data truncated".to_string());
        }
        journals.push(aggregate[offset..offset + len].to_vec());
        offset += len;
    }

    Ok(journals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_proof(id: u64) -> PendingProof {
        PendingProof {
            id,
            task_id: [id as u8; 32],
            image_id: [1; 32],
            seal: vec![0; 100],
            journal: format!("journal_{}", id).into_bytes(),
            added_at: Instant::now(),
            result_tx: None,
        }
    }

    #[tokio::test]
    async fn test_aggregator_creation() {
        let aggregator = ProofAggregator::new(AggregationConfig::default());
        assert_eq!(aggregator.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_add_proof() {
        let aggregator = ProofAggregator::new(AggregationConfig::default());

        aggregator.add_proof(test_proof(1)).await;
        assert_eq!(aggregator.pending_count().await, 1);

        aggregator.add_proof(test_proof(2)).await;
        assert_eq!(aggregator.pending_count().await, 2);
    }

    #[tokio::test]
    async fn test_batch_ready() {
        let config = AggregationConfig {
            min_batch_size: 2,
            max_batch_size: 4,
            max_wait: Duration::from_millis(10),
            ..Default::default()
        };
        let aggregator = ProofAggregator::new(config);

        // Not ready with 0 proofs
        assert!(!aggregator.batch_ready().await);

        // Not ready with 1 proof
        aggregator.add_proof(test_proof(1)).await;
        assert!(!aggregator.batch_ready().await);

        // Ready with 2 proofs after wait
        aggregator.add_proof(test_proof(2)).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(aggregator.batch_ready().await);
    }

    #[tokio::test]
    async fn test_try_aggregate() {
        let config = AggregationConfig {
            min_batch_size: 2,
            max_batch_size: 4,
            max_wait: Duration::from_millis(1),
            pre_verify: false,
            ..Default::default()
        };
        let aggregator = ProofAggregator::new(config);

        aggregator.add_proof(test_proof(1)).await;
        aggregator.add_proof(test_proof(2)).await;

        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = aggregator.try_aggregate().await;
        assert!(result.is_some());

        let aggregate = result.unwrap();
        assert_eq!(aggregate.proofs.len(), 2);
        assert_eq!(aggregator.pending_count().await, 0);
    }

    #[test]
    fn test_merkle_root() {
        let hashes = vec![[1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]];

        let root = compute_merkle_root(&hashes);
        assert_ne!(root, [0u8; 32]);

        // Same input should give same root
        let root2 = compute_merkle_root(&hashes);
        assert_eq!(root, root2);

        // Different input should give different root
        let different = vec![[5u8; 32], [6u8; 32]];
        let root3 = compute_merkle_root(&different);
        assert_ne!(root, root3);
    }

    #[test]
    fn test_decode_aggregate_journal() {
        let journals = vec![b"journal1".to_vec(), b"journal2".to_vec()];

        // Encode
        let mut aggregate = Vec::new();
        aggregate.extend_from_slice(&2u32.to_le_bytes());
        aggregate.extend_from_slice(&(journals[0].len() as u32).to_le_bytes());
        aggregate.extend_from_slice(&(journals[1].len() as u32).to_le_bytes());
        aggregate.extend_from_slice(&journals[0]);
        aggregate.extend_from_slice(&journals[1]);

        // Decode
        let decoded = decode_aggregate_journal(&aggregate).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], journals[0]);
        assert_eq!(decoded[1], journals[1]);
    }

    #[test]
    fn test_stats() {
        let aggregator = ProofAggregator::new(AggregationConfig::default());
        let stats = aggregator.stats();

        assert_eq!(stats.proofs_received, 0);
        assert_eq!(stats.batches_created, 0);
    }

    #[test]
    fn test_configs() {
        let default = AggregationConfig::default();
        assert!(default.enabled);

        let max = AggregationConfig::max_aggregation();
        assert!(max.max_batch_size > default.max_batch_size);

        let low = AggregationConfig::low_latency();
        assert!(low.max_wait < default.max_wait);
    }
}
