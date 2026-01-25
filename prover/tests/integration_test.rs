//! Integration Tests for World ZK Compute
//!
//! Tests the full job lifecycle:
//! 1. Submit job to ExecutionEngine
//! 2. Detect job via events/polling
//! 3. Claim job
//! 4. Execute in zkVM (mocked)
//! 5. Submit proof
//! 6. Verify payment

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Mock job for testing
#[derive(Debug, Clone)]
pub struct MockJob {
    pub request_id: u64,
    pub image_id: [u8; 32],
    pub input_hash: [u8; 32],
    pub tip: u64,
    pub expires_at: u64,
    pub status: JobStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Pending,
    Claimed,
    Completed,
    Expired,
}

/// Mock ExecutionEngine for testing
pub struct MockExecutionEngine {
    jobs: Arc<RwLock<Vec<MockJob>>>,
    next_request_id: Arc<RwLock<u64>>,
    prover_balance: Arc<RwLock<u64>>,
}

impl MockExecutionEngine {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(Vec::new())),
            next_request_id: Arc::new(RwLock::new(1)),
            prover_balance: Arc::new(RwLock::new(0)),
        }
    }

    /// Submit a new job
    pub async fn submit_job(&self, image_id: [u8; 32], input_hash: [u8; 32], tip: u64) -> u64 {
        let mut jobs = self.jobs.write().await;
        let mut id = self.next_request_id.write().await;

        let request_id = *id;
        *id += 1;

        let job = MockJob {
            request_id,
            image_id,
            input_hash,
            tip,
            expires_at: current_timestamp() + 3600, // 1 hour
            status: JobStatus::Pending,
        };

        jobs.push(job);
        request_id
    }

    /// Get pending jobs
    pub async fn get_pending_jobs(&self) -> Vec<MockJob> {
        let jobs = self.jobs.read().await;
        jobs.iter()
            .filter(|j| j.status == JobStatus::Pending)
            .cloned()
            .collect()
    }

    /// Claim a job
    pub async fn claim_job(&self, request_id: u64) -> Result<(), String> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.iter_mut().find(|j| j.request_id == request_id) {
            if job.status != JobStatus::Pending {
                return Err("Job not pending".to_string());
            }
            if job.expires_at < current_timestamp() {
                return Err("Job expired".to_string());
            }
            job.status = JobStatus::Claimed;
            Ok(())
        } else {
            Err("Job not found".to_string())
        }
    }

    /// Submit proof and complete job
    pub async fn submit_proof(&self, request_id: u64, _proof: Vec<u8>, _journal: Vec<u8>) -> Result<u64, String> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.iter_mut().find(|j| j.request_id == request_id) {
            if job.status != JobStatus::Claimed {
                return Err("Job not claimed".to_string());
            }

            // In real implementation, verify proof here
            job.status = JobStatus::Completed;

            // Pay prover
            let mut balance = self.prover_balance.write().await;
            *balance += job.tip;

            Ok(job.tip)
        } else {
            Err("Job not found".to_string())
        }
    }

    /// Get prover balance
    pub async fn get_prover_balance(&self) -> u64 {
        *self.prover_balance.read().await
    }

    /// Get job by ID
    pub async fn get_job(&self, request_id: u64) -> Option<MockJob> {
        let jobs = self.jobs.read().await;
        jobs.iter().find(|j| j.request_id == request_id).cloned()
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Mock Prover that simulates zkVM execution
pub struct MockProver {
    /// Simulated proof generation time
    pub proof_time: Duration,
    /// Should fail proving?
    pub should_fail: bool,
}

impl MockProver {
    pub fn new() -> Self {
        Self {
            proof_time: Duration::from_millis(100),
            should_fail: false,
        }
    }

    pub fn with_failure(mut self) -> Self {
        self.should_fail = true;
        self
    }

    /// Simulate proof generation
    pub async fn prove(&self, _elf: &[u8], _input: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        tokio::time::sleep(self.proof_time).await;

        if self.should_fail {
            return Err("Proof generation failed".to_string());
        }

        // Return mock proof and journal
        let proof = vec![0u8; 256]; // Mock STARK proof
        let journal = vec![1, 2, 3, 4]; // Mock journal

        Ok((proof, journal))
    }
}

/// Full job processor for integration testing
pub struct JobProcessor {
    engine: Arc<MockExecutionEngine>,
    prover: MockProver,
}

impl JobProcessor {
    pub fn new(engine: Arc<MockExecutionEngine>) -> Self {
        Self {
            engine,
            prover: MockProver::new(),
        }
    }

    /// Process a single job end-to-end
    pub async fn process_job(&self, request_id: u64) -> Result<u64, String> {
        // 1. Claim job
        self.engine.claim_job(request_id).await?;

        // 2. Get job details
        let job = self.engine.get_job(request_id).await
            .ok_or("Job not found after claim")?;

        // 3. Generate proof (mocked)
        let (proof, journal) = self.prover.prove(&job.image_id, &job.input_hash).await?;

        // 4. Submit proof
        let reward = self.engine.submit_proof(request_id, proof, journal).await?;

        Ok(reward)
    }

    /// Process all pending jobs
    pub async fn process_all_pending(&self) -> Vec<Result<u64, String>> {
        let pending = self.engine.get_pending_jobs().await;
        let mut results = Vec::new();

        for job in pending {
            let result = self.process_job(job.request_id).await;
            results.push(result);
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_full_job_lifecycle() {
        // Setup
        let engine = Arc::new(MockExecutionEngine::new());
        let processor = JobProcessor::new(engine.clone());

        // 1. Submit job
        let image_id = [1u8; 32];
        let input_hash = [2u8; 32];
        let tip = 1000;

        let request_id = engine.submit_job(image_id, input_hash, tip).await;
        assert_eq!(request_id, 1);

        // 2. Verify job is pending
        let job = engine.get_job(request_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Pending);

        // 3. Process job (claim -> prove -> submit)
        let reward = processor.process_job(request_id).await.unwrap();
        assert_eq!(reward, tip);

        // 4. Verify job is completed
        let job = engine.get_job(request_id).await.unwrap();
        assert_eq!(job.status, JobStatus::Completed);

        // 5. Verify prover got paid
        let balance = engine.get_prover_balance().await;
        assert_eq!(balance, tip);
    }

    #[tokio::test]
    async fn test_multiple_jobs() {
        let engine = Arc::new(MockExecutionEngine::new());
        let processor = JobProcessor::new(engine.clone());

        // Submit 5 jobs
        for i in 0..5 {
            engine.submit_job([i as u8; 32], [0u8; 32], 100 * (i + 1) as u64).await;
        }

        // Verify all pending
        let pending = engine.get_pending_jobs().await;
        assert_eq!(pending.len(), 5);

        // Process all
        let results = processor.process_all_pending().await;
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| r.is_ok()));

        // Verify all completed
        let pending = engine.get_pending_jobs().await;
        assert_eq!(pending.len(), 0);

        // Verify total earnings: 100 + 200 + 300 + 400 + 500 = 1500
        let balance = engine.get_prover_balance().await;
        assert_eq!(balance, 1500);
    }

    #[tokio::test]
    async fn test_claim_already_claimed() {
        let engine = Arc::new(MockExecutionEngine::new());

        let request_id = engine.submit_job([1u8; 32], [2u8; 32], 100).await;

        // First claim succeeds
        assert!(engine.claim_job(request_id).await.is_ok());

        // Second claim fails
        let result = engine.claim_job(request_id).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not pending"));
    }

    #[tokio::test]
    async fn test_submit_proof_not_claimed() {
        let engine = Arc::new(MockExecutionEngine::new());

        let request_id = engine.submit_job([1u8; 32], [2u8; 32], 100).await;

        // Try to submit proof without claiming
        let result = engine.submit_proof(request_id, vec![], vec![]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not claimed"));
    }

    #[tokio::test]
    async fn test_job_not_found() {
        let engine = Arc::new(MockExecutionEngine::new());

        // Try to claim non-existent job
        let result = engine.claim_job(999).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_parallel_job_processing() {
        let engine = Arc::new(MockExecutionEngine::new());

        // Submit 10 jobs
        for i in 0..10 {
            engine.submit_job([i as u8; 32], [0u8; 32], 100).await;
        }

        // Process jobs in parallel (simulate multiple provers)
        let handles: Vec<_> = (1..=10).map(|id| {
            let engine = engine.clone();
            tokio::spawn(async move {
                let prover = MockProver::new();

                // Claim
                if engine.claim_job(id).await.is_ok() {
                    // Prove
                    let (proof, journal) = prover.prove(&[], &[]).await.unwrap();
                    // Submit
                    engine.submit_proof(id, proof, journal).await
                } else {
                    Err("Failed to claim".to_string())
                }
            })
        }).collect();

        // Wait for all
        let results: Vec<_> = futures::future::join_all(handles).await;

        // All should succeed (no race condition on claim)
        let successes = results.iter()
            .filter(|r| r.as_ref().map(|r| r.is_ok()).unwrap_or(false))
            .count();
        assert_eq!(successes, 10);
    }

    #[tokio::test]
    async fn test_proof_cache_simulation() {
        use std::collections::HashMap;

        let engine = Arc::new(MockExecutionEngine::new());
        let mut cache: HashMap<([u8; 32], [u8; 32]), (Vec<u8>, Vec<u8>)> = HashMap::new();

        let image_id = [1u8; 32];
        let input_hash = [2u8; 32];

        // Submit same job twice
        let id1 = engine.submit_job(image_id, input_hash, 100).await;
        let id2 = engine.submit_job(image_id, input_hash, 100).await;

        // First job - cache miss, generate proof
        engine.claim_job(id1).await.unwrap();
        let prover = MockProver::new();
        let (proof, journal) = prover.prove(&image_id, &input_hash).await.unwrap();

        // Cache the proof
        cache.insert((image_id, input_hash), (proof.clone(), journal.clone()));
        engine.submit_proof(id1, proof, journal).await.unwrap();

        // Second job - cache hit!
        engine.claim_job(id2).await.unwrap();
        let cached = cache.get(&(image_id, input_hash)).unwrap();
        engine.submit_proof(id2, cached.0.clone(), cached.1.clone()).await.unwrap();

        // Both completed
        assert_eq!(engine.get_prover_balance().await, 200);
    }

    #[tokio::test]
    async fn test_job_expiry() {
        let engine = Arc::new(MockExecutionEngine::new());

        // Create expired job manually
        {
            let mut jobs = engine.jobs.write().await;
            jobs.push(MockJob {
                request_id: 1,
                image_id: [1u8; 32],
                input_hash: [2u8; 32],
                tip: 100,
                expires_at: current_timestamp() - 1, // Already expired
                status: JobStatus::Pending,
            });
        }

        // Try to claim expired job
        let result = engine.claim_job(1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expired"));
    }

    #[tokio::test]
    async fn test_prover_stats() {
        let engine = Arc::new(MockExecutionEngine::new());
        let processor = JobProcessor::new(engine.clone());

        // Track stats
        let mut jobs_processed = 0u64;
        let mut total_earnings = 0u64;

        // Process 5 jobs
        for i in 1..=5 {
            let tip = i * 100;
            let id = engine.submit_job([i as u8; 32], [0u8; 32], tip).await;

            if let Ok(reward) = processor.process_job(id).await {
                jobs_processed += 1;
                total_earnings += reward;
            }
        }

        assert_eq!(jobs_processed, 5);
        assert_eq!(total_earnings, 1500); // 100+200+300+400+500
        assert_eq!(engine.get_prover_balance().await, 1500);
    }
}
