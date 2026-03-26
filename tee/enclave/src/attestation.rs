//! ECDSA attestation signing for TEE enclave.
//!
//! Produces signatures compatible with TEEMLVerifier.sol's verification logic:
//!
//! ```text
//! resultHash  = keccak256(result)
//! message     = keccak256(abi.encodePacked(modelHash, inputHash, resultHash, chainId, nonce, timestamp))
//! ethSigned   = keccak256("\x19Ethereum Signed Message:\n32" || message)
//! signer      = ecrecover(ethSigned, signature)
//! ```
//!
//! `alloy-signer`'s `sign_message()` automatically adds the EIP-191 prefix,
//! so we pass the raw 32-byte `message` (NOT the ethSignedHash).

use std::sync::atomic::{AtomicU64, Ordering};

use alloy_primitives::{keccak256, Address, B256};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;

/// An ECDSA attestor that signs inference results with replay protection.
pub struct Attestor {
    signer: PrivateKeySigner,
    /// Monotonically increasing nonce for replay protection.
    nonce_counter: AtomicU64,
    /// Chain ID for cross-chain replay protection (default: 1 for Ethereum mainnet).
    chain_id: u64,
}

/// The result of signing an attestation.
pub struct AttestationResult {
    /// keccak256 of the result bytes.
    pub result_hash: B256,
    /// 65-byte ECDSA signature: r(32) || s(32) || v(1).
    pub signature: Vec<u8>,
    /// Chain ID included in the signed payload.
    pub chain_id: u64,
    /// Monotonic nonce included in the signed payload.
    pub nonce: u64,
    /// Unix timestamp (seconds) included in the signed payload.
    pub timestamp: u64,
}

impl Attestor {
    /// Create an attestor from a hex-encoded private key (with or without `0x` prefix).
    pub fn from_private_key(hex_key: &str) -> Result<Self, String> {
        let key = hex_key.trim().strip_prefix("0x").unwrap_or(hex_key.trim());
        let signer: PrivateKeySigner = key
            .parse()
            .map_err(|e| format!("Invalid private key: {}", e))?;
        Ok(Attestor {
            signer,
            nonce_counter: AtomicU64::new(0),
            chain_id: 1,
        })
    }

    /// Generate an attestor with a random private key.
    pub fn random() -> Self {
        Attestor {
            signer: PrivateKeySigner::random(),
            nonce_counter: AtomicU64::new(0),
            chain_id: 1,
        }
    }

    /// Generate an attestor with a random private key and a specific chain ID.
    #[allow(dead_code)]
    pub fn random_with_chain_id(chain_id: u64) -> Self {
        Attestor {
            signer: PrivateKeySigner::random(),
            nonce_counter: AtomicU64::new(0),
            chain_id,
        }
    }

    /// Set the chain ID for this attestor.
    pub fn set_chain_id(&mut self, chain_id: u64) {
        self.chain_id = chain_id;
    }

    /// Get the current chain ID.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Get the current nonce value (next nonce to be used).
    #[allow(dead_code)]
    pub fn current_nonce(&self) -> u64 {
        self.nonce_counter.load(Ordering::SeqCst)
    }

    /// Get the Ethereum address of this attestor.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Sign an attestation with replay protection.
    ///
    /// 1. `result_hash = keccak256(result_bytes)`
    /// 2. `nonce = atomic_fetch_add(1)` (monotonically increasing)
    /// 3. `timestamp = current unix time in seconds`
    /// 4. `message = keccak256(model_hash || input_hash || result_hash || chainId || nonce || timestamp)`
    ///    where chainId, nonce, and timestamp are each encoded as 32-byte big-endian uint256
    /// 5. Sign `message` with EIP-191 prefix (applied automatically by `sign_message_sync`)
    /// 6. Return 65-byte signature in `r || s || v` format along with chain_id, nonce, timestamp
    pub fn sign_attestation(
        &self,
        model_hash: B256,
        input_hash: B256,
        result_bytes: &[u8],
    ) -> Result<AttestationResult, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("Failed to get timestamp: {}", e))?
            .as_secs();

        self.sign_attestation_with_timestamp(model_hash, input_hash, result_bytes, timestamp)
    }

    /// Sign an attestation with an explicit timestamp (useful for testing).
    ///
    /// The nonce is always atomically incremented from the internal counter.
    pub fn sign_attestation_with_timestamp(
        &self,
        model_hash: B256,
        input_hash: B256,
        result_bytes: &[u8],
        timestamp: u64,
    ) -> Result<AttestationResult, String> {
        // Step 1: hash the result
        let result_hash = keccak256(result_bytes);

        // Step 2: get and increment nonce atomically
        let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);

        // Step 3: pack all fields
        // model_hash(32) + input_hash(32) + result_hash(32) + chainId(32) + nonce(32) + timestamp(32) = 192 bytes
        let packed = Self::pack_attestation_payload(
            model_hash,
            input_hash,
            result_hash,
            self.chain_id,
            nonce,
            timestamp,
        );

        // Step 4: message = keccak256(packed)
        let message = keccak256(&packed);

        // Step 5: sign with EIP-191 prefix (sign_message_sync adds "\x19Ethereum Signed Message:\n32")
        let sig = self
            .signer
            .sign_message_sync(message.as_slice())
            .map_err(|e| format!("Signing failed: {}", e))?;

        // Step 6: convert to 65-byte r || s || v format
        let sig_bytes = sig.as_bytes();

        Ok(AttestationResult {
            result_hash,
            signature: sig_bytes.to_vec(),
            chain_id: self.chain_id,
            nonce,
            timestamp,
        })
    }

    /// Sign an attestation using EIP-712 typed data, matching TEEMLVerifier.sol.
    ///
    /// The contract verifies:
    /// ```text
    /// structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, resultHash))
    /// digest = keccak256("\x19\x01" || DOMAIN_SEPARATOR || structHash)
    /// signer = ecrecover(digest, v, r, s)
    /// ```
    ///
    /// This method computes the same EIP-712 digest and signs it with `sign_hash_sync`
    /// (raw ecrecover-compatible signing, no EIP-191 prefix).
    ///
    /// # Arguments
    /// * `model_hash` - keccak256 of the model
    /// * `input_hash` - keccak256 of the input
    /// * `result_bytes` - raw inference result (hashed internally)
    /// * `verifier_address` - address of the TEEMLVerifier contract
    pub fn sign_eip712_attestation(
        &self,
        model_hash: B256,
        input_hash: B256,
        result_bytes: &[u8],
        verifier_address: Address,
    ) -> Result<AttestationResult, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("Failed to get timestamp: {}", e))?
            .as_secs();
        let nonce = self.nonce_counter.fetch_add(1, Ordering::SeqCst);
        let result_hash = keccak256(result_bytes);

        // Compute EIP-712 struct hash: keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, resultHash))
        let result_typehash: B256 = keccak256(b"TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)");
        let struct_hash = keccak256(Self::abi_encode_struct(
            result_typehash,
            model_hash,
            input_hash,
            result_hash,
        ));

        // Compute EIP-712 domain separator
        let domain_separator = Self::compute_domain_separator(self.chain_id, verifier_address);

        // Compute EIP-712 digest: keccak256("\x19\x01" || domainSeparator || structHash)
        let mut digest_input = Vec::with_capacity(66);
        digest_input.extend_from_slice(b"\x19\x01");
        digest_input.extend_from_slice(domain_separator.as_slice());
        digest_input.extend_from_slice(struct_hash.as_slice());
        let digest = keccak256(&digest_input);

        // Sign the raw digest (no EIP-191 prefix — contract uses ecrecover on this digest directly)
        let sig = self
            .signer
            .sign_hash_sync(&digest)
            .map_err(|e| format!("EIP-712 signing failed: {}", e))?;

        let sig_bytes = sig.as_bytes();

        Ok(AttestationResult {
            result_hash,
            signature: sig_bytes.to_vec(),
            chain_id: self.chain_id,
            nonce,
            timestamp,
        })
    }

    /// Compute the EIP-712 domain separator matching TEEMLVerifier.sol.
    ///
    /// `keccak256(abi.encode(EIP712_DOMAIN_TYPEHASH, nameHash, versionHash, chainId, verifyingContract))`
    pub fn compute_domain_separator(chain_id: u64, verifier_address: Address) -> B256 {
        let domain_typehash = keccak256(
            b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
        );
        let name_hash = keccak256(b"TEEMLVerifier");
        let version_hash = keccak256(b"1");

        let mut encoded = Vec::with_capacity(160);
        encoded.extend_from_slice(domain_typehash.as_slice());
        encoded.extend_from_slice(name_hash.as_slice());
        encoded.extend_from_slice(version_hash.as_slice());
        // chainId as uint256 (32 bytes, big-endian)
        let mut chain_id_bytes = [0u8; 32];
        chain_id_bytes[24..32].copy_from_slice(&chain_id.to_be_bytes());
        encoded.extend_from_slice(&chain_id_bytes);
        // address as uint256 (32 bytes, zero-padded)
        let mut addr_bytes = [0u8; 32];
        addr_bytes[12..32].copy_from_slice(verifier_address.as_slice());
        encoded.extend_from_slice(&addr_bytes);

        keccak256(&encoded)
    }

    /// ABI-encode a struct: keccak256(typehash || field1 || field2 || field3)
    fn abi_encode_struct(typehash: B256, field1: B256, field2: B256, field3: B256) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(128);
        encoded.extend_from_slice(typehash.as_slice());
        encoded.extend_from_slice(field1.as_slice());
        encoded.extend_from_slice(field2.as_slice());
        encoded.extend_from_slice(field3.as_slice());
        encoded
    }

    /// Pack the attestation payload for hashing.
    ///
    /// Layout: model_hash(32) || input_hash(32) || result_hash(32) || chainId(32) || nonce(32) || timestamp(32)
    /// chainId, nonce, and timestamp are encoded as 32-byte big-endian uint256 for Solidity abi.encodePacked compatibility.
    pub fn pack_attestation_payload(
        model_hash: B256,
        input_hash: B256,
        result_hash: B256,
        chain_id: u64,
        nonce: u64,
        timestamp: u64,
    ) -> Vec<u8> {
        let mut packed = Vec::with_capacity(192);
        packed.extend_from_slice(model_hash.as_slice());
        packed.extend_from_slice(input_hash.as_slice());
        packed.extend_from_slice(result_hash.as_slice());

        // Encode chain_id as 32-byte big-endian uint256
        let mut chain_id_bytes = [0u8; 32];
        chain_id_bytes[24..32].copy_from_slice(&chain_id.to_be_bytes());
        packed.extend_from_slice(&chain_id_bytes);

        // Encode nonce as 32-byte big-endian uint256
        let mut nonce_bytes = [0u8; 32];
        nonce_bytes[24..32].copy_from_slice(&nonce.to_be_bytes());
        packed.extend_from_slice(&nonce_bytes);

        // Encode timestamp as 32-byte big-endian uint256
        let mut timestamp_bytes = [0u8; 32];
        timestamp_bytes[24..32].copy_from_slice(&timestamp.to_be_bytes());
        packed.extend_from_slice(&timestamp_bytes);

        packed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_recover() {
        let mut attestor = Attestor::from_private_key(
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap();
        attestor.set_chain_id(1);

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

        // Verify chain_id, nonce, timestamp are populated
        assert_eq!(attestation.chain_id, 1);
        assert_eq!(attestation.nonce, 0); // first attestation
        assert!(attestation.timestamp > 0);

        // Recover the signer address from the signature to verify correctness
        let packed = Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            attestation.result_hash,
            attestation.chain_id,
            attestation.nonce,
            attestation.timestamp,
        );
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

        // New fields should be populated
        assert_eq!(attestation.chain_id, 1); // default
        assert_eq!(attestation.nonce, 0); // first attestation
        assert!(attestation.timestamp > 0);
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

    // -----------------------------------------------------------------------
    // New tests for replay protection (T65)
    // -----------------------------------------------------------------------

    #[test]
    fn test_different_chain_ids_produce_different_attestations() {
        // Create two attestors with different chain IDs but same key
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let mut attestor_mainnet = Attestor::from_private_key(key).unwrap();
        attestor_mainnet.set_chain_id(1);

        let mut attestor_sepolia = Attestor::from_private_key(key).unwrap();
        attestor_sepolia.set_chain_id(11155111);

        let model_hash = keccak256(b"model data");
        let input_hash = keccak256(b"input data");
        let result_bytes = b"prediction result";
        let timestamp = 1700000000u64;

        let att_mainnet = attestor_mainnet
            .sign_attestation_with_timestamp(model_hash, input_hash, result_bytes, timestamp)
            .unwrap();

        let att_sepolia = attestor_sepolia
            .sign_attestation_with_timestamp(model_hash, input_hash, result_bytes, timestamp)
            .unwrap();

        // Same result hash (same input)
        assert_eq!(att_mainnet.result_hash, att_sepolia.result_hash);

        // Different chain IDs
        assert_eq!(att_mainnet.chain_id, 1);
        assert_eq!(att_sepolia.chain_id, 11155111);

        // Signatures must differ because the message hash includes chain_id
        assert_ne!(
            att_mainnet.signature, att_sepolia.signature,
            "Signatures for different chain IDs must differ"
        );

        // Verify that the underlying message hashes are different
        let packed_mainnet = Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            att_mainnet.result_hash,
            1,
            att_mainnet.nonce,
            timestamp,
        );
        let packed_sepolia = Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            att_sepolia.result_hash,
            11155111,
            att_sepolia.nonce,
            timestamp,
        );
        assert_ne!(
            keccak256(&packed_mainnet),
            keccak256(&packed_sepolia),
            "Message hashes for different chain IDs must differ"
        );
    }

    #[test]
    fn test_nonce_increments_monotonically() {
        let attestor = Attestor::random();
        let model_hash = B256::ZERO;
        let input_hash = B256::ZERO;
        let result_bytes = b"test";

        // Verify initial nonce is 0
        assert_eq!(attestor.current_nonce(), 0);

        let att0 = attestor
            .sign_attestation(model_hash, input_hash, result_bytes)
            .unwrap();
        assert_eq!(att0.nonce, 0);
        assert_eq!(attestor.current_nonce(), 1);

        let att1 = attestor
            .sign_attestation(model_hash, input_hash, result_bytes)
            .unwrap();
        assert_eq!(att1.nonce, 1);
        assert_eq!(attestor.current_nonce(), 2);

        let att2 = attestor
            .sign_attestation(model_hash, input_hash, result_bytes)
            .unwrap();
        assert_eq!(att2.nonce, 2);
        assert_eq!(attestor.current_nonce(), 3);

        // Nonces are strictly increasing
        assert!(att0.nonce < att1.nonce);
        assert!(att1.nonce < att2.nonce);

        // All signatures should differ (different nonces produce different messages)
        assert_ne!(att0.signature, att1.signature);
        assert_ne!(att1.signature, att2.signature);
        assert_ne!(att0.signature, att2.signature);
    }

    #[test]
    fn test_timestamp_is_included_in_hash() {
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

        // Create two attestors with same key so nonces both start at 0
        let attestor1 = Attestor::from_private_key(key).unwrap();
        let attestor2 = Attestor::from_private_key(key).unwrap();

        let model_hash = keccak256(b"model");
        let input_hash = keccak256(b"input");
        let result_bytes = b"result";

        // Sign with different timestamps
        let att_t1 = attestor1
            .sign_attestation_with_timestamp(model_hash, input_hash, result_bytes, 1000)
            .unwrap();
        let att_t2 = attestor2
            .sign_attestation_with_timestamp(model_hash, input_hash, result_bytes, 2000)
            .unwrap();

        // Same nonce (both start at 0), same chain_id, same inputs
        assert_eq!(att_t1.nonce, att_t2.nonce);
        assert_eq!(att_t1.chain_id, att_t2.chain_id);
        assert_eq!(att_t1.result_hash, att_t2.result_hash);

        // But different timestamps
        assert_eq!(att_t1.timestamp, 1000);
        assert_eq!(att_t2.timestamp, 2000);

        // Signatures must differ because timestamp is part of the hash
        assert_ne!(
            att_t1.signature, att_t2.signature,
            "Signatures for different timestamps must differ"
        );

        // Verify message hashes differ
        let msg1 = keccak256(Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            att_t1.result_hash,
            att_t1.chain_id,
            att_t1.nonce,
            1000,
        ));
        let msg2 = keccak256(Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            att_t2.result_hash,
            att_t2.chain_id,
            att_t2.nonce,
            2000,
        ));
        assert_ne!(
            msg1, msg2,
            "Message hashes must differ for different timestamps"
        );
    }

    #[test]
    fn test_pack_attestation_payload_layout() {
        let model_hash = B256::ZERO;
        let input_hash = B256::ZERO;
        let result_hash = B256::ZERO;

        let packed = Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            result_hash,
            1,          // chain_id
            42,         // nonce
            1700000000, // timestamp
        );

        // Total: 6 * 32 = 192 bytes
        assert_eq!(packed.len(), 192);

        // Verify chain_id encoding (bytes 96..128)
        let chain_id_slice = &packed[96..128];
        assert_eq!(chain_id_slice[..24], [0u8; 24]); // leading zeros
        let chain_id_val = u64::from_be_bytes(chain_id_slice[24..32].try_into().unwrap());
        assert_eq!(chain_id_val, 1);

        // Verify nonce encoding (bytes 128..160)
        let nonce_slice = &packed[128..160];
        assert_eq!(nonce_slice[..24], [0u8; 24]);
        let nonce_val = u64::from_be_bytes(nonce_slice[24..32].try_into().unwrap());
        assert_eq!(nonce_val, 42);

        // Verify timestamp encoding (bytes 160..192)
        let ts_slice = &packed[160..192];
        assert_eq!(ts_slice[..24], [0u8; 24]);
        let ts_val = u64::from_be_bytes(ts_slice[24..32].try_into().unwrap());
        assert_eq!(ts_val, 1700000000);
    }

    #[test]
    fn test_signature_recovery_with_all_fields() {
        let key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let mut attestor = Attestor::from_private_key(key).unwrap();
        attestor.set_chain_id(137); // Polygon

        let expected_addr: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse()
            .unwrap();

        let model_hash = keccak256(b"my model");
        let input_hash = keccak256(b"my input");
        let result_bytes = b"prediction";
        let timestamp = 1700000000u64;

        let att = attestor
            .sign_attestation_with_timestamp(model_hash, input_hash, result_bytes, timestamp)
            .unwrap();

        // Reconstruct the full message a verifier would compute
        let packed = Attestor::pack_attestation_payload(
            model_hash,
            input_hash,
            att.result_hash,
            att.chain_id,
            att.nonce,
            att.timestamp,
        );
        let message = keccak256(&packed);

        // EIP-191 prefix
        let mut eth_msg = Vec::new();
        eth_msg.extend_from_slice(b"\x19Ethereum Signed Message:\n32");
        eth_msg.extend_from_slice(message.as_slice());
        let eth_hash = keccak256(&eth_msg);

        let sig = alloy_primitives::Signature::try_from(att.signature.as_slice()).unwrap();
        let recovered = sig.recover_address_from_prehash(&eth_hash).unwrap();
        assert_eq!(recovered, expected_addr);

        // Verify chain_id is 137
        assert_eq!(att.chain_id, 137);
    }

    #[test]
    fn test_random_with_chain_id() {
        let attestor = Attestor::random_with_chain_id(42161); // Arbitrum
        assert_eq!(attestor.chain_id(), 42161);
        assert_eq!(attestor.current_nonce(), 0);

        let att = attestor
            .sign_attestation(B256::ZERO, B256::ZERO, b"test")
            .unwrap();
        assert_eq!(att.chain_id, 42161);
    }

    #[test]
    fn test_concurrent_nonce_safety() {
        // Verify that AtomicU64 prevents duplicate nonces even from &self
        let attestor = Attestor::random();
        let model_hash = B256::ZERO;
        let input_hash = B256::ZERO;
        let result_bytes = b"test";

        // Sign 100 attestations and collect nonces
        let mut nonces: Vec<u64> = Vec::new();
        for _ in 0..100 {
            let att = attestor
                .sign_attestation(model_hash, input_hash, result_bytes)
                .unwrap();
            nonces.push(att.nonce);
        }

        // All nonces should be unique
        let unique: std::collections::HashSet<u64> = nonces.iter().cloned().collect();
        assert_eq!(unique.len(), 100, "All 100 nonces must be unique");

        // Nonces should be sequential 0..99
        for (i, nonce) in nonces.iter().enumerate() {
            assert_eq!(*nonce, i as u64, "Nonce {} should be {}", nonce, i);
        }
    }

    #[test]
    fn test_sign_eip712_and_recover() {
        let mut attestor = Attestor::from_private_key(
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap();
        attestor.set_chain_id(31337); // Anvil default chain ID

        let expected_addr: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse()
            .unwrap();

        let verifier_address: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();

        let model_hash = keccak256(b"model data");
        let input_hash = keccak256(b"input data");
        let result_bytes = b"some result";

        let attestation = attestor
            .sign_eip712_attestation(model_hash, input_hash, result_bytes, verifier_address)
            .unwrap();

        assert_eq!(attestation.result_hash, keccak256(b"some result"));
        assert_eq!(attestation.signature.len(), 65);

        // Reconstruct the EIP-712 digest (same as contract would compute)
        let result_typehash = keccak256(
            b"TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)",
        );
        let struct_hash = keccak256(Attestor::abi_encode_struct(
            result_typehash,
            model_hash,
            input_hash,
            attestation.result_hash,
        ));
        let domain_sep =
            Attestor::compute_domain_separator(31337, verifier_address);
        let mut digest_input = Vec::new();
        digest_input.extend_from_slice(b"\x19\x01");
        digest_input.extend_from_slice(domain_sep.as_slice());
        digest_input.extend_from_slice(struct_hash.as_slice());
        let digest = keccak256(&digest_input);

        // Recover signer from the EIP-712 digest (no EIP-191 prefix)
        let sig =
            alloy_primitives::Signature::try_from(attestation.signature.as_slice()).unwrap();
        let recovered = sig.recover_address_from_prehash(&digest).unwrap();
        assert_eq!(recovered, expected_addr, "EIP-712 signer recovery failed");
    }

    #[test]
    fn test_domain_separator_deterministic() {
        let addr: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();
        let ds1 = Attestor::compute_domain_separator(1, addr);
        let ds2 = Attestor::compute_domain_separator(1, addr);
        assert_eq!(ds1, ds2, "Domain separator should be deterministic");

        // Different chain ID → different separator
        let ds3 = Attestor::compute_domain_separator(2, addr);
        assert_ne!(ds1, ds3, "Different chain IDs should give different separators");
    }
}
