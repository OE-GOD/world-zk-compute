//! Proof caching module for the warm prover.
//!
//! Provides an LRU-evicting, TTL-aware, thread-safe cache that maps
//! `(model_hash, features)` to previously generated proofs. This avoids
//! re-proving identical inputs within the TTL window.
//!
//! Configuration via environment variables:
//! - `PROOF_CACHE_SIZE`: maximum number of entries (default 1000)
//! - `PROOF_CACHE_TTL_SECS`: seconds before an entry expires (default 3600)

use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default maximum number of cached proofs.
const DEFAULT_CACHE_SIZE: usize = 1000;

/// Default TTL in seconds for cached proofs.
const DEFAULT_TTL_SECS: u64 = 3600;

/// A 32-byte SHA-256 cache key.
pub type CacheKey = [u8; 32];

/// A cached proof entry storing the raw proof output.
#[derive(Debug, Clone)]
pub struct CachedProof {
    /// The predicted class from model inference.
    pub predicted_class: u32,
    /// ABI-encoded proof bytes.
    pub proof_bytes: Vec<u8>,
    /// Circuit hash bytes.
    pub circuit_hash: Vec<u8>,
    /// Public inputs bytes.
    pub public_inputs: Vec<u8>,
}

/// Internal entry that wraps a `CachedProof` with insertion metadata.
struct CacheEntry {
    proof: CachedProof,
    inserted_at: Instant,
}

/// Cumulative cache statistics.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of successful cache lookups.
    pub hits: u64,
    /// Number of cache misses (key absent or expired).
    pub misses: u64,
    /// Number of entries evicted due to capacity or TTL expiry.
    pub evictions: u64,
    /// Current number of live entries in the cache.
    pub current_size: usize,
}

/// Internal mutable state protected by a `Mutex`.
struct CacheInner {
    /// Key-value store for cached proofs.
    entries: HashMap<CacheKey, CacheEntry>,
    /// LRU order: front = oldest (evict first), back = most recently used.
    order: VecDeque<CacheKey>,
    /// Cumulative statistics.
    stats: CacheStats,
}

/// Thread-safe, LRU-evicting, TTL-aware proof cache.
///
/// The cache is safe to share across threads without external synchronization.
/// All public methods acquire an internal `Mutex` and can be called from any
/// thread.
///
/// # Usage
///
/// ```ignore
/// let cache = ProofCache::from_env();
/// let key = ProofCache::make_key(&model_hash, &features);
///
/// // Try cache first
/// if let Some(proof) = cache.get(&key) {
///     // use cached proof
/// } else {
///     // generate proof ...
///     cache.insert(key, CachedProof { ... });
/// }
/// ```
pub struct ProofCache {
    inner: Mutex<CacheInner>,
    max_entries: usize,
    ttl: Duration,
}

impl ProofCache {
    /// Create a new `ProofCache` with explicit capacity and TTL.
    pub fn new(max_entries: usize, ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                entries: HashMap::with_capacity(max_entries),
                order: VecDeque::with_capacity(max_entries),
                stats: CacheStats::default(),
            }),
            max_entries,
            ttl,
        }
    }

    /// Create a `ProofCache` configured from environment variables.
    ///
    /// - `PROOF_CACHE_SIZE` (default 1000)
    /// - `PROOF_CACHE_TTL_SECS` (default 3600)
    pub fn from_env() -> Self {
        let max_entries = std::env::var("PROOF_CACHE_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_CACHE_SIZE);

        let ttl_secs = std::env::var("PROOF_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TTL_SECS);

        Self::new(max_entries, Duration::from_secs(ttl_secs))
    }

    /// Compute a cache key from a model hash and a feature vector.
    ///
    /// The key is `SHA-256(model_hash || f64_le_bytes(features[0]) || f64_le_bytes(features[1]) || ...)`,
    /// where each `f64` is serialized as its little-endian 8-byte representation.
    pub fn make_key(model_hash: &[u8], features: &[f64]) -> CacheKey {
        let mut hasher = Sha256::new();
        hasher.update(model_hash);
        for f in features {
            hasher.update(f.to_le_bytes());
        }
        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result);
        key
    }

    /// Look up a proof by key. Returns `None` if the key is absent or the
    /// entry has expired. On hit the entry is promoted to most-recently-used.
    pub fn get(&self, key: &CacheKey) -> Option<CachedProof> {
        let mut inner = self.inner.lock().expect("ProofCache mutex poisoned");

        // Check existence and TTL validity in a two-step process to satisfy
        // the borrow checker (we cannot hold an immutable ref into `entries`
        // while also mutating `inner.stats`).
        let status = match inner.entries.get(key) {
            Some(entry) if entry.inserted_at.elapsed() < self.ttl => Some(entry.proof.clone()),
            Some(_) => None, // expired
            None => {
                inner.stats.misses += 1;
                return None;
            }
        };

        match status {
            Some(proof) => {
                // Valid entry -- promote in LRU order.
                inner.stats.hits += 1;
                Self::promote_key(&mut inner.order, key);
                Some(proof)
            }
            None => {
                // Entry exists but is expired -- remove it.
                inner.entries.remove(key);
                Self::remove_key_from_order(&mut inner.order, key);
                inner.stats.evictions += 1;
                inner.stats.current_size = inner.entries.len();
                inner.stats.misses += 1;
                None
            }
        }
    }

    /// Insert a proof into the cache. If the cache is at capacity, the
    /// least-recently-used entry is evicted first. A cache with zero capacity
    /// silently discards all inserts.
    pub fn insert(&self, key: CacheKey, proof: CachedProof) {
        if self.max_entries == 0 {
            return;
        }

        let mut inner = self.inner.lock().expect("ProofCache mutex poisoned");

        // If key already exists, update in place and promote.
        if inner.entries.contains_key(&key) {
            inner.entries.insert(
                key,
                CacheEntry {
                    proof,
                    inserted_at: Instant::now(),
                },
            );
            Self::promote_key(&mut inner.order, &key);
            inner.stats.current_size = inner.entries.len();
            return;
        }

        // Evict expired entries from the front (oldest-first heuristic).
        Self::evict_expired(&mut inner, self.ttl);

        // Evict LRU entries until we have room.
        while inner.entries.len() >= self.max_entries {
            if let Some(evict_key) = inner.order.pop_front() {
                inner.entries.remove(&evict_key);
                inner.stats.evictions += 1;
            } else {
                break;
            }
        }

        inner.entries.insert(
            key,
            CacheEntry {
                proof,
                inserted_at: Instant::now(),
            },
        );
        inner.order.push_back(key);
        inner.stats.current_size = inner.entries.len();
    }

    /// Return a snapshot of the current cache statistics.
    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.lock().expect("ProofCache mutex poisoned");
        inner.stats.clone()
    }

    // -- private helpers --

    /// Remove expired entries from the front of the order deque.
    fn evict_expired(inner: &mut CacheInner, ttl: Duration) {
        while let Some(&front_key) = inner.order.front() {
            if let Some(entry) = inner.entries.get(&front_key) {
                if entry.inserted_at.elapsed() >= ttl {
                    inner.entries.remove(&front_key);
                    inner.order.pop_front();
                    inner.stats.evictions += 1;
                } else {
                    break;
                }
            } else {
                // Stale order entry -- clean it up.
                inner.order.pop_front();
            }
        }
    }

    /// Move `key` to the back (most recently used) of the order deque.
    fn promote_key(order: &mut VecDeque<CacheKey>, key: &CacheKey) {
        Self::remove_key_from_order(order, key);
        order.push_back(*key);
    }

    /// Remove a single occurrence of `key` from the order deque.
    fn remove_key_from_order(order: &mut VecDeque<CacheKey>, key: &CacheKey) {
        if let Some(pos) = order.iter().position(|k| k == key) {
            order.remove(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Helper: create a dummy proof with a given class.
    fn dummy_proof(class: u32) -> CachedProof {
        CachedProof {
            predicted_class: class,
            proof_bytes: vec![0xDE, 0xAD],
            circuit_hash: vec![0xBE, 0xEF],
            public_inputs: vec![0xCA, 0xFE],
        }
    }

    // ---------------------------------------------------------------
    // Basic hit / miss
    // ---------------------------------------------------------------

    #[test]
    fn test_cache_hit() {
        let cache = ProofCache::new(10, Duration::from_secs(60));
        let key = ProofCache::make_key(b"model1", &[1.0, 2.0, 3.0]);

        cache.insert(key, dummy_proof(7));

        let result = cache.get(&key);
        assert!(result.is_some(), "Expected cache hit");
        assert_eq!(result.unwrap().predicted_class, 7);
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn test_cache_miss() {
        let cache = ProofCache::new(10, Duration::from_secs(60));
        let key = ProofCache::make_key(b"model1", &[1.0, 2.0, 3.0]);

        let result = cache.get(&key);
        assert!(result.is_none(), "Expected cache miss for absent key");
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn test_multiple_entries() {
        let cache = ProofCache::new(10, Duration::from_secs(60));

        for i in 0..5u32 {
            let key = ProofCache::make_key(b"m", &[i as f64]);
            cache.insert(key, dummy_proof(i));
        }

        for i in 0..5u32 {
            let key = ProofCache::make_key(b"m", &[i as f64]);
            let result = cache.get(&key);
            assert!(result.is_some(), "Expected hit for entry {}", i);
            assert_eq!(result.unwrap().predicted_class, i);
        }

        assert_eq!(cache.stats().hits, 5);
        assert_eq!(cache.stats().current_size, 5);
    }

    // ---------------------------------------------------------------
    // LRU eviction
    // ---------------------------------------------------------------

    #[test]
    fn test_eviction_when_full() {
        let cache = ProofCache::new(3, Duration::from_secs(60));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);
        let k3 = ProofCache::make_key(b"m", &[3.0]);
        let k4 = ProofCache::make_key(b"m", &[4.0]);

        cache.insert(k1, dummy_proof(1));
        cache.insert(k2, dummy_proof(2));
        cache.insert(k3, dummy_proof(3));

        // Cache is full (3/3). Inserting k4 should evict k1 (LRU).
        cache.insert(k4, dummy_proof(4));

        assert!(cache.get(&k1).is_none(), "k1 should have been evicted");
        assert!(cache.get(&k2).is_some(), "k2 should still be present");
        assert!(cache.get(&k3).is_some(), "k3 should still be present");
        assert!(cache.get(&k4).is_some(), "k4 should be present");

        let stats = cache.stats();
        assert_eq!(stats.evictions, 1, "Expected exactly 1 eviction");
        assert_eq!(stats.current_size, 3, "Cache should remain at max capacity");
    }

    #[test]
    fn test_lru_promotion_on_get() {
        // Verify that accessing an entry promotes it so a different entry
        // gets evicted instead.
        let cache = ProofCache::new(3, Duration::from_secs(60));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);
        let k3 = ProofCache::make_key(b"m", &[3.0]);
        let k4 = ProofCache::make_key(b"m", &[4.0]);

        cache.insert(k1, dummy_proof(1));
        cache.insert(k2, dummy_proof(2));
        cache.insert(k3, dummy_proof(3));

        // Touch k1 so it becomes most recently used.
        let _ = cache.get(&k1);

        // Insert k4 -- should evict k2 (now the LRU), not k1.
        cache.insert(k4, dummy_proof(4));

        assert!(
            cache.get(&k1).is_some(),
            "k1 was promoted and should survive"
        );
        assert!(
            cache.get(&k2).is_none(),
            "k2 should have been evicted as LRU"
        );
        assert!(cache.get(&k3).is_some(), "k3 should still be present");
        assert!(cache.get(&k4).is_some(), "k4 should be present");
    }

    #[test]
    fn test_multiple_evictions() {
        let cache = ProofCache::new(2, Duration::from_secs(60));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);
        let k3 = ProofCache::make_key(b"m", &[3.0]);
        let k4 = ProofCache::make_key(b"m", &[4.0]);

        cache.insert(k1, dummy_proof(1));
        cache.insert(k2, dummy_proof(2));
        cache.insert(k3, dummy_proof(3)); // evicts k1
        cache.insert(k4, dummy_proof(4)); // evicts k2

        assert!(cache.get(&k1).is_none());
        assert!(cache.get(&k2).is_none());
        assert!(cache.get(&k3).is_some());
        assert!(cache.get(&k4).is_some());
        assert_eq!(cache.stats().evictions, 2);
        assert_eq!(cache.stats().current_size, 2);
    }

    // ---------------------------------------------------------------
    // TTL expiry
    // ---------------------------------------------------------------

    #[test]
    fn test_ttl_expiry() {
        // Use a very short TTL so we can test expiry without long waits.
        let cache = ProofCache::new(10, Duration::from_millis(50));
        let key = ProofCache::make_key(b"model1", &[1.0]);

        cache.insert(key, dummy_proof(5));

        // Should be a hit immediately.
        assert!(cache.get(&key).is_some(), "Should hit before TTL");
        assert_eq!(cache.stats().hits, 1);

        // Wait for TTL to expire.
        thread::sleep(Duration::from_millis(80));

        assert!(cache.get(&key).is_none(), "Should miss after TTL expiry");
        assert_eq!(cache.stats().misses, 1);

        // The expired entry should have been removed from the map.
        assert_eq!(
            cache.stats().current_size,
            0,
            "Expired entry should be removed"
        );
    }

    #[test]
    fn test_ttl_expiry_frees_capacity() {
        let cache = ProofCache::new(2, Duration::from_millis(50));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);
        let k3 = ProofCache::make_key(b"m", &[3.0]);
        let k4 = ProofCache::make_key(b"m", &[4.0]);

        cache.insert(k1, dummy_proof(1));
        cache.insert(k2, dummy_proof(2));

        // Wait for TTL to expire.
        thread::sleep(Duration::from_millis(80));

        // Insert new entries -- expired ones should be cleaned up during insert.
        cache.insert(k3, dummy_proof(3));
        cache.insert(k4, dummy_proof(4));

        assert!(cache.get(&k3).is_some());
        assert!(cache.get(&k4).is_some());
        assert_eq!(cache.stats().current_size, 2);
    }

    // ---------------------------------------------------------------
    // Stats accuracy
    // ---------------------------------------------------------------

    #[test]
    fn test_stats_accuracy() {
        let cache = ProofCache::new(10, Duration::from_secs(60));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);

        // Initial state: everything zero.
        let s = cache.stats();
        assert_eq!(s.hits, 0);
        assert_eq!(s.misses, 0);
        assert_eq!(s.evictions, 0);
        assert_eq!(s.current_size, 0);

        // Miss on absent key.
        let _ = cache.get(&k1);
        assert_eq!(cache.stats().misses, 1);

        // Insert and hit.
        cache.insert(k1, dummy_proof(1));
        assert_eq!(cache.stats().current_size, 1);
        let _ = cache.get(&k1);
        let s = cache.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 1);

        // Another miss.
        let _ = cache.get(&k2);
        assert_eq!(cache.stats().misses, 2);

        // Insert second key.
        cache.insert(k2, dummy_proof(2));
        assert_eq!(cache.stats().current_size, 2);
    }

    #[test]
    fn test_stats_evictions_counted() {
        let cache = ProofCache::new(2, Duration::from_secs(60));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);
        let k3 = ProofCache::make_key(b"m", &[3.0]);
        let k4 = ProofCache::make_key(b"m", &[4.0]);

        cache.insert(k1, dummy_proof(1));
        cache.insert(k2, dummy_proof(2));
        cache.insert(k3, dummy_proof(3)); // evicts k1
        cache.insert(k4, dummy_proof(4)); // evicts k2

        assert_eq!(cache.stats().evictions, 2);
        assert_eq!(cache.stats().current_size, 2);
    }

    // ---------------------------------------------------------------
    // Thread safety
    // ---------------------------------------------------------------

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;

        let cache = Arc::new(ProofCache::new(1000, Duration::from_secs(60)));
        let mut handles = Vec::new();

        // Spawn 8 threads, each inserting 100 entries and reading them back.
        for t in 0..8u64 {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                let model_hash = t.to_le_bytes();
                for i in 0..100u64 {
                    let features = [i as f64, t as f64];
                    let key = ProofCache::make_key(&model_hash, &features);
                    cache.insert(key, dummy_proof(i as u32));
                    // Immediately read back.
                    let result = cache.get(&key);
                    assert!(result.is_some(), "Thread {} key {} should hit", t, i);
                    assert_eq!(result.unwrap().predicted_class, i as u32);
                }
            }));
        }

        for h in handles {
            h.join().expect("Thread panicked");
        }

        let s = cache.stats();
        // 8 threads * 100 inserts = 800 entries (cache holds 1000, no evictions).
        assert_eq!(s.current_size, 800);
        assert_eq!(s.evictions, 0);
        // 8 threads * 100 reads = 800 hits.
        assert_eq!(s.hits, 800);
    }

    #[test]
    fn test_concurrent_reads_and_writes() {
        use std::sync::Arc;

        let cache = Arc::new(ProofCache::new(50, Duration::from_secs(60)));
        let mut handles = Vec::new();

        // Writer threads fill the cache.
        for t in 0..4u8 {
            let c = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..30u32 {
                    let key = ProofCache::make_key(&[t], &[i as f64]);
                    c.insert(key, dummy_proof(t as u32 * 100 + i));
                }
            }));
        }

        // Reader threads try to read (some hits, some misses).
        for t in 0..4u8 {
            let c = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..30u32 {
                    let key = ProofCache::make_key(&[t], &[i as f64]);
                    let _ = c.get(&key); // may or may not hit
                }
            }));
        }

        for h in handles {
            h.join().expect("Thread panicked");
        }

        // Cache should not have more than max_entries.
        let stats = cache.stats();
        assert!(
            stats.current_size <= 50,
            "Cache size {} should not exceed capacity 50",
            stats.current_size
        );
    }

    // ---------------------------------------------------------------
    // Key generation
    // ---------------------------------------------------------------

    #[test]
    fn test_make_key_deterministic() {
        let k1 = ProofCache::make_key(b"model_abc", &[1.0, 2.0, 3.0]);
        let k2 = ProofCache::make_key(b"model_abc", &[1.0, 2.0, 3.0]);
        assert_eq!(k1, k2, "Same inputs should produce the same key");
    }

    #[test]
    fn test_make_key_different_features() {
        let k1 = ProofCache::make_key(b"model_abc", &[1.0, 2.0, 3.0]);
        let k2 = ProofCache::make_key(b"model_abc", &[1.0, 2.0, 4.0]);
        assert_ne!(k1, k2, "Different features should produce different keys");
    }

    #[test]
    fn test_make_key_different_models() {
        let k1 = ProofCache::make_key(b"model_a", &[1.0, 2.0]);
        let k2 = ProofCache::make_key(b"model_b", &[1.0, 2.0]);
        assert_ne!(
            k1, k2,
            "Different model hashes should produce different keys"
        );
    }

    #[test]
    fn test_make_key_empty_features() {
        let k1 = ProofCache::make_key(b"model_a", &[]);
        let k2 = ProofCache::make_key(b"model_a", &[]);
        assert_eq!(
            k1, k2,
            "Empty features should still produce a consistent key"
        );
    }

    // ---------------------------------------------------------------
    // Edge cases
    // ---------------------------------------------------------------

    #[test]
    fn test_insert_duplicate_key_updates() {
        let cache = ProofCache::new(10, Duration::from_secs(60));
        let key = ProofCache::make_key(b"m", &[1.0]);

        cache.insert(key, dummy_proof(1));
        cache.insert(key, dummy_proof(2));

        let result = cache.get(&key).expect("Key should exist");
        assert_eq!(result.predicted_class, 2, "Should return updated proof");
        assert_eq!(
            cache.stats().current_size,
            1,
            "Duplicate should not increase size"
        );
    }

    #[test]
    fn test_zero_capacity_cache() {
        // A cache with 0 max entries should not panic. Nothing can be stored.
        let cache = ProofCache::new(0, Duration::from_secs(60));
        let key = ProofCache::make_key(b"m", &[1.0]);

        cache.insert(key, dummy_proof(1));
        assert!(
            cache.get(&key).is_none(),
            "Zero-capacity cache should hold nothing"
        );
    }

    #[test]
    fn test_capacity_one() {
        let cache = ProofCache::new(1, Duration::from_secs(60));

        let k1 = ProofCache::make_key(b"m", &[1.0]);
        let k2 = ProofCache::make_key(b"m", &[2.0]);

        cache.insert(k1, dummy_proof(1));
        cache.insert(k2, dummy_proof(2));

        assert!(cache.get(&k1).is_none(), "k1 should have been evicted");
        assert!(cache.get(&k2).is_some(), "k2 should be present");
        assert_eq!(cache.stats().evictions, 1);
        assert_eq!(cache.stats().current_size, 1);
    }

    #[test]
    fn test_empty_proof_value() {
        let cache = ProofCache::new(10, Duration::from_secs(60));
        let key = ProofCache::make_key(b"m", &[1.0]);
        cache.insert(
            key,
            CachedProof {
                predicted_class: 0,
                proof_bytes: vec![],
                circuit_hash: vec![],
                public_inputs: vec![],
            },
        );

        let result = cache.get(&key);
        assert!(result.is_some());
        assert!(result.unwrap().proof_bytes.is_empty());
    }

    #[test]
    fn test_from_env_defaults() {
        // Clear env vars to test defaults.
        std::env::remove_var("PROOF_CACHE_SIZE");
        std::env::remove_var("PROOF_CACHE_TTL_SECS");

        let cache = ProofCache::from_env();
        assert_eq!(cache.max_entries, DEFAULT_CACHE_SIZE);
        assert_eq!(cache.ttl, Duration::from_secs(DEFAULT_TTL_SECS));
    }
}
