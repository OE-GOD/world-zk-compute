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
//!
//! # Performance: Using Precompiles
//!
//! For 10-100x faster proving, use RISC Zero's accelerated crypto crates
//! in your guest programs. See [`PRECOMPILES.md`](../PRECOMPILES.md) for the complete guide.
//!
//! Quick reference for guest `Cargo.toml`:
//!
//! ```toml
//! # SHA-256 (100x faster)
//! sha2 = { git = "https://github.com/risc0/RustCrypto-hashes", tag = "sha2-v0.10.8-risczero.0" }
//!
//! # Ethereum ECDSA (100x faster)
//! k256 = { git = "https://github.com/risc0/RustCrypto-elliptic-curves", tag = "k256-v0.13.4-risczero.1", features = ["ecdsa", "unstable"] }
//!
//! # RSA (100x faster)
//! rsa = { git = "https://github.com/risc0/rust-rsa", tag = "rsa-v0.9.6-risczero.0", features = ["unstable"] }
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

/// Precompile configuration for guest programs
///
/// Use these constants to configure accelerated crypto in your guest's Cargo.toml.
/// See `PRECOMPILES.md` for the complete guide.
pub mod precompiles {
    /// SHA-256 precompile (~100x faster)
    pub const SHA2_GIT: &str = "https://github.com/risc0/RustCrypto-hashes";
    pub const SHA2_TAG: &str = "sha2-v0.10.8-risczero.0";

    /// secp256k1 ECDSA precompile - Ethereum signatures (~100x faster)
    pub const K256_GIT: &str = "https://github.com/risc0/RustCrypto-elliptic-curves";
    pub const K256_TAG: &str = "k256-v0.13.4-risczero.1";

    /// P-256 ECDSA precompile (~100x faster)
    pub const P256_GIT: &str = "https://github.com/risc0/RustCrypto-elliptic-curves";
    pub const P256_TAG: &str = "p256-v0.13.2-risczero.0";

    /// RSA precompile (~100x faster)
    pub const RSA_GIT: &str = "https://github.com/risc0/rust-rsa";
    pub const RSA_TAG: &str = "rsa-v0.9.6-risczero.0";

    /// Ed25519/curve25519 precompile (~100x faster)
    pub const CURVE25519_GIT: &str = "https://github.com/risc0/curve25519-dalek";
    pub const CURVE25519_TAG: &str = "curve25519-dalek-v4.1.3-risczero.0";

    /// BLS12-381 precompile
    pub const BLS12_381_GIT: &str = "https://github.com/risc0/bls12_381";
    pub const BLS12_381_TAG: &str = "bls12_381-v0.8.0-risczero.0";

    /// Generate Cargo.toml dependency line for a precompile
    ///
    /// # Example
    /// ```
    /// use world_zk_sdk::precompiles;
    /// let dep = precompiles::cargo_dep("sha2", precompiles::SHA2_GIT, precompiles::SHA2_TAG, &[]);
    /// assert!(dep.contains("sha2"));
    /// ```
    pub fn cargo_dep(name: &str, git: &str, tag: &str, features: &[&str]) -> String {
        if features.is_empty() {
            format!("{} = {{ git = \"{}\", tag = \"{}\" }}", name, git, tag)
        } else {
            format!(
                "{} = {{ git = \"{}\", tag = \"{}\", features = {:?} }}",
                name, git, tag, features
            )
        }
    }

    /// Print all precompile dependencies for copy-paste into Cargo.toml
    pub fn print_all_deps() {
        println!("# RISC Zero Accelerated Crypto Precompiles");
        println!("# Add these to your guest's Cargo.toml for 10-100x speedup");
        println!();
        println!("# SHA-256 (~100x faster)");
        println!("{}", cargo_dep("sha2", SHA2_GIT, SHA2_TAG, &[]));
        println!();
        println!("# Ethereum ECDSA - secp256k1 (~100x faster)");
        println!("{}", cargo_dep("k256", K256_GIT, K256_TAG, &["ecdsa", "unstable"]));
        println!();
        println!("# Standard ECDSA - P-256 (~100x faster)");
        println!("{}", cargo_dep("p256", P256_GIT, P256_TAG, &["ecdsa"]));
        println!();
        println!("# RSA (~100x faster)");
        println!("{}", cargo_dep("rsa", RSA_GIT, RSA_TAG, &["unstable"]));
        println!();
        println!("# Ed25519 (~100x faster)");
        println!("{}", cargo_dep("curve25519-dalek", CURVE25519_GIT, CURVE25519_TAG, &[]));
        println!();
        println!("# BLS12-381");
        println!("{}", cargo_dep("bls12_381", BLS12_381_GIT, BLS12_381_TAG, &[]));
    }
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
