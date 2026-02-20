//! SP1 Proving Backend (feature-gated)
//!
//! Implements `ZkVmBackend` for Succinct's SP1 zkVM.
//! The entire module is behind `#[cfg(feature = "sp1")]`.
//!
//! SP1 guest programs use `sp1_zkvm::io::read::<T>()` which reads bincode
//! from stdin. The prover passes raw bytes via `SP1Stdin::write_slice()`.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use sp1_sdk::{ProverClient, SP1Stdin};
use std::time::Instant;
use tracing::info;

use crate::zkvm_backend::{ExecutionResult, ProofResult, ZkVmBackend};

/// SP1 proving backend.
///
/// Uses the SP1 SDK `ProverClient` for execution and proving.
/// The client is configured via environment variables (e.g., `SP1_PROVER`).
pub struct Sp1Backend {
    client: ProverClient,
}

impl Sp1Backend {
    /// Create a new SP1 backend.
    ///
    /// Initializes `ProverClient::from_env()` which reads SP1 configuration
    /// from environment variables. Returns an error if initialization fails.
    pub fn new() -> Result<Self> {
        let client = ProverClient::from_env();
        info!("SP1 backend initialized");
        Ok(Self { client })
    }

    /// Build an SP1Stdin from raw input bytes.
    fn build_stdin(input: &[u8]) -> SP1Stdin {
        let mut stdin = SP1Stdin::new();
        stdin.write_slice(input);
        stdin
    }
}

#[async_trait]
impl ZkVmBackend for Sp1Backend {
    async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult> {
        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();

        // SP1 execution is blocking, run in a spawn_blocking
        let client = self.client.clone();

        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let stdin = Self::build_stdin(&input_owned);
            let (output, report) = client
                .execute(&elf_owned, &stdin)
                .run()
                .map_err(|e| anyhow!("SP1 execution failed: {}", e))?;

            let execution_time = start.elapsed();
            let cycles = report.total_instruction_count();

            info!("SP1 execution: {} cycles, {:?}", cycles, execution_time);

            Ok(ExecutionResult {
                journal: output.as_bytes().to_vec(),
                cycles,
                memory_estimate_bytes: 0, // SP1 doesn't expose this
                segment_count: 0,         // SP1 manages segments internally
                execution_time,
            })
        })
        .await?
    }

    async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();
        let client = self.client.clone();

        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let stdin = Self::build_stdin(&input_owned);
            let (pk, _vk) = client.setup(&elf_owned);

            let proof = client
                .prove(&pk, &stdin)
                .compressed()
                .run()
                .map_err(|e| anyhow!("SP1 compressed proving failed: {}", e))?;

            let prove_time = start.elapsed();
            let journal = proof.public_values.as_bytes().to_vec();
            let seal = bincode::serialize(&proof.proof)
                .map_err(|e| anyhow!("Failed to serialize SP1 proof: {}", e))?;

            info!(
                "SP1 compressed proof: {} bytes seal, {:?}",
                seal.len(),
                prove_time
            );

            Ok(ProofResult {
                seal,
                journal,
                cycles: 0,
                prove_time,
            })
        })
        .await?
    }

    async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();
        let client = self.client.clone();

        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let stdin = Self::build_stdin(&input_owned);
            let (pk, _vk) = client.setup(&elf_owned);

            let proof = client
                .prove(&pk, &stdin)
                .groth16()
                .run()
                .map_err(|e| anyhow!("SP1 Groth16 proving failed: {}", e))?;

            let prove_time = start.elapsed();
            let journal = proof.public_values.as_bytes().to_vec();
            let seal = bincode::serialize(&proof.proof)
                .map_err(|e| anyhow!("Failed to serialize SP1 Groth16 proof: {}", e))?;

            info!(
                "SP1 Groth16 proof: {} bytes seal, {:?}",
                seal.len(),
                prove_time
            );

            Ok(ProofResult {
                seal,
                journal,
                cycles: 0,
                prove_time,
            })
        })
        .await?
    }
}

impl Clone for Sp1Backend {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_stdin() {
        let stdin = Sp1Backend::build_stdin(&[1, 2, 3]);
        // Just verify it doesn't panic
        let _ = stdin;
    }
}
