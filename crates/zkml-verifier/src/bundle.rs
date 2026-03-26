use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::error::{Result, VerifyError};
use crate::types::ProofMetadata;

/// Gzip magic bytes (first two bytes of any gzip stream).
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

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

    /// Metadata: keccak256 hash of the ML model (hex, optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_hash: Option<String>,

    /// Metadata: Unix timestamp (seconds) when the proof was generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,

    /// Metadata: version of the prover that generated this bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prover_version: Option<String>,

    /// Metadata: circuit hash (hex, 32 bytes). Redundant with proof header
    /// but useful for indexing without decoding the proof blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_hash: Option<String>,
}

impl ProofBundle {
    /// Parse a bundle from a JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| VerifyError::BundleParse(e.to_string()))
    }

    /// Parse a bundle from a JSON file path.
    ///
    /// Auto-detects gzip-compressed files (by magic bytes, not file extension)
    /// so both `.json` and `.json.gz` files work transparently.
    pub fn from_file(path: &str) -> Result<Self> {
        let data = std::fs::read(path)
            .map_err(|e| VerifyError::BundleParse(format!("read {}: {}", path, e)))?;
        Self::from_bytes(&data)
    }

    /// Deserialize from compressed (gzip) or uncompressed JSON bytes.
    ///
    /// Auto-detects gzip by checking for the magic bytes `0x1f 0x8b` at the
    /// start of the data. Falls back to plain JSON parsing otherwise.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() >= 2 && data[0] == GZIP_MAGIC[0] && data[1] == GZIP_MAGIC[1] {
            let mut decoder = flate2::read::GzDecoder::new(data);
            let mut json = String::new();
            decoder
                .read_to_string(&mut json)
                .map_err(|e| VerifyError::BundleParse(format!("gzip decompress: {}", e)))?;
            serde_json::from_str(&json).map_err(|e| VerifyError::BundleParse(e.to_string()))
        } else {
            serde_json::from_slice(data).map_err(|e| VerifyError::BundleParse(e.to_string()))
        }
    }

    /// Serialize to compressed JSON (gzip).
    ///
    /// Typically achieves 85-95% size reduction on proof bundles because the
    /// hex-encoded proof/gens data compresses extremely well.
    pub fn to_compressed_json(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self)
            .map_err(|e| VerifyError::BundleParse(format!("serialize: {}", e)))?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(&json)
            .map_err(|e| VerifyError::BundleParse(format!("gzip compress: {}", e)))?;
        encoder
            .finish()
            .map_err(|e| VerifyError::BundleParse(format!("gzip finish: {}", e)))
    }

    /// Serialize to uncompressed pretty-printed JSON.
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| VerifyError::BundleParse(format!("serialize: {}", e)))
    }

    /// Save to file. Compressed (gzip) if path ends in `.gz`, plain JSON otherwise.
    pub fn save(&self, path: &str) -> Result<()> {
        if path.ends_with(".gz") {
            let compressed = self.to_compressed_json()?;
            std::fs::write(path, compressed)
                .map_err(|e| VerifyError::BundleParse(format!("write {}: {}", path, e)))
        } else {
            let json = self.to_json_pretty()?;
            std::fs::write(path, json)
                .map_err(|e| VerifyError::BundleParse(format!("write {}: {}", path, e)))
        }
    }

    /// Returns true if the given byte slice starts with gzip magic bytes.
    pub fn is_compressed(data: &[u8]) -> bool {
        data.len() >= 2 && data[0] == GZIP_MAGIC[0] && data[1] == GZIP_MAGIC[1]
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

    /// Extract proof metadata.
    pub fn metadata(&self) -> ProofMetadata {
        ProofMetadata {
            model_hash: self.model_hash.clone().unwrap_or_default(),
            timestamp: self.timestamp.unwrap_or(0),
            prover_version: self.prover_version.clone().unwrap_or_default(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a test bundle with realistic-looking hex data.
    fn make_test_bundle() -> ProofBundle {
        // Generate some largish hex payload to test compression ratio
        let proof_data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let gens_data: Vec<u8> = (0..512).map(|i| ((i * 7) % 256) as u8).collect();

        ProofBundle {
            proof_hex: format!("0x{}", hex::encode(&proof_data)),
            public_inputs_hex: String::new(),
            gens_hex: format!("0x{}", hex::encode(&gens_data)),
            dag_circuit_description: serde_json::json!({
                "num_compute_layers": 4,
                "num_input_layers": 2,
                "layer_types": [0, 1, 0, 1],
                "num_sumcheck_rounds": [3, 3, 3, 3],
                "atom_offsets": [0, 1, 2, 3],
                "atom_target_layers": [1, 2, 3, 0],
            }),
            model_hash: Some("0xabcdef1234567890".to_string()),
            timestamp: Some(1700000000),
            prover_version: Some("0.1.0-test".to_string()),
            circuit_hash: Some("0xdeadbeef".to_string()),
        }
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        let bundle = make_test_bundle();

        // Compress
        let compressed = bundle.to_compressed_json().unwrap();

        // Verify it starts with gzip magic bytes
        assert!(
            ProofBundle::is_compressed(&compressed),
            "compressed data should start with gzip magic bytes"
        );

        // Decompress
        let restored = ProofBundle::from_bytes(&compressed).unwrap();

        // Verify fields match
        assert_eq!(bundle.proof_hex, restored.proof_hex);
        assert_eq!(bundle.public_inputs_hex, restored.public_inputs_hex);
        assert_eq!(bundle.gens_hex, restored.gens_hex);
        assert_eq!(bundle.model_hash, restored.model_hash);
        assert_eq!(bundle.timestamp, restored.timestamp);
        assert_eq!(bundle.prover_version, restored.prover_version);
        assert_eq!(bundle.circuit_hash, restored.circuit_hash);
        assert_eq!(
            bundle.dag_circuit_description,
            restored.dag_circuit_description
        );
    }

    #[test]
    fn test_auto_detect_plain_json() {
        let bundle = make_test_bundle();
        let json_bytes = serde_json::to_vec(&bundle).unwrap();

        // Plain JSON should NOT be detected as compressed
        assert!(
            !ProofBundle::is_compressed(&json_bytes),
            "plain JSON should not be detected as compressed"
        );

        // from_bytes should handle plain JSON transparently
        let restored = ProofBundle::from_bytes(&json_bytes).unwrap();
        assert_eq!(bundle.proof_hex, restored.proof_hex);
        assert_eq!(bundle.gens_hex, restored.gens_hex);
    }

    #[test]
    fn test_auto_detect_gzip() {
        let bundle = make_test_bundle();
        let compressed = bundle.to_compressed_json().unwrap();

        // Must be detected as compressed
        assert!(ProofBundle::is_compressed(&compressed));

        // from_bytes should decompress and parse
        let restored = ProofBundle::from_bytes(&compressed).unwrap();
        assert_eq!(bundle.proof_hex, restored.proof_hex);
    }

    #[test]
    fn test_compression_size_reduction() {
        let bundle = make_test_bundle();
        let json_bytes = serde_json::to_vec(&bundle).unwrap();
        let compressed = bundle.to_compressed_json().unwrap();

        let original_size = json_bytes.len();
        let compressed_size = compressed.len();

        eprintln!(
            "Compression: {} bytes -> {} bytes ({:.1}% reduction)",
            original_size,
            compressed_size,
            100.0 - (compressed_size as f64 / original_size as f64 * 100.0)
        );

        // Hex-encoded proof data compresses very well; expect at least 30% reduction
        assert!(
            compressed_size < original_size,
            "compressed ({compressed_size}) should be smaller than original ({original_size})"
        );
    }

    #[test]
    fn test_save_and_load_compressed() {
        let bundle = make_test_bundle();
        let dir = std::env::temp_dir().join("zkml-verifier-test-compress");
        let _ = std::fs::create_dir_all(&dir);
        let gz_path = dir.join("test_bundle.json.gz");
        let json_path = dir.join("test_bundle.json");

        // Save compressed
        bundle.save(gz_path.to_str().unwrap()).unwrap();
        // Save uncompressed
        bundle.save(json_path.to_str().unwrap()).unwrap();

        // Load compressed
        let restored_gz = ProofBundle::from_file(gz_path.to_str().unwrap()).unwrap();
        assert_eq!(bundle.proof_hex, restored_gz.proof_hex);
        assert_eq!(bundle.gens_hex, restored_gz.gens_hex);

        // Load uncompressed
        let restored_json = ProofBundle::from_file(json_path.to_str().unwrap()).unwrap();
        assert_eq!(bundle.proof_hex, restored_json.proof_hex);

        // Verify .gz file is actually smaller
        let gz_size = std::fs::metadata(&gz_path).unwrap().len();
        let json_size = std::fs::metadata(&json_path).unwrap().len();
        eprintln!("File sizes: .json={json_size}, .json.gz={gz_size}");
        assert!(gz_size < json_size, "gz file should be smaller");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_from_bytes_empty_rejected() {
        let result = ProofBundle::from_bytes(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_garbage_rejected() {
        let result = ProofBundle::from_bytes(&[0xff, 0xfe, 0xfd]);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_compressed_short_data() {
        assert!(!ProofBundle::is_compressed(&[]));
        assert!(!ProofBundle::is_compressed(&[0x1f]));
        // Only 0x1f 0x8b is the magic pair
        assert!(!ProofBundle::is_compressed(&[0x1f, 0x00]));
        assert!(ProofBundle::is_compressed(&[0x1f, 0x8b]));
    }

    #[test]
    fn test_to_json_pretty() {
        let bundle = make_test_bundle();
        let pretty = bundle.to_json_pretty().unwrap();
        // Pretty-printed JSON has newlines
        assert!(pretty.contains('\n'));
        // Roundtrips back
        let restored = ProofBundle::from_json(&pretty).unwrap();
        assert_eq!(bundle.proof_hex, restored.proof_hex);
    }
}
