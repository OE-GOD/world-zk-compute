//! Proof Caching for Instant Results
//!
//! Caches completed proofs by (image_id, input_hash) to skip re-proving
//! identical computations. If the same job is requested again, we return
//! the cached proof instantly.
//!
//! ## Benefits
//!
//! - **Instant results**: Cached proofs returned in <1ms vs minutes for proving
//! - **Cost savings**: No compute/Bonsai costs for repeated jobs
//! - **Higher throughput**: More jobs processed per unit time
//!
//! ## Cache Key
//!
//! The cache key is `(image_id, input_hash)` which uniquely identifies a computation:
//! - `image_id`: The program being executed (deterministic)
//! - `input_hash`: Hash of the input data
//!
//! Since zkVM execution is deterministic, same program + same input = same output.

use alloy::primitives::B256;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Cache key for a proof: (image_id, input_hash)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProofCacheKey {
    pub image_id: B256,
    pub input_hash: B256,
}

impl ProofCacheKey {
    pub fn new(image_id: B256, input_hash: B256) -> Self {
        Self { image_id, input_hash }
    }

    /// Convert to hex string for file naming
    pub fn to_hex(&self) -> String {
        format!("{}_{}", hex::encode(self.image_id), hex::encode(self.input_hash))
    }
}

/// Cached proof entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedProof {
    /// The proof bytes (STARK or SNARK)
    pub proof: Vec<u8>,
    /// The journal (public outputs)
    pub journal: Vec<u8>,
    /// Number of cycles used
    pub cycles: u64,
    /// When the proof was generated (Unix timestamp)
    pub generated_at: u64,
    /// How long proving took (for metrics)
    pub proof_time_ms: u64,
}

impl CachedProof {
    pub fn new(proof: Vec<u8>, journal: Vec<u8>, cycles: u64, proof_time: Duration) -> Self {
        let generated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            proof,
            journal,
            cycles,
            generated_at,
            proof_time_ms: proof_time.as_millis() as u64,
        }
    }
}

/// In-memory LRU entry with access tracking
struct CacheEntry {
    proof: CachedProof,
    last_accessed: Instant,
    access_count: u64,
}

/// Proof cache with memory + optional disk persistence
pub struct ProofCache {
    /// In-memory cache (LRU-style)
    memory: Arc<RwLock<HashMap<ProofCacheKey, CacheEntry>>>,
    /// Maximum entries in memory
    max_memory_entries: usize,
    /// Disk cache directory (optional)
    disk_path: Option<PathBuf>,
    /// Cache TTL (proofs older than this are evicted)
    ttl: Duration,
    /// Statistics
    stats: Arc<RwLock<CacheStats>>,
}

/// Cache statistics
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub evictions: u64,
    pub disk_hits: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl ProofCache {
    /// Create a new proof cache
    pub fn new(max_memory_entries: usize, disk_path: Option<PathBuf>, ttl: Duration) -> Self {
        // Create disk directory if specified
        if let Some(ref path) = disk_path {
            if let Err(e) = std::fs::create_dir_all(path) {
                warn!("Failed to create proof cache directory: {}", e);
            }
        }

        Self {
            memory: Arc::new(RwLock::new(HashMap::with_capacity(max_memory_entries))),
            max_memory_entries,
            disk_path,
            ttl,
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Create with default settings
    pub fn with_defaults() -> Self {
        Self::new(
            1000,                           // 1000 proofs in memory
            Some(PathBuf::from("./cache/proofs")), // Disk persistence
            Duration::from_secs(24 * 3600), // 24 hour TTL
        )
    }

    /// Get a cached proof
    pub async fn get(&self, key: &ProofCacheKey) -> Option<CachedProof> {
        // Try memory first
        {
            let mut memory = self.memory.write().await;
            if let Some(entry) = memory.get_mut(key) {
                // Check TTL
                let age = Duration::from_secs(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        - entry.proof.generated_at,
                );

                if age > self.ttl {
                    debug!("Proof cache entry expired for {:?}", key);
                    memory.remove(key);
                } else {
                    // Update access tracking
                    entry.last_accessed = Instant::now();
                    entry.access_count += 1;

                    // Record hit
                    let mut stats = self.stats.write().await;
                    stats.hits += 1;

                    debug!(
                        "Proof cache HIT (memory): {:?} (accessed {} times)",
                        key, entry.access_count
                    );
                    return Some(entry.proof.clone());
                }
            }
        }

        // Try disk if available
        if let Some(ref disk_path) = self.disk_path {
            if let Some(proof) = self.load_from_disk(disk_path, key).await {
                // Check TTL
                let age = Duration::from_secs(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        - proof.generated_at,
                );

                if age <= self.ttl {
                    // Promote to memory cache
                    self.insert_memory(key, proof.clone()).await;

                    let mut stats = self.stats.write().await;
                    stats.hits += 1;
                    stats.disk_hits += 1;

                    debug!("Proof cache HIT (disk): {:?}", key);
                    return Some(proof);
                }
            }
        }

        // Record miss
        {
            let mut stats = self.stats.write().await;
            stats.misses += 1;
        }

        debug!("Proof cache MISS: {:?}", key);
        None
    }

    /// Store a proof in the cache
    pub async fn put(&self, key: &ProofCacheKey, proof: CachedProof) {
        // Insert into memory
        self.insert_memory(key, proof.clone()).await;

        // Persist to disk if available
        if let Some(ref disk_path) = self.disk_path {
            self.save_to_disk(disk_path, key, &proof).await;
        }

        let mut stats = self.stats.write().await;
        stats.inserts += 1;

        info!(
            "Proof cached: {:?} ({} bytes, {} cycles)",
            key,
            proof.proof.len(),
            proof.cycles
        );
    }

    /// Insert into memory cache with LRU eviction
    async fn insert_memory(&self, key: &ProofCacheKey, proof: CachedProof) {
        let mut memory = self.memory.write().await;

        // Evict if at capacity
        while memory.len() >= self.max_memory_entries {
            // Find least recently used
            let lru_key = memory
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed)
                .map(|(k, _)| *k);

            if let Some(k) = lru_key {
                memory.remove(&k);
                let mut stats = self.stats.write().await;
                stats.evictions += 1;
                debug!("Evicted LRU proof: {:?}", k);
            } else {
                break;
            }
        }

        memory.insert(
            *key,
            CacheEntry {
                proof,
                last_accessed: Instant::now(),
                access_count: 1,
            },
        );
    }

    /// Load proof from disk
    async fn load_from_disk(&self, disk_path: &PathBuf, key: &ProofCacheKey) -> Option<CachedProof> {
        let file_path = disk_path.join(format!("{}.bin", key.to_hex()));

        match tokio::fs::read(&file_path).await {
            Ok(data) => match bincode::deserialize(&data) {
                Ok(proof) => Some(proof),
                Err(e) => {
                    warn!("Failed to deserialize cached proof: {}", e);
                    None
                }
            },
            Err(_) => None,
        }
    }

    /// Save proof to disk
    async fn save_to_disk(&self, disk_path: &PathBuf, key: &ProofCacheKey, proof: &CachedProof) {
        let file_path = disk_path.join(format!("{}.bin", key.to_hex()));

        match bincode::serialize(proof) {
            Ok(data) => {
                if let Err(e) = tokio::fs::write(&file_path, data).await {
                    warn!("Failed to write proof to disk: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to serialize proof: {}", e);
            }
        }
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        self.stats.read().await.clone()
    }

    /// Get current cache size
    pub async fn size(&self) -> usize {
        self.memory.read().await.len()
    }

    /// Clear expired entries
    pub async fn cleanup_expired(&self) -> usize {
        let mut memory = self.memory.write().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let before = memory.len();

        memory.retain(|_, entry| {
            let age = now - entry.proof.generated_at;
            age < self.ttl.as_secs()
        });

        let removed = before - memory.len();
        if removed > 0 {
            info!("Cleaned up {} expired proof cache entries", removed);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_put_get() {
        let cache = ProofCache::new(10, None, Duration::from_secs(3600));

        let key = ProofCacheKey::new(B256::ZERO, B256::repeat_byte(1));
        let proof = CachedProof::new(
            vec![1, 2, 3],
            vec![4, 5, 6],
            1000,
            Duration::from_secs(10),
        );

        cache.put(&key, proof.clone()).await;

        let cached = cache.get(&key).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().cycles, 1000);
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = ProofCache::new(10, None, Duration::from_secs(3600));

        let key = ProofCacheKey::new(B256::ZERO, B256::ZERO);
        let cached = cache.get(&key).await;
        assert!(cached.is_none());

        let stats = cache.stats().await;
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);
    }

    #[tokio::test]
    async fn test_cache_lru_eviction() {
        let cache = ProofCache::new(2, None, Duration::from_secs(3600));

        let key1 = ProofCacheKey::new(B256::repeat_byte(1), B256::ZERO);
        let key2 = ProofCacheKey::new(B256::repeat_byte(2), B256::ZERO);
        let key3 = ProofCacheKey::new(B256::repeat_byte(3), B256::ZERO);

        let proof = CachedProof::new(vec![1], vec![], 100, Duration::from_secs(1));

        cache.put(&key1, proof.clone()).await;
        cache.put(&key2, proof.clone()).await;

        // Access key1 to make it more recent
        cache.get(&key1).await;

        // Insert key3, should evict key2 (LRU)
        cache.put(&key3, proof).await;

        assert!(cache.get(&key1).await.is_some());
        assert!(cache.get(&key2).await.is_none()); // Evicted
        assert!(cache.get(&key3).await.is_some());
    }

    #[test]
    fn test_cache_key_hex() {
        let key = ProofCacheKey::new(B256::repeat_byte(0xAB), B256::repeat_byte(0xCD));
        let hex = key.to_hex();
        assert!(hex.contains("abab"));
        assert!(hex.contains("cdcd"));
    }
}
