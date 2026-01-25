//! Error Recovery for Crashed Jobs
//!
//! Handles recovery when the prover crashes or fails mid-job:
//! - Persists claimed jobs to disk
//! - Recovers incomplete jobs on restart
//! - Releases stale claims after timeout
//!
//! ## Recovery Flow
//!
//! 1. Before claiming: Record job ID to recovery file
//! 2. After completion: Remove job ID from recovery file
//! 3. On startup: Check for incomplete jobs and resume or release
//!
//! ## File Format
//!
//! Simple JSON file with claimed job IDs and timestamps.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};

/// Recovery state for a claimed job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedJob {
    pub request_id: u64,
    pub image_id: String,
    pub input_hash: String,
    pub claimed_at: u64,
    pub expires_at: u64,
    pub status: ClaimStatus,
}

/// Status of a claimed job
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ClaimStatus {
    Claimed,
    Proving,
    Submitting,
    Completed,
    Failed,
}

/// Recovery manager configuration
#[derive(Debug, Clone)]
pub struct RecoveryConfig {
    /// Path to recovery file
    pub recovery_file: PathBuf,
    /// Timeout for stale claims (default: 30 minutes)
    pub stale_timeout: Duration,
    /// Whether to auto-resume jobs on startup
    pub auto_resume: bool,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            recovery_file: PathBuf::from("./data/recovery.json"),
            stale_timeout: Duration::from_secs(30 * 60), // 30 minutes
            auto_resume: true,
        }
    }
}

/// Recovery manager for handling crashed jobs
pub struct RecoveryManager {
    config: RecoveryConfig,
    claims: RwLock<HashMap<u64, ClaimedJob>>,
}

impl RecoveryManager {
    /// Create new recovery manager
    pub fn new(config: RecoveryConfig) -> Self {
        Self {
            config,
            claims: RwLock::new(HashMap::new()),
        }
    }

    /// Load recovery state from disk
    pub async fn load(&self) -> anyhow::Result<Vec<ClaimedJob>> {
        if !self.config.recovery_file.exists() {
            return Ok(vec![]);
        }

        let content = tokio::fs::read_to_string(&self.config.recovery_file).await?;
        let claims: HashMap<u64, ClaimedJob> = serde_json::from_str(&content)?;

        let mut guard = self.claims.write().await;
        *guard = claims.clone();

        let jobs: Vec<_> = claims.into_values().collect();
        info!("Loaded {} jobs from recovery file", jobs.len());

        Ok(jobs)
    }

    /// Save recovery state to disk
    async fn save(&self) -> anyhow::Result<()> {
        let claims = self.claims.read().await;

        // Ensure directory exists
        if let Some(parent) = self.config.recovery_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let content = serde_json::to_string_pretty(&*claims)?;
        tokio::fs::write(&self.config.recovery_file, content).await?;

        debug!("Saved {} claims to recovery file", claims.len());
        Ok(())
    }

    /// Record a newly claimed job
    pub async fn record_claim(
        &self,
        request_id: u64,
        image_id: &str,
        input_hash: &str,
        expires_at: u64,
    ) -> anyhow::Result<()> {
        let job = ClaimedJob {
            request_id,
            image_id: image_id.to_string(),
            input_hash: input_hash.to_string(),
            claimed_at: current_timestamp(),
            expires_at,
            status: ClaimStatus::Claimed,
        };

        {
            let mut claims = self.claims.write().await;
            claims.insert(request_id, job);
        }

        self.save().await?;
        info!("Recorded claim for job {}", request_id);
        Ok(())
    }

    /// Update job status
    pub async fn update_status(&self, request_id: u64, status: ClaimStatus) -> anyhow::Result<()> {
        {
            let mut claims = self.claims.write().await;
            if let Some(job) = claims.get_mut(&request_id) {
                job.status = status;
            }
        }

        self.save().await?;
        debug!("Updated job {} status to {:?}", request_id, status);
        Ok(())
    }

    /// Mark job as completed and remove from recovery
    pub async fn complete_job(&self, request_id: u64) -> anyhow::Result<()> {
        {
            let mut claims = self.claims.write().await;
            claims.remove(&request_id);
        }

        self.save().await?;
        info!("Completed and removed job {} from recovery", request_id);
        Ok(())
    }

    /// Mark job as failed
    pub async fn fail_job(&self, request_id: u64, _error: &str) -> anyhow::Result<()> {
        self.update_status(request_id, ClaimStatus::Failed).await?;
        warn!("Marked job {} as failed", request_id);
        Ok(())
    }

    /// Get all incomplete jobs (for recovery on startup)
    pub async fn get_incomplete_jobs(&self) -> Vec<ClaimedJob> {
        let claims = self.claims.read().await;

        claims
            .values()
            .filter(|j| j.status != ClaimStatus::Completed && j.status != ClaimStatus::Failed)
            .cloned()
            .collect()
    }

    /// Get stale claims (past timeout, should be released)
    pub async fn get_stale_claims(&self) -> Vec<ClaimedJob> {
        let claims = self.claims.read().await;
        let now = current_timestamp();
        let timeout_secs = self.config.stale_timeout.as_secs();

        claims
            .values()
            .filter(|j| {
                j.status != ClaimStatus::Completed
                    && j.status != ClaimStatus::Failed
                    && now > j.claimed_at + timeout_secs
            })
            .cloned()
            .collect()
    }

    /// Clean up stale claims
    pub async fn cleanup_stale(&self) -> anyhow::Result<usize> {
        let stale = self.get_stale_claims().await;
        let count = stale.len();

        if count > 0 {
            let mut claims = self.claims.write().await;
            for job in &stale {
                claims.remove(&job.request_id);
                warn!("Removed stale claim for job {}", job.request_id);
            }
            drop(claims);
            self.save().await?;
        }

        Ok(count)
    }

    /// Check if a job can be resumed
    pub fn can_resume(job: &ClaimedJob) -> bool {
        let now = current_timestamp();

        // Can resume if:
        // 1. Job hasn't expired on-chain
        // 2. Job is in a resumable state
        job.expires_at > now
            && matches!(
                job.status,
                ClaimStatus::Claimed | ClaimStatus::Proving | ClaimStatus::Submitting
            )
    }

    /// Get jobs that can be resumed
    pub async fn get_resumable_jobs(&self) -> Vec<ClaimedJob> {
        let incomplete = self.get_incomplete_jobs().await;

        incomplete
            .into_iter()
            .filter(|j| Self::can_resume(j))
            .collect()
    }

    /// Get count of active claims
    pub async fn active_count(&self) -> usize {
        let claims = self.claims.read().await;
        claims
            .values()
            .filter(|j| j.status != ClaimStatus::Completed && j.status != ClaimStatus::Failed)
            .count()
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Recovery statistics
#[derive(Debug, Clone, Default)]
pub struct RecoveryStats {
    pub jobs_recovered: u64,
    pub jobs_released: u64,
    pub jobs_failed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_record_and_complete() {
        let dir = tempdir().unwrap();
        let config = RecoveryConfig {
            recovery_file: dir.path().join("recovery.json"),
            ..Default::default()
        };

        let manager = RecoveryManager::new(config);

        // Record claim
        manager
            .record_claim(1, "image123", "input456", current_timestamp() + 3600)
            .await
            .unwrap();

        // Should have 1 incomplete job
        let incomplete = manager.get_incomplete_jobs().await;
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].request_id, 1);

        // Complete job
        manager.complete_job(1).await.unwrap();

        // Should have 0 incomplete jobs
        let incomplete = manager.get_incomplete_jobs().await;
        assert_eq!(incomplete.len(), 0);
    }

    #[tokio::test]
    async fn test_status_updates() {
        let dir = tempdir().unwrap();
        let config = RecoveryConfig {
            recovery_file: dir.path().join("recovery.json"),
            ..Default::default()
        };

        let manager = RecoveryManager::new(config);

        manager
            .record_claim(1, "image", "input", current_timestamp() + 3600)
            .await
            .unwrap();

        // Update to proving
        manager
            .update_status(1, ClaimStatus::Proving)
            .await
            .unwrap();

        let jobs = manager.get_incomplete_jobs().await;
        assert_eq!(jobs[0].status, ClaimStatus::Proving);

        // Update to submitting
        manager
            .update_status(1, ClaimStatus::Submitting)
            .await
            .unwrap();

        let jobs = manager.get_incomplete_jobs().await;
        assert_eq!(jobs[0].status, ClaimStatus::Submitting);
    }

    #[tokio::test]
    async fn test_stale_cleanup() {
        let dir = tempdir().unwrap();
        let config = RecoveryConfig {
            recovery_file: dir.path().join("recovery.json"),
            stale_timeout: Duration::from_secs(1), // 1 second timeout for test
            ..Default::default()
        };

        let manager = RecoveryManager::new(config);

        // Record old claim
        {
            let mut claims = manager.claims.write().await;
            claims.insert(
                1,
                ClaimedJob {
                    request_id: 1,
                    image_id: "test".to_string(),
                    input_hash: "test".to_string(),
                    claimed_at: current_timestamp() - 10, // 10 seconds ago
                    expires_at: current_timestamp() + 3600,
                    status: ClaimStatus::Claimed,
                },
            );
        }

        // Should be stale
        let stale = manager.get_stale_claims().await;
        assert_eq!(stale.len(), 1);

        // Cleanup
        let count = manager.cleanup_stale().await.unwrap();
        assert_eq!(count, 1);

        // Should be gone
        let stale = manager.get_stale_claims().await;
        assert_eq!(stale.len(), 0);
    }

    #[tokio::test]
    async fn test_persistence() {
        let dir = tempdir().unwrap();
        let config = RecoveryConfig {
            recovery_file: dir.path().join("recovery.json"),
            ..Default::default()
        };

        // Create and save
        {
            let manager = RecoveryManager::new(config.clone());
            manager
                .record_claim(1, "image", "input", current_timestamp() + 3600)
                .await
                .unwrap();
            manager
                .record_claim(2, "image2", "input2", current_timestamp() + 3600)
                .await
                .unwrap();
        }

        // Load in new instance
        {
            let manager = RecoveryManager::new(config);
            let jobs = manager.load().await.unwrap();
            assert_eq!(jobs.len(), 2);
        }
    }

    #[tokio::test]
    async fn test_can_resume() {
        let future_expiry = current_timestamp() + 3600;
        let past_expiry = current_timestamp() - 100;

        // Can resume: not expired, in progress
        let job = ClaimedJob {
            request_id: 1,
            image_id: "test".to_string(),
            input_hash: "test".to_string(),
            claimed_at: current_timestamp(),
            expires_at: future_expiry,
            status: ClaimStatus::Proving,
        };
        assert!(RecoveryManager::can_resume(&job));

        // Cannot resume: expired
        let job = ClaimedJob {
            expires_at: past_expiry,
            ..job.clone()
        };
        assert!(!RecoveryManager::can_resume(&job));

        // Cannot resume: already completed
        let job = ClaimedJob {
            expires_at: future_expiry,
            status: ClaimStatus::Completed,
            ..job.clone()
        };
        assert!(!RecoveryManager::can_resume(&job));
    }
}
