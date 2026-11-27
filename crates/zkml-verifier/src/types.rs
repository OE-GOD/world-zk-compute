//! Proof metadata type for the public API.
//!
//! This module holds only `ProofMetadata`. The proof bundle type
//! (`ProofBundle`) and the verification result types are in `bundle.rs`.

use serde::{Deserialize, Serialize};

/// Metadata about the proof (model, prover version, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProofMetadata {
    /// Hash (hex) of the model used to generate the proof.
    #[serde(default)]
    pub model_hash: String,

    /// Unix timestamp of when the proof was generated.
    #[serde(default)]
    pub timestamp: u64,

    /// Prover software version string.
    #[serde(default)]
    pub prover_version: String,
}
