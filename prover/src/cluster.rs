//! Multi-Prover Cluster Support
//!
//! Enables multiple provers to coordinate and work together:
//! - Prover registration and discovery
//! - Load balancing across provers
//! - Job distribution based on capacity
//! - Health monitoring of peer provers
//!
//! ## Architecture
//!
//! Provers communicate via a simple gossip protocol over HTTP.
//! Each prover maintains a list of known peers and their status.
//!
//! ## Job Selection Strategy
//!
//! Jobs are assigned based on:
//! 1. Prover availability (not at capacity)
//! 2. Prover affinity (has program cached)
//! 3. Load balancing (spread work evenly)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};

/// Unique identifier for a prover in the cluster
pub type ProverId = String;

/// Cluster configuration
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// This prover's unique ID
    pub prover_id: ProverId,
    /// Address this prover listens on for cluster communication
    pub listen_addr: String,
    /// Seed peers to connect to on startup
    pub seed_peers: Vec<String>,
    /// How often to send heartbeats
    pub heartbeat_interval: Duration,
    /// How long before a peer is considered dead
    pub peer_timeout: Duration,
    /// Maximum jobs this prover can handle concurrently
    pub max_concurrent_jobs: usize,
    /// Enable cluster mode
    pub enabled: bool,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            prover_id: uuid::Uuid::new_v4().to_string(),
            listen_addr: "0.0.0.0:9091".to_string(),
            seed_peers: vec![],
            heartbeat_interval: Duration::from_secs(5),
            peer_timeout: Duration::from_secs(30),
            max_concurrent_jobs: 4,
            enabled: false,
        }
    }
}

/// Status of a prover in the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProverStatus {
    /// Prover's unique ID
    pub prover_id: ProverId,
    /// Address for cluster communication
    pub addr: String,
    /// Current number of active jobs
    pub active_jobs: usize,
    /// Maximum concurrent jobs
    pub max_jobs: usize,
    /// Programs this prover has cached (image IDs)
    pub cached_programs: Vec<String>,
    /// Timestamp of this status
    pub timestamp: u64,
    /// Is this prover healthy?
    pub healthy: bool,
}

impl ProverStatus {
    /// Check if prover has capacity for more jobs
    pub fn has_capacity(&self) -> bool {
        self.active_jobs < self.max_jobs && self.healthy
    }

    /// Calculate load as percentage
    pub fn load(&self) -> f64 {
        if self.max_jobs == 0 {
            return 100.0;
        }
        (self.active_jobs as f64 / self.max_jobs as f64) * 100.0
    }

    /// Check if prover has program cached
    pub fn has_program(&self, image_id: &str) -> bool {
        self.cached_programs.contains(&image_id.to_string())
    }
}

/// Peer info tracked by cluster
#[derive(Debug, Clone)]
struct PeerInfo {
    status: ProverStatus,
    last_seen: Instant,
}

/// Cluster message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClusterMessage {
    /// Heartbeat with prover status
    Heartbeat { status: ProverStatus },
    /// Request for peer list
    PeerListRequest,
    /// Response with known peers
    PeerListResponse { peers: Vec<String> },
    /// Job claim announcement (to prevent duplicates)
    JobClaimed { job_id: u64, prover_id: ProverId },
    /// Job completion announcement
    JobCompleted { job_id: u64, prover_id: ProverId },
}

/// Multi-prover cluster manager
pub struct ClusterManager {
    config: ClusterConfig,
    /// Known peers and their status
    peers: RwLock<HashMap<ProverId, PeerInfo>>,
    /// Jobs currently claimed by provers in cluster
    claimed_jobs: RwLock<HashMap<u64, ProverId>>,
    /// Our current status
    local_status: RwLock<ProverStatus>,
    /// HTTP client for peer communication
    client: reqwest::Client,
}

impl ClusterManager {
    /// Create a new cluster manager
    pub fn new(config: ClusterConfig) -> Arc<Self> {
        let local_status = ProverStatus {
            prover_id: config.prover_id.clone(),
            addr: config.listen_addr.clone(),
            active_jobs: 0,
            max_jobs: config.max_concurrent_jobs,
            cached_programs: vec![],
            timestamp: current_timestamp(),
            healthy: true,
        };

        Arc::new(Self {
            config,
            peers: RwLock::new(HashMap::new()),
            claimed_jobs: RwLock::new(HashMap::new()),
            local_status: RwLock::new(local_status),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        })
    }

    /// Start the cluster manager
    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Cluster mode disabled, running standalone");
            return Ok(());
        }

        info!(
            "Starting cluster manager: id={}, listen={}",
            self.config.prover_id, self.config.listen_addr
        );

        // Connect to seed peers
        for peer in &self.config.seed_peers {
            if let Err(e) = self.connect_to_peer(peer).await {
                warn!("Failed to connect to seed peer {}: {}", peer, e);
            }
        }

        // Start background tasks
        let manager = self.clone();
        tokio::spawn(async move {
            manager.heartbeat_loop().await;
        });

        let manager = self.clone();
        tokio::spawn(async move {
            manager.peer_cleanup_loop().await;
        });

        Ok(())
    }

    /// Connect to a peer
    async fn connect_to_peer(&self, addr: &str) -> anyhow::Result<()> {
        let url = format!("http://{}/cluster/peers", addr);
        let resp: ClusterMessage = self.client.get(&url).send().await?.json().await?;

        if let ClusterMessage::PeerListResponse { peers } = resp {
            for peer in peers {
                if peer != self.config.listen_addr {
                    self.discover_peer(&peer).await;
                }
            }
        }

        info!("Connected to peer: {}", addr);
        Ok(())
    }

    /// Discover a new peer
    async fn discover_peer(&self, addr: &str) {
        debug!("Discovering peer: {}", addr);
        // Will be populated when we receive their heartbeat
    }

    /// Heartbeat loop
    async fn heartbeat_loop(&self) {
        loop {
            tokio::time::sleep(self.config.heartbeat_interval).await;

            let status = self.local_status.read().await.clone();
            let message = ClusterMessage::Heartbeat { status };

            // Send heartbeat to all known peers
            let peers = self.peers.read().await;
            for (_, peer) in peers.iter() {
                let url = format!("http://{}/cluster/heartbeat", peer.status.addr);
                let msg = message.clone();

                let client = self.client.clone();
                tokio::spawn(async move {
                    if let Err(e) = client.post(&url).json(&msg).send().await {
                        debug!("Failed to send heartbeat to {}: {}", url, e);
                    }
                });
            }
        }
    }

    /// Peer cleanup loop - remove stale peers
    async fn peer_cleanup_loop(&self) {
        loop {
            tokio::time::sleep(self.config.peer_timeout / 2).await;

            let mut peers = self.peers.write().await;
            let now = Instant::now();
            let timeout = self.config.peer_timeout;

            peers.retain(|id, info| {
                let alive = now.duration_since(info.last_seen) < timeout;
                if !alive {
                    warn!("Peer {} timed out, removing", id);
                }
                alive
            });
        }
    }

    /// Handle incoming cluster message
    pub async fn handle_message(&self, message: ClusterMessage) -> Option<ClusterMessage> {
        match message {
            ClusterMessage::Heartbeat { status } => {
                self.update_peer(status).await;
                None
            }
            ClusterMessage::PeerListRequest => {
                let peers = self.peers.read().await;
                let addrs: Vec<String> = peers.values().map(|p| p.status.addr.clone()).collect();
                Some(ClusterMessage::PeerListResponse { peers: addrs })
            }
            ClusterMessage::JobClaimed { job_id, prover_id } => {
                self.record_claim(job_id, prover_id).await;
                None
            }
            ClusterMessage::JobCompleted { job_id, prover_id } => {
                self.record_completion(job_id, &prover_id).await;
                None
            }
            _ => None,
        }
    }

    /// Update peer status from heartbeat
    async fn update_peer(&self, status: ProverStatus) {
        let mut peers = self.peers.write().await;
        peers.insert(
            status.prover_id.clone(),
            PeerInfo {
                status,
                last_seen: Instant::now(),
            },
        );
    }

    /// Record a job claim
    async fn record_claim(&self, job_id: u64, prover_id: ProverId) {
        let mut claims = self.claimed_jobs.write().await;
        claims.insert(job_id, prover_id);
    }

    /// Record job completion
    async fn record_completion(&self, job_id: u64, _prover_id: &str) {
        let mut claims = self.claimed_jobs.write().await;
        claims.remove(&job_id);
    }

    /// Check if we should claim a job (cluster coordination)
    pub async fn should_claim_job(&self, job_id: u64, image_id: &str) -> bool {
        if !self.config.enabled {
            return true; // Standalone mode, always claim
        }

        // Check if already claimed by another prover
        let claims = self.claimed_jobs.read().await;
        if let Some(claimer) = claims.get(&job_id) {
            if *claimer != self.config.prover_id {
                debug!("Job {} already claimed by {}", job_id, claimer);
                return false;
            }
        }
        drop(claims);

        // Check if we're the best prover for this job
        let local = self.local_status.read().await;
        if !local.has_capacity() {
            debug!("No capacity for job {}", job_id);
            return false;
        }

        let local_has_cache = local.has_program(image_id);
        let local_load = local.load();
        drop(local);

        // Check if another prover is better suited
        let peers = self.peers.read().await;
        for (_, peer) in peers.iter() {
            if !peer.status.has_capacity() {
                continue;
            }

            let peer_has_cache = peer.status.has_program(image_id);
            let peer_load = peer.status.load();

            // Prefer prover with cache hit
            if peer_has_cache && !local_has_cache {
                debug!(
                    "Peer {} has program cached, deferring job {}",
                    peer.status.prover_id, job_id
                );
                return false;
            }

            // If same cache status, prefer lower load
            if peer_has_cache == local_has_cache && peer_load < local_load - 20.0 {
                debug!(
                    "Peer {} has lower load ({:.1}% vs {:.1}%), deferring job {}",
                    peer.status.prover_id, peer_load, local_load, job_id
                );
                return false;
            }
        }

        true
    }

    /// Announce that we claimed a job
    pub async fn announce_claim(&self, job_id: u64) {
        if !self.config.enabled {
            return;
        }

        let message = ClusterMessage::JobClaimed {
            job_id,
            prover_id: self.config.prover_id.clone(),
        };

        self.broadcast_message(message).await;

        // Update local state
        let mut status = self.local_status.write().await;
        status.active_jobs += 1;
    }

    /// Announce that we completed a job
    pub async fn announce_completion(&self, job_id: u64) {
        if !self.config.enabled {
            return;
        }

        let message = ClusterMessage::JobCompleted {
            job_id,
            prover_id: self.config.prover_id.clone(),
        };

        self.broadcast_message(message).await;

        // Update local state
        let mut status = self.local_status.write().await;
        if status.active_jobs > 0 {
            status.active_jobs -= 1;
        }
    }

    /// Broadcast a message to all peers
    async fn broadcast_message(&self, message: ClusterMessage) {
        let peers = self.peers.read().await;

        for (_, peer) in peers.iter() {
            let url = format!("http://{}/cluster/message", peer.status.addr);
            let msg = message.clone();
            let client = self.client.clone();

            tokio::spawn(async move {
                if let Err(e) = client.post(&url).json(&msg).send().await {
                    debug!("Failed to broadcast to {}: {}", url, e);
                }
            });
        }
    }

    /// Update local cached programs
    pub async fn update_cached_programs(&self, programs: Vec<String>) {
        let mut status = self.local_status.write().await;
        status.cached_programs = programs;
    }

    /// Get cluster statistics
    pub async fn get_stats(&self) -> ClusterStats {
        let peers = self.peers.read().await;
        let claims = self.claimed_jobs.read().await;
        let local = self.local_status.read().await;

        let total_capacity: usize = peers.values().map(|p| p.status.max_jobs).sum::<usize>()
            + local.max_jobs;

        let total_active: usize = peers.values().map(|p| p.status.active_jobs).sum::<usize>()
            + local.active_jobs;

        ClusterStats {
            prover_id: self.config.prover_id.clone(),
            peer_count: peers.len(),
            total_capacity,
            total_active_jobs: total_active,
            claimed_jobs: claims.len(),
            cluster_load: if total_capacity > 0 {
                (total_active as f64 / total_capacity as f64) * 100.0
            } else {
                0.0
            },
        }
    }

    /// List all peers
    pub async fn list_peers(&self) -> Vec<ProverStatus> {
        let peers = self.peers.read().await;
        peers.values().map(|p| p.status.clone()).collect()
    }
}

/// Cluster statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStats {
    pub prover_id: ProverId,
    pub peer_count: usize,
    pub total_capacity: usize,
    pub total_active_jobs: usize,
    pub claimed_jobs: usize,
    pub cluster_load: f64,
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prover_status_capacity() {
        let status = ProverStatus {
            prover_id: "test".to_string(),
            addr: "127.0.0.1:9091".to_string(),
            active_jobs: 2,
            max_jobs: 4,
            cached_programs: vec!["prog1".to_string()],
            timestamp: 0,
            healthy: true,
        };

        assert!(status.has_capacity());
        assert_eq!(status.load(), 50.0);
        assert!(status.has_program("prog1"));
        assert!(!status.has_program("prog2"));
    }

    #[test]
    fn test_prover_status_no_capacity() {
        let status = ProverStatus {
            prover_id: "test".to_string(),
            addr: "127.0.0.1:9091".to_string(),
            active_jobs: 4,
            max_jobs: 4,
            cached_programs: vec![],
            timestamp: 0,
            healthy: true,
        };

        assert!(!status.has_capacity());
        assert_eq!(status.load(), 100.0);
    }

    #[test]
    fn test_prover_status_unhealthy() {
        let status = ProverStatus {
            prover_id: "test".to_string(),
            addr: "127.0.0.1:9091".to_string(),
            active_jobs: 0,
            max_jobs: 4,
            cached_programs: vec![],
            timestamp: 0,
            healthy: false,
        };

        assert!(!status.has_capacity()); // Unhealthy means no capacity
    }

    #[tokio::test]
    async fn test_cluster_manager_creation() {
        let config = ClusterConfig::default();
        let manager = ClusterManager::new(config);

        let stats = manager.get_stats().await;
        assert_eq!(stats.peer_count, 0);
        assert_eq!(stats.total_active_jobs, 0);
    }

    #[tokio::test]
    async fn test_should_claim_standalone() {
        let config = ClusterConfig {
            enabled: false,
            ..Default::default()
        };
        let manager = ClusterManager::new(config);

        // Standalone mode always claims
        assert!(manager.should_claim_job(1, "image1").await);
    }

    #[tokio::test]
    async fn test_claim_coordination() {
        let config = ClusterConfig {
            enabled: true,
            max_concurrent_jobs: 2,
            ..Default::default()
        };
        let manager = ClusterManager::new(config);

        // First claim should succeed
        assert!(manager.should_claim_job(1, "image1").await);
        manager.announce_claim(1).await;

        // Check stats updated
        let status = manager.local_status.read().await;
        assert_eq!(status.active_jobs, 1);
    }

    #[tokio::test]
    async fn test_peer_update() {
        let config = ClusterConfig::default();
        let manager = ClusterManager::new(config);

        let peer_status = ProverStatus {
            prover_id: "peer1".to_string(),
            addr: "192.168.1.2:9091".to_string(),
            active_jobs: 1,
            max_jobs: 4,
            cached_programs: vec!["prog1".to_string()],
            timestamp: current_timestamp(),
            healthy: true,
        };

        manager.update_peer(peer_status).await;

        let peers = manager.list_peers().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].prover_id, "peer1");
    }

    #[tokio::test]
    async fn test_cached_programs_update() {
        let config = ClusterConfig::default();
        let manager = ClusterManager::new(config);

        manager
            .update_cached_programs(vec!["img1".to_string(), "img2".to_string()])
            .await;

        let status = manager.local_status.read().await;
        assert_eq!(status.cached_programs.len(), 2);
    }

    #[test]
    fn test_cluster_message_serialization() {
        let msg = ClusterMessage::JobClaimed {
            job_id: 123,
            prover_id: "prover1".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("JobClaimed"));
        assert!(json.contains("123"));

        let parsed: ClusterMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            ClusterMessage::JobClaimed { job_id, prover_id } => {
                assert_eq!(job_id, 123);
                assert_eq!(prover_id, "prover1");
            }
            _ => panic!("Wrong message type"),
        }
    }
}
