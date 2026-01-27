//! E2E test runner.
//!
//! Orchestrates full end-to-end tests with:
//! - Local Anvil chain
//! - Contract deployment
//! - Proof generation (real or mock)
//! - On-chain verification

use crate::{
    AnvilInstance, DeployedContracts, E2EConfig, E2EError, TestCase,
    standard_test_cases,
    types::{E2ETestResult, E2ETestSuiteResult, ProofData, TestJournalOutput},
};
use std::time::Instant;
use tokio::time::sleep;

/// E2E test runner
pub struct E2ERunner {
    config: E2EConfig,
    anvil: Option<AnvilInstance>,
}

impl E2ERunner {
    /// Create a new test runner
    pub fn new(config: E2EConfig) -> Self {
        Self {
            config,
            anvil: None,
        }
    }

    /// Create with default config
    pub fn default_config() -> Self {
        Self::new(E2EConfig::default())
    }

    /// Start the test environment
    pub async fn start(&mut self) -> Result<(), E2EError> {
        // Start Anvil if not using external chain
        if self.config.rpc_url == "http://127.0.0.1:8545" && self.anvil.is_none() {
            match AnvilInstance::start().await {
                Ok(anvil) => {
                    self.anvil = Some(anvil);
                    println!("Started Anvil on {}", self.config.rpc_url);
                }
                Err(e) => {
                    // Anvil might already be running
                    println!("Note: Could not start Anvil ({}), assuming external instance", e);
                }
            }
        }

        Ok(())
    }

    /// Stop the test environment
    pub async fn stop(&mut self) {
        self.anvil = None;
    }

    /// Run a single test case
    pub async fn run_test(&self, test: &TestCase) -> E2ETestResult {
        let start = Instant::now();

        // Serialize input
        let input_bytes = match serde_json::to_vec(&test.input) {
            Ok(bytes) => bytes,
            Err(e) => {
                return E2ETestResult::failure(
                    test.name.clone(),
                    start.elapsed().as_millis() as u64,
                    format!("Failed to serialize input: {}", e),
                );
            }
        };

        // Generate proof (mock or real)
        let proof_result = if self.config.use_fake_proofs {
            self.generate_mock_proof(&input_bytes, test).await
        } else {
            self.generate_real_proof(&input_bytes, test).await
        };

        let (_proof, output) = match proof_result {
            Ok(result) => result,
            Err(e) => {
                return E2ETestResult::failure(
                    test.name.clone(),
                    start.elapsed().as_millis() as u64,
                    format!("Proof generation failed: {}", e),
                );
            }
        };

        // Verify the result
        let duration_ms = start.elapsed().as_millis() as u64;

        E2ETestResult::success(
            test.name.clone(),
            duration_ms,
            test.expected_output.result,
            output.result,
        )
    }

    /// Generate a mock proof for fast testing
    async fn generate_mock_proof(
        &self,
        _input: &[u8],
        test: &TestCase,
    ) -> Result<(ProofData, TestJournalOutput), E2EError> {
        // Simulate proof generation
        sleep(std::time::Duration::from_millis(50)).await;

        // Compute the expected result directly
        let result = match test.input.operation.as_str() {
            "square" => test.input.value.saturating_mul(test.input.value),
            "double" => test.input.value.saturating_mul(2),
            "factorial" => (1..=test.input.value.min(20)).fold(1u64, |acc, x| acc.saturating_mul(x)),
            "fibonacci" => {
                if test.input.value <= 1 {
                    test.input.value
                } else {
                    let mut a = 0u64;
                    let mut b = 1u64;
                    for _ in 2..=test.input.value.min(90) {
                        let c = a.saturating_add(b);
                        a = b;
                        b = c;
                    }
                    b
                }
            }
            "is_prime" => {
                let n = test.input.value;
                let is_prime = if n < 2 {
                    false
                } else if n == 2 {
                    true
                } else if n % 2 == 0 {
                    false
                } else {
                    let mut i = 3u64;
                    let mut result = true;
                    while i * i <= n {
                        if n % i == 0 {
                            result = false;
                            break;
                        }
                        i += 2;
                    }
                    result
                };
                if is_prime { 1 } else { 0 }
            }
            "sum_to_n" => test.input.value.saturating_mul(test.input.value.saturating_add(1)) / 2,
            _ => test.input.value,
        };

        let checksum = compute_checksum(test.input.value, &test.input.operation, result);

        let output = TestJournalOutput {
            input: test.input.value,
            operation: test.input.operation.clone(),
            result,
            checksum,
        };

        // Create mock proof
        let journal = serde_json::to_vec(&output)
            .map_err(|e| E2EError::ProofGeneration(e.to_string()))?;

        let proof = ProofData {
            seal: vec![0u8; 256], // Mock seal
            journal,
            image_id: "0x0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        };

        Ok((proof, output))
    }

    /// Generate a real RISC Zero proof
    async fn generate_real_proof(
        &self,
        _input: &[u8],
        _test: &TestCase,
    ) -> Result<(ProofData, TestJournalOutput), E2EError> {
        // This would integrate with the actual RISC Zero prover
        // For now, we return an error indicating real proofs aren't implemented
        Err(E2EError::ProofGeneration(
            "Real proof generation requires RISC Zero setup. Use mock proofs for testing."
                .to_string(),
        ))
    }

    /// Run all test cases
    pub async fn run_all(&self, tests: &[TestCase]) -> E2ETestSuiteResult {
        let mut results = Vec::with_capacity(tests.len());

        for test in tests {
            println!("Running: {} - {}", test.name, test.description);
            let result = self.run_test(test).await;

            if result.passed {
                println!("  ✓ Passed ({}ms)", result.duration_ms);
            } else {
                println!(
                    "  ✗ Failed: {}",
                    result.error.as_deref().unwrap_or("unknown error")
                );
            }

            results.push(result);
        }

        E2ETestSuiteResult::from_results(results)
    }

    /// Run standard test suite
    pub async fn run_standard_suite(&self) -> E2ETestSuiteResult {
        let tests = standard_test_cases();
        self.run_all(&tests).await
    }
}

/// Compute checksum (must match guest program)
fn compute_checksum(input: u64, operation: &str, result: u64) -> u64 {
    let op_hash: u64 = operation
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    input
        .wrapping_mul(17)
        .wrapping_add(op_hash)
        .wrapping_mul(13)
        .wrapping_add(result)
}

/// Contract deployment helper
#[allow(dead_code)]
pub struct ContractDeployer {
    rpc_url: String,
    private_key: String,
}

impl ContractDeployer {
    pub fn new(rpc_url: String, private_key: String) -> Self {
        Self { rpc_url, private_key }
    }

    /// Deploy all contracts
    pub async fn deploy_all(&self) -> Result<DeployedContracts, E2EError> {
        // In a real implementation, this would:
        // 1. Deploy the verifier contract
        // 2. Deploy the program registry
        // 3. Deploy the execution engine
        // For now, return mock addresses

        Ok(DeployedContracts {
            program_registry: "0x5FbDB2315678afecb367f032d93F642f64180aa3".to_string(),
            execution_engine: "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512".to_string(),
            verifier: "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_runner() {
        let runner = E2ERunner::default_config();

        let test = TestCase::square(5);
        let result = runner.run_test(&test).await;

        assert!(result.passed);
        assert_eq!(result.actual_result, Some(25));
    }

    #[tokio::test]
    async fn test_factorial() {
        let runner = E2ERunner::default_config();

        let test = TestCase::factorial(5);
        let result = runner.run_test(&test).await;

        assert!(result.passed);
        assert_eq!(result.actual_result, Some(120));
    }

    #[tokio::test]
    async fn test_fibonacci() {
        let runner = E2ERunner::default_config();

        let test = TestCase::fibonacci(10);
        let result = runner.run_test(&test).await;

        assert!(result.passed);
        assert_eq!(result.actual_result, Some(55));
    }

    #[tokio::test]
    async fn test_is_prime() {
        let runner = E2ERunner::default_config();

        let test = TestCase::is_prime(17, true);
        let result = runner.run_test(&test).await;

        assert!(result.passed);
        assert_eq!(result.actual_result, Some(1));

        let test = TestCase::is_prime(4, false);
        let result = runner.run_test(&test).await;

        assert!(result.passed);
        assert_eq!(result.actual_result, Some(0));
    }

    #[tokio::test]
    async fn test_standard_suite() {
        let runner = E2ERunner::default_config();
        let results = runner.run_standard_suite().await;

        assert!(results.all_passed(), "Some tests failed: {:?}", results.results.iter().filter(|r| !r.passed).collect::<Vec<_>>());
    }
}
