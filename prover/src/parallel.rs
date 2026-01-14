//! Parallel proof processing for maximum throughput
//!
//! Handles multiple proof requests concurrently using a work-stealing
//! approach for optimal resource utilization.

#![allow(dead_code)]

use alloy::primitives::{B256, U256};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tracing::{info, warn, error, debug};

use crate::bonsai::ProvingMode;
use crate::prover::execute_and_prove;

/// Configuration for parallel processing
#[derive(Clone, Debug)]
pub struct ParallelConfig {
    /// Maximum concurrent proofs
    pub max_concurrent: usize,
    /// Channel buffer size
    pub channel_buffer: usize,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4, // Bonsai can handle many concurrent sessions
            channel_buffer: 100,
        }
    }
}

/// A proof job to be processed
#[derive(Debug, Clone)]
pub struct ProofJob {
    pub request_id: U256,
    pub image_id: B256,
    pub input_url: String,
    pub input_digest: B256,
    pub proving_mode: ProvingMode,
}

/// Result of a proof job
#[derive(Debug)]
pub struct ProofResult {
    pub request_id: U256,
    pub seal: Vec<u8>,
    pub journal: Vec<u8>,
}

/// Error from a proof job
#[derive(Debug)]
pub struct ProofError {
    pub request_id: U256,
    pub error: String,
}

/// Parallel proof processor
pub struct ParallelProver {
    config: ParallelConfig,
    semaphore: Arc<Semaphore>,
}

impl ParallelProver {
    /// Create a new parallel prover
    pub fn new(config: ParallelConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        Self { config, semaphore }
    }

    /// Process multiple proof jobs in parallel
    pub async fn process_batch(
        &self,
        jobs: Vec<ProofJob>,
    ) -> (Vec<ProofResult>, Vec<ProofError>) {
        let total = jobs.len();
        info!("Processing {} proof jobs with max {} concurrent", total, self.config.max_concurrent);

        let (result_tx, mut result_rx) = mpsc::channel::<Result<ProofResult, ProofError>>(self.config.channel_buffer);

        // Spawn tasks for each job
        let mut handles = Vec::new();
        for job in jobs {
            let semaphore = self.semaphore.clone();
            let tx = result_tx.clone();

            let handle = tokio::spawn(async move {
                // Acquire permit (limits concurrency)
                let _permit = semaphore.acquire().await.unwrap();

                debug!("Starting proof for request {}", job.request_id);

                let result = execute_and_prove(
                    &job.image_id,
                    &job.input_url,
                    &job.input_digest,
                    job.proving_mode,
                ).await;

                let send_result = match result {
                    Ok((seal, journal)) => {
                        info!("Proof completed for request {}", job.request_id);
                        tx.send(Ok(ProofResult {
                            request_id: job.request_id,
                            seal,
                            journal,
                        })).await
                    }
                    Err(e) => {
                        error!("Proof failed for request {}: {}", job.request_id, e);
                        tx.send(Err(ProofError {
                            request_id: job.request_id,
                            error: e.to_string(),
                        })).await
                    }
                };

                if send_result.is_err() {
                    warn!("Failed to send proof result");
                }
            });

            handles.push(handle);
        }

        // Drop sender to allow receiver to complete
        drop(result_tx);

        // Collect results
        let mut results = Vec::new();
        let mut errors = Vec::new();

        while let Some(result) = result_rx.recv().await {
            match result {
                Ok(r) => results.push(r),
                Err(e) => errors.push(e),
            }
        }

        // Wait for all tasks to complete
        for handle in handles {
            let _ = handle.await;
        }

        info!(
            "Batch complete: {} succeeded, {} failed",
            results.len(),
            errors.len()
        );

        (results, errors)
    }

    /// Process a single job (still uses semaphore for rate limiting)
    pub async fn process_one(&self, job: ProofJob) -> Result<ProofResult, ProofError> {
        let _permit = self.semaphore.acquire().await.unwrap();

        match execute_and_prove(
            &job.image_id,
            &job.input_url,
            &job.input_digest,
            job.proving_mode,
        ).await {
            Ok((seal, journal)) => Ok(ProofResult {
                request_id: job.request_id,
                seal,
                journal,
            }),
            Err(e) => Err(ProofError {
                request_id: job.request_id,
                error: e.to_string(),
            }),
        }
    }

    /// Get current stats
    pub fn stats(&self) -> ParallelStats {
        ParallelStats {
            max_concurrent: self.config.max_concurrent,
            available_permits: self.semaphore.available_permits(),
        }
    }
}

/// Statistics for parallel processing
#[derive(Debug, Clone)]
pub struct ParallelStats {
    pub max_concurrent: usize,
    pub available_permits: usize,
}

impl ParallelStats {
    pub fn active_jobs(&self) -> usize {
        self.max_concurrent - self.available_permits
    }
}

impl std::fmt::Display for ParallelStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Parallel: {}/{} slots in use",
            self.active_jobs(),
            self.max_concurrent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_config_default() {
        let config = ParallelConfig::default();
        assert_eq!(config.max_concurrent, 4);
    }

    #[test]
    fn test_parallel_stats() {
        let prover = ParallelProver::new(ParallelConfig::default());
        let stats = prover.stats();
        assert_eq!(stats.available_permits, 4);
        assert_eq!(stats.active_jobs(), 0);
    }
}
