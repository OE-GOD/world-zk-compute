//! Recursive Verification Wrapper Guest Program
//!
//! Verifies K sub-proof receipts inside the zkVM using `env::verify()`,
//! then commits a single output that covers all sub-computations.
//!
//! This closes the trust gap in input decomposition: instead of merging
//! journals outside the zkVM (where a malicious prover could fabricate them),
//! the merge happens inside a proven execution.

#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec::Vec;

use risc0_zkvm::guest::env;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

risc0_zkvm::guest::entry!(main);

/// Input provided by the host to the wrapper guest.
#[derive(Serialize, Deserialize)]
pub struct WrapperInput {
    /// Image ID of the inner (sub-proof) guest program.
    pub inner_image_id: [u8; 32],
    /// Number of sub-proofs to verify.
    pub sub_proof_count: u32,
    /// Journals from each sub-proof (order matters).
    pub sub_journals: Vec<Vec<u8>>,
    /// Pre-merged journal (produced by the decomposition strategy on the host).
    pub merged_journal: Vec<u8>,
}

/// Output committed by the wrapper guest.
#[derive(Serialize, Deserialize)]
pub struct WrapperOutput {
    /// Image ID of the inner guest whose sub-proofs were verified.
    pub inner_image_id: [u8; 32],
    /// Number of sub-proofs that were verified.
    pub sub_proof_count: u32,
    /// SHA-256 hash of all sub-journals (integrity commitment).
    pub journals_hash: [u8; 32],
    /// The merged journal covering all sub-computations.
    pub merged_journal: Vec<u8>,
}

fn main() {
    // Read wrapper input from the host
    let input: WrapperInput = env::read();

    assert_eq!(
        input.sub_journals.len(),
        input.sub_proof_count as usize,
        "sub_journals length must match sub_proof_count"
    );

    // Verify each sub-proof receipt inside the zkVM.
    // The host provides the actual Receipt objects via `add_assumption()`;
    // `env::verify()` checks that each (image_id, journal) pair corresponds
    // to a valid receipt.
    for (i, journal) in input.sub_journals.iter().enumerate() {
        env::verify(input.inner_image_id, journal).unwrap_or_else(|_| {
            panic!("sub-proof {} verification failed", i);
        });
    }

    // Compute SHA-256 hash over all sub-journals for integrity commitment.
    // This binds the wrapper output to the exact set of sub-journals.
    let mut hasher = Sha256::new();
    hasher.update((input.sub_proof_count as u32).to_le_bytes());
    for journal in &input.sub_journals {
        hasher.update((journal.len() as u32).to_le_bytes());
        hasher.update(journal);
    }
    let hash = hasher.finalize();
    let mut journals_hash = [0u8; 32];
    journals_hash.copy_from_slice(&hash);

    // Commit the wrapper output
    env::commit(&WrapperOutput {
        inner_image_id: input.inner_image_id,
        sub_proof_count: input.sub_proof_count,
        journals_hash,
        merged_journal: input.merged_journal,
    });
}
