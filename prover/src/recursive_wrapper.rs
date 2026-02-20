//! Recursive Verification Wrapper
//!
//! Wraps K sub-proof receipts from input decomposition into a single
//! proof by running a wrapper guest that calls `env::verify()` on each
//! sub-proof inside the zkVM.
//!
//! This closes the trust gap where journals were previously merged outside
//! the zkVM — a malicious prover could have omitted or fabricated journals.
//! With recursive verification, the merge is proven correct.
//!
//! ## Flow
//!
//! ```text
//! [K sub-proof Receipts] → wrapper guest (verifies each via env::verify)
//!                        → single wrapper Receipt
//!                        → extract_seal() → one on-chain proof
//! ```

use anyhow::{anyhow, Result};
use risc0_zkvm::{default_prover, ExecutorEnv, ProverOpts, Receipt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::input_decomposer::DecompositionStrategy;
use crate::prover::extract_seal;

// ═══════════════════════════════════════════════════════════════════════════════
// Shared Types (mirror the guest program's types)
// ═══════════════════════════════════════════════════════════════════════════════

/// Input to the wrapper guest program.
///
/// Must match `WrapperInput` in the guest exactly.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WrapperInput {
    /// Image ID of the inner guest program.
    pub inner_image_id: [u8; 32],
    /// Number of sub-proofs to verify.
    pub sub_proof_count: u32,
    /// Journals from each sub-proof.
    pub sub_journals: Vec<Vec<u8>>,
    /// Pre-merged journal from the decomposition strategy.
    pub merged_journal: Vec<u8>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for the recursive wrapper.
#[derive(Clone, Debug)]
pub struct RecursiveWrapperConfig {
    /// Path to the wrapper guest ELF binary.
    pub wrapper_elf_path: PathBuf,
    /// Whether to generate SNARK (Groth16) proofs for the wrapper.
    pub use_snark: bool,
    /// Maximum number of sub-proofs the wrapper can handle.
    pub max_sub_proofs: usize,
}

impl Default for RecursiveWrapperConfig {
    fn default() -> Self {
        Self {
            wrapper_elf_path: PathBuf::from("./programs/recursive-wrapper.elf"),
            use_snark: false,
            max_sub_proofs: 16,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Wrap Result
// ═══════════════════════════════════════════════════════════════════════════════

/// Result of recursive wrapping.
#[derive(Debug)]
#[allow(dead_code)]
pub struct RecursiveWrapResult {
    /// The wrapper Receipt (proves all sub-proofs valid + merge).
    pub wrapper_receipt: Receipt,
    /// Seal extracted from the wrapper receipt (for on-chain submission).
    pub wrapper_seal: Vec<u8>,
    /// Journal from the wrapper receipt.
    pub wrapper_journal: Vec<u8>,
    /// The merged journal from all sub-computations.
    pub merged_journal: Vec<u8>,
    /// First sub-proof's seal (for V1 on-chain submission with original imageId).
    pub first_sub_seal: Vec<u8>,
    /// First sub-proof's journal.
    pub first_sub_journal: Vec<u8>,
    /// Time taken for the wrapper proof.
    pub wrap_time: Duration,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Recursive Wrapper
// ═══════════════════════════════════════════════════════════════════════════════

/// Orchestrates recursive verification of sub-proof receipts.
pub struct RecursiveWrapper {
    config: RecursiveWrapperConfig,
    wrapper_elf: Vec<u8>,
}

impl RecursiveWrapper {
    /// Create a new recursive wrapper, loading the wrapper ELF from disk.
    pub fn new(config: RecursiveWrapperConfig) -> Result<Self> {
        let wrapper_elf = std::fs::read(&config.wrapper_elf_path).map_err(|e| {
            anyhow!(
                "Failed to load wrapper ELF from {:?}: {}",
                config.wrapper_elf_path,
                e
            )
        })?;

        info!(
            "Recursive wrapper loaded: {} bytes from {:?}",
            wrapper_elf.len(),
            config.wrapper_elf_path
        );

        Ok(Self {
            config,
            wrapper_elf,
        })
    }

    /// Wrap K sub-proof receipts into a single recursive proof.
    ///
    /// 1. Extracts journals from each sub-receipt
    /// 2. Merges journals via the decomposition strategy
    /// 3. Builds ExecutorEnv with wrapper input + sub-receipt assumptions
    /// 4. Proves the wrapper guest → single Receipt
    /// 5. Extracts seal and returns the result
    pub async fn wrap(
        &self,
        inner_image_id: [u8; 32],
        sub_receipts: Vec<Receipt>,
        strategy: &dyn DecompositionStrategy,
    ) -> Result<RecursiveWrapResult> {
        let start = Instant::now();
        let sub_count = sub_receipts.len();

        if sub_count == 0 {
            return Err(anyhow!("No sub-receipts to wrap"));
        }

        if sub_count > self.config.max_sub_proofs {
            return Err(anyhow!(
                "Too many sub-proofs: {} exceeds max {}",
                sub_count,
                self.config.max_sub_proofs
            ));
        }

        info!(
            "Recursive wrapper: wrapping {} sub-proofs for image {:?}",
            sub_count,
            hex::encode(inner_image_id)
        );

        // Extract journals from sub-receipts
        let sub_journals: Vec<Vec<u8>> = sub_receipts
            .iter()
            .map(|r| r.journal.bytes.clone())
            .collect();

        // Save first sub-proof's seal and journal for V1 on-chain fallback
        let first_sub_seal = extract_seal(&sub_receipts[0])?;
        let first_sub_journal = sub_journals[0].clone();

        // Merge journals using the decomposition strategy
        let merged_journal = strategy.merge_journals(&sub_journals)?;

        // Build wrapper input
        let wrapper_input = WrapperInput {
            inner_image_id,
            sub_proof_count: sub_count as u32,
            sub_journals,
            merged_journal: merged_journal.clone(),
        };

        // Build executor environment with assumptions
        let wrapper_elf = self.wrapper_elf.clone();
        let use_snark = self.config.use_snark;

        let wrapper_receipt = tokio::task::spawn_blocking(move || {
            let mut env_builder = ExecutorEnv::builder();

            // Write the wrapper input
            env_builder
                .write(&wrapper_input)
                .map_err(|e| anyhow!("Failed to write wrapper input: {}", e))?;

            // Add each sub-receipt as an assumption so env::verify() can find them
            for (i, receipt) in sub_receipts.into_iter().enumerate() {
                env_builder.add_assumption(receipt);
                info!("Added sub-receipt {} as assumption", i);
            }

            let env = env_builder
                .build()
                .map_err(|e| anyhow!("Failed to build wrapper env: {}", e))?;

            let prover = default_prover();

            let receipt = if use_snark {
                if cfg!(not(target_arch = "x86_64")) {
                    anyhow::bail!("Groth16 SNARK proving requires x86_64 architecture");
                }
                info!("Generating Groth16 wrapper proof...");
                let opts = ProverOpts::groth16();
                let prove_info = prover
                    .prove_with_opts(env, &wrapper_elf, &opts)
                    .map_err(|e| anyhow!("Wrapper Groth16 proving failed: {}", e))?;
                prove_info.receipt
            } else {
                let prove_info = prover
                    .prove(env, &wrapper_elf)
                    .map_err(|e| anyhow!("Wrapper proving failed: {}", e))?;
                prove_info.receipt
            };

            Ok::<Receipt, anyhow::Error>(receipt)
        })
        .await??;

        let wrapper_seal = extract_seal(&wrapper_receipt)?;
        let wrapper_journal = wrapper_receipt.journal.bytes.clone();
        let wrap_time = start.elapsed();

        info!(
            "Recursive wrapper proof complete: {} sub-proofs verified, \
             seal={} bytes, journal={} bytes, time={:?}",
            sub_count,
            wrapper_seal.len(),
            wrapper_journal.len(),
            wrap_time,
        );

        Ok(RecursiveWrapResult {
            wrapper_receipt,
            wrapper_seal,
            wrapper_journal,
            merged_journal,
            first_sub_seal,
            first_sub_journal,
            wrap_time,
        })
    }
}

/// Try to load and run the recursive wrapper.
///
/// Returns `None` if the wrapper ELF is not available (graceful degradation).
pub fn try_load_wrapper(use_snark: bool) -> Option<RecursiveWrapper> {
    let config = RecursiveWrapperConfig {
        use_snark,
        ..Default::default()
    };

    match RecursiveWrapper::new(config) {
        Ok(wrapper) => {
            info!("Recursive wrapper available");
            Some(wrapper)
        }
        Err(e) => {
            warn!(
                "Recursive wrapper not available (this is OK, will use first sub-proof): {}",
                e
            );
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = RecursiveWrapperConfig::default();
        assert_eq!(
            config.wrapper_elf_path,
            PathBuf::from("./programs/recursive-wrapper.elf")
        );
        assert!(!config.use_snark);
        assert_eq!(config.max_sub_proofs, 16);
    }

    #[test]
    fn test_wrapper_input_serialization() {
        let input = WrapperInput {
            inner_image_id: [0xAA; 32],
            sub_proof_count: 3,
            sub_journals: vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]],
            merged_journal: vec![10, 11, 12],
        };

        // Round-trip through bincode
        let encoded = bincode::serialize(&input).unwrap();
        let decoded: WrapperInput = bincode::deserialize(&encoded).unwrap();

        assert_eq!(decoded.inner_image_id, [0xAA; 32]);
        assert_eq!(decoded.sub_proof_count, 3);
        assert_eq!(decoded.sub_journals.len(), 3);
        assert_eq!(decoded.merged_journal, vec![10, 11, 12]);
    }

    #[test]
    fn test_try_load_wrapper_missing_elf() {
        // When wrapper ELF doesn't exist, should return None gracefully
        let result = try_load_wrapper(false);
        assert!(result.is_none());
    }
}
