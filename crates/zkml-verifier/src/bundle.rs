use serde::{Deserialize, Serialize};

use crate::error::{Result, VerifyError};

/// A self-contained proof bundle with everything needed for verification.
///
/// All binary fields are hex-encoded (with optional "0x" prefix).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofBundle {
    /// Hex-encoded proof data (including "REM1" selector prefix).
    pub proof_hex: String,

    /// Hex-encoded public inputs (may be empty; verifier uses embedded inputs).
    #[serde(default)]
    pub public_inputs_hex: String,

    /// Hex-encoded Pedersen generators.
    pub gens_hex: String,

    /// DAG circuit description as structured JSON.
    pub dag_circuit_description: serde_json::Value,
}

impl ProofBundle {
    /// Parse a bundle from a JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| VerifyError::BundleParse(e.to_string()))
    }

    /// Parse a bundle from a JSON file path.
    pub fn from_file(path: &str) -> Result<Self> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| VerifyError::BundleParse(format!("read {}: {}", path, e)))?;
        Self::from_json(&json)
    }

    /// Decode proof_hex to bytes.
    pub fn proof_data(&self) -> Result<Vec<u8>> {
        decode_hex(&self.proof_hex, "proof_hex")
    }

    /// Decode public_inputs_hex to bytes.
    pub fn public_inputs_data(&self) -> Result<Vec<u8>> {
        if self.public_inputs_hex.is_empty() {
            return Ok(Vec::new());
        }
        decode_hex(&self.public_inputs_hex, "public_inputs_hex")
    }

    /// Decode gens_hex to bytes.
    pub fn gens_data(&self) -> Result<Vec<u8>> {
        decode_hex(&self.gens_hex, "gens_hex")
    }
}

/// Decode a hex string (with optional "0x" prefix) to bytes.
fn decode_hex(hex_str: &str, field_name: &str) -> Result<Vec<u8>> {
    let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    hex::decode(stripped).map_err(|e| VerifyError::BundleParse(format!("{}: {}", field_name, e)))
}

/// Result of successful verification.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// The 32-byte circuit hash extracted from the proof.
    pub circuit_hash: [u8; 32],

    /// Whether the full verification (including EC checks) passed.
    pub verified: bool,
}

/// Result of hybrid verification (transcript replay only, no EC).
#[derive(Debug, Clone)]
pub struct HybridVerificationResult {
    /// Circuit hash (first 32 bytes of proof body).
    pub circuit_hash: [u8; 32],

    /// Sponge state digest after all transcript operations.
    pub transcript_digest: [u8; 32],

    /// Per compute layer Fr outputs.
    pub compute_fr: crate::gkr::FrOutputCollector,

    /// Input layer Fr outputs.
    pub input_fr: crate::hyrax::InputFrOutputs,
}
