//! ECDSA signing utilities for verification receipts.
//!
//! Uses secp256k1 (k256 crate) for signing. The signing key is loaded from
//! the `REGISTRY_SIGNING_KEY` environment variable (hex-encoded 32-byte private key),
//! or read from a file on disk, or auto-generated on first start and saved to disk.

use k256::ecdsa::{SigningKey, VerifyingKey};

/// Load or generate an ECDSA signing key.
///
/// Resolution order:
/// 1. `REGISTRY_SIGNING_KEY` env var (hex-encoded 32-byte private key)
/// 2. Read from `<key_path>` on disk
/// 3. Generate a new key and save to `<key_path>`
pub fn load_or_generate_key(key_path: &str) -> Result<SigningKey, String> {
    // 1. Try environment variable.
    if let Ok(hex_key) = std::env::var("REGISTRY_SIGNING_KEY") {
        let hex_key = hex_key.trim().trim_start_matches("0x");
        let bytes =
            hex::decode(hex_key).map_err(|e| format!("invalid REGISTRY_SIGNING_KEY hex: {e}"))?;
        return SigningKey::from_slice(&bytes)
            .map_err(|e| format!("invalid REGISTRY_SIGNING_KEY: {e}"));
    }

    // 2. Try reading from disk.
    if let Ok(contents) = std::fs::read_to_string(key_path) {
        let hex_key = contents.trim().trim_start_matches("0x");
        if !hex_key.is_empty() {
            let bytes = hex::decode(hex_key)
                .map_err(|e| format!("invalid signing key file hex: {e}"))?;
            return SigningKey::from_slice(&bytes)
                .map_err(|e| format!("invalid signing key file: {e}"));
        }
    }

    // 3. Generate a new key.
    let key = SigningKey::random(&mut rand_core::OsRng);
    let key_hex = hex::encode(key.to_bytes());
    std::fs::write(key_path, &key_hex)
        .map_err(|e| format!("failed to save signing key to {key_path}: {e}"))?;

    tracing::warn!("Generated new ECDSA signing key and saved to {key_path}");
    Ok(key)
}

/// Get the verifying (public) key from a signing key.
pub fn verifying_key(signing_key: &SigningKey) -> VerifyingKey {
    *signing_key.verifying_key()
}

/// Encode a verifying key as a hex string (compressed SEC1 format, 33 bytes).
pub fn verifying_key_hex(vk: &VerifyingKey) -> String {
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    let point = vk.to_encoded_point(true);
    hex::encode(point.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_or_generate_key_creates_file() {
        // Ensure env var does not interfere.
        std::env::remove_var("REGISTRY_SIGNING_KEY");

        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("test_signing_key.hex");
        let key_path_str = key_path.to_str().unwrap();

        // Should not exist yet.
        assert!(!key_path.exists());

        // Generate.
        let key = load_or_generate_key(key_path_str).unwrap();

        // File should now exist.
        assert!(key_path.exists());

        // Loading again should return the same key.
        let key2 = load_or_generate_key(key_path_str).unwrap();
        assert_eq!(key.to_bytes(), key2.to_bytes());
    }

    #[test]
    fn test_load_from_env() {
        // Generate a known key.
        let key = SigningKey::random(&mut rand_core::OsRng);
        let key_hex = hex::encode(key.to_bytes());

        // Set env var.
        std::env::set_var("REGISTRY_SIGNING_KEY", &key_hex);

        let loaded = load_or_generate_key("/nonexistent/path").unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());

        // Clean up.
        std::env::remove_var("REGISTRY_SIGNING_KEY");
    }

    #[test]
    fn test_verifying_key_hex_roundtrip() {
        let key = SigningKey::random(&mut rand_core::OsRng);
        let vk = verifying_key(&key);
        let hex_str = verifying_key_hex(&vk);

        // Should be 33 bytes compressed = 66 hex chars.
        assert_eq!(hex_str.len(), 66);

        // Should be parseable back.
        let bytes = hex::decode(&hex_str).unwrap();
        let vk2 = VerifyingKey::from_sec1_bytes(&bytes).unwrap();
        assert_eq!(vk, vk2);
    }
}
