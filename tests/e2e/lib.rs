//! E2E Test Library for World ZK Compute
//!
//! This crate provides infrastructure for running end-to-end tests
//! with real RISC Zero proofs or mock proofs for fast CI testing.

pub mod types;
pub mod runner;

// Re-export main types
pub use types::*;
pub use runner::*;

use serde::{Deserialize, Serialize};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::time::sleep;

/// Test environment configuration
#[derive(Debug, Clone)]
pub struct E2EConfig {
    /// Anvil RPC URL
    pub rpc_url: String,
    /// Anvil chain ID
    pub chain_id: u64,
    /// Test private key (anvil default)
    pub private_key: String,
    /// Deployed contract addresses
    pub contracts: Option<DeployedContracts>,
    /// Use fake proofs for faster testing
    pub use_fake_proofs: bool,
    /// Timeout for proof generation
    pub proof_timeout: Duration,
}

impl Default for E2EConfig {
    fn default() -> Self {
        Self {
            rpc_url: "http://127.0.0.1:8545".to_string(),
            chain_id: 31337,
            // Anvil default private key (account 0)
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .to_string(),
            contracts: None,
            use_fake_proofs: true, // Default to fake for CI speed
            proof_timeout: Duration::from_secs(300),
        }
    }
}

/// Deployed contract addresses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployedContracts {
    pub program_registry: String,
    pub execution_engine: String,
    pub verifier: String,
}

/// Anvil process handle
pub struct AnvilInstance {
    process: Child,
    pub rpc_url: String,
    pub chain_id: u64,
    pub accounts: Vec<AnvilAccount>,
}

/// Anvil test account
#[derive(Debug, Clone)]
pub struct AnvilAccount {
    pub address: String,
    pub private_key: String,
}

impl AnvilInstance {
    /// Start a new Anvil instance
    pub async fn start() -> Result<Self, E2EError> {
        let process = Command::new("anvil")
            .args([
                "--port",
                "8545",
                "--chain-id",
                "31337",
                "--block-time",
                "1",
                "--accounts",
                "10",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| E2EError::AnvilStart(e.to_string()))?;

        // Wait for Anvil to start
        sleep(Duration::from_secs(2)).await;

        // Default Anvil accounts
        let accounts = vec![
            AnvilAccount {
                address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
                private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                    .to_string(),
            },
            AnvilAccount {
                address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8".to_string(),
                private_key: "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
                    .to_string(),
            },
            AnvilAccount {
                address: "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC".to_string(),
                private_key: "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
                    .to_string(),
            },
        ];

        Ok(Self {
            process,
            rpc_url: "http://127.0.0.1:8545".to_string(),
            chain_id: 31337,
            accounts,
        })
    }

    /// Get the deployer account
    pub fn deployer(&self) -> &AnvilAccount {
        &self.accounts[0]
    }

    /// Get a prover account
    pub fn prover(&self) -> &AnvilAccount {
        &self.accounts[1]
    }

    /// Get a user account
    pub fn user(&self) -> &AnvilAccount {
        &self.accounts[2]
    }
}

impl Drop for AnvilInstance {
    fn drop(&mut self) {
        let _ = self.process.kill();
    }
}

/// E2E test errors
#[derive(Debug, Clone)]
pub enum E2EError {
    AnvilStart(String),
    ContractDeploy(String),
    TransactionFailed(String),
    ProofGeneration(String),
    Verification(String),
    Timeout(String),
    InvalidState(String),
}

impl std::fmt::Display for E2EError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnvilStart(msg) => write!(f, "Failed to start Anvil: {}", msg),
            Self::ContractDeploy(msg) => write!(f, "Contract deployment failed: {}", msg),
            Self::TransactionFailed(msg) => write!(f, "Transaction failed: {}", msg),
            Self::ProofGeneration(msg) => write!(f, "Proof generation failed: {}", msg),
            Self::Verification(msg) => write!(f, "Verification failed: {}", msg),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::InvalidState(msg) => write!(f, "Invalid state: {}", msg),
        }
    }
}

impl std::error::Error for E2EError {}

/// Test case definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub name: String,
    pub description: String,
    pub input: TestInput,
    pub expected_output: ExpectedOutput,
}

/// Test input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestInput {
    pub value: u64,
    pub operation: String,
}

/// Expected output for verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedOutput {
    pub result: u64,
}

impl TestCase {
    /// Create a "square" test case
    pub fn square(value: u64) -> Self {
        Self {
            name: format!("square_{}", value),
            description: format!("Compute {} squared", value),
            input: TestInput {
                value,
                operation: "square".to_string(),
            },
            expected_output: ExpectedOutput {
                result: value.saturating_mul(value),
            },
        }
    }

    /// Create a "factorial" test case
    pub fn factorial(value: u64) -> Self {
        let result = (1..=value).fold(1u64, |acc, x| acc.saturating_mul(x));
        Self {
            name: format!("factorial_{}", value),
            description: format!("Compute {}!", value),
            input: TestInput {
                value,
                operation: "factorial".to_string(),
            },
            expected_output: ExpectedOutput { result },
        }
    }

    /// Create a "fibonacci" test case
    pub fn fibonacci(n: u64) -> Self {
        let result = {
            if n <= 1 {
                n
            } else {
                let mut a = 0u64;
                let mut b = 1u64;
                for _ in 2..=n {
                    let c = a.saturating_add(b);
                    a = b;
                    b = c;
                }
                b
            }
        };
        Self {
            name: format!("fibonacci_{}", n),
            description: format!("Compute fibonacci({})", n),
            input: TestInput {
                value: n,
                operation: "fibonacci".to_string(),
            },
            expected_output: ExpectedOutput { result },
        }
    }

    /// Create an "is_prime" test case
    pub fn is_prime(value: u64, expected: bool) -> Self {
        Self {
            name: format!("is_prime_{}", value),
            description: format!("Check if {} is prime", value),
            input: TestInput {
                value,
                operation: "is_prime".to_string(),
            },
            expected_output: ExpectedOutput {
                result: if expected { 1 } else { 0 },
            },
        }
    }
}

/// Standard test suite
pub fn standard_test_cases() -> Vec<TestCase> {
    vec![
        // Square tests
        TestCase::square(0),
        TestCase::square(1),
        TestCase::square(5),
        TestCase::square(100),
        TestCase::square(12345),
        // Factorial tests
        TestCase::factorial(0),
        TestCase::factorial(1),
        TestCase::factorial(5),
        TestCase::factorial(10),
        TestCase::factorial(20),
        // Fibonacci tests
        TestCase::fibonacci(0),
        TestCase::fibonacci(1),
        TestCase::fibonacci(10),
        TestCase::fibonacci(20),
        TestCase::fibonacci(50),
        // Prime tests
        TestCase::is_prime(2, true),
        TestCase::is_prime(17, true),
        TestCase::is_prime(97, true),
        TestCase::is_prime(4, false),
        TestCase::is_prime(100, false),
    ]
}
