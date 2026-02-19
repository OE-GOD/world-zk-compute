//! Risc0 Backend Implementation
//!
//! Wraps the existing `UnifiedProver` behind the `ZkVmBackend` trait.
//! No behavior change — just restructuring for multi-VM support.


use anyhow::{anyhow, Result};
use async_trait::async_trait;
use risc0_zkvm::{default_executor, ExecutorEnv};
use std::time::Instant;
use tracing::info;

use crate::bonsai::ProvingMode;
use crate::prover::UnifiedProver;
use crate::zkvm_backend::{ExecutionResult, ProofResult, ZkVmBackend, ZkVmType};

/// Risc0 proving backend.
///
/// Delegates execution to `risc0_zkvm::default_executor()` and proving
/// to `UnifiedProver` (which handles Local / GPU / Bonsai modes).
pub struct Risc0Backend {
    proving_mode: ProvingMode,
}

impl Risc0Backend {
    /// Create a new Risc0 backend with the given proving mode.
    pub fn new(proving_mode: ProvingMode) -> Self {
        Self { proving_mode }
    }
}

#[async_trait]
impl ZkVmBackend for Risc0Backend {
    fn name(&self) -> &str {
        "risc0"
    }

    fn vm_type(&self) -> ZkVmType {
        ZkVmType::Risc0
    }

    async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult> {
        let elf_owned = elf.to_vec();
        let input_owned = input.to_vec();

        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let env = ExecutorEnv::builder()
                .write_slice(&input_owned)
                .build()
                .map_err(|e| anyhow!("Failed to build executor env: {}", e))?;

            let executor = default_executor();
            let exec_info = executor
                .execute(env, &elf_owned)
                .map_err(|e| anyhow!("Execution failed: {}", e))?;

            let cycles = exec_info.cycles();
            let segment_count = exec_info.segments.len();
            let memory_estimate_bytes = segment_count * 16 * 1024 * 1024;
            let journal = exec_info.journal.bytes.clone();
            let execution_time = start.elapsed();

            info!(
                "risc0 execution: {} cycles, {} segments, {:?}",
                cycles, segment_count, execution_time
            );

            Ok(ExecutionResult {
                journal,
                cycles,
                memory_estimate_bytes,
                segment_count,
                execution_time,
            })
        })
        .await?
    }

    async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let start = Instant::now();
        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove(elf, input).await?;

        Ok(ProofResult {
            seal,
            journal,
            cycles: 0, // Not tracked in this path
            prove_time: start.elapsed(),
        })
    }

    async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let start = Instant::now();
        let prover = UnifiedProver::new(self.proving_mode.clone())?;
        let (seal, journal) = prover.prove_with_snark(elf, input, true).await?;

        Ok(ProofResult {
            seal,
            journal,
            cycles: 0,
            prove_time: start.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risc0_backend_new() {
        let backend = Risc0Backend::new(ProvingMode::Local);
        assert_eq!(backend.name(), "risc0");
        assert_eq!(backend.vm_type(), ZkVmType::Risc0);
    }
}
