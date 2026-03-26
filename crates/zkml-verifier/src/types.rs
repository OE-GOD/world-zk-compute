//! Public types for the zkml-verifier crate.
//!
//! This module provides the self-contained proof bundle type (`ProofBundle`),
//! verification result types, and metadata types used by the public API.

use serde::{Deserialize, Serialize};

/// Metadata about the proof (model, prover version, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for ProofMetadata {
    fn default() -> Self {
        Self {
            model_hash: String::new(),
            timestamp: 0,
            prover_version: String::new(),
        }
    }
}
