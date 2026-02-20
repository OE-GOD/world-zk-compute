//! Multi-VM Router
//!
//! Auto-detects whether an ELF binary targets risc0 or SP1, then
//! dispatches execution and proving to the correct backend.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use tracing::info;
#[cfg(feature = "jolt")]
use tracing::warn;

use crate::bonsai::ProvingMode;
use crate::risc0_backend::Risc0Backend;
use crate::zkvm_backend::{detect_vm_type, ExecutionResult, ProofResult, ZkVmBackend, ZkVmType};

/// Multi-VM proving router.
///
/// Maintains a map of `ZkVmType → Backend` and dispatches based on
/// auto-detection of the ELF binary. Always registers Risc0; optionally
/// registers SP1 when the `sp1` feature is enabled and initialization succeeds.
pub struct MultiVmProver {
    backends: HashMap<ZkVmType, Box<dyn ZkVmBackend>>,
}

impl MultiVmProver {
    /// Create a new multi-VM router.
    ///
    /// Always registers the Risc0 backend. If `sp1` feature is enabled,
    /// attempts to register the SP1 backend (logs a warning on failure).
    pub fn new(proving_mode: ProvingMode) -> Self {
        let mut backends: HashMap<ZkVmType, Box<dyn ZkVmBackend>> = HashMap::new();

        // Always register risc0
        backends.insert(ZkVmType::Risc0, Box::new(Risc0Backend::new(proving_mode)));
        info!("Multi-VM: risc0 backend registered");

        // Conditionally register SP1
        #[cfg(feature = "sp1")]
        {
            match crate::sp1_prover::Sp1Backend::new() {
                Ok(sp1) => {
                    backends.insert(ZkVmType::Sp1, Box::new(sp1));
                    info!("Multi-VM: SP1 backend registered");
                }
                Err(e) => {
                    warn!(
                        "Multi-VM: SP1 backend init failed (SP1 programs won't be provable): {}",
                        e
                    );
                }
            }
        }

        #[cfg(not(feature = "sp1"))]
        {
            info!("Multi-VM: SP1 backend not available (compile with --features sp1)");
        }

        // Conditionally register Jolt
        #[cfg(feature = "jolt")]
        {
            match crate::jolt_backend::JoltBackend::new() {
                Ok(jolt) => {
                    backends.insert(ZkVmType::Jolt, Box::new(jolt));
                    info!("Multi-VM: Jolt backend registered (experimental)");
                }
                Err(e) => {
                    warn!("Multi-VM: Jolt backend init failed: {}", e);
                }
            }
        }

        #[cfg(not(feature = "jolt"))]
        {
            info!("Multi-VM: Jolt backend not available (compile with --features jolt)");
        }

        Self { backends }
    }

    /// Get the backend for a given VM type.
    fn get_backend(&self, vm_type: ZkVmType) -> Result<&dyn ZkVmBackend> {
        self.backends
            .get(&vm_type)
            .map(|b| b.as_ref())
            .ok_or_else(|| {
                anyhow!(
                    "No backend registered for {}. {}",
                    vm_type,
                    match vm_type {
                        ZkVmType::Sp1 => "Compile with --features sp1 to enable SP1 support.",
                        ZkVmType::Jolt => "Compile with --features jolt to enable Jolt support.",
                        _ => "This should not happen.",
                    }
                )
            })
    }

    /// Detect VM type and execute without proving.
    pub async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult> {
        let vm_type = detect_vm_type(elf);
        info!("Multi-VM: detected {} program, executing...", vm_type);
        let backend = self.get_backend(vm_type)?;
        backend.execute(elf, input).await
    }

    /// Detect VM type and generate a proof.
    pub async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let vm_type = detect_vm_type(elf);
        info!("Multi-VM: detected {} program, proving...", vm_type);
        let backend = self.get_backend(vm_type)?;
        backend.prove(elf, input).await
    }

    /// Detect VM type and generate an on-chain verifiable SNARK proof.
    pub async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let vm_type = detect_vm_type(elf);
        info!(
            "Multi-VM: detected {} program, proving with SNARK...",
            vm_type
        );
        let backend = self.get_backend(vm_type)?;
        backend.prove_with_snark(elf, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_vm_new() {
        let router = MultiVmProver::new(ProvingMode::Local);
        // Risc0 backend should always be available
        assert!(router.get_backend(ZkVmType::Risc0).is_ok());
    }

    #[test]
    fn test_multi_vm_detect_risc0() {
        let elf = vec![0x7f, 0x45, 0x4c, 0x46]; // standard ELF
        assert_eq!(detect_vm_type(&elf), ZkVmType::Risc0);
    }

    #[test]
    fn test_get_backend_missing() {
        let router = MultiVmProver::new(ProvingMode::Local);
        // Without sp1 feature, SP1 backend won't be available
        #[cfg(not(feature = "sp1"))]
        assert!(router.get_backend(ZkVmType::Sp1).is_err());
    }
}
