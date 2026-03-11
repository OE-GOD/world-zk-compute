//! Hash utilities for computing model_hash, input_hash, and result_hash.
//!
//! These functions produce hashes that are compatible with the TEE enclave's
//! hashing behavior (see `tee/enclave/src/main.rs`). Using these utilities
//! ensures that on-chain verification will match the enclave's attestation.
//!
//! # Example
//!
//! ```rust
//! use world_zk_sdk::hash::{compute_model_hash, compute_input_hash, compute_result_hash};
//!
//! // Compute model hash from raw model file bytes
//! let model_bytes = b"{\"learner\":{}}";
//! let model_hash = compute_model_hash(model_bytes);
//!
//! // Compute input hash from feature vector
//! let features = vec![1.0, 2.5, 3.7];
//! let input_hash = compute_input_hash(&features);
//!
//! // Compute result hash from prediction scores
//! let scores = vec![0.85];
//! let result_hash = compute_result_hash(&scores);
//! ```

use alloy::primitives::{keccak256, B256};

/// Compute the model hash from raw model file bytes.
///
/// This matches the enclave's computation: `keccak256(raw_file_bytes)`.
/// The model file is hashed as-is, without any parsing or normalization.
pub fn compute_model_hash(model_bytes: &[u8]) -> B256 {
    keccak256(model_bytes)
}

/// Compute the model hash from a file path.
///
/// Reads the file and returns `keccak256(file_contents)`.
pub fn compute_model_hash_from_file(path: &str) -> std::io::Result<B256> {
    let bytes = std::fs::read(path)?;
    Ok(compute_model_hash(&bytes))
}

/// Compute the input hash from a feature vector.
///
/// This matches the enclave's computation:
/// `keccak256(serde_json::to_vec(&features))`.
///
/// The features are JSON-serialized (e.g. `[1.0,2.5,3.7]`) and then hashed.
/// The JSON serialization must match exactly what `serde_json::to_vec` produces
/// for a `Vec<f64>` in order to produce the same hash as the enclave.
pub fn compute_input_hash(features: &[f64]) -> B256 {
    let json_bytes = serde_json::to_vec(features).expect("f64 values are always JSON-serializable");
    keccak256(&json_bytes)
}

/// Compute the result hash from prediction scores.
///
/// This matches the enclave's computation:
/// `keccak256(serde_json::to_vec(&scores))`.
///
/// The scores are JSON-serialized (e.g. `[0.85]`) and then hashed.
pub fn compute_result_hash(scores: &[f64]) -> B256 {
    let json_bytes = serde_json::to_vec(scores).expect("f64 values are always JSON-serializable");
    keccak256(&json_bytes)
}

/// Compute the input hash from raw JSON bytes.
///
/// Use this when you already have the JSON-serialized feature array
/// and want to skip serialization. The bytes must be a valid JSON array
/// of numbers matching what `serde_json::to_vec(&features)` would produce.
pub fn compute_input_hash_from_json(json_bytes: &[u8]) -> B256 {
    keccak256(json_bytes)
}

/// Compute the result hash from raw result bytes.
///
/// Use this when you already have the raw result bytes (e.g. from the
/// enclave's response `result` field after hex-decoding).
pub fn compute_result_hash_from_bytes(result_bytes: &[u8]) -> B256 {
    keccak256(result_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_model_hash() {
        let model_bytes = b"hello model";
        let hash = compute_model_hash(model_bytes);
        // keccak256 of b"hello model" -- should be deterministic
        assert_ne!(hash, B256::ZERO);

        // Same input produces same hash
        let hash2 = compute_model_hash(model_bytes);
        assert_eq!(hash, hash2);

        // Different input produces different hash
        let hash3 = compute_model_hash(b"different model");
        assert_ne!(hash, hash3);
    }

    #[test]
    fn test_compute_model_hash_empty() {
        let hash = compute_model_hash(b"");
        // keccak256 of empty input is a well-known constant
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn test_compute_input_hash_matches_enclave() {
        // The enclave does: serde_json::to_vec(&features) then keccak256
        let features = vec![1.0, 2.5, 3.7];

        // Manually reproduce the enclave's computation
        let json_bytes = serde_json::to_vec(&features).unwrap();
        let expected = keccak256(&json_bytes);

        let hash = compute_input_hash(&features);
        assert_eq!(hash, expected, "input_hash must match enclave behavior");
    }

    #[test]
    fn test_compute_input_hash_json_format() {
        // Verify the JSON serialization format
        let features = vec![1.0, 2.5, 3.7];
        let json_bytes = serde_json::to_vec(&features).unwrap();
        let json_str = String::from_utf8(json_bytes).unwrap();
        assert_eq!(json_str, "[1.0,2.5,3.7]");
    }

    #[test]
    fn test_compute_input_hash_integer_floats() {
        // serde_json serializes 1.0 as "1.0" (not "1")
        let features = vec![1.0, 2.0, 3.0];
        let json_bytes = serde_json::to_vec(&features).unwrap();
        let json_str = String::from_utf8(json_bytes).unwrap();
        assert_eq!(json_str, "[1.0,2.0,3.0]");
    }

    #[test]
    fn test_compute_input_hash_single_feature() {
        let features = vec![42.5];
        let hash = compute_input_hash(&features);

        let json_bytes = serde_json::to_vec(&features).unwrap();
        assert_eq!(json_bytes, b"[42.5]");
        assert_eq!(hash, keccak256(&json_bytes));
    }

    #[test]
    fn test_compute_input_hash_empty_features() {
        let features: Vec<f64> = vec![];
        let hash = compute_input_hash(&features);

        let json_bytes = serde_json::to_vec(&features).unwrap();
        assert_eq!(json_bytes, b"[]");
        assert_eq!(hash, keccak256(b"[]"));
    }

    #[test]
    fn test_compute_result_hash() {
        let scores = vec![0.85];
        let hash = compute_result_hash(&scores);

        let json_bytes = serde_json::to_vec(&scores).unwrap();
        let expected = keccak256(&json_bytes);
        assert_eq!(hash, expected, "result_hash must match enclave behavior");
    }

    #[test]
    fn test_compute_result_hash_multiclass() {
        let scores = vec![0.1, 0.8, 0.1];
        let hash = compute_result_hash(&scores);

        let json_bytes = serde_json::to_vec(&scores).unwrap();
        let expected = keccak256(&json_bytes);
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_input_hash_from_json_matches() {
        let features = vec![1.0, 2.5, 3.7];
        let hash_from_vec = compute_input_hash(&features);

        let json_bytes = serde_json::to_vec(&features).unwrap();
        let hash_from_json = compute_input_hash_from_json(&json_bytes);

        assert_eq!(hash_from_vec, hash_from_json);
    }

    #[test]
    fn test_result_hash_from_bytes_matches() {
        let scores = vec![0.85];
        let hash_from_scores = compute_result_hash(&scores);

        let result_bytes = serde_json::to_vec(&scores).unwrap();
        let hash_from_bytes = compute_result_hash_from_bytes(&result_bytes);

        assert_eq!(hash_from_scores, hash_from_bytes);
    }

    #[test]
    fn test_negative_features() {
        // Verify negative floats serialize correctly
        let features = vec![-1.5, 0.0, 2.3];
        let json_bytes = serde_json::to_vec(&features).unwrap();
        let json_str = String::from_utf8(json_bytes).unwrap();
        assert_eq!(json_str, "[-1.5,0.0,2.3]");

        let hash = compute_input_hash(&features);
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn test_known_keccak_value() {
        // keccak256("") is a well-known constant:
        // 0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        let hash = compute_model_hash(b"");
        let expected: B256 = "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
            .parse()
            .unwrap();
        assert_eq!(
            hash, expected,
            "keccak256 of empty bytes must match known constant"
        );
    }

    #[test]
    fn test_feature_ordering_matters() {
        let features_a = vec![1.0, 2.0, 3.0];
        let features_b = vec![3.0, 2.0, 1.0];

        let hash_a = compute_input_hash(&features_a);
        let hash_b = compute_input_hash(&features_b);

        assert_ne!(
            hash_a, hash_b,
            "Different feature orderings must produce different hashes"
        );
    }
}
