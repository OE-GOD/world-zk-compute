//! Model validation: SHA-256 integrity checks for loaded models.
//!
//! Before serving inference, the enclave computes a SHA-256 hash of the raw
//! model bytes and optionally compares it against an expected hash from the
//! `EXPECTED_MODEL_HASH` environment variable.  This prevents tampered or
//! corrupted models from being loaded.

use sha2::{Digest, Sha256};

/// Compute the SHA-256 hex digest of `model_bytes`.
pub fn compute_model_hash(model_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model_bytes);
    let result = hasher.finalize();
    hex::encode(result)
}

/// Validate a model's integrity by computing its SHA-256 hash and optionally
/// comparing against an expected hash.
///
/// - If `expected_hash` is `Some`, the computed hash is compared (case-insensitive,
///   with optional `0x` prefix stripped).  On mismatch an `Err` is returned.
/// - If `expected_hash` is `None`, the hash is computed but no comparison is made.
///
/// Returns `Ok(computed_hash_hex)` on success.
pub fn validate_model(model_bytes: &[u8], expected_hash: Option<&str>) -> Result<String, String> {
    let computed = compute_model_hash(model_bytes);

    if let Some(expected) = expected_hash {
        let expected_clean = expected
            .strip_prefix("0x")
            .unwrap_or(expected)
            .to_ascii_lowercase();

        if computed != expected_clean {
            return Err(format!(
                "Model hash mismatch: expected {}, computed {}",
                expected_clean, computed
            ));
        }
    }

    Ok(computed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_model_hash_deterministic() {
        let data = b"hello world model bytes";
        let h1 = compute_model_hash(data);
        let h2 = compute_model_hash(data);
        assert_eq!(h1, h2);
        // SHA-256 output is 64 hex chars
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_validate_model_hash_match() {
        let data = b"test model data for validation";
        let hash = compute_model_hash(data);

        // Exact match
        let result = validate_model(data, Some(&hash));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), hash);

        // Match with 0x prefix
        let result = validate_model(data, Some(&format!("0x{}", hash)));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), hash);

        // Match with uppercase
        let result = validate_model(data, Some(&hash.to_uppercase()));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), hash);
    }

    #[test]
    fn test_validate_model_hash_mismatch() {
        let data = b"test model data";
        let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";

        let result = validate_model(data, Some(wrong_hash));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Model hash mismatch"));
        assert!(err.contains("expected"));
        assert!(err.contains("computed"));
    }

    #[test]
    fn test_validate_model_no_expected_hash() {
        let data = b"any model bytes";
        let result = validate_model(data, None);
        assert!(result.is_ok());

        // Should still return the computed hash
        let hash = result.unwrap();
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, compute_model_hash(data));
    }
}
