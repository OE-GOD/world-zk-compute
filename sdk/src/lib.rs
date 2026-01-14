//! World ZK Compute SDK
//!
//! Easy-to-use SDK for submitting detection jobs to the World ZK Compute network.
//!
//! # Example
//!
//! ```rust,no_run
//! use world_zk_sdk::{Client, DetectionJob};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create client
//!     let client = Client::new(
//!         "https://rpc.world.org",
//!         "0x...", // private key
//!         "0x...", // engine address
//!     ).await?;
//!
//!     // Submit detection job
//!     let job = DetectionJob::new(
//!         image_id,
//!         input_data,
//!     ).with_bounty(0.01);
//!
//!     let result = client.submit_and_wait(job).await?;
//!
//!     println!("Anomalies found: {}", result.anomalies_found);
//!     Ok(())
//! }
//! ```

use alloy::primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::time::Duration;

/// SDK client for World ZK Compute
pub struct Client {
    rpc_url: String,
    engine_address: Address,
    // In production, would include provider and signer
}

impl Client {
    /// Create a new client
    pub async fn new(
        rpc_url: &str,
        _private_key: &str,
        engine_address: &str,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            rpc_url: rpc_url.to_string(),
            engine_address: engine_address.parse()?,
        })
    }

    /// Submit a detection job
    pub async fn submit(&self, job: DetectionJob) -> anyhow::Result<JobHandle> {
        // Compute input hash
        let input_hash = job.compute_input_hash();

        // Upload input (mock)
        let input_url = job.upload_input().await?;

        // Submit to contract (mock)
        let request_id = U256::from(1); // Would come from contract

        Ok(JobHandle {
            request_id,
            input_hash,
            input_url,
            engine_address: self.engine_address,
        })
    }

    /// Submit and wait for result
    pub async fn submit_and_wait(&self, job: DetectionJob) -> anyhow::Result<DetectionResult> {
        let handle = self.submit(job).await?;
        handle.wait(Duration::from_secs(300)).await
    }

    /// Get job status
    pub async fn get_status(&self, request_id: U256) -> anyhow::Result<JobStatus> {
        // Would query contract
        Ok(JobStatus::Pending)
    }
}

/// A detection job to submit
#[derive(Debug, Clone)]
pub struct DetectionJob {
    /// Program image ID
    pub image_id: B256,
    /// Input data (will be serialized)
    pub input_data: Vec<u8>,
    /// Bounty in ETH
    pub bounty_eth: f64,
    /// Maximum delay in seconds
    pub max_delay_secs: u64,
    /// Callback address (optional)
    pub callback: Option<Address>,
}

impl DetectionJob {
    /// Create a new detection job
    pub fn new(image_id: B256, input_data: Vec<u8>) -> Self {
        Self {
            image_id,
            input_data,
            bounty_eth: 0.01,
            max_delay_secs: 3600,
            callback: None,
        }
    }

    /// Set bounty amount
    pub fn with_bounty(mut self, eth: f64) -> Self {
        self.bounty_eth = eth;
        self
    }

    /// Set maximum delay
    pub fn with_max_delay(mut self, secs: u64) -> Self {
        self.max_delay_secs = secs;
        self
    }

    /// Set callback address
    pub fn with_callback(mut self, callback: Address) -> Self {
        self.callback = Some(callback);
        self
    }

    /// Compute input hash
    pub fn compute_input_hash(&self) -> B256 {
        let mut hasher = Sha256::new();
        hasher.update(&self.input_data);
        let result = hasher.finalize();
        B256::from_slice(&result)
    }

    /// Upload input data (mock)
    pub async fn upload_input(&self) -> anyhow::Result<String> {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&self.input_data);
        Ok(format!("data:application/octet-stream;base64,{}", b64))
    }
}

/// Handle for a submitted job
#[derive(Debug)]
pub struct JobHandle {
    pub request_id: U256,
    pub input_hash: B256,
    pub input_url: String,
    pub engine_address: Address,
}

impl JobHandle {
    /// Wait for job completion
    pub async fn wait(&self, _timeout: Duration) -> anyhow::Result<DetectionResult> {
        // Would poll contract for status
        // For now, return mock result
        Ok(DetectionResult {
            request_id: self.request_id,
            total_analyzed: 100,
            anomalies_found: 5,
            flagged_ids: vec![],
            risk_score: 0.05,
            proof_verified: true,
        })
    }
}

/// Job status
#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Pending,
    Claimed { prover: Address },
    Completed { result: DetectionResult },
    Failed { reason: String },
    Expired,
}

/// Detection result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectionResult {
    pub request_id: U256,
    pub total_analyzed: usize,
    pub anomalies_found: usize,
    pub flagged_ids: Vec<B256>,
    pub risk_score: f64,
    pub proof_verified: bool,
}

/// Standard input format for detection algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardInput<T> {
    /// Data points to analyze
    pub data: Vec<T>,
    /// Detection threshold (0.0 - 1.0)
    pub threshold: f64,
    /// Algorithm-specific parameters
    pub params: serde_json::Value,
}

impl<T: Serialize> StandardInput<T> {
    /// Create new standard input
    pub fn new(data: Vec<T>, threshold: f64) -> Self {
        Self {
            data,
            threshold,
            params: serde_json::Value::Null,
        }
    }

    /// Set parameters
    pub fn with_params<P: Serialize>(mut self, params: P) -> anyhow::Result<Self> {
        self.params = serde_json::to_value(params)?;
        Ok(self)
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(bincode::serialize(self)?)
    }
}

/// Standard output format for detection algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardOutput {
    /// Total items analyzed
    pub total_analyzed: usize,
    /// Number of anomalies detected
    pub anomalies_found: usize,
    /// IDs of flagged items
    pub flagged_ids: Vec<[u8; 32]>,
    /// Overall risk score (0.0 - 1.0)
    pub risk_score: f64,
    /// Hash of input for verification
    pub input_hash: [u8; 32],
}

/// Prelude for easy imports
pub mod prelude {
    pub use super::{
        Client, DetectionJob, DetectionResult, JobHandle, JobStatus,
        StandardInput, StandardOutput,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_input_hash() {
        let job = DetectionJob::new(B256::ZERO, vec![1, 2, 3, 4]);
        let hash = job.compute_input_hash();
        assert!(!hash.is_zero());
    }

    #[test]
    fn test_standard_input() {
        let input = StandardInput::new(vec![1, 2, 3], 0.8);
        let bytes = input.to_bytes().unwrap();
        assert!(!bytes.is_empty());
    }
}
