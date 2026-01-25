//! P2P Coordination for Decentralized Prover Network
//!
//! Enables provers to:
//! - Discover each other
//! - Coordinate job claiming (avoid duplicate work)
//! - Share proof computation (distributed proving)
//! - Gossip about network state

#![allow(dead_code)]

use alloy::primitives::{Address, B256, U256};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, info, warn};

// ============================================================================
// TYPES
// ============================================================================

/// Unique identifier for a prover in the network
pub type PeerId = String;

/// Message types for P2P communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum P2PMessage {
    /// Announce presence to the network
    Announce {
        peer_id: PeerId,
        address: Address,
        stake: U256,
        reputation: u64,
        capabilities: Vec<String>,
    },

    /// Intent to claim a job (for coordination)
    ClaimIntent {
        peer_id: PeerId,
        request_id: U256,
        timestamp: u64,
        priority_score: u64,
    },

    /// Job has been claimed on-chain
    JobClaimed {
        peer_id: PeerId,
        request_id: U256,
        tx_hash: B256,
    },

    /// Job completed with proof
    JobCompleted {
        peer_id: PeerId,
        request_id: U256,
        proof_hash: B256,
        cycles: u64,
        duration_ms: u64,
    },

    /// Request help with distributed proving
    ProofAssistRequest {
        peer_id: PeerId,
        request_id: U256,
        segment_index: u32,
        total_segments: u32,
        elf_hash: B256,
    },

    /// Offer to help with distributed proving
    ProofAssistOffer {
        peer_id: PeerId,
        request_id: U256,
        segment_index: u32,
        available_until: u64,
    },

    /// Heartbeat for liveness
    Heartbeat {
        peer_id: PeerId,
        timestamp: u64,
        active_jobs: u32,
        queue_depth: u32,
    },

    /// Peer leaving the network
    Goodbye {
        peer_id: PeerId,
        reason: String,
    },
}

/// Information about a peer
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub address: Address,
    pub stake: U256,
    pub reputation: u64,
    pub capabilities: Vec<String>,
    pub last_seen: Instant,
    pub active_jobs: u32,
    pub queue_depth: u32,
}

/// Claim coordination result
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimDecision {
    /// We should claim this job
    Proceed,
    /// Another prover has priority
    Defer { to_peer: PeerId },
    /// Multiple provers competing, use on-chain resolution
    Compete,
}

/// Configuration for P2P network
#[derive(Debug, Clone)]
pub struct P2PConfig {
    /// Our peer ID
    pub peer_id: PeerId,
    /// Our on-chain address
    pub address: Address,
    /// Our stake amount
    pub stake: U256,
    /// Our reputation score (0-10000)
    pub reputation: u64,
    /// Capabilities we support
    pub capabilities: Vec<String>,
    /// How long before a peer is considered dead
    pub peer_timeout: Duration,
    /// How often to send heartbeats
    pub heartbeat_interval: Duration,
    /// Coordination window for claim intents (ms)
    pub claim_coordination_window_ms: u64,
}

impl Default for P2PConfig {
    fn default() -> Self {
        Self {
            peer_id: format!("prover-{}", rand::random::<u32>()),
            address: Address::ZERO,
            stake: U256::ZERO,
            reputation: 5000,
            capabilities: vec!["stark".to_string(), "local".to_string()],
            peer_timeout: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(15),
            claim_coordination_window_ms: 2000, // 2 second window
        }
    }
}

// ============================================================================
// P2P NETWORK
// ============================================================================

/// P2P Network coordinator
pub struct P2PNetwork {
    config: P2PConfig,
    /// Known peers
    peers: Arc<RwLock<HashMap<PeerId, PeerInfo>>>,
    /// Active claim intents (request_id -> list of intents)
    claim_intents: Arc<RwLock<HashMap<U256, Vec<ClaimIntent>>>>,
    /// Jobs we've seen claimed
    claimed_jobs: Arc<RwLock<HashSet<U256>>>,
    /// Channel for outgoing messages
    outbound_tx: broadcast::Sender<P2PMessage>,
    /// Channel for incoming messages
    inbound_tx: mpsc::Sender<P2PMessage>,
    inbound_rx: Arc<RwLock<mpsc::Receiver<P2PMessage>>>,
}

#[derive(Debug, Clone)]
struct ClaimIntent {
    peer_id: PeerId,
    timestamp: u64,
    priority_score: u64,
}

impl P2PNetwork {
    /// Create a new P2P network coordinator
    pub fn new(config: P2PConfig) -> Self {
        let (outbound_tx, _) = broadcast::channel(1000);
        let (inbound_tx, inbound_rx) = mpsc::channel(1000);

        Self {
            config,
            peers: Arc::new(RwLock::new(HashMap::new())),
            claim_intents: Arc::new(RwLock::new(HashMap::new())),
            claimed_jobs: Arc::new(RwLock::new(HashSet::new())),
            outbound_tx,
            inbound_tx,
            inbound_rx: Arc::new(RwLock::new(inbound_rx)),
        }
    }

    /// Start the P2P network
    pub async fn start(&self) -> Result<()> {
        info!("Starting P2P network as peer: {}", self.config.peer_id);

        // Announce ourselves
        self.announce().await?;

        // Start heartbeat task
        let network = self.clone_refs();
        tokio::spawn(async move {
            network.heartbeat_loop().await;
        });

        // Start message processing task
        let network = self.clone_refs();
        tokio::spawn(async move {
            network.process_messages().await;
        });

        // Start peer cleanup task
        let network = self.clone_refs();
        tokio::spawn(async move {
            network.cleanup_loop().await;
        });

        Ok(())
    }

    /// Announce our presence to the network
    pub async fn announce(&self) -> Result<()> {
        let msg = P2PMessage::Announce {
            peer_id: self.config.peer_id.clone(),
            address: self.config.address,
            stake: self.config.stake,
            reputation: self.config.reputation,
            capabilities: self.config.capabilities.clone(),
        };

        self.broadcast(msg).await
    }

    /// Coordinate claiming a job
    ///
    /// Returns whether we should proceed with claiming based on P2P coordination
    pub async fn coordinate_claim(&self, request_id: U256) -> Result<ClaimDecision> {
        // Check if already claimed
        if self.claimed_jobs.read().await.contains(&request_id) {
            debug!("Job {} already claimed by another prover", request_id);
            return Ok(ClaimDecision::Defer {
                to_peer: "unknown".to_string(),
            });
        }

        // Calculate our priority score (stake * reputation)
        let our_priority = self.calculate_priority();

        // Broadcast our intent
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let intent = P2PMessage::ClaimIntent {
            peer_id: self.config.peer_id.clone(),
            request_id,
            timestamp: now,
            priority_score: our_priority,
        };

        self.broadcast(intent).await?;

        // Record our own intent
        {
            let mut intents = self.claim_intents.write().await;
            intents.entry(request_id).or_default().push(ClaimIntent {
                peer_id: self.config.peer_id.clone(),
                timestamp: now,
                priority_score: our_priority,
            });
        }

        // Wait for coordination window
        tokio::time::sleep(Duration::from_millis(
            self.config.claim_coordination_window_ms,
        ))
        .await;

        // Check all intents and decide
        let decision = {
            let intents = self.claim_intents.read().await;
            if let Some(job_intents) = intents.get(&request_id) {
                self.decide_claim(job_intents, our_priority)
            } else {
                ClaimDecision::Proceed
            }
        };

        // Clean up intents
        {
            let mut intents = self.claim_intents.write().await;
            intents.remove(&request_id);
        }

        Ok(decision)
    }

    /// Announce that we've claimed a job on-chain
    pub async fn announce_claim(&self, request_id: U256, tx_hash: B256) -> Result<()> {
        // Record locally
        self.claimed_jobs.write().await.insert(request_id);

        // Broadcast to network
        let msg = P2PMessage::JobClaimed {
            peer_id: self.config.peer_id.clone(),
            request_id,
            tx_hash,
        };

        self.broadcast(msg).await
    }

    /// Announce job completion
    pub async fn announce_completion(
        &self,
        request_id: U256,
        proof_hash: B256,
        cycles: u64,
        duration_ms: u64,
    ) -> Result<()> {
        let msg = P2PMessage::JobCompleted {
            peer_id: self.config.peer_id.clone(),
            request_id,
            proof_hash,
            cycles,
            duration_ms,
        };

        self.broadcast(msg).await
    }

    /// Request distributed proving assistance
    pub async fn request_proof_assist(
        &self,
        request_id: U256,
        segment_index: u32,
        total_segments: u32,
        elf_hash: B256,
    ) -> Result<()> {
        let msg = P2PMessage::ProofAssistRequest {
            peer_id: self.config.peer_id.clone(),
            request_id,
            segment_index,
            total_segments,
            elf_hash,
        };

        self.broadcast(msg).await
    }

    /// Get list of active peers
    pub async fn get_peers(&self) -> Vec<PeerInfo> {
        self.peers.read().await.values().cloned().collect()
    }

    /// Get number of active peers
    pub async fn peer_count(&self) -> usize {
        self.peers.read().await.len()
    }

    /// Check if a job has been claimed by any peer
    pub async fn is_job_claimed(&self, request_id: &U256) -> bool {
        self.claimed_jobs.read().await.contains(request_id)
    }

    /// Subscribe to outbound messages (for network transport)
    pub fn subscribe(&self) -> broadcast::Receiver<P2PMessage> {
        self.outbound_tx.subscribe()
    }

    /// Receive an inbound message from the network
    pub async fn receive(&self, msg: P2PMessage) -> Result<()> {
        self.inbound_tx.send(msg).await?;
        Ok(())
    }

    // ========================================================================
    // INTERNAL METHODS
    // ========================================================================

    fn calculate_priority(&self) -> u64 {
        // Priority = stake (in wei, scaled down) * reputation
        let stake_score = self.config.stake.to::<u128>() / 1_000_000_000_000_000; // Scale to manageable range
        (stake_score as u64) * self.config.reputation / 10000
    }

    fn decide_claim(&self, intents: &[ClaimIntent], our_priority: u64) -> ClaimDecision {
        if intents.is_empty() {
            return ClaimDecision::Proceed;
        }

        // Find highest priority
        let max_priority = intents.iter().map(|i| i.priority_score).max().unwrap_or(0);

        // If we have highest priority, proceed
        if our_priority >= max_priority {
            // Check for ties
            let ties: Vec<_> = intents
                .iter()
                .filter(|i| i.priority_score == max_priority)
                .collect();

            if ties.len() > 1 {
                // Tie-break by earliest timestamp
                let earliest = ties.iter().min_by_key(|i| i.timestamp).unwrap();
                if earliest.peer_id == self.config.peer_id {
                    ClaimDecision::Proceed
                } else {
                    ClaimDecision::Defer {
                        to_peer: earliest.peer_id.clone(),
                    }
                }
            } else {
                ClaimDecision::Proceed
            }
        } else {
            // Someone else has higher priority
            let highest = intents
                .iter()
                .max_by_key(|i| i.priority_score)
                .unwrap();
            ClaimDecision::Defer {
                to_peer: highest.peer_id.clone(),
            }
        }
    }

    async fn broadcast(&self, msg: P2PMessage) -> Result<()> {
        // Broadcast to local subscribers
        let _ = self.outbound_tx.send(msg.clone());

        // In production, this would also send via libp2p gossipsub
        // For now, we just log
        debug!("Broadcasting P2P message: {:?}", msg);

        Ok(())
    }

    async fn process_messages(&self) {
        loop {
            let msg = {
                let mut rx = self.inbound_rx.write().await;
                rx.recv().await
            };

            match msg {
                Some(msg) => self.handle_message(msg).await,
                None => break,
            }
        }
    }

    async fn handle_message(&self, msg: P2PMessage) {
        match msg {
            P2PMessage::Announce {
                peer_id,
                address,
                stake,
                reputation,
                capabilities,
            } => {
                let info = PeerInfo {
                    peer_id: peer_id.clone(),
                    address,
                    stake,
                    reputation,
                    capabilities,
                    last_seen: Instant::now(),
                    active_jobs: 0,
                    queue_depth: 0,
                };

                self.peers.write().await.insert(peer_id.clone(), info);
                info!("Peer announced: {}", peer_id);
            }

            P2PMessage::ClaimIntent {
                peer_id,
                request_id,
                timestamp,
                priority_score,
            } => {
                let mut intents = self.claim_intents.write().await;
                intents.entry(request_id).or_default().push(ClaimIntent {
                    peer_id,
                    timestamp,
                    priority_score,
                });
            }

            P2PMessage::JobClaimed {
                peer_id,
                request_id,
                tx_hash,
            } => {
                self.claimed_jobs.write().await.insert(request_id);
                debug!(
                    "Job {} claimed by {} in tx {:?}",
                    request_id, peer_id, tx_hash
                );
            }

            P2PMessage::JobCompleted {
                peer_id,
                request_id,
                cycles,
                duration_ms,
                ..
            } => {
                info!(
                    "Job {} completed by {}: {} cycles in {}ms",
                    request_id, peer_id, cycles, duration_ms
                );
            }

            P2PMessage::Heartbeat {
                peer_id,
                active_jobs,
                queue_depth,
                ..
            } => {
                if let Some(peer) = self.peers.write().await.get_mut(&peer_id) {
                    peer.last_seen = Instant::now();
                    peer.active_jobs = active_jobs;
                    peer.queue_depth = queue_depth;
                }
            }

            P2PMessage::Goodbye { peer_id, reason } => {
                self.peers.write().await.remove(&peer_id);
                info!("Peer {} left: {}", peer_id, reason);
            }

            _ => {}
        }
    }

    async fn heartbeat_loop(&self) {
        loop {
            tokio::time::sleep(self.config.heartbeat_interval).await;

            let msg = P2PMessage::Heartbeat {
                peer_id: self.config.peer_id.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                active_jobs: 0,  // TODO: Get from processor
                queue_depth: 0,  // TODO: Get from queue
            };

            let _ = self.broadcast(msg).await;
        }
    }

    async fn cleanup_loop(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;

            let timeout = self.config.peer_timeout;
            let mut peers = self.peers.write().await;

            let dead_peers: Vec<_> = peers
                .iter()
                .filter(|(_, p)| p.last_seen.elapsed() > timeout)
                .map(|(id, _)| id.clone())
                .collect();

            for peer_id in dead_peers {
                peers.remove(&peer_id);
                warn!("Peer {} timed out", peer_id);
            }

            // Also clean up old claimed jobs (keep last 1000)
            let mut claimed = self.claimed_jobs.write().await;
            if claimed.len() > 1000 {
                let to_remove = claimed.len() - 1000;
                let remove_ids: Vec<_> = claimed.iter().take(to_remove).cloned().collect();
                for id in remove_ids {
                    claimed.remove(&id);
                }
            }
        }
    }

    fn clone_refs(&self) -> Self {
        Self {
            config: self.config.clone(),
            peers: self.peers.clone(),
            claim_intents: self.claim_intents.clone(),
            claimed_jobs: self.claimed_jobs.clone(),
            outbound_tx: self.outbound_tx.clone(),
            inbound_tx: self.inbound_tx.clone(),
            inbound_rx: self.inbound_rx.clone(),
        }
    }
}

// ============================================================================
// DISTRIBUTED PROVING
// ============================================================================

/// Coordinator for distributed proof generation
pub struct DistributedProver {
    network: Arc<P2PNetwork>,
    /// Segments we're currently proving for others
    helping_with: Arc<RwLock<HashMap<(U256, u32), HelpingJob>>>,
}

#[derive(Debug)]
struct HelpingJob {
    request_id: U256,
    segment_index: u32,
    requester: PeerId,
    started_at: Instant,
}

impl DistributedProver {
    pub fn new(network: Arc<P2PNetwork>) -> Self {
        Self {
            network,
            helping_with: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Split a job into segments for distributed proving
    pub async fn distribute_proving(
        &self,
        request_id: U256,
        elf_hash: B256,
        total_segments: u32,
    ) -> Result<Vec<PeerId>> {
        let mut assigned = vec![];

        // Request help for each segment (except first which we do ourselves)
        for i in 1..total_segments {
            self.network
                .request_proof_assist(request_id, i, total_segments, elf_hash)
                .await?;
        }

        // Wait for offers
        tokio::time::sleep(Duration::from_secs(5)).await;

        // TODO: Collect offers and assign segments
        // For now, return empty (we'll do all segments ourselves)

        Ok(assigned)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_claim_coordination() {
        let config = P2PConfig {
            peer_id: "test-peer".to_string(),
            stake: U256::from(1000000000000000000u128), // 1 ETH
            reputation: 7500,
            ..Default::default()
        };

        let network = P2PNetwork::new(config);
        let request_id = U256::from(123);

        // Should proceed if no other intents
        let decision = network.coordinate_claim(request_id).await.unwrap();
        assert_eq!(decision, ClaimDecision::Proceed);
    }

    #[test]
    fn test_priority_calculation() {
        let config = P2PConfig {
            stake: U256::from(10_000_000_000_000_000_000u128), // 10 ETH
            reputation: 8000, // 80%
            ..Default::default()
        };

        let network = P2PNetwork::new(config);
        let priority = network.calculate_priority();

        // 10 ETH = 10 * 10^18 wei
        // scaled down by 10^15 = 10000
        // * 8000 / 10000 = 8000
        assert!(priority > 0);
    }
}
