//! Cryptographically signed verification receipts.
//!
//! After verifying a proof, the registry generates a [`VerificationReceipt`]
//! that is ECDSA-signed (secp256k1) with the registry's private key. The receipt
//! serves as tamper-evident evidence that verification occurred at a specific time,
//! by a specific verifier, with a specific result.

use k256::ecdsa::{signature::Signer, signature::Verifier, Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A cryptographically signed verification receipt.
///
/// Contains the verification result along with metadata about the verifier and
/// a secp256k1 ECDSA signature over the SHA-256 hash of the canonical JSON
/// representation of all fields (excluding the signature itself).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReceipt {
    /// Unique receipt identifier (UUID v4).
    pub receipt_id: String,
    /// The proof that was verified.
    pub proof_id: String,
    /// Circuit hash from the verification result.
    pub circuit_hash: String,
    /// Model hash from the proof metadata.
    pub model_hash: String,
    /// Whether verification succeeded.
    pub verified: bool,
    /// ISO 8601 timestamp of when verification occurred.
    pub verified_at: String,
    /// Version string of the verifier software.
    pub verifier_version: String,
    /// Hostname or identifier of the verifier instance.
    pub verifier_host: String,
    /// Hex-encoded ECDSA signature over the receipt content.
    pub signature: String,
    /// Verification error message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl VerificationReceipt {
    /// Create a new unsigned receipt.
    pub fn new(
        proof_id: &str,
        circuit_hash: &str,
        model_hash: &str,
        verified: bool,
        error: Option<String>,
    ) -> Self {
        let receipt_id = uuid::Uuid::new_v4().to_string();
        let verified_at = chrono::Utc::now().to_rfc3339();

        let verifier_version =
            std::env::var("VERIFIER_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into());

        let verifier_host = std::env::var("VERIFIER_HOST")
            .or_else(|_| std::env::var("HOSTNAME"))
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());

        Self {
            receipt_id,
            proof_id: proof_id.to_string(),
            circuit_hash: circuit_hash.to_string(),
            model_hash: model_hash.to_string(),
            verified,
            verified_at,
            verifier_version,
            verifier_host,
            signature: String::new(),
            error,
        }
    }

    /// Compute the canonical message to sign: SHA-256 of the deterministic JSON
    /// representation of all fields except `signature`.
    fn signing_message(&self) -> Vec<u8> {
        // Build a deterministic JSON object with fields in a fixed order,
        // excluding the signature field.
        let canonical = serde_json::json!({
            "receipt_id": self.receipt_id,
            "proof_id": self.proof_id,
            "circuit_hash": self.circuit_hash,
            "model_hash": self.model_hash,
            "verified": self.verified,
            "verified_at": self.verified_at,
            "verifier_version": self.verifier_version,
            "verifier_host": self.verifier_host,
            "error": self.error,
        });
        let json_bytes = serde_json::to_vec(&canonical).expect("JSON serialization cannot fail");
        let hash = Sha256::digest(&json_bytes);
        hash.to_vec()
    }

    /// Sign this receipt with the given ECDSA signing key.
    /// Populates the `signature` field with the hex-encoded signature.
    pub fn sign(&mut self, key: &SigningKey) {
        let message = self.signing_message();
        let sig: Signature = key.sign(&message);
        self.signature = hex::encode(sig.to_bytes());
    }

    /// Verify the receipt's signature against the given public key.
    /// Returns `true` if the signature is valid.
    pub fn verify_signature(&self, pubkey: &VerifyingKey) -> bool {
        if self.signature.is_empty() {
            return false;
        }

        let sig_bytes = match hex::decode(&self.signature) {
            Ok(b) => b,
            Err(_) => return false,
        };

        let sig = match Signature::from_slice(&sig_bytes) {
            Ok(s) => s,
            Err(_) => return false,
        };

        let message = self.signing_message();
        pubkey.verify(&message, &sig).is_ok()
    }

    /// Serialize the receipt to a pretty-printed JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("receipt serialization cannot fail")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;

    fn test_signing_key() -> SigningKey {
        // Deterministic key for tests.
        let bytes = [42u8; 32];
        SigningKey::from_slice(&bytes).unwrap()
    }

    #[test]
    fn test_receipt_creation() {
        let receipt = VerificationReceipt::new("proof-123", "0xabc", "0xdef", true, None);

        assert!(!receipt.receipt_id.is_empty());
        assert_eq!(receipt.proof_id, "proof-123");
        assert_eq!(receipt.circuit_hash, "0xabc");
        assert_eq!(receipt.model_hash, "0xdef");
        assert!(receipt.verified);
        assert!(!receipt.verified_at.is_empty());
        assert!(!receipt.verifier_version.is_empty());
        assert!(receipt.signature.is_empty()); // Not yet signed.
        assert!(receipt.error.is_none());
    }

    #[test]
    fn test_receipt_creation_with_error() {
        let receipt = VerificationReceipt::new(
            "proof-456",
            "",
            "0xmodel",
            false,
            Some("invalid proof format".to_string()),
        );

        assert!(!receipt.verified);
        assert_eq!(receipt.error.as_deref(), Some("invalid proof format"));
    }

    #[test]
    fn test_sign_and_verify() {
        let key = test_signing_key();
        let pubkey = *key.verifying_key();

        let mut receipt = VerificationReceipt::new("proof-789", "0xcircuit", "0xmodel", true, None);
        assert!(!receipt.verify_signature(&pubkey)); // Unsigned.

        receipt.sign(&key);
        assert!(!receipt.signature.is_empty());
        assert!(receipt.verify_signature(&pubkey));
    }

    #[test]
    fn test_signature_detects_tampering() {
        let key = test_signing_key();
        let pubkey = *key.verifying_key();

        let mut receipt = VerificationReceipt::new("proof-tamper", "0xaaa", "0xbbb", true, None);
        receipt.sign(&key);

        // Tamper with the receipt.
        receipt.verified = false;
        assert!(!receipt.verify_signature(&pubkey));
    }

    #[test]
    fn test_signature_wrong_key() {
        let key1 = test_signing_key();
        let key2 = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let pubkey2 = *key2.verifying_key();

        let mut receipt = VerificationReceipt::new("proof-wrongkey", "0xa", "0xb", true, None);
        receipt.sign(&key1);

        // Verify with wrong key should fail.
        assert!(!receipt.verify_signature(&pubkey2));
    }

    #[test]
    fn test_empty_signature_fails_verification() {
        let key = test_signing_key();
        let pubkey = *key.verifying_key();

        let receipt = VerificationReceipt::new("proof-empty", "0xa", "0xb", true, None);
        assert!(!receipt.verify_signature(&pubkey));
    }

    #[test]
    fn test_invalid_signature_hex_fails() {
        let key = test_signing_key();
        let pubkey = *key.verifying_key();

        let mut receipt = VerificationReceipt::new("proof-badhex", "0xa", "0xb", true, None);
        receipt.signature = "not-valid-hex!!!".to_string();
        assert!(!receipt.verify_signature(&pubkey));
    }

    #[test]
    fn test_to_json() {
        let receipt = VerificationReceipt::new("proof-json", "0xc", "0xd", true, None);
        let json = receipt.to_json();

        // Should be valid JSON that deserializes back.
        let parsed: VerificationReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.proof_id, "proof-json");
        assert_eq!(parsed.circuit_hash, "0xc");
        assert!(parsed.verified);
    }

    #[test]
    fn test_json_roundtrip_with_signature() {
        let key = test_signing_key();
        let pubkey = *key.verifying_key();

        let mut receipt = VerificationReceipt::new("proof-rt", "0xe", "0xf", true, None);
        receipt.sign(&key);

        let json = receipt.to_json();
        let parsed: VerificationReceipt = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.signature, receipt.signature);
        assert!(parsed.verify_signature(&pubkey));
    }

    #[test]
    fn test_signing_is_deterministic() {
        let key = test_signing_key();

        // Create two identical receipts (same fields).
        let mut r1 = VerificationReceipt {
            receipt_id: "fixed-id".to_string(),
            proof_id: "proof-det".to_string(),
            circuit_hash: "0xaaa".to_string(),
            model_hash: "0xbbb".to_string(),
            verified: true,
            verified_at: "2024-01-01T00:00:00Z".to_string(),
            verifier_version: "0.1.0".to_string(),
            verifier_host: "test-host".to_string(),
            signature: String::new(),
            error: None,
        };

        let mut r2 = r1.clone();

        r1.sign(&key);
        r2.sign(&key);

        // Same content should produce same signature.
        assert_eq!(r1.signature, r2.signature);
    }
}
