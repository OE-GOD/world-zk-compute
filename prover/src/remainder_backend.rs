//! Remainder (GKR+Hyrax) Proving Backend -- **Experimental Placeholder**
//!
//! This backend is a structural skeleton for future Remainder integration.
//! It does NOT contain a real GKR prover. All proving methods return errors
//! unless the `remainder-experimental` Cargo feature is enabled, and even
//! then the prover is a placeholder that cannot produce valid proofs.
//!
//! **Do not use in production.** This module exists so that the multi-VM
//! router can compile with `--features remainder` and route Remainder
//! circuit requests, but actual proof generation requires integrating the
//! Remainder_CE crate (not yet available as a public dependency).
//!
//! Implements the ZkVmBackend trait for the Remainder proof system.
//! Unlike risc0/SP1 which are general-purpose zkVMs, Remainder operates
//! on pre-compiled GKR circuits (not ELF binaries). The "elf" parameter
//! is reinterpreted as a serialized circuit description.
//!
//! Proof flow (when Remainder_CE is integrated):
//! 1. Deserialize circuit description from "elf" bytes
//! 2. Deserialize witness/input from "input" bytes
//! 3. Run GKR prover to generate Hyrax-committed proof
//! 4. ABI-encode the proof for on-chain verification

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::time::Instant;
use tracing::warn;

use crate::zkvm_backend::{ExecutionResult, ProofResult, ZkVmBackend};

/// Guard macro: returns an error if the `remainder-experimental` feature is not enabled.
/// This prevents accidental use of the placeholder backend in production.
macro_rules! require_experimental {
    ($method:expr) => {
        if cfg!(not(feature = "remainder-experimental")) {
            return Err(anyhow!(
                "RemainderBackend::{}() is disabled. This backend is an experimental \
                 placeholder that cannot produce valid proofs. Enable the \
                 'remainder-experimental' Cargo feature to use it for testing. \
                 See prover/Cargo.toml for details.",
                $method
            ));
        }
    };
}

/// Remainder GKR+Hyrax proving backend (**experimental placeholder**).
///
/// This backend handles Remainder circuit proofs, which are structurally
/// different from zkVM proofs:
/// - No ELF binary execution (circuits are pre-compiled)
/// - Proof structure is GKR layers + Hyrax PCS (not STARK/Groth16)
/// - Verification uses custom Solidity verifier (not risc0 verifier)
///
/// **WARNING:** This is a placeholder. It does not contain a real GKR prover.
/// All methods return errors unless `remainder-experimental` feature is enabled,
/// and even then the proof output is a dummy that will NOT verify on-chain.
pub struct RemainderBackend {
    // Configuration fields will be added when Remainder_CE is integrated
}

impl RemainderBackend {
    pub fn new() -> Result<Self> {
        Ok(Self {})
    }
}

#[async_trait]
impl ZkVmBackend for RemainderBackend {
    /// Execute the circuit without generating a proof.
    ///
    /// For Remainder, "execution" means evaluating the GKR circuit
    /// on the given input to produce public outputs.
    ///
    /// The `elf` parameter contains the serialized circuit description.
    /// The `input` parameter contains the serialized witness values.
    ///
    /// Returns an error unless the `remainder-experimental` feature is enabled.
    async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult> {
        require_experimental!("execute");
        let start = Instant::now();

        // Validate circuit description header
        if elf.len() < 20 || &elf[..20] != b"REMAINDER_XGBOOST_V1" {
            return Err(anyhow!(
                "Invalid Remainder circuit description (expected REMAINDER_XGBOOST_V1 header)"
            ));
        }

        // Parse the circuit description to determine structure
        let num_features = u32::from_be_bytes(elf[20..24].try_into()?) as usize;

        // Parse the input witness
        // Input format: JSON with "features" array and "expected_class"
        let input_str =
            std::str::from_utf8(input).map_err(|_| anyhow!("Input must be valid UTF-8 JSON"))?;
        let input_json: serde_json::Value = serde_json::from_str(input_str)?;

        let features = input_json["features"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing 'features' array in input"))?;

        if features.len() != num_features {
            return Err(anyhow!(
                "Feature count mismatch: circuit expects {}, got {}",
                num_features,
                features.len()
            ));
        }

        // For execution, we produce the public outputs (prediction)
        // without generating a proof. The actual GKR circuit evaluation
        // would happen here when Remainder_CE is integrated.
        let predicted_class = input_json["expected_class"].as_u64().unwrap_or(0) as u32;

        let journal = predicted_class.to_be_bytes().to_vec();

        Ok(ExecutionResult {
            journal,
            cycles: 0, // Not applicable for Remainder
            memory_estimate_bytes: elf.len() + input.len(),
            segment_count: 0,
            execution_time: start.elapsed(),
        })
    }

    /// Generate a GKR+Hyrax proof.
    ///
    /// **Currently a placeholder** -- returns an error explaining that the
    /// Remainder_CE prover is not yet integrated. When integrated, this will
    /// produce a proof verifiable by RemainderVerifier.sol containing:
    /// - Per-layer sumcheck transcripts
    /// - Hyrax polynomial commitment evaluation proofs
    /// - Fiat-Shamir transcript (Poseidon-based)
    ///
    /// Returns an error unless the `remainder-experimental` feature is enabled.
    async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        warn!("RemainderBackend::prove() producing PLACEHOLDER proof — not a real GKR proof. Do not use in production.");
        require_experimental!("prove");
        let start = Instant::now();

        // First execute to get the outputs
        let exec_result = self.execute(elf, input).await?;

        // Integration point for Remainder_CE (not yet available as a public dependency):
        //
        // ```rust
        // use remainder::prelude::*;
        //
        // let circuit = CircuitDescription::deserialize(elf)?;
        // let witness = Witness::deserialize(input)?;
        // let proof = circuit.prove::<BN254, PoseidonSponge>(&witness)?;
        // let seal = bincode::serialize(&proof)?;
        // ```

        // Return an error: real proof generation is not yet implemented.
        // The execute() above validates inputs; this error indicates that
        // the prover itself is missing, not that the inputs are invalid.
        Err(anyhow!(
            "RemainderBackend::prove() cannot produce valid proofs yet. \
             The Remainder_CE prover crate is not integrated. \
             This placeholder backend validates inputs but cannot generate \
             real GKR+Hyrax proofs. Execution result had {} journal bytes, \
             took {:?}.",
            exec_result.journal.len(),
            start.elapsed()
        ))
    }

    /// Generate an on-chain verifiable proof.
    ///
    /// For Remainder, the base GKR+Hyrax proof is already on-chain verifiable
    /// (unlike STARK proofs which need Groth16 wrapping). This method
    /// returns the same proof as `prove()`, but ABI-encoded.
    ///
    /// Future: Could wrap in Groth16 for cheaper verification (~200K gas).
    ///
    /// Returns an error unless the `remainder-experimental` feature is enabled.
    async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        require_experimental!("prove_with_snark");
        // For now, Remainder proofs are directly verifiable on-chain
        // without SNARK compression. The GKR verifier contract handles
        // verification natively.
        self.prove(elf, input).await
    }
}

// NOTE: The previous `generate_remainder_proof_seal()` function has been removed.
// It generated a fake 1024-byte zero-filled proof that could never verify on-chain,
// creating a false sense of functionality. When Remainder_CE is integrated, real
// proof generation will be added to the `prove()` method directly.

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_circuit_desc() -> Vec<u8> {
        let mut desc = Vec::new();
        desc.extend_from_slice(b"REMAINDER_XGBOOST_V1");
        desc.extend_from_slice(&5u32.to_be_bytes()); // num_features
        desc.extend_from_slice(&2u32.to_be_bytes()); // num_classes
        desc.extend_from_slice(&3u32.to_be_bytes()); // max_depth
        desc.extend_from_slice(&2u32.to_be_bytes()); // num_trees
        desc
    }

    fn sample_input() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "features": [0.6, 0.2, 0.8, 0.5, 0.3],
            "expected_class": 1
        }))
        .unwrap()
    }

    /// Without `remainder-experimental`, execute() returns a clear error.
    #[tokio::test]
    #[cfg(not(feature = "remainder-experimental"))]
    async fn test_execute_blocked_without_experimental() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend
            .execute(&sample_circuit_desc(), &sample_input())
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("remainder-experimental"),
            "error should mention the feature flag: {msg}"
        );
    }

    /// With `remainder-experimental`, execute() validates inputs and returns results.
    #[tokio::test]
    #[cfg(feature = "remainder-experimental")]
    async fn test_execute() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend
            .execute(&sample_circuit_desc(), &sample_input())
            .await;
        assert!(result.is_ok());
        let exec = result.unwrap();
        // Journal should contain the predicted class
        assert_eq!(exec.journal.len(), 4);
    }

    /// prove() returns an error explaining the prover is not yet integrated.
    /// With `remainder-experimental`, it validates inputs first then errors on missing prover.
    /// Without `remainder-experimental`, it errors immediately on the feature guard.
    #[tokio::test]
    async fn test_prove_returns_error() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend.prove(&sample_circuit_desc(), &sample_input()).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        // Should mention either the feature gate or the missing prover
        assert!(
            msg.contains("remainder-experimental") || msg.contains("cannot produce valid proofs"),
            "error should be informative: {msg}"
        );
    }

    /// Invalid circuit description is caught (with experimental enabled).
    #[tokio::test]
    #[cfg(feature = "remainder-experimental")]
    async fn test_execute_invalid_circuit() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend.execute(b"INVALID", &sample_input()).await;
        assert!(result.is_err());
    }

    /// Feature count mismatch is caught (with experimental enabled).
    #[tokio::test]
    #[cfg(feature = "remainder-experimental")]
    async fn test_execute_feature_mismatch() {
        let backend = RemainderBackend::new().unwrap();
        let bad_input = serde_json::to_vec(&serde_json::json!({
            "features": [0.1, 0.2],  // Only 2 features, circuit expects 5
            "expected_class": 0
        }))
        .unwrap();
        let result = backend.execute(&sample_circuit_desc(), &bad_input).await;
        assert!(result.is_err());
    }

    /// Without `remainder-experimental`, invalid circuit still gets the feature guard error first.
    #[tokio::test]
    #[cfg(not(feature = "remainder-experimental"))]
    async fn test_execute_invalid_circuit_blocked() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend.execute(b"INVALID", &sample_input()).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("remainder-experimental"));
    }
}
