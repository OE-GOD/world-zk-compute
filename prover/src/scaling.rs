//! Horizontal scaling support for prover clusters.
//!
//! This module provides:
//! - Worker registration and heartbeats
//! - Leader election for coordination tasks
//! - Load balancing across workers
//! - Health monitoring and failover

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

// ============================================================================
// Types
// ============================================================================

/// Worker status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    Starting,
    Healthy,
    Degraded,
    Unhealthy,
    Draining,
    Stopped,
}

/// Worker capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerCapabilities {
    /// Maximum concurrent jobs
    pub max_concurrent: u32,
    /// Supported image IDs (empty = all)
    pub supported_images: Vec<String>,
    /// Has GPU acceleration
    pub has_gpu: bool,
    /// GPU memory in GB (if has_gpu)
    pub gpu_memory_gb: u32,
    /// RAM in GB
    pub ram_gb: u32,
    /// CPU cores
    pub cpu_cores: u32,
    /// Supports Bonsai proving
    pub supports_bonsai: bool,
    /// Geographic region
    pub region: Option<String>,
}

impl Default for WorkerCapabilities {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            supported_images: vec![],
            has_gpu: false,
            gpu_memory_gb: 0,
            ram_gb: 16,
            cpu_cores: 4,
            supports_bonsai: true,
            region: None,
        }
    }
}

/// Worker load metrics
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WorkerLoad {
    /// Current active jobs
    pub active_jobs: u32,
    /// Jobs completed in last minute
    pub jobs_per_minute: f64,
    /// Average proof time (ms)
    pub avg_proof_time_ms: u64,
    /// CPU utilization (0-100)
    pub cpu_percent: f32,
    /// Memory utilization (0-100)
    pub memory_percent: f32,
    /// GPU utilization (0-100, if available)
    pub gpu_percent: Option<f32>,
}

impl WorkerLoad {
    /// Calculate load score (0-100, lower is better)
    pub fn score(&self, capabilities: &WorkerCapabilities) -> f32 {
        let job_ratio = if capabilities.max_concurrent > 0 {
            (self.active_jobs as f32 / capabilities.max_concurrent as f32) * 100.0
        } else {
            100.0
        };

        // Weighted average of different metrics
        let weights = [
            (job_ratio, 0.5),
            (self.cpu_percent, 0.25),
            (self.memory_percent, 0.15),
            (self.gpu_percent.unwrap_or(0.0), 0.1),
        ];

        weights.iter().map(|(val, weight)| val * weight).sum()
    }
}

/// Worker registration info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    /// Unique worker ID
    pub id: String,
    /// Worker endpoint (for P2P communication)
    pub endpoint: String,
    /// Worker status
    pub status: WorkerStatus,
    /// Worker capabilities
    pub capabilities: WorkerCapabilities,
    /// Current load
    pub load: WorkerLoad,
    /// Assigned shards (if sharding enabled)
    pub assigned_shards: Vec<u32>,
    /// Registration timestamp
    pub registered_at: u64,
    /// Last heartbeat timestamp
    pub last_heartbeat: u64,
    /// Version info
    pub version: String,
}

impl WorkerInfo {
    /// Create a new worker info
    pub fn new(id: String, endpoint: String, capabilities: WorkerCapabilities) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            id,
            endpoint,
            status: WorkerStatus::Starting,
            capabilities,
            load: WorkerLoad::default(),
            assigned_shards: vec![],
            registered_at: now,
            last_heartbeat: now,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Check if worker is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self.status, WorkerStatus::Healthy | WorkerStatus::Degraded)
    }

    /// Check if heartbeat is stale
    pub fn is_stale(&self, timeout_secs: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now - self.last_heartbeat > timeout_secs
    }

    /// Check if worker can handle a job
    pub fn can_handle(&self, image_id: &str) -> bool {
        if !self.is_healthy() {
            return false;
        }

        if self.load.active_jobs >= self.capabilities.max_concurrent {
            return false;
        }

        // Check if worker supports this image
        if !self.capabilities.supported_images.is_empty() {
            return self.capabilities.supported_images.contains(&image_id.to_string());
        }

        true
    }
}

// ============================================================================
// Cluster Coordinator
// ============================================================================

/// Configuration for cluster coordination
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Heartbeat interval (seconds)
    pub heartbeat_interval: u64,
    /// Heartbeat timeout (seconds)
    pub heartbeat_timeout: u64,
    /// Leader lease duration (seconds)
    pub leader_lease_duration: u64,
    /// Number of shards for job distribution
    pub num_shards: u32,
    /// Enable automatic shard rebalancing
    pub auto_rebalance: bool,
    /// Minimum workers before rebalancing
    pub min_workers_for_rebalance: u32,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: 10,
            heartbeat_timeout: 30,
            leader_lease_duration: 60,
            num_shards: 16,
            auto_rebalance: true,
            min_workers_for_rebalance: 2,
        }
    }
}

/// Cluster coordinator for managing workers
pub struct ClusterCoordinator {
    config: ClusterConfig,
    /// This worker's info
    self_info: Arc<RwLock<WorkerInfo>>,
    /// All known workers
    workers: Arc<RwLock<HashMap<String, WorkerInfo>>>,
    /// Current leader ID
    leader_id: Arc<RwLock<Option<String>>>,
    /// Leader lease expiry
    leader_lease_expiry: Arc<RwLock<u64>>,
}

impl ClusterCoordinator {
    /// Create a new cluster coordinator
    pub fn new(config: ClusterConfig, self_info: WorkerInfo) -> Self {
        let self_id = self_info.id.clone();
        let mut workers = HashMap::new();
        workers.insert(self_id, self_info.clone());

        Self {
            config,
            self_info: Arc::new(RwLock::new(self_info)),
            workers: Arc::new(RwLock::new(workers)),
            leader_id: Arc::new(RwLock::new(None)),
            leader_lease_expiry: Arc::new(RwLock::new(0)),
        }
    }

    /// Get this worker's ID
    pub async fn self_id(&self) -> String {
        self.self_info.read().await.id.clone()
    }

    /// Register a new worker
    pub async fn register_worker(&self, worker: WorkerInfo) {
        let mut workers = self.workers.write().await;
        workers.insert(worker.id.clone(), worker);

        // Trigger rebalance if needed
        if self.config.auto_rebalance {
            drop(workers);
            self.maybe_rebalance().await;
        }
    }

    /// Update worker heartbeat
    pub async fn heartbeat(&self, worker_id: &str, load: WorkerLoad) -> bool {
        let mut workers = self.workers.write().await;

        if let Some(worker) = workers.get_mut(worker_id) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            worker.last_heartbeat = now;
            worker.load = load;

            // Update status based on load
            worker.status = if load.cpu_percent > 90.0 || load.memory_percent > 90.0 {
                WorkerStatus::Degraded
            } else {
                WorkerStatus::Healthy
            };

            return true;
        }

        false
    }

    /// Remove a worker
    pub async fn remove_worker(&self, worker_id: &str) {
        let mut workers = self.workers.write().await;
        workers.remove(worker_id);

        // Check if removed worker was leader
        let leader = self.leader_id.read().await;
        if leader.as_ref() == Some(&worker_id.to_string()) {
            drop(leader);
            let mut leader = self.leader_id.write().await;
            *leader = None;
        }

        drop(workers);

        // Trigger rebalance
        if self.config.auto_rebalance {
            self.maybe_rebalance().await;
        }
    }

    /// Get all healthy workers
    pub async fn healthy_workers(&self) -> Vec<WorkerInfo> {
        let workers = self.workers.read().await;
        workers
            .values()
            .filter(|w| w.is_healthy() && !w.is_stale(self.config.heartbeat_timeout))
            .cloned()
            .collect()
    }

    /// Get worker count
    pub async fn worker_count(&self) -> usize {
        self.workers.read().await.len()
    }

    /// Check and clean up stale workers
    pub async fn cleanup_stale_workers(&self) -> Vec<String> {
        let mut workers = self.workers.write().await;
        let timeout = self.config.heartbeat_timeout;

        let stale: Vec<String> = workers
            .iter()
            .filter(|(_, w)| w.is_stale(timeout))
            .map(|(id, _)| id.clone())
            .collect();

        for id in &stale {
            workers.remove(id);
        }

        stale
    }

    // ========================================================================
    // Leader Election
    // ========================================================================

    /// Try to become the leader
    pub async fn try_become_leader(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut leader_id = self.leader_id.write().await;
        let mut lease_expiry = self.leader_lease_expiry.write().await;

        // Check if current lease is expired
        if *lease_expiry > now && leader_id.is_some() {
            return false;
        }

        // Become leader
        let self_id = self.self_info.read().await.id.clone();
        *leader_id = Some(self_id);
        *lease_expiry = now + self.config.leader_lease_duration;

        tracing::info!("Became cluster leader");
        true
    }

    /// Renew leader lease
    pub async fn renew_leader_lease(&self) -> bool {
        let self_id = self.self_info.read().await.id.clone();
        let leader_id = self.leader_id.read().await;

        if leader_id.as_ref() != Some(&self_id) {
            return false;
        }

        drop(leader_id);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut lease_expiry = self.leader_lease_expiry.write().await;
        *lease_expiry = now + self.config.leader_lease_duration;

        true
    }

    /// Check if this worker is the leader
    pub async fn is_leader(&self) -> bool {
        let self_id = self.self_info.read().await.id.clone();
        let leader_id = self.leader_id.read().await;
        let lease_expiry = self.leader_lease_expiry.read().await;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        leader_id.as_ref() == Some(&self_id) && *lease_expiry > now
    }

    /// Get current leader ID
    pub async fn current_leader(&self) -> Option<String> {
        let lease_expiry = self.leader_lease_expiry.read().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if *lease_expiry <= now {
            return None;
        }

        self.leader_id.read().await.clone()
    }

    // ========================================================================
    // Load Balancing
    // ========================================================================

    /// Select best worker for a job
    pub async fn select_worker(&self, image_id: &str, prefer_region: Option<&str>) -> Option<WorkerInfo> {
        let workers = self.workers.read().await;

        let mut candidates: Vec<_> = workers
            .values()
            .filter(|w| w.can_handle(image_id))
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // Sort by load score (lower is better)
        candidates.sort_by(|a, b| {
            let score_a = a.load.score(&a.capabilities);
            let score_b = b.load.score(&b.capabilities);

            // Prefer workers in the requested region
            let region_bonus_a = if prefer_region.is_some()
                && a.capabilities.region.as_deref() == prefer_region
            {
                -20.0
            } else {
                0.0
            };
            let region_bonus_b = if prefer_region.is_some()
                && b.capabilities.region.as_deref() == prefer_region
            {
                -20.0
            } else {
                0.0
            };

            (score_a + region_bonus_a)
                .partial_cmp(&(score_b + region_bonus_b))
                .unwrap()
        });

        candidates.first().cloned().cloned()
    }

    /// Get load distribution across workers
    pub async fn load_distribution(&self) -> HashMap<String, f32> {
        let workers = self.workers.read().await;

        workers
            .iter()
            .map(|(id, w)| (id.clone(), w.load.score(&w.capabilities)))
            .collect()
    }

    // ========================================================================
    // Shard Management
    // ========================================================================

    /// Rebalance shards across workers
    pub async fn maybe_rebalance(&self) {
        if !self.is_leader().await {
            return;
        }

        let mut workers = self.workers.write().await;
        let healthy_count = workers
            .values()
            .filter(|w| w.is_healthy())
            .count();

        if healthy_count < self.config.min_workers_for_rebalance as usize {
            return;
        }

        // Calculate shards per worker
        let shards_per_worker = self.config.num_shards / healthy_count as u32;
        let extra_shards = self.config.num_shards % healthy_count as u32;

        // Assign shards to healthy workers
        let mut shard = 0u32;
        let mut extra_given = 0u32;

        for worker in workers.values_mut() {
            if !worker.is_healthy() {
                worker.assigned_shards.clear();
                continue;
            }

            let mut num_shards = shards_per_worker;
            if extra_given < extra_shards {
                num_shards += 1;
                extra_given += 1;
            }

            worker.assigned_shards = (shard..shard + num_shards).collect();
            shard += num_shards;
        }

        tracing::info!(
            healthy_workers = healthy_count,
            total_shards = self.config.num_shards,
            "Rebalanced shards"
        );
    }

    /// Get shards assigned to this worker
    pub async fn assigned_shards(&self) -> Vec<u32> {
        self.self_info.read().await.assigned_shards.clone()
    }

    /// Update this worker's shard assignment
    pub async fn set_assigned_shards(&self, shards: Vec<u32>) {
        let mut info = self.self_info.write().await;
        info.assigned_shards = shards.clone();

        // Also update in workers map
        let mut workers = self.workers.write().await;
        if let Some(w) = workers.get_mut(&info.id) {
            w.assigned_shards = shards;
        }
    }
}

// ============================================================================
// Worker Pool
// ============================================================================

/// Configuration for worker pool
#[derive(Debug, Clone)]
pub struct WorkerPoolConfig {
    /// Maximum workers in pool
    pub max_workers: u32,
    /// Minimum workers to maintain
    pub min_workers: u32,
    /// Scale up threshold (load score)
    pub scale_up_threshold: f32,
    /// Scale down threshold (load score)
    pub scale_down_threshold: f32,
    /// Cooldown between scaling events (seconds)
    pub scale_cooldown: u64,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            max_workers: 10,
            min_workers: 1,
            scale_up_threshold: 80.0,
            scale_down_threshold: 20.0,
            scale_cooldown: 300,
        }
    }
}

/// Autoscaling recommendation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleRecommendation {
    ScaleUp(u32),
    ScaleDown(u32),
    NoChange,
}

/// Worker pool for autoscaling
pub struct WorkerPool {
    config: WorkerPoolConfig,
    coordinator: Arc<ClusterCoordinator>,
    last_scale_time: Arc<RwLock<u64>>,
}

impl WorkerPool {
    /// Create a new worker pool
    pub fn new(config: WorkerPoolConfig, coordinator: Arc<ClusterCoordinator>) -> Self {
        Self {
            config,
            coordinator,
            last_scale_time: Arc::new(RwLock::new(0)),
        }
    }

    /// Get scaling recommendation
    pub async fn recommend_scaling(&self) -> ScaleRecommendation {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Check cooldown
        let last_scale = *self.last_scale_time.read().await;
        if now - last_scale < self.config.scale_cooldown {
            return ScaleRecommendation::NoChange;
        }

        let workers = self.coordinator.healthy_workers().await;
        let worker_count = workers.len() as u32;

        if worker_count == 0 {
            return ScaleRecommendation::ScaleUp(self.config.min_workers);
        }

        // Calculate average load
        let avg_load: f32 = workers
            .iter()
            .map(|w| w.load.score(&w.capabilities))
            .sum::<f32>()
            / worker_count as f32;

        if avg_load > self.config.scale_up_threshold && worker_count < self.config.max_workers {
            let new_workers = (worker_count / 2).max(1).min(self.config.max_workers - worker_count);
            return ScaleRecommendation::ScaleUp(new_workers);
        }

        if avg_load < self.config.scale_down_threshold && worker_count > self.config.min_workers {
            let remove_workers = (worker_count / 4).max(1).min(worker_count - self.config.min_workers);
            return ScaleRecommendation::ScaleDown(remove_workers);
        }

        ScaleRecommendation::NoChange
    }

    /// Record a scaling event
    pub async fn record_scale_event(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut last_scale = self.last_scale_time.write().await;
        *last_scale = now;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_worker(id: &str) -> WorkerInfo {
        WorkerInfo::new(
            id.to_string(),
            format!("http://worker-{}.local:8080", id),
            WorkerCapabilities::default(),
        )
    }

    #[tokio::test]
    async fn test_worker_registration() {
        let config = ClusterConfig::default();
        let self_info = create_test_worker("worker-1");
        let coordinator = ClusterCoordinator::new(config, self_info);

        // Register another worker
        let worker2 = create_test_worker("worker-2");
        coordinator.register_worker(worker2).await;

        assert_eq!(coordinator.worker_count().await, 2);
    }

    #[tokio::test]
    async fn test_leader_election() {
        let config = ClusterConfig::default();
        let self_info = create_test_worker("worker-1");
        let coordinator = ClusterCoordinator::new(config, self_info);

        // Initially no leader
        assert!(coordinator.current_leader().await.is_none());

        // Become leader
        assert!(coordinator.try_become_leader().await);
        assert!(coordinator.is_leader().await);
        assert_eq!(coordinator.current_leader().await, Some("worker-1".to_string()));

        // Can renew lease
        assert!(coordinator.renew_leader_lease().await);
    }

    #[tokio::test]
    async fn test_worker_selection() {
        let config = ClusterConfig::default();
        let mut self_info = create_test_worker("worker-1");
        self_info.status = WorkerStatus::Healthy;
        let coordinator = ClusterCoordinator::new(config, self_info);

        // Add another worker with lower load
        let mut worker2 = create_test_worker("worker-2");
        worker2.status = WorkerStatus::Healthy;
        worker2.load.active_jobs = 0;
        coordinator.register_worker(worker2).await;

        // Update first worker to have higher load
        coordinator
            .heartbeat("worker-1", WorkerLoad {
                active_jobs: 3,
                cpu_percent: 80.0,
                ..Default::default()
            })
            .await;

        // Worker 2 should be selected (lower load)
        let selected = coordinator.select_worker("0x1234", None).await;
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().id, "worker-2");
    }

    #[tokio::test]
    async fn test_shard_rebalancing() {
        let config = ClusterConfig {
            num_shards: 8,
            auto_rebalance: true,
            min_workers_for_rebalance: 2,
            ..Default::default()
        };

        let mut self_info = create_test_worker("worker-1");
        self_info.status = WorkerStatus::Healthy;
        let coordinator = ClusterCoordinator::new(config, self_info);

        // Become leader
        coordinator.try_become_leader().await;

        // Add another healthy worker
        let mut worker2 = create_test_worker("worker-2");
        worker2.status = WorkerStatus::Healthy;
        coordinator.register_worker(worker2).await;

        // Trigger rebalance
        coordinator.maybe_rebalance().await;

        // Both workers should have shards assigned
        let workers = coordinator.healthy_workers().await;
        for worker in workers {
            assert!(!worker.assigned_shards.is_empty());
        }
    }

    #[tokio::test]
    async fn test_autoscaling_recommendation() {
        let config = ClusterConfig::default();
        let mut self_info = create_test_worker("worker-1");
        self_info.status = WorkerStatus::Healthy;
        self_info.load = WorkerLoad {
            active_jobs: 4,
            cpu_percent: 90.0,
            memory_percent: 85.0,
            ..Default::default()
        };

        let coordinator = Arc::new(ClusterCoordinator::new(config, self_info));

        let pool_config = WorkerPoolConfig::default();
        let pool = WorkerPool::new(pool_config, coordinator);

        // High load should recommend scale up
        let recommendation = pool.recommend_scaling().await;
        assert!(matches!(recommendation, ScaleRecommendation::ScaleUp(_)));
    }

    #[tokio::test]
    async fn test_stale_worker_cleanup() {
        let config = ClusterConfig {
            heartbeat_timeout: 0, // Immediate timeout for testing
            ..Default::default()
        };

        let self_info = create_test_worker("worker-1");
        let coordinator = ClusterCoordinator::new(config, self_info);

        // Add a worker with old heartbeat (will be immediately stale)
        let mut worker2 = create_test_worker("worker-2");
        worker2.last_heartbeat = 1; // Very old (1970)
        coordinator.register_worker(worker2).await;

        // Cleanup should remove worker-2 (heartbeat_timeout=0 means always stale)
        let stale = coordinator.cleanup_stale_workers().await;
        assert!(stale.contains(&"worker-2".to_string()));
        assert_eq!(coordinator.worker_count().await, 1);
    }
}
