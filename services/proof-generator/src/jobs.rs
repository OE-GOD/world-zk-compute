//! Async job store for background proof generation.
//!
//! When a client submits a proof request via `POST /v1/prove/async`, the service
//! creates a job immediately and returns the job ID. The actual proof generation
//! runs in a background task. The client can poll `GET /v1/jobs/{id}` to check
//! progress.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Status of a background proof job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Job created, waiting to start proving.
    Pending,
    /// Proof generation in progress.
    Proving,
    /// Proof generation completed successfully.
    Done,
    /// Proof generation failed.
    Failed,
}

/// A background proof generation job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique job identifier (UUID).
    pub id: String,
    /// Current status of the job.
    pub status: JobStatus,
    /// Model ID that this job is proving against.
    pub model_id: String,
    /// ISO-8601 timestamp when the job was created.
    pub created_at: String,
    /// ISO-8601 timestamp when the job completed (success or failure).
    pub completed_at: Option<String>,
    /// Proof ID from the prove response (set on success).
    pub proof_id: Option<String>,
    /// Inference output label (e.g. "approved" / "denied", set on success).
    pub output: Option<String>,
    /// Error message (set on failure).
    pub error: Option<String>,
}

/// Thread-safe in-memory store for background proof jobs.
pub struct JobStore {
    jobs: RwLock<HashMap<String, Job>>,
}

impl JobStore {
    /// Create a new empty job store.
    pub fn new() -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new pending job for the given model.
    ///
    /// Returns the newly assigned job ID (UUID v4).
    pub async fn create(&self, model_id: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let job = Job {
            id: id.clone(),
            status: JobStatus::Pending,
            model_id: model_id.to_string(),
            created_at: now,
            completed_at: None,
            proof_id: None,
            output: None,
            error: None,
        };

        self.jobs.write().await.insert(id.clone(), job);
        id
    }

    /// Get a job by ID.
    pub async fn get(&self, id: &str) -> Option<Job> {
        self.jobs.read().await.get(id).cloned()
    }

    /// Update a job's status (e.g. Pending -> Proving).
    pub async fn update_status(&self, id: &str, status: JobStatus) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.status = status;
        }
    }

    /// Mark a job as successfully completed.
    pub async fn complete(&self, id: &str, proof_id: &str, output: &str) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.status = JobStatus::Done;
            job.completed_at = Some(chrono::Utc::now().to_rfc3339());
            job.proof_id = Some(proof_id.to_string());
            job.output = Some(output.to_string());
        }
    }

    /// Mark a job as failed with an error message.
    pub async fn fail(&self, id: &str, error: &str) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.status = JobStatus::Failed;
            job.completed_at = Some(chrono::Utc::now().to_rfc3339());
            job.error = Some(error.to_string());
        }
    }

    /// List recent jobs, ordered by creation time (newest first), up to `limit`.
    pub async fn list(&self, limit: usize) -> Vec<Job> {
        let jobs = self.jobs.read().await;
        let mut all: Vec<Job> = jobs.values().cloned().collect();
        // Sort newest first by created_at (ISO-8601 sorts lexicographically).
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        all.truncate(limit);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_job() {
        let store = JobStore::new();
        let job_id = store.create("model-abc").await;

        assert!(!job_id.is_empty());

        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.id, job_id);
        assert_eq!(job.model_id, "model-abc");
        assert_eq!(job.status, JobStatus::Pending);
        assert!(job.completed_at.is_none());
        assert!(job.proof_id.is_none());
        assert!(job.output.is_none());
        assert!(job.error.is_none());
    }

    #[tokio::test]
    async fn test_get_nonexistent_job() {
        let store = JobStore::new();
        assert!(store.get("no-such-job").await.is_none());
    }

    #[tokio::test]
    async fn test_update_status() {
        let store = JobStore::new();
        let job_id = store.create("model-1").await;

        store.update_status(&job_id, JobStatus::Proving).await;
        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Proving);
    }

    #[tokio::test]
    async fn test_complete_job() {
        let store = JobStore::new();
        let job_id = store.create("model-2").await;

        store.update_status(&job_id, JobStatus::Proving).await;
        store.complete(&job_id, "proof-xyz", "approved").await;

        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Done);
        assert!(job.completed_at.is_some());
        assert_eq!(job.proof_id.as_deref(), Some("proof-xyz"));
        assert_eq!(job.output.as_deref(), Some("approved"));
        assert!(job.error.is_none());
    }

    #[tokio::test]
    async fn test_fail_job() {
        let store = JobStore::new();
        let job_id = store.create("model-3").await;

        store.update_status(&job_id, JobStatus::Proving).await;
        store.fail(&job_id, "prover crashed").await;

        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert!(job.completed_at.is_some());
        assert_eq!(job.error.as_deref(), Some("prover crashed"));
        assert!(job.proof_id.is_none());
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let store = JobStore::new();
        // Create jobs with slight time separation to ensure ordering
        let id1 = store.create("m1").await;
        let id2 = store.create("m2").await;
        let id3 = store.create("m3").await;

        let all = store.list(10).await;
        assert_eq!(all.len(), 3);
        // Newest first — id3 was created last
        assert_eq!(all[0].id, id3);
        assert_eq!(all[1].id, id2);
        assert_eq!(all[2].id, id1);
    }

    #[tokio::test]
    async fn test_list_jobs_with_limit() {
        let store = JobStore::new();
        store.create("m1").await;
        store.create("m2").await;
        store.create("m3").await;

        let limited = store.list(2).await;
        assert_eq!(limited.len(), 2);
    }

    #[tokio::test]
    async fn test_list_empty() {
        let store = JobStore::new();
        let empty = store.list(10).await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_update_nonexistent_is_noop() {
        let store = JobStore::new();
        // These should not panic
        store.update_status("ghost", JobStatus::Proving).await;
        store.complete("ghost", "proof", "output").await;
        store.fail("ghost", "error").await;
    }

    #[tokio::test]
    async fn test_job_status_serialization() {
        assert_eq!(
            serde_json::to_string(&JobStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Proving).unwrap(),
            "\"proving\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Done).unwrap(),
            "\"done\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::Failed).unwrap(),
            "\"failed\""
        );
    }

    #[tokio::test]
    async fn test_job_serialization_roundtrip() {
        let job = Job {
            id: "test-id".to_string(),
            status: JobStatus::Done,
            model_id: "model-x".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: Some("2026-01-01T00:01:00Z".to_string()),
            proof_id: Some("proof-123".to_string()),
            output: Some("approved".to_string()),
            error: None,
        };

        let json = serde_json::to_string(&job).unwrap();
        let deserialized: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-id");
        assert_eq!(deserialized.status, JobStatus::Done);
        assert_eq!(deserialized.proof_id.as_deref(), Some("proof-123"));
    }

    #[tokio::test]
    async fn test_full_lifecycle() {
        let store = JobStore::new();
        let job_id = store.create("model-lifecycle").await;

        // Initially pending
        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Pending);

        // Transition to proving
        store.update_status(&job_id, JobStatus::Proving).await;
        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Proving);

        // Complete
        store.complete(&job_id, "proof-final", "denied").await;
        let job = store.get(&job_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Done);
        assert_eq!(job.output.as_deref(), Some("denied"));
        assert_eq!(job.proof_id.as_deref(), Some("proof-final"));
        assert!(job.completed_at.is_some());
    }
}
