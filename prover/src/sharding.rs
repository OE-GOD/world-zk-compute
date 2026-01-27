//! Job sharding and geographic routing for distributed proving.
//!
//! This module provides:
//! - Consistent hashing for job distribution
//! - Image ID-based sharding
//! - Geographic routing to minimize latency
//! - Shard migration and rebalancing

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Consistent Hashing
// ============================================================================

/// A point on the hash ring
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RingPoint {
    hash: u64,
    node_id: String,
    virtual_node: u32,
}

/// Consistent hash ring for job distribution
pub struct ConsistentHashRing {
    /// Points on the ring (sorted by hash)
    ring: BTreeMap<u64, String>,
    /// Number of virtual nodes per real node
    virtual_nodes: u32,
    /// Nodes in the ring
    nodes: HashSet<String>,
}

impl ConsistentHashRing {
    /// Create a new hash ring
    pub fn new(virtual_nodes: u32) -> Self {
        Self {
            ring: BTreeMap::new(),
            virtual_nodes: virtual_nodes.max(1),
            nodes: HashSet::new(),
        }
    }

    /// Add a node to the ring
    pub fn add_node(&mut self, node_id: &str) {
        if self.nodes.contains(node_id) {
            return;
        }

        self.nodes.insert(node_id.to_string());

        for i in 0..self.virtual_nodes {
            let hash = self.hash_key(&format!("{}:{}", node_id, i));
            self.ring.insert(hash, node_id.to_string());
        }
    }

    /// Remove a node from the ring
    pub fn remove_node(&mut self, node_id: &str) {
        if !self.nodes.remove(node_id) {
            return;
        }

        for i in 0..self.virtual_nodes {
            let hash = self.hash_key(&format!("{}:{}", node_id, i));
            self.ring.remove(&hash);
        }
    }

    /// Get the node responsible for a key
    pub fn get_node(&self, key: &str) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }

        let hash = self.hash_key(key);

        // Find the first node with hash >= key hash
        if let Some((_, node_id)) = self.ring.range(hash..).next() {
            return Some(node_id);
        }

        // Wrap around to the first node
        self.ring.values().next().map(|s| s.as_str())
    }

    /// Get N nodes for a key (for replication)
    pub fn get_nodes(&self, key: &str, count: usize) -> Vec<String> {
        if self.ring.is_empty() {
            return vec![];
        }

        let mut result = Vec::with_capacity(count);
        let mut seen = HashSet::new();
        let hash = self.hash_key(key);

        // Start from the key's position and walk around the ring
        for (_, node_id) in self.ring.range(hash..).chain(self.ring.iter()) {
            if seen.insert(node_id.clone()) {
                result.push(node_id.clone());
                if result.len() >= count {
                    break;
                }
            }
        }

        result
    }

    /// Hash a key to a ring position
    fn hash_key(&self, key: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish()
    }

    /// Get all nodes
    pub fn all_nodes(&self) -> Vec<String> {
        self.nodes.iter().cloned().collect()
    }

    /// Get node count
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// ============================================================================
// Shard Manager
// ============================================================================

/// Shard configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardConfig {
    /// Total number of shards
    pub num_shards: u32,
    /// Replication factor
    pub replication_factor: u32,
    /// Virtual nodes per worker
    pub virtual_nodes: u32,
    /// Shard assignment strategy
    pub strategy: ShardStrategy,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            num_shards: 256,
            replication_factor: 2,
            virtual_nodes: 150,
            strategy: ShardStrategy::ConsistentHash,
        }
    }
}

/// Shard assignment strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShardStrategy {
    /// Consistent hashing (recommended)
    ConsistentHash,
    /// Range-based (simple but less balanced)
    Range,
    /// Round-robin (good for uniform distribution)
    RoundRobin,
}

/// Shard assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardAssignment {
    pub shard_id: u32,
    pub primary: String,
    pub replicas: Vec<String>,
}

/// Shard manager for distributing jobs
pub struct ShardManager {
    config: ShardConfig,
    hash_ring: Arc<RwLock<ConsistentHashRing>>,
    assignments: Arc<RwLock<HashMap<u32, ShardAssignment>>>,
    worker_shards: Arc<RwLock<HashMap<String, Vec<u32>>>>,
}

impl ShardManager {
    /// Create a new shard manager
    pub fn new(config: ShardConfig) -> Self {
        Self {
            hash_ring: Arc::new(RwLock::new(ConsistentHashRing::new(config.virtual_nodes))),
            assignments: Arc::new(RwLock::new(HashMap::new())),
            worker_shards: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Add a worker
    pub async fn add_worker(&self, worker_id: &str) {
        let mut ring = self.hash_ring.write().await;
        ring.add_node(worker_id);
        drop(ring);

        self.rebalance().await;
    }

    /// Remove a worker
    pub async fn remove_worker(&self, worker_id: &str) {
        let mut ring = self.hash_ring.write().await;
        ring.remove_node(worker_id);
        drop(ring);

        self.rebalance().await;
    }

    /// Get shard for a key (e.g., image_id)
    pub fn get_shard(&self, key: &str) -> u32 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() % self.config.num_shards as u64) as u32
    }

    /// Get worker responsible for a shard
    pub async fn get_worker_for_shard(&self, shard_id: u32) -> Option<String> {
        let assignments = self.assignments.read().await;
        assignments.get(&shard_id).map(|a| a.primary.clone())
    }

    /// Get worker for a key
    pub async fn get_worker_for_key(&self, key: &str) -> Option<String> {
        let shard = self.get_shard(key);
        self.get_worker_for_shard(shard).await
    }

    /// Get workers for a key (including replicas)
    pub async fn get_workers_for_key(&self, key: &str) -> Vec<String> {
        let shard = self.get_shard(key);
        let assignments = self.assignments.read().await;

        if let Some(assignment) = assignments.get(&shard) {
            let mut workers = vec![assignment.primary.clone()];
            workers.extend(assignment.replicas.clone());
            workers
        } else {
            vec![]
        }
    }

    /// Get shards assigned to a worker
    pub async fn get_worker_shards(&self, worker_id: &str) -> Vec<u32> {
        let worker_shards = self.worker_shards.read().await;
        worker_shards.get(worker_id).cloned().unwrap_or_default()
    }

    /// Rebalance shards across workers
    pub async fn rebalance(&self) {
        let ring = self.hash_ring.read().await;

        if ring.node_count() == 0 {
            return;
        }

        let mut assignments = HashMap::new();
        let mut worker_shards: HashMap<String, Vec<u32>> = HashMap::new();

        for shard_id in 0..self.config.num_shards {
            let key = format!("shard:{}", shard_id);
            let workers = ring.get_nodes(&key, self.config.replication_factor as usize + 1);

            if workers.is_empty() {
                continue;
            }

            let primary = workers[0].clone();
            let replicas: Vec<String> = workers.into_iter().skip(1).collect();

            // Track which shards each worker owns
            worker_shards
                .entry(primary.clone())
                .or_default()
                .push(shard_id);

            for replica in &replicas {
                worker_shards.entry(replica.clone()).or_default().push(shard_id);
            }

            assignments.insert(
                shard_id,
                ShardAssignment {
                    shard_id,
                    primary,
                    replicas,
                },
            );
        }

        *self.assignments.write().await = assignments;
        *self.worker_shards.write().await = worker_shards;

        tracing::info!(
            num_shards = self.config.num_shards,
            workers = ring.node_count(),
            "Rebalanced shards"
        );
    }

    /// Get shard distribution statistics
    pub async fn get_distribution_stats(&self) -> ShardDistributionStats {
        let worker_shards = self.worker_shards.read().await;

        let counts: Vec<usize> = worker_shards.values().map(|s| s.len()).collect();

        if counts.is_empty() {
            return ShardDistributionStats::default();
        }

        let min = *counts.iter().min().unwrap_or(&0);
        let max = *counts.iter().max().unwrap_or(&0);
        let avg = counts.iter().sum::<usize>() as f64 / counts.len() as f64;

        // Calculate standard deviation
        let variance = counts
            .iter()
            .map(|&c| {
                let diff = c as f64 - avg;
                diff * diff
            })
            .sum::<f64>()
            / counts.len() as f64;
        let std_dev = variance.sqrt();

        ShardDistributionStats {
            total_shards: self.config.num_shards,
            total_workers: worker_shards.len() as u32,
            min_shards_per_worker: min as u32,
            max_shards_per_worker: max as u32,
            avg_shards_per_worker: avg,
            std_deviation: std_dev,
        }
    }
}

/// Shard distribution statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShardDistributionStats {
    pub total_shards: u32,
    pub total_workers: u32,
    pub min_shards_per_worker: u32,
    pub max_shards_per_worker: u32,
    pub avg_shards_per_worker: f64,
    pub std_deviation: f64,
}

// ============================================================================
// Geographic Routing
// ============================================================================

/// Geographic region
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Region {
    UsEast,
    UsWest,
    EuWest,
    EuCentral,
    AsiaPacific,
    Custom(String),
}

impl Region {
    /// Parse region from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "us-east" | "us-east-1" | "us-east-2" => Region::UsEast,
            "us-west" | "us-west-1" | "us-west-2" => Region::UsWest,
            "eu-west" | "eu-west-1" | "eu-west-2" => Region::EuWest,
            "eu-central" | "eu-central-1" => Region::EuCentral,
            "ap" | "asia-pacific" | "ap-southeast-1" => Region::AsiaPacific,
            other => Region::Custom(other.to_string()),
        }
    }

    /// Get approximate latency to another region (ms)
    pub fn latency_to(&self, other: &Region) -> u32 {
        if self == other {
            return 5; // Same region
        }

        // Approximate cross-region latencies
        match (self, other) {
            (Region::UsEast, Region::UsWest) | (Region::UsWest, Region::UsEast) => 70,
            (Region::UsEast, Region::EuWest) | (Region::EuWest, Region::UsEast) => 80,
            (Region::UsEast, Region::EuCentral) | (Region::EuCentral, Region::UsEast) => 90,
            (Region::UsWest, Region::AsiaPacific) | (Region::AsiaPacific, Region::UsWest) => 150,
            (Region::EuWest, Region::EuCentral) | (Region::EuCentral, Region::EuWest) => 20,
            (Region::EuWest, Region::AsiaPacific) | (Region::AsiaPacific, Region::EuWest) => 200,
            _ => 150, // Default for unknown combinations
        }
    }
}

/// Worker with geographic information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoWorker {
    pub id: String,
    pub region: Region,
    pub latency_ms: u32, // Measured latency from this node
    pub capacity: u32,   // Available capacity (0-100)
}

/// Geographic router for selecting workers
pub struct GeoRouter {
    /// Workers by region
    workers: Arc<RwLock<HashMap<Region, Vec<GeoWorker>>>>,
    /// This node's region
    local_region: Region,
    /// Maximum acceptable latency
    max_latency_ms: u32,
}

impl GeoRouter {
    /// Create a new geo router
    pub fn new(local_region: Region, max_latency_ms: u32) -> Self {
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            local_region,
            max_latency_ms,
        }
    }

    /// Register a worker
    pub async fn register_worker(&self, worker: GeoWorker) {
        let mut workers = self.workers.write().await;
        workers
            .entry(worker.region.clone())
            .or_default()
            .push(worker);
    }

    /// Remove a worker
    pub async fn remove_worker(&self, worker_id: &str) {
        let mut workers = self.workers.write().await;
        for region_workers in workers.values_mut() {
            region_workers.retain(|w| w.id != worker_id);
        }
    }

    /// Update worker status
    pub async fn update_worker(&self, worker_id: &str, latency_ms: u32, capacity: u32) {
        let mut workers = self.workers.write().await;
        for region_workers in workers.values_mut() {
            if let Some(worker) = region_workers.iter_mut().find(|w| w.id == worker_id) {
                worker.latency_ms = latency_ms;
                worker.capacity = capacity;
            }
        }
    }

    /// Select best worker for a request
    pub async fn select_worker(&self, prefer_region: Option<&Region>) -> Option<GeoWorker> {
        let workers = self.workers.read().await;

        // Collect all candidates
        let mut candidates: Vec<(GeoWorker, u32)> = vec![];

        for (region, region_workers) in workers.iter() {
            let region_latency = self.local_region.latency_to(region);

            for worker in region_workers {
                let total_latency = region_latency + worker.latency_ms;

                if total_latency > self.max_latency_ms {
                    continue;
                }

                if worker.capacity == 0 {
                    continue;
                }

                candidates.push((worker.clone(), total_latency));
            }
        }

        if candidates.is_empty() {
            return None;
        }

        // Sort by preference
        candidates.sort_by(|(a, lat_a), (b, lat_b)| {
            // Prefer workers in requested region
            let region_pref_a = prefer_region
                .map(|r| if &a.region == r { 0 } else { 1 })
                .unwrap_or(0);
            let region_pref_b = prefer_region
                .map(|r| if &b.region == r { 0 } else { 1 })
                .unwrap_or(0);

            // Sort by: region preference, then latency, then capacity
            region_pref_a
                .cmp(&region_pref_b)
                .then(lat_a.cmp(lat_b))
                .then(b.capacity.cmp(&a.capacity))
        });

        candidates.first().map(|(w, _)| w.clone())
    }

    /// Get workers by region
    pub async fn workers_by_region(&self) -> HashMap<Region, Vec<GeoWorker>> {
        self.workers.read().await.clone()
    }

    /// Get total worker count
    pub async fn worker_count(&self) -> usize {
        self.workers
            .read()
            .await
            .values()
            .map(|v| v.len())
            .sum()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consistent_hash_ring() {
        let mut ring = ConsistentHashRing::new(100);

        ring.add_node("node-1");
        ring.add_node("node-2");
        ring.add_node("node-3");

        assert_eq!(ring.node_count(), 3);

        // Same key should always map to same node
        let key = "test-key";
        let node1 = ring.get_node(key).unwrap();
        let node2 = ring.get_node(key).unwrap();
        assert_eq!(node1, node2);

        // Different keys should distribute across nodes
        let mut distribution: HashMap<&str, u32> = HashMap::new();
        for i in 0..1000 {
            let key = format!("key-{}", i);
            if let Some(node) = ring.get_node(&key) {
                *distribution.entry(node).or_default() += 1;
            }
        }

        // All nodes should have some keys
        assert!(distribution.len() == 3);
        for count in distribution.values() {
            assert!(*count > 100); // At least 10% of keys
        }
    }

    #[test]
    fn test_consistent_hash_stability() {
        let mut ring = ConsistentHashRing::new(100);

        ring.add_node("node-1");
        ring.add_node("node-2");

        // Record assignments
        let mut assignments: HashMap<String, String> = HashMap::new();
        for i in 0..100 {
            let key = format!("key-{}", i);
            if let Some(node) = ring.get_node(&key) {
                assignments.insert(key, node.to_string());
            }
        }

        // Add a new node
        ring.add_node("node-3");

        // Most assignments should stay the same
        let mut changed = 0;
        for (key, old_node) in &assignments {
            if let Some(new_node) = ring.get_node(key) {
                if new_node != old_node {
                    changed += 1;
                }
            }
        }

        // Less than 50% should change (ideally ~33% for adding 1 of 3 nodes)
        assert!(changed < 50);
    }

    #[tokio::test]
    async fn test_shard_manager() {
        let config = ShardConfig {
            num_shards: 16,
            replication_factor: 1,
            virtual_nodes: 50,
            strategy: ShardStrategy::ConsistentHash,
        };

        let manager = ShardManager::new(config);

        manager.add_worker("worker-1").await;
        manager.add_worker("worker-2").await;

        // All shards should be assigned
        for shard in 0..16 {
            let worker = manager.get_worker_for_shard(shard).await;
            assert!(worker.is_some());
        }

        // Distribution should be roughly even
        let stats = manager.get_distribution_stats().await;
        assert_eq!(stats.total_shards, 16);
        assert_eq!(stats.total_workers, 2);
        assert!(stats.std_deviation < 5.0); // Reasonably balanced
    }

    #[tokio::test]
    async fn test_shard_key_routing() {
        let config = ShardConfig {
            num_shards: 256,
            ..Default::default()
        };

        let manager = ShardManager::new(config);
        manager.add_worker("worker-1").await;

        // Same key should always route to same shard
        let image_id = "0x1234567890abcdef";
        let shard1 = manager.get_shard(image_id);
        let shard2 = manager.get_shard(image_id);
        assert_eq!(shard1, shard2);

        // Different keys should distribute
        let mut shards: HashSet<u32> = HashSet::new();
        for i in 0..100 {
            let key = format!("0x{:064x}", i);
            shards.insert(manager.get_shard(&key));
        }
        // Should hit many different shards
        assert!(shards.len() > 50);
    }

    #[test]
    fn test_region_latency() {
        let us_east = Region::UsEast;
        let us_west = Region::UsWest;
        let eu_west = Region::EuWest;

        // Same region is fast
        assert_eq!(us_east.latency_to(&us_east), 5);

        // US regions are relatively close
        assert!(us_east.latency_to(&us_west) < 100);

        // Cross-Atlantic is medium
        assert!(us_east.latency_to(&eu_west) < 150);
    }

    #[tokio::test]
    async fn test_geo_router() {
        let router = GeoRouter::new(Region::UsEast, 200);

        // Add workers in different regions
        router
            .register_worker(GeoWorker {
                id: "worker-us-1".to_string(),
                region: Region::UsEast,
                latency_ms: 10,
                capacity: 80,
            })
            .await;

        router
            .register_worker(GeoWorker {
                id: "worker-eu-1".to_string(),
                region: Region::EuWest,
                latency_ms: 20,
                capacity: 90,
            })
            .await;

        // Should prefer local region
        let selected = router.select_worker(None).await.unwrap();
        assert_eq!(selected.region, Region::UsEast);

        // Can request specific region
        let selected = router.select_worker(Some(&Region::EuWest)).await.unwrap();
        assert_eq!(selected.region, Region::EuWest);
    }

    #[tokio::test]
    async fn test_geo_router_latency_filter() {
        let router = GeoRouter::new(Region::UsEast, 50); // Very strict latency

        // Add a distant worker
        router
            .register_worker(GeoWorker {
                id: "worker-ap-1".to_string(),
                region: Region::AsiaPacific,
                latency_ms: 50, // Total will be ~200ms
                capacity: 100,
            })
            .await;

        // Should not select due to latency
        let selected = router.select_worker(None).await;
        assert!(selected.is_none());
    }
}
