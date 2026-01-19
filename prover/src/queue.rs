//! Job Queue Management
//!
//! Priority queue for execution requests with smart job selection.
//!
//! ## Features
//!
//! - Priority-based ordering (higher tip = higher priority)
//! - Deadline awareness (expiring jobs processed first)
//! - Duplicate detection
//! - Profitability filtering
//!
//! ## Job Selection Strategy
//!
//! Jobs are scored by:
//! 1. **Tip amount** - Higher tips = higher priority
//! 2. **Time urgency** - Jobs expiring soon get boosted
//! 3. **Program familiarity** - Cached programs preferred
//! 4. **Estimated complexity** - Simpler jobs first for throughput

#![allow(dead_code)]

use alloy::primitives::{Address, B256, U256};
use std::collections::{BinaryHeap, HashSet};
use std::cmp::Ordering;
use std::time::Instant;
use tracing::{debug, info};

/// A job in the queue
#[derive(Debug, Clone)]
pub struct QueuedJob {
    /// Request ID
    pub request_id: u64,
    /// Program image ID
    pub image_id: B256,
    /// Input hash
    pub input_hash: B256,
    /// Input URL
    pub input_url: String,
    /// Tip amount (wei)
    pub tip: U256,
    /// Requester address
    pub requester: Address,
    /// Expiration timestamp (Unix seconds)
    pub expires_at: u64,
    /// When the job was added to queue
    pub queued_at: Instant,
    /// Estimated cycles (if known)
    pub estimated_cycles: Option<u64>,
    /// Whether program is cached
    pub program_cached: bool,
    /// Prefetched input data (if available)
    /// This is populated by the InputPrefetcher to reduce latency
    pub prefetched_input: Option<Vec<u8>>,
}

impl QueuedJob {
    /// Calculate job score for priority ordering
    pub fn score(&self) -> JobScore {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Time until expiration (0 if expired)
        let time_remaining = self.expires_at.saturating_sub(now);

        // Base score from tip (normalize to a reasonable range)
        let tip_score = (self.tip / U256::from(1_000_000_000_000_000u64)) // per 0.001 ETH
            .try_into()
            .unwrap_or(u64::MAX);

        // Urgency boost for jobs expiring soon (within 10 minutes)
        let urgency_boost = if time_remaining < 600 {
            100 - (time_remaining / 6) // 0-100 boost
        } else {
            0
        };

        // Cache bonus
        let cache_bonus = if self.program_cached { 50 } else { 0 };

        // Complexity penalty (prefer simpler jobs for throughput)
        let complexity_penalty = self.estimated_cycles
            .map(|c| c / 10_000_000) // penalty per 10M cycles
            .unwrap_or(0);

        JobScore {
            tip_score,
            urgency_boost,
            cache_bonus,
            complexity_penalty,
            expires_at: self.expires_at,
        }
    }
}

/// Composite score for job prioritization
#[derive(Debug, Clone, Copy)]
pub struct JobScore {
    pub tip_score: u64,
    pub urgency_boost: u64,
    pub cache_bonus: u64,
    pub complexity_penalty: u64,
    pub expires_at: u64,
}

impl JobScore {
    pub fn total(&self) -> i64 {
        (self.tip_score + self.urgency_boost + self.cache_bonus) as i64
            - self.complexity_penalty as i64
    }
}

/// Wrapper for heap ordering
struct ScoredJob {
    job: QueuedJob,
    score: JobScore,
}

impl PartialEq for ScoredJob {
    fn eq(&self, other: &Self) -> bool {
        self.job.request_id == other.job.request_id
    }
}

impl Eq for ScoredJob {}

impl PartialOrd for ScoredJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher score = higher priority
        self.score.total().cmp(&other.score.total())
    }
}

/// Priority queue for jobs
pub struct JobQueue {
    /// Priority heap
    heap: BinaryHeap<ScoredJob>,
    /// Set of request IDs for deduplication
    seen: HashSet<u64>,
    /// Maximum queue size
    max_size: usize,
    /// Minimum tip threshold
    min_tip: U256,
}

impl JobQueue {
    /// Create a new job queue
    pub fn new(max_size: usize, min_tip: U256) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(max_size),
            seen: HashSet::with_capacity(max_size),
            max_size,
            min_tip,
        }
    }

    /// Add a job to the queue
    pub fn push(&mut self, job: QueuedJob) -> bool {
        // Check if already seen
        if self.seen.contains(&job.request_id) {
            debug!("Job {} already in queue", job.request_id);
            return false;
        }

        // Check tip threshold
        if job.tip < self.min_tip {
            debug!("Job {} below min tip threshold", job.request_id);
            return false;
        }

        // Check expiration
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if job.expires_at <= now {
            debug!("Job {} already expired", job.request_id);
            return false;
        }

        // Check capacity
        if self.heap.len() >= self.max_size {
            // Only add if better than worst job
            if let Some(worst) = self.heap.peek() {
                let new_score = job.score();
                if new_score.total() <= worst.score.total() {
                    debug!("Job {} not better than worst in queue", job.request_id);
                    return false;
                }
                // Remove worst to make room
                if let Some(removed) = self.heap.pop() {
                    self.seen.remove(&removed.job.request_id);
                }
            }
        }

        let score = job.score();
        let request_id = job.request_id;

        self.heap.push(ScoredJob { job, score });
        self.seen.insert(request_id);

        info!("Added job {} to queue (score: {})", request_id, score.total());
        true
    }

    /// Get the next best job
    pub fn pop(&mut self) -> Option<QueuedJob> {
        // Pop jobs until we find one that's still valid
        while let Some(scored) = self.heap.pop() {
            self.seen.remove(&scored.job.request_id);

            // Check if still valid (not expired)
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if scored.job.expires_at > now {
                return Some(scored.job);
            }
            debug!("Skipping expired job {}", scored.job.request_id);
        }
        None
    }

    /// Peek at the next job without removing
    pub fn peek(&self) -> Option<&QueuedJob> {
        self.heap.peek().map(|s| &s.job)
    }

    /// Current queue size
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Iterate over jobs in the queue (for prefetching)
    pub fn iter_jobs(&self) -> impl Iterator<Item = &QueuedJob> {
        self.heap.iter().map(|scored| &scored.job)
    }

    /// Remove expired jobs
    pub fn cleanup_expired(&mut self) -> usize {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let before = self.heap.len();

        // Rebuild heap without expired jobs
        let jobs: Vec<_> = std::mem::take(&mut self.heap)
            .into_iter()
            .filter(|s| s.job.expires_at > now)
            .collect();

        self.seen.clear();
        for scored in jobs {
            self.seen.insert(scored.job.request_id);
            self.heap.push(scored);
        }

        let removed = before - self.heap.len();
        if removed > 0 {
            info!("Cleaned up {} expired jobs", removed);
        }
        removed
    }

    /// Get queue statistics
    pub fn stats(&self) -> QueueStats {
        let mut total_tip = U256::ZERO;
        let mut min_tip = U256::MAX;
        let mut max_tip = U256::ZERO;

        for scored in self.heap.iter() {
            total_tip += scored.job.tip;
            if scored.job.tip < min_tip {
                min_tip = scored.job.tip;
            }
            if scored.job.tip > max_tip {
                max_tip = scored.job.tip;
            }
        }

        QueueStats {
            size: self.heap.len(),
            total_tip_wei: total_tip,
            min_tip_wei: if self.heap.is_empty() { U256::ZERO } else { min_tip },
            max_tip_wei: max_tip,
        }
    }
}

/// Queue statistics
#[derive(Debug)]
pub struct QueueStats {
    pub size: usize,
    pub total_tip_wei: U256,
    pub min_tip_wei: U256,
    pub max_tip_wei: U256,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job(id: u64, tip_eth: f64, expires_in: u64) -> QueuedJob {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        QueuedJob {
            request_id: id,
            image_id: B256::ZERO,
            input_hash: B256::ZERO,
            input_url: "https://example.com".to_string(),
            tip: U256::from((tip_eth * 1e18) as u128),
            requester: Address::ZERO,
            expires_at: now + expires_in,
            queued_at: Instant::now(),
            estimated_cycles: None,
            program_cached: false,
            prefetched_input: None,
        }
    }

    #[test]
    fn test_priority_ordering() {
        let mut queue = JobQueue::new(100, U256::ZERO);

        // Add jobs with different tips
        queue.push(make_job(1, 0.01, 3600)); // Low tip
        queue.push(make_job(2, 0.1, 3600));  // High tip
        queue.push(make_job(3, 0.05, 3600)); // Medium tip

        // Should get highest tip first
        assert_eq!(queue.pop().unwrap().request_id, 2);
        assert_eq!(queue.pop().unwrap().request_id, 3);
        assert_eq!(queue.pop().unwrap().request_id, 1);
    }

    #[test]
    fn test_urgency_boost() {
        let mut queue = JobQueue::new(100, U256::ZERO);

        // Job with lower tip but expiring soon
        queue.push(make_job(1, 0.01, 60)); // Expires in 1 minute
        // Job with higher tip but plenty of time
        queue.push(make_job(2, 0.02, 3600)); // Expires in 1 hour

        // Urgent job should come first despite lower tip
        let first = queue.pop().unwrap();
        assert_eq!(first.request_id, 1);
    }

    #[test]
    fn test_deduplication() {
        let mut queue = JobQueue::new(100, U256::ZERO);

        assert!(queue.push(make_job(1, 0.01, 3600)));
        assert!(!queue.push(make_job(1, 0.01, 3600))); // Duplicate

        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_min_tip_filter() {
        let min_tip = U256::from(10_000_000_000_000_000u64); // 0.01 ETH
        let mut queue = JobQueue::new(100, min_tip);

        assert!(!queue.push(make_job(1, 0.001, 3600))); // Below threshold
        assert!(queue.push(make_job(2, 0.02, 3600)));   // Above threshold

        assert_eq!(queue.len(), 1);
    }
}
