//! Async proof generation job queue.
//!
//! Proofs take 4-5 seconds. This module provides a job queue so clients
//! can submit a prove request and poll for status instead of blocking.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Proving,
    Completed,
    Failed,
}

/// Alias: some code references `Job` instead of `ProveJob`.
pub type Job = ProveJob;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProveJob {
    pub id: String,
    pub model_id: String,
    pub features: Vec<f64>,
    pub status: JobStatus,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// In-memory job store.
pub struct JobStore {
    jobs: RwLock<HashMap<String, ProveJob>>,
}

impl JobStore {
    pub fn new() -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
        }
    }

    /// Create a job with all fields.
    pub fn create_full(&self, id: &str, model_id: &str, features: Vec<f64>) -> ProveJob {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let job = ProveJob {
            id: id.to_string(),
            model_id: model_id.to_string(),
            features,
            status: JobStatus::Queued,
            created_at: now,
            completed_at: None,
            result: None,
            error: None,
        };

        if let Ok(mut jobs) = self.jobs.write() {
            jobs.insert(id.to_string(), job.clone());
        }
        job
    }

    /// Create a job by model_id only (generates UUID, used by async handler).
    pub async fn create(&self, model_id: &str) -> String {
        let id = format!("{:016x}", {
            use std::collections::hash_map::RandomState;
            use std::hash::{BuildHasher, Hasher};
            RandomState::new().build_hasher().finish()
        });
        self.create_full(&id, model_id, Vec::new());
        id
    }

    pub async fn get(&self, id: &str) -> Option<ProveJob> {
        self.jobs.read().ok()?.get(id).cloned()
    }

    pub async fn update_status(&self, id: &str, status: JobStatus) {
        if let Ok(mut jobs) = self.jobs.write() {
            if let Some(job) = jobs.get_mut(id) {
                job.status = status;
            }
        }
    }

    pub async fn complete(&self, id: &str, proof_id: &str, output: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Ok(mut jobs) = self.jobs.write() {
            if let Some(job) = jobs.get_mut(id) {
                job.status = JobStatus::Completed;
                job.completed_at = Some(now);
                job.result = Some(serde_json::json!({
                    "proof_id": proof_id,
                    "output": output,
                }));
            }
        }
    }

    pub async fn list(&self, limit: usize) -> Vec<ProveJob> {
        if let Ok(jobs) = self.jobs.read() {
            let mut all: Vec<ProveJob> = jobs.values().cloned().collect();
            all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            all.truncate(limit);
            all
        } else {
            Vec::new()
        }
    }

    pub async fn fail(&self, id: &str, error: &str) {
        if let Ok(mut jobs) = self.jobs.write() {
            if let Some(job) = jobs.get_mut(id) {
                job.status = JobStatus::Failed;
                job.error = Some(error.to_string());
            }
        }
    }
}

impl Default for JobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_job_lifecycle() {
        let store = JobStore::new();
        let id = store.create("credit-v1").await;

        store.update_status(&id, JobStatus::Running).await;
        let j = store.get(&id).await.unwrap();
        assert_eq!(j.status, JobStatus::Running);

        store.complete(&id, "proof-123", "class_1").await;
        let j = store.get(&id).await.unwrap();
        assert_eq!(j.status, JobStatus::Completed);
        assert!(j.result.is_some());
        assert!(j.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_job_failure() {
        let store = JobStore::new();
        let id = store.create("model").await;
        store.fail(&id, "model not found").await;
        let j = store.get(&id).await.unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.error.as_deref(), Some("model not found"));
    }

    #[tokio::test]
    async fn test_unknown_job() {
        let store = JobStore::new();
        assert!(store.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let store = JobStore::new();
        store.create("m1").await;
        store.create("m2").await;
        let jobs = store.list(10).await;
        assert_eq!(jobs.len(), 2);
    }
}
