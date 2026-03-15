//! World ZK Compute SDK
//!
//! Rust SDK for interacting with the RemainderVerifier contract on-chain.
//! Supports DAG circuit registration, single-tx verification, and multi-tx batch verification.
//!
//! # Example
//!
//! ```rust,no_run
//! use world_zk_sdk::{Client, DAGVerifier, DAGFixture};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = Client::new(
//!         "http://localhost:8545",
//!         "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
//!         "0x5FbDB2315678afecb367f032d93F642f64180aa3",
//!     )?;
//!
//!     let fixture = DAGFixture::load("path/to/fixture.json")?;
//!     let proof = fixture.to_proof_data()?;
//!     let desc = fixture.to_dag_description()?;
//!
//!     let verifier = DAGVerifier::new(client);
//!     let valid = verifier.verify_single_tx(&proof).await?;
//!     println!("Proof valid: {valid}");
//!     Ok(())
//! }
//! ```

pub mod abi;
pub mod client;
pub mod error;
pub mod event_watcher;
pub mod execution_engine;
pub mod fixture;
pub mod gas_estimation;
pub mod hash;
pub mod networks;
pub mod precompiles;
pub mod prover_registry;
pub mod retry;
pub mod tee;
pub mod verifier;

pub use client::Client;
pub use event_watcher::{TEEEvent, TEEEventWatcher};
pub use execution_engine::{ExecutionEngineClient, RequestStatus};
pub use fixture::{DAGFixture, ProofData};
pub use hash::{
    compute_input_hash, compute_input_hash_from_json, compute_model_hash,
    compute_model_hash_from_file, compute_result_hash, compute_result_hash_from_bytes,
};
pub use retry::{is_retryable, retry_with_backoff, RetryPolicy};
pub use gas_estimation::{
    estimate_batch_session, estimate_continue_gas, estimate_finalize_gas,
    estimate_input_groups, estimate_start_gas, estimate_total_batches,
    estimate_total_cost_eth, estimate_total_cost_usd, xgboost_reference_estimate,
    BatchGasEstimate, GasRange, RpcBatchGasEstimate, RpcGasEstimator,
    GROUPS_PER_FINALIZE_BATCH, LAYERS_PER_BATCH,
};
pub use prover_registry::ProverRegistryClient;
pub use tee::TEEVerifier;
pub use verifier::{BatchProgress, BatchSession, DAGVerifier};
