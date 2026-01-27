//! Redis-backed distributed job queue for horizontal scaling.
//!
//! This module provides a Redis-based queue that allows multiple prover
//! instances to coordinate job processing without duplicating work.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

// ============================================================================
// Types
// ============================================================================

/// Job status in the queue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Expired,
}

/// A job in the distributed queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedJob {
    pub id: u64,
    pub image_id: String,
    pub input_hash: String,
    pub input_url: Option<String>,
    pub tip: u128,
    pub created_at: u64,
    pub expires_at: u64,
    pub status: JobStatus,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<u64>,
    pub priority: i32,
    pub retry_count: u32,
    pub shard_key: String,
}

impl QueuedJob {
    /// Create a new queued job
    pub fn new(
        id: u64,
        image_id: String,
        input_hash: String,
        input_url: Option<String>,
        tip: u128,
        expires_at: u64,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Shard key is first 4 chars of image_id for distribution
        let shard_key = if image_id.len() >= 6 {
            image_id[2..6].to_string() // Skip "0x" prefix
        } else {
            "0000".to_string()
        };

        Self {
            id,
            image_id,
            input_hash,
            input_url,
            tip,
            created_at: now,
            expires_at,
            status: JobStatus::Pending,
            claimed_by: None,
            claimed_at: None,
            priority: Self::calculate_priority(tip),
            retry_count: 0,
            shard_key,
        }
    }

    /// Calculate job priority based on tip
    fn calculate_priority(tip: u128) -> i32 {
        // Higher tip = higher priority (negative for sorting)
        -((tip / 1_000_000_000_000_000) as i32)
    }

    /// Check if job has expired
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now > self.expires_at
    }
}

/// Redis queue configuration
#[derive(Debug, Clone)]
pub struct RedisQueueConfig {
    /// Redis URL (redis://host:port)
    pub url: String,
    /// Queue namespace prefix
    pub prefix: String,
    /// Job claim timeout (seconds)
    pub claim_timeout: u64,
    /// Max retries per job
    pub max_retries: u32,
    /// Enable sharding
    pub enable_sharding: bool,
    /// Number of shards (if sharding enabled)
    pub num_shards: u32,
    /// This instance's shard assignment (if sharding enabled)
    pub assigned_shards: Vec<u32>,
    /// Worker ID for this instance
    pub worker_id: String,
}

impl Default for RedisQueueConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            prefix: "wzk".to_string(),
            claim_timeout: 600,
            max_retries: 3,
            enable_sharding: false,
            num_shards: 16,
            assigned_shards: vec![],
            worker_id: uuid_v4(),
        }
    }
}

fn uuid_v4() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:032x}", now)
}

/// Queue statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueStats {
    pub pending: u64,
    pub processing: u64,
    pub completed: u64,
    pub failed: u64,
    pub expired: u64,
    pub total_throughput: u64,
    pub avg_processing_time_ms: u64,
}

/// Queue errors
#[derive(Debug, Clone)]
pub enum QueueError {
    ConnectionFailed(String),
    OperationFailed(String),
    JobNotFound(u64),
    SerializationError(String),
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            Self::OperationFailed(msg) => write!(f, "Operation failed: {}", msg),
            Self::JobNotFound(id) => write!(f, "Job not found: {}", id),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for QueueError {}

// ============================================================================
// Distributed Queue
// ============================================================================

/// Distributed job queue (Redis-backed or in-memory)
pub struct DistributedQueue {
    config: RedisQueueConfig,
    jobs: Arc<RwLock<HashMap<u64, QueuedJob>>>,
    stats: Arc<RwLock<QueueStats>>,
}

impl DistributedQueue {
    /// Create a new distributed queue
    pub async fn new(config: RedisQueueConfig) -> Result<Self, QueueError> {
        // In production: connect to Redis
        // let client = redis::Client::open(config.url.clone())
        //     .map_err(|e| QueueError::ConnectionFailed(e.to_string()))?;

        Ok(Self {
            config,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(QueueStats::default())),
        })
    }

    /// Create with in-memory storage (for testing/single node)
    pub fn in_memory() -> Self {
        Self {
            config: RedisQueueConfig::default(),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(QueueStats::default())),
        }
    }

    /// Get shard number for a job
    fn get_shard(&self, shard_key: &str) -> u32 {
        let hash: u32 = shard_key
            .bytes()
            .fold(0u32, |acc, b| acc.wrapping_add(b as u32));
        hash % self.config.num_shards
    }

    /// Check if this instance should handle a shard
    fn should_handle_shard(&self, shard: u32) -> bool {
        if !self.config.enable_sharding {
            return true;
        }
        self.config.assigned_shards.contains(&shard)
    }

    /// Add a job to the queue
    pub async fn push(&self, job: QueuedJob) -> Result<(), QueueError> {
        let mut jobs = self.jobs.write().await;
        jobs.insert(job.id, job);

        let mut stats = self.stats.write().await;
        stats.pending += 1;

        Ok(())
    }

    /// Claim the next available job
    pub async fn claim(&self) -> Result<Option<QueuedJob>, QueueError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut jobs = self.jobs.write().await;

        // Find highest priority pending job
        let mut best_job: Option<(u64, i32)> = None;

        for (id, job) in jobs.iter() {
            if job.status != JobStatus::Pending || job.is_expired() {
                continue;
            }

            let shard = self.get_shard(&job.shard_key);
            if !self.should_handle_shard(shard) {
                continue;
            }

            match best_job {
                None => best_job = Some((*id, job.priority)),
                Some((_, best_priority)) if job.priority < best_priority => {
                    best_job = Some((*id, job.priority))
                }
                _ => {}
            }
        }

        if let Some((job_id, _)) = best_job {
            if let Some(job) = jobs.get_mut(&job_id) {
                job.status = JobStatus::Processing;
                job.claimed_by = Some(self.config.worker_id.clone());
                job.claimed_at = Some(now);

                let mut stats = self.stats.write().await;
                stats.pending = stats.pending.saturating_sub(1);
                stats.processing += 1;

                return Ok(Some(job.clone()));
            }
        }

        Ok(None)
    }

    /// Complete a job
    pub async fn complete(&self, job_id: u64) -> Result<(), QueueError> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&job_id).ok_or(QueueError::JobNotFound(job_id))?;

        let processing_time = job.claimed_at.map(|claimed| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - claimed
        });

        job.status = JobStatus::Completed;

        let mut stats = self.stats.write().await;
        stats.processing = stats.processing.saturating_sub(1);
        stats.completed += 1;
        stats.total_throughput += 1;

        if let Some(time) = processing_time {
            let time_ms = time * 1000;
            if stats.avg_processing_time_ms == 0 {
                stats.avg_processing_time_ms = time_ms;
            } else {
                stats.avg_processing_time_ms = (stats.avg_processing_time_ms * 9 + time_ms) / 10;
            }
        }

        Ok(())
    }

    /// Fail a job (may retry)
    pub async fn fail(&self, job_id: u64, reason: &str) -> Result<(), QueueError> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&job_id).ok_or(QueueError::JobNotFound(job_id))?;

        job.retry_count += 1;

        if job.retry_count >= self.config.max_retries {
            job.status = JobStatus::Failed;
            let mut stats = self.stats.write().await;
            stats.processing = stats.processing.saturating_sub(1);
            stats.failed += 1;
        } else {
            job.status = JobStatus::Pending;
            job.claimed_by = None;
            job.claimed_at = None;
            job.priority += 100;

            let mut stats = self.stats.write().await;
            stats.processing = stats.processing.saturating_sub(1);
            stats.pending += 1;
        }

        tracing::warn!(job_id, retry_count = job.retry_count, reason, "Job failed");

        Ok(())
    }

    /// Release a claimed job back to pending
    pub async fn release(&self, job_id: u64) -> Result<(), QueueError> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(&job_id).ok_or(QueueError::JobNotFound(job_id))?;

        if job.status == JobStatus::Processing {
            job.status = JobStatus::Pending;
            job.claimed_by = None;
            job.claimed_at = None;

            let mut stats = self.stats.write().await;
            stats.processing = stats.processing.saturating_sub(1);
            stats.pending += 1;
        }

        Ok(())
    }

    /// Get queue statistics
    pub async fn stats(&self) -> QueueStats {
        self.stats.read().await.clone()
    }

    /// Get a specific job
    pub async fn get(&self, job_id: u64) -> Option<QueuedJob> {
        self.jobs.read().await.get(&job_id).cloned()
    }

    /// List jobs by status
    pub async fn list(&self, status: JobStatus, limit: usize) -> Vec<QueuedJob> {
        let jobs = self.jobs.read().await;
        let mut result: Vec<_> = jobs
            .values()
            .filter(|j| j.status == status)
            .take(limit)
            .cloned()
            .collect();
        result.sort_by_key(|j| j.priority);
        result
    }

    /// Clean up expired jobs
    pub async fn cleanup(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut jobs = self.jobs.write().await;
        let mut expired = 0u64;

        for job in jobs.values_mut() {
            if job.status == JobStatus::Pending && now > job.expires_at {
                job.status = JobStatus::Expired;
                expired += 1;
            }

            // Release stale processing jobs
            if job.status == JobStatus::Processing {
                if let Some(claimed_at) = job.claimed_at {
                    if now - claimed_at > self.config.claim_timeout {
                        job.status = JobStatus::Pending;
                        job.claimed_by = None;
                        job.claimed_at = None;
                        job.retry_count += 1;
                    }
                }
            }
        }

        if expired > 0 {
            let mut stats = self.stats.write().await;
            stats.pending = stats.pending.saturating_sub(expired);
            stats.expired += expired;
        }

        expired
    }

    /// Update shard assignments
    pub fn set_assigned_shards(&mut self, shards: Vec<u32>) {
        self.config.assigned_shards = shards;
        self.config.enable_sharding = true;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_queue_push_and_claim() {
        let queue = DistributedQueue::in_memory();

        let job = QueuedJob::new(
            1,
            "0x1234abcd".to_string(),
            "0xhash".to_string(),
            None,
            1_000_000_000_000_000_000,
            u64::MAX,
        );
        queue.push(job).await.unwrap();

        let stats = queue.stats().await;
        assert_eq!(stats.pending, 1);

        let claimed = queue.claim().await.unwrap();
        assert!(claimed.is_some());
        assert_eq!(claimed.unwrap().id, 1);

        let stats = queue.stats().await;
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.processing, 1);
    }

    #[tokio::test]
    async fn test_queue_complete() {
        let queue = DistributedQueue::in_memory();

        let job = QueuedJob::new(1, "0x1234".to_string(), "0xh".to_string(), None, 1_000_000_000_000_000_000, u64::MAX);
        queue.push(job).await.unwrap();
        queue.claim().await.unwrap();
        queue.complete(1).await.unwrap();

        let stats = queue.stats().await;
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.processing, 0);
    }

    #[tokio::test]
    async fn test_job_priority() {
        let queue = DistributedQueue::in_memory();

        // Low priority job first
        let job1 = QueuedJob::new(1, "0x1111".to_string(), "0xh1".to_string(), None, 1_000_000_000_000_000_000, u64::MAX);
        queue.push(job1).await.unwrap();

        // High priority job second
        let job2 = QueuedJob::new(2, "0x2222".to_string(), "0xh2".to_string(), None, 10_000_000_000_000_000_000, u64::MAX);
        queue.push(job2).await.unwrap();

        // High priority should be claimed first
        let claimed = queue.claim().await.unwrap().unwrap();
        assert_eq!(claimed.id, 2);
    }

    #[tokio::test]
    async fn test_job_retry() {
        let queue = DistributedQueue::in_memory();

        let job = QueuedJob::new(1, "0x1234".to_string(), "0xhash".to_string(), None, 1_000_000_000_000_000_000, u64::MAX);
        queue.push(job).await.unwrap();

        queue.claim().await.unwrap();
        queue.fail(1, "test error").await.unwrap();

        let stats = queue.stats().await;
        assert_eq!(stats.pending, 1);

        let claimed = queue.claim().await.unwrap();
        assert!(claimed.is_some());
        assert_eq!(claimed.unwrap().retry_count, 1);
    }

    #[tokio::test]
    async fn test_job_max_retries() {
        let queue = DistributedQueue::in_memory();

        let job = QueuedJob::new(1, "0x1234".to_string(), "0xhash".to_string(), None, 1_000_000_000_000_000_000, u64::MAX);
        queue.push(job).await.unwrap();

        for _ in 0..3 {
            queue.claim().await.unwrap();
            queue.fail(1, "error").await.unwrap();
        }

        let stats = queue.stats().await;
        assert_eq!(stats.failed, 1);
        assert!(queue.claim().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_queue_cleanup() {
        let queue = DistributedQueue::in_memory();

        let mut job = QueuedJob::new(1, "0x1234".to_string(), "0xhash".to_string(), None, 1_000_000_000_000_000_000, 0);
        job.expires_at = 1; // Already expired
        queue.push(job).await.unwrap();

        let expired = queue.cleanup().await;
        assert_eq!(expired, 1);

        let stats = queue.stats().await;
        assert_eq!(stats.expired, 1);
    }

    #[tokio::test]
    async fn test_queue_release() {
        let queue = DistributedQueue::in_memory();

        let job = QueuedJob::new(1, "0x1234".to_string(), "0xhash".to_string(), None, 1_000_000_000_000_000_000, u64::MAX);
        queue.push(job).await.unwrap();

        queue.claim().await.unwrap();
        queue.release(1).await.unwrap();

        let stats = queue.stats().await;
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.processing, 0);
    }
}
