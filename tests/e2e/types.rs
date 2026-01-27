//! E2E test type definitions.

use serde::{Deserialize, Serialize};

/// Proof data from the zkVM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofData {
    /// The proof seal (SNARK proof)
    pub seal: Vec<u8>,
    /// The journal (public outputs)
    pub journal: Vec<u8>,
    /// Image ID that was executed
    pub image_id: String,
}

/// Journal output from the test guest program
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestJournalOutput {
    pub input: u64,
    pub operation: String,
    pub result: u64,
    pub checksum: u64,
}

/// Execution request for E2E testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2ERequest {
    pub id: u64,
    pub image_id: String,
    pub input_hash: String,
    pub input_data: Vec<u8>,
    pub status: RequestStatus,
}

/// Request status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestStatus {
    Pending,
    Claimed,
    ProofSubmitted,
    Verified,
    Failed,
}

/// E2E test result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2ETestResult {
    pub test_name: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub proof_generated: bool,
    pub proof_verified: bool,
    pub expected_result: u64,
    pub actual_result: Option<u64>,
    pub error: Option<String>,
}

impl E2ETestResult {
    /// Create a successful result
    pub fn success(
        test_name: String,
        duration_ms: u64,
        expected: u64,
        actual: u64,
    ) -> Self {
        Self {
            test_name,
            passed: expected == actual,
            duration_ms,
            proof_generated: true,
            proof_verified: true,
            expected_result: expected,
            actual_result: Some(actual),
            error: if expected != actual {
                Some(format!("Expected {} but got {}", expected, actual))
            } else {
                None
            },
        }
    }

    /// Create a failed result
    pub fn failure(test_name: String, duration_ms: u64, error: String) -> Self {
        Self {
            test_name,
            passed: false,
            duration_ms,
            proof_generated: false,
            proof_verified: false,
            expected_result: 0,
            actual_result: None,
            error: Some(error),
        }
    }
}

/// E2E test suite result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2ETestSuiteResult {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub total_duration_ms: u64,
    pub results: Vec<E2ETestResult>,
}

impl E2ETestSuiteResult {
    /// Create from a list of results
    pub fn from_results(results: Vec<E2ETestResult>) -> Self {
        let total = results.len();
        let passed = results.iter().filter(|r| r.passed).count();
        let failed = total - passed;
        let total_duration_ms: u64 = results.iter().map(|r| r.duration_ms).sum();

        Self {
            total,
            passed,
            failed,
            total_duration_ms,
            results,
        }
    }

    /// Check if all tests passed
    pub fn all_passed(&self) -> bool {
        self.failed == 0
    }

    /// Print summary
    pub fn print_summary(&self) {
        println!("\n============================================================");
        println!("E2E Test Results");
        println!("============================================================");
        println!("Total:  {}", self.total);
        println!("Passed: {} ✓", self.passed);
        println!("Failed: {} ✗", self.failed);
        println!("Duration: {}ms", self.total_duration_ms);
        println!();

        if self.failed > 0 {
            println!("Failed tests:");
            for result in &self.results {
                if !result.passed {
                    println!("  - {}: {}", result.test_name, result.error.as_deref().unwrap_or("unknown"));
                }
            }
        }
    }
}

/// Mock prover for testing without real proofs
pub struct MockProver {
    /// Simulated proof generation time in ms
    pub simulated_latency_ms: u64,
}

impl MockProver {
    pub fn new() -> Self {
        Self {
            simulated_latency_ms: 100,
        }
    }

    /// Generate a mock proof
    pub async fn prove(&self, input: &[u8]) -> ProofData {
        // Simulate proof generation time
        tokio::time::sleep(std::time::Duration::from_millis(self.simulated_latency_ms)).await;

        // Generate deterministic mock data
        let mut seal = vec![0u8; 256];
        for (i, b) in input.iter().enumerate() {
            seal[i % 256] ^= b;
        }

        ProofData {
            seal,
            journal: input.to_vec(),
            image_id: "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        }
    }
}

impl Default for MockProver {
    fn default() -> Self {
        Self::new()
    }
}
