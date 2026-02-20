//! Jolt Proving Backend (feature-gated, experimental)
//!
//! Implements `ZkVmBackend` for a]6z's Jolt zkVM.
//! The entire module is behind `#[cfg(feature = "jolt")]`.
//!
//! Jolt is currently alpha-stage. Its programmatic API for loading arbitrary
//! ELF binaries (outside the `#[jolt::provable]` macro) is not yet stable.
//! This backend is a fully-wired skeleton with placeholder implementations
//! that return clear errors, ready to fill in once the API stabilizes.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use crate::zkvm_backend::{ExecutionResult, ProofResult, ZkVmBackend};

/// Jolt proving backend (experimental skeleton).
///
/// Jolt does not yet have a stable programmatic API for loading arbitrary
/// RISC-V ELF binaries. This backend is wired into the multi-VM router
/// and will return informative errors until the API stabilizes.
pub struct JoltBackend;

impl JoltBackend {
    /// Create a new Jolt backend.
    ///
    /// Logs an experimental warning. Always succeeds since there is no
    /// client to initialize (Jolt has no persistent prover state).
    pub fn new() -> Result<Self> {
        warn!(
            "Jolt backend initialized (EXPERIMENTAL). \
             Jolt's programmatic ELF loading API is not yet stable. \
             Proving will return errors until jolt-sdk stabilizes."
        );
        info!("Jolt backend ready (skeleton — awaiting stable jolt-sdk API)");
        Ok(Self)
    }
}

#[async_trait]
impl ZkVmBackend for JoltBackend {
    async fn execute(&self, _elf: &[u8], _input: &[u8]) -> Result<ExecutionResult> {
        // Jolt's programmatic API for loading arbitrary ELF binaries is not
        // yet stable. The `#[jolt::provable]` macro works for integrated
        // builds, but runtime ELF loading requires API stabilization.
        tokio::task::spawn_blocking(|| {
            Err(anyhow!(
                "Jolt execute() is not yet available: \
                 waiting for stable jolt-sdk programmatic ELF loading API. \
                 See https://github.com/a16z/jolt for progress."
            ))
        })
        .await?
    }

    async fn prove(&self, _elf: &[u8], _input: &[u8]) -> Result<ProofResult> {
        // Same as execute — requires stable programmatic API
        tokio::task::spawn_blocking(|| {
            Err(anyhow!(
                "Jolt prove() is not yet available: \
                 waiting for stable jolt-sdk programmatic ELF loading API. \
                 See https://github.com/a16z/jolt for progress."
            ))
        })
        .await?
    }

    async fn prove_with_snark(&self, _elf: &[u8], _input: &[u8]) -> Result<ProofResult> {
        // Jolt does not support Groth16 wrapping yet. Its native proof
        // system uses sumcheck + Binius commitments, which don't have an
        // on-chain Groth16 verifier wrapper.
        tokio::task::spawn_blocking(|| {
            Err(anyhow!(
                "Jolt does not yet support Groth16/SNARK proof wrapping. \
                 Jolt proofs use sumcheck-based verification which is not \
                 yet compatible with on-chain Groth16 verifiers."
            ))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jolt_backend_new() {
        let backend = JoltBackend::new();
        assert!(backend.is_ok());
    }

    #[tokio::test]
    async fn test_jolt_execute_returns_error() {
        let backend = JoltBackend::new().unwrap();
        let result = backend.execute(&[], &[]).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not yet available"));
        assert!(err_msg.contains("jolt-sdk"));
    }

    #[tokio::test]
    async fn test_jolt_prove_returns_error() {
        let backend = JoltBackend::new().unwrap();
        let result = backend.prove(&[], &[]).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not yet available"));
    }

    #[tokio::test]
    async fn test_jolt_prove_with_snark_returns_groth16_error() {
        let backend = JoltBackend::new().unwrap();
        let result = backend.prove_with_snark(&[], &[]).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Groth16"));
        assert!(err_msg.contains("sumcheck"));
    }
}
