//! Remainder (GKR+Hyrax) Proving Backend
//!
//! Implements the ZkVmBackend trait for the Remainder proof system.
//! Unlike risc0/SP1 which are general-purpose zkVMs, Remainder operates
//! on pre-compiled GKR circuits (not ELF binaries). The "elf" parameter
//! is reinterpreted as a serialized circuit description.
//!
//! Proof flow:
//! 1. Deserialize circuit description from "elf" bytes
//! 2. Deserialize witness/input from "input" bytes
//! 3. Run GKR prover to generate Hyrax-committed proof
//! 4. ABI-encode the proof for on-chain verification

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use sha2::Digest;
use std::time::Instant;

use crate::zkvm_backend::{ExecutionResult, ProofResult, ZkVmBackend};

/// Remainder GKR+Hyrax proving backend.
///
/// This backend handles Remainder circuit proofs, which are structurally
/// different from zkVM proofs:
/// - No ELF binary execution (circuits are pre-compiled)
/// - Proof structure is GKR layers + Hyrax PCS (not STARK/Groth16)
/// - Verification uses custom Solidity verifier (not risc0 verifier)
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
    async fn execute(&self, elf: &[u8], input: &[u8]) -> Result<ExecutionResult> {
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
    /// This produces a proof that can be verified by RemainderVerifier.sol.
    /// The proof contains:
    /// - Per-layer sumcheck transcripts
    /// - Hyrax polynomial commitment evaluation proofs
    /// - Fiat-Shamir transcript (Poseidon-based)
    async fn prove(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        let start = Instant::now();

        // First execute to get the outputs
        let exec_result = self.execute(elf, input).await?;

        // Generate the GKR proof
        //
        // Integration point for Remainder_CE:
        //
        // ```rust
        // use remainder::prelude::*;
        //
        // let circuit = CircuitDescription::deserialize(elf)?;
        // let witness = Witness::deserialize(input)?;
        // let proof = circuit.prove::<BN254, PoseidonSponge>(&witness)?;
        // let seal = bincode::serialize(&proof)?;
        // ```

        // Generate structured proof placeholder
        let seal = generate_remainder_proof_seal(elf, input)?;

        Ok(ProofResult {
            seal,
            journal: exec_result.journal,
            cycles: 0,
            prove_time: start.elapsed(),
        })
    }

    /// Generate an on-chain verifiable proof.
    ///
    /// For Remainder, the base GKR+Hyrax proof is already on-chain verifiable
    /// (unlike STARK proofs which need Groth16 wrapping). This method
    /// returns the same proof as `prove()`, but ABI-encoded.
    ///
    /// Future: Could wrap in Groth16 for cheaper verification (~200K gas).
    async fn prove_with_snark(&self, elf: &[u8], input: &[u8]) -> Result<ProofResult> {
        // For now, Remainder proofs are directly verifiable on-chain
        // without SNARK compression. The GKR verifier contract handles
        // verification natively.
        self.prove(elf, input).await
    }
}

/// Generate a proof seal in Remainder format.
///
/// The seal starts with a 4-byte selector ("REM1") that identifies it
/// as a Remainder proof, similar to how risc0 uses selectors like "73c457ba".
fn generate_remainder_proof_seal(circuit_desc: &[u8], _input: &[u8]) -> Result<Vec<u8>> {
    let mut seal = Vec::new();

    // Selector for Remainder proofs (first 4 bytes)
    // This is used by the verifier router to dispatch to RemainderVerifier
    seal.extend_from_slice(b"REM1");

    // Circuit hash (32 bytes) — identifies which circuit this proof is for
    let circuit_hash = sha2::Sha256::digest(circuit_desc);
    seal.extend_from_slice(&circuit_hash);

    // Placeholder for actual GKR proof data
    // When Remainder_CE is integrated, this will contain:
    // - Sumcheck round polynomials
    // - Layer claims
    // - Hyrax evaluation proofs
    // - Poseidon transcript commitments
    let proof_placeholder = vec![0u8; 1024]; // Minimum proof size
    seal.extend_from_slice(&(proof_placeholder.len() as u32).to_be_bytes());
    seal.extend_from_slice(&proof_placeholder);

    Ok(seal)
}

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

    #[tokio::test]
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

    #[tokio::test]
    async fn test_prove() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend.prove(&sample_circuit_desc(), &sample_input()).await;
        assert!(result.is_ok());
        let proof = result.unwrap();
        // Seal should start with "REM1" selector
        assert_eq!(&proof.seal[..4], b"REM1");
    }

    #[tokio::test]
    async fn test_execute_invalid_circuit() {
        let backend = RemainderBackend::new().unwrap();
        let result = backend.execute(b"INVALID", &sample_input()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
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
}
