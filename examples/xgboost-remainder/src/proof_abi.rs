//! ABI Encoding for Remainder Proofs
//!
//! Converts Remainder's native proof format to ABI-encoded bytes
//! suitable for on-chain verification by RemainderVerifier.sol.
//!
//! Encoding scheme:
//! - G1 points → (uint256 x, uint256 y) — 64 bytes
//! - Fr scalars → uint256 — 32 bytes
//! - Arrays → uint256 length prefix + elements
//! - Nested structures → concatenated with length prefixes

use anyhow::Result;

/// ABI-encode a Remainder proof for the Solidity verifier.
///
/// The proof is already in a binary format with field elements and curve points.
/// This function wraps it in ABI encoding with proper padding and length prefixes.
///
/// Expected layout for RemainderVerifier.sol:
/// ```text
/// offset 0x00: bytes offset to proof data
/// offset 0x20: uint256 numGkrLayers
/// offset 0x40+: per-layer sumcheck data
/// ...
/// offset N:    Hyrax PCS evaluation proof
/// ```
pub fn encode_proof(proof_bytes: &[u8]) -> Result<Vec<u8>> {
    // The proof bytes are already structured with:
    // - 4-byte selector ("REM1")
    // - Layer count
    // - Per-layer sumcheck polynomials
    // - Hyrax commitment data
    //
    // For ABI encoding, we wrap this as `bytes` with proper padding

    let mut encoded = Vec::new();

    // ABI encoding of bytes: offset (32) + length (32) + padded data
    // Offset to data start
    encoded.extend_from_slice(&pad_left_32(&32u64.to_be_bytes()));
    // Length of proof data
    encoded.extend_from_slice(&pad_left_32(&(proof_bytes.len() as u64).to_be_bytes()));
    // Proof data padded to 32-byte boundary
    encoded.extend_from_slice(proof_bytes);
    // Pad to 32-byte alignment
    let padding = (32 - (proof_bytes.len() % 32)) % 32;
    encoded.extend_from_slice(&vec![0u8; padding]);

    Ok(encoded)
}

/// ABI-encode public inputs for the Solidity verifier.
///
/// Public inputs are encoded as a dynamic bytes array containing
/// the predicted class and any other public outputs.
pub fn encode_public_inputs(public_inputs: &[u8]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    // ABI encoding of bytes
    encoded.extend_from_slice(&pad_left_32(&32u64.to_be_bytes()));
    encoded.extend_from_slice(&pad_left_32(&(public_inputs.len() as u64).to_be_bytes()));
    encoded.extend_from_slice(public_inputs);
    let padding = (32 - (public_inputs.len() % 32)) % 32;
    encoded.extend_from_slice(&vec![0u8; padding]);

    Ok(encoded)
}

/// Encode a BN254 G1 point as (uint256, uint256)
pub fn encode_g1_point(x: &[u8; 32], y: &[u8; 32]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(64);
    encoded.extend_from_slice(x);
    encoded.extend_from_slice(y);
    encoded
}

/// Encode a BN254 Fr scalar as uint256
pub fn encode_fr(scalar: &[u8; 32]) -> Vec<u8> {
    scalar.to_vec()
}

/// Encode an array of Fr scalars with length prefix
pub fn encode_fr_array(scalars: &[[u8; 32]]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&pad_left_32(&(scalars.len() as u64).to_be_bytes()));
    for scalar in scalars {
        encoded.extend_from_slice(scalar);
    }
    encoded
}

/// Encode an array of G1 points with length prefix
pub fn encode_g1_array(points: &[([u8; 32], [u8; 32])]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&pad_left_32(&(points.len() as u64).to_be_bytes()));
    for (x, y) in points {
        encoded.extend_from_slice(x);
        encoded.extend_from_slice(y);
    }
    encoded
}

/// Left-pad a byte slice to 32 bytes
fn pad_left_32(data: &[u8]) -> [u8; 32] {
    let mut padded = [0u8; 32];
    let start = 32 - data.len().min(32);
    padded[start..].copy_from_slice(&data[..data.len().min(32)]);
    padded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_proof() {
        let proof = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let encoded = encode_proof(&proof).unwrap();
        // Should have offset (32) + length (32) + padded data (32)
        assert_eq!(encoded.len(), 96);
        // Data offset should be 32
        assert_eq!(encoded[31], 32);
        // Data length should be 4
        assert_eq!(encoded[63], 4);
        // First 4 bytes of data section
        assert_eq!(&encoded[64..68], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_encode_g1_point() {
        let x = [1u8; 32];
        let y = [2u8; 32];
        let encoded = encode_g1_point(&x, &y);
        assert_eq!(encoded.len(), 64);
        assert_eq!(&encoded[..32], &[1u8; 32]);
        assert_eq!(&encoded[32..], &[2u8; 32]);
    }

    #[test]
    fn test_pad_left_32() {
        let input = [0x01, 0x02];
        let padded = pad_left_32(&input);
        assert_eq!(padded[30], 0x01);
        assert_eq!(padded[31], 0x02);
        assert_eq!(padded[0], 0x00);
    }
}
