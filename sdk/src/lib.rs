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
pub mod fixture;
pub mod precompiles;
pub mod verifier;

pub use client::Client;
pub use fixture::{DAGFixture, ProofData};
pub use verifier::{BatchProgress, BatchSession, DAGVerifier};
