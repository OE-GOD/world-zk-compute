//! ECDSA attestation signing for TEE enclave.
//!
//! Produces signatures compatible with TEEMLVerifier.sol's verification logic:
//!
//! ```text
//! resultHash  = keccak256(result)
//! message     = keccak256(abi.encodePacked(modelHash, inputHash, resultHash))
//! ethSigned   = keccak256("\x19Ethereum Signed Message:\n32" || message)
//! signer      = ecrecover(ethSigned, signature)
//! ```
//!
//! `alloy-signer`'s `sign_message()` automatically adds the EIP-191 prefix,
//! so we pass the raw 32-byte `message` (NOT the ethSignedHash).

use alloy_primitives::{keccak256, Address, B256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;

/// An ECDSA attestor that signs inference results.
pub struct Attestor {
    signer: PrivateKeySigner,
}

/// The result of signing an attestation.
pub struct AttestationResult {
    /// keccak256 of the result bytes.
    pub result_hash: B256,
    /// 65-byte ECDSA signature: r(32) || s(32) || v(1).
    pub signature: Vec<u8>,
}

impl Attestor {
    /// Create an attestor from a hex-encoded private key (with or without `0x` prefix).
    pub fn from_private_key(hex_key: &str) -> Result<Self, String> {
        let key = hex_key.trim().strip_prefix("0x").unwrap_or(hex_key.trim());
        let signer: PrivateKeySigner = key
            .parse()
            .map_err(|e| format!("Invalid private key: {}", e))?;
        Ok(Attestor { signer })
    }

    /// Generate an attestor with a random private key.
    pub fn random() -> Self {
        Attestor {
            signer: PrivateKeySigner::random(),
        }
    }

    /// Get the Ethereum address of this attestor.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Sign an attestation matching TEEMLVerifier.sol's verification.
    ///
    /// 1. `result_hash = keccak256(result_bytes)`
    /// 2. `message = keccak256(model_hash || input_hash || result_hash)` (96 bytes packed)
    /// 3. Sign `message` with EIP-191 prefix (applied automatically by `sign_message_sync`)
    /// 4. Return 65-byte signature in `r || s || v` format
    pub fn sign_attestation(
        &self,
        model_hash: B256,
        input_hash: B256,
        result_bytes: &[u8],
    ) -> Result<AttestationResult, String> {
        // Step 1: hash the result
        let result_hash = keccak256(result_bytes);

        // Step 2: pack model_hash + input_hash + result_hash (96 bytes)
        let mut packed = Vec::with_capacity(96);
        packed.extend_from_slice(model_hash.as_slice());
        packed.extend_from_slice(input_hash.as_slice());
        packed.extend_from_slice(result_hash.as_slice());

        // Step 3: message = keccak256(packed)
        let message = keccak256(&packed);

        // Step 4: sign with EIP-191 prefix (sign_message_sync adds "\x19Ethereum Signed Message:\n32")
        let sig = self
            .signer
            .sign_message_sync(message.as_slice())
            .map_err(|e| format!("Signing failed: {}", e))?;

        // Step 5: convert to 65-byte r || s || v format
        let sig_bytes = sig.as_bytes();

        Ok(AttestationResult {
            result_hash,
            signature: sig_bytes.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_recover() {
        let attestor = Attestor::from_private_key(
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap();

        // Known Anvil account 0 address
        let expected_addr: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse()
            .unwrap();
        assert_eq!(attestor.address(), expected_addr);

        let model_hash = keccak256(b"model data");
        let input_hash = keccak256(b"input data");
        let result_bytes = b"some result";

        let attestation = attestor
            .sign_attestation(model_hash, input_hash, result_bytes)
            .unwrap();

        // Verify result hash
        assert_eq!(attestation.result_hash, keccak256(b"some result"));

        // Verify signature is 65 bytes
        assert_eq!(attestation.signature.len(), 65);

        // Verify v is 27 or 28
        let v = attestation.signature[64];
        assert!(v == 27 || v == 28, "v should be 27 or 28, got {}", v);

        // Recover the signer address from the signature to verify correctness
        let mut packed = Vec::with_capacity(96);
        packed.extend_from_slice(model_hash.as_slice());
        packed.extend_from_slice(input_hash.as_slice());
        packed.extend_from_slice(attestation.result_hash.as_slice());
        let message = keccak256(&packed);

        // Reconstruct the EIP-191 prefixed hash
        let mut eth_message_input = Vec::new();
        eth_message_input.extend_from_slice(b"\x19Ethereum Signed Message:\n32");
        eth_message_input.extend_from_slice(message.as_slice());
        let eth_hash = keccak256(&eth_message_input);

        // Use alloy to recover
        let sig = alloy_primitives::Signature::try_from(attestation.signature.as_slice()).unwrap();
        let recovered = sig.recover_address_from_prehash(&eth_hash).unwrap();
        assert_eq!(recovered, expected_addr);
    }

    #[test]
    fn test_attestation_format() {
        let attestor = Attestor::random();
        let model_hash = B256::ZERO;
        let input_hash = B256::ZERO;
        let result_bytes = b"test";

        let attestation = attestor
            .sign_attestation(model_hash, input_hash, result_bytes)
            .unwrap();

        // Must be exactly 65 bytes
        assert_eq!(attestation.signature.len(), 65);

        // r and s should be non-zero (32 bytes each)
        let r = &attestation.signature[..32];
        let s = &attestation.signature[32..64];
        assert!(r.iter().any(|&b| b != 0), "r should be non-zero");
        assert!(s.iter().any(|&b| b != 0), "s should be non-zero");

        // v should be 27 or 28
        let v = attestation.signature[64];
        assert!(v == 27 || v == 28, "v should be 27 or 28, got {}", v);
    }

    #[test]
    fn test_from_private_key_with_0x_prefix() {
        let attestor = Attestor::from_private_key(
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap();
        let expected_addr: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse()
            .unwrap();
        assert_eq!(attestor.address(), expected_addr);
    }
}
