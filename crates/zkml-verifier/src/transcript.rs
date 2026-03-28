/// Fiat-Shamir transcript setup for the GKR DAG verifier.
/// Ported from RemainderVerifier.sol `_setupTranscript`.
///
/// The transcript initialization absorbs:
///   1. Circuit hash as two Fq elements (16-byte halves, LE interpreted)
///   2. Public input values (one absorb per element)
///   3. SHA-256 hash chain of public inputs (absorbed as Fq pair)
///   4. EC commitment coordinates (absorbed as Fq pairs)
///   5. SHA-256 hash chain of EC commitment coordinates (absorbed as Fq pair)
use sha2::{Digest, Sha256};

use crate::field::{Fq, U256};
use crate::poseidon::PoseidonSponge;

fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Set up the initial Fiat-Shamir transcript matching Remainder's ECTranscript protocol.
///
/// # Arguments
/// * `circuit_hash` - 32-byte SHA3-256 hash of the circuit description
/// * `pub_inputs` - Public input Fq values
/// * `input_commit_coords` - Flattened EC commitment coordinates [x0, y0, x1, y1, ...]
///
/// # Returns
/// An initialized PoseidonSponge ready for GKR verification.
pub fn setup_transcript(
    circuit_hash: &[u8; 32],
    pub_inputs: &[Fq],
    input_commit_coords: &[U256],
) -> PoseidonSponge {
    let mut sponge = PoseidonSponge::init();

    // 1. Absorb circuit hash as two Fq elements (16-byte halves, LE interpreted)
    let (fq1, fq2) = hash_to_fq_pair(circuit_hash);
    sponge.absorb_u256(&fq1.0);
    sponge.absorb_u256(&fq2.0);

    // 2. Absorb public input values
    for pub_input in pub_inputs {
        sponge.absorb_u256(&pub_input.0);
    }

    // 3. Absorb SHA-256 hash chain of public inputs
    let pub_u256s: Vec<U256> = pub_inputs.iter().map(|fq| fq.0).collect();
    let (pub_hash1, pub_hash2) = sha256_hash_chain(&pub_u256s);
    sponge.absorb_u256(&pub_hash1.0);
    sponge.absorb_u256(&pub_hash2.0);

    // 4. Absorb EC commitment points (pairs of coordinates)
    assert!(
        input_commit_coords.len() % 2 == 0,
        "input_commit_coords must have even length (x,y pairs)"
    );
    for pair in input_commit_coords.chunks(2) {
        sponge.absorb_u256(&pair[0]);
        sponge.absorb_u256(&pair[1]);
    }

    // 5. Absorb SHA-256 hash chain of EC commitment coordinates
    let (ec_hash1, ec_hash2) = sha256_hash_chain(input_commit_coords);
    sponge.absorb_u256(&ec_hash1.0);
    sponge.absorb_u256(&ec_hash2.0);

    sponge
}

/// Convert a 32-byte hash into two Fq elements.
fn hash_to_fq_pair(hash: &[u8; 32]) -> (Fq, Fq) {
    // First half: hash[0..16] interpreted as little-endian
    let fq1_val = le_bytes_to_u256(&hash[0..16]);
    // Second half: hash[16..32] interpreted as little-endian
    let fq2_val = le_bytes_to_u256(&hash[16..32]);
    (Fq(fq1_val), Fq(fq2_val))
}

/// Interpret up to 16 bytes as a little-endian integer, zero-extended to U256.
fn le_bytes_to_u256(bytes: &[u8]) -> U256 {
    assert!(bytes.len() <= 16, "le_bytes_to_u256: max 16 bytes");
    let mut limb0_bytes = [0u8; 8];
    let mut limb1_bytes = [0u8; 8];

    let copy_len_0 = bytes.len().min(8);
    limb0_bytes[..copy_len_0].copy_from_slice(&bytes[..copy_len_0]);

    if bytes.len() > 8 {
        let copy_len_1 = bytes.len() - 8;
        limb1_bytes[..copy_len_1].copy_from_slice(&bytes[8..8 + copy_len_1]);
    }

    U256([
        u64::from_le_bytes(limb0_bytes),
        u64::from_le_bytes(limb1_bytes),
        0,
        0,
    ])
}

/// Compute a 1001-iteration SHA-256 hash chain, returning two Fq elements.
fn sha256_hash_chain(fq_elements: &[U256]) -> (Fq, Fq) {
    // Step 1+2: Convert each element to 32-byte LE and concatenate
    let mut input_bytes = Vec::with_capacity(fq_elements.len() * 32);
    for elem in fq_elements {
        input_bytes.extend_from_slice(&u256_to_le_bytes(elem));
    }

    // Step 3: Initial SHA-256 hash
    let mut hash_vec = sha256(&input_bytes);

    // Step 4: Iterate SHA-256 1000 more times
    for _ in 0..1000 {
        hash_vec = sha256(&hash_vec);
    }

    // Step 5: Split 32-byte result into two 16-byte LE Fq values
    let fq1_val = le_bytes_to_u256(&hash_vec[0..16]);
    let fq2_val = le_bytes_to_u256(&hash_vec[16..32]);
    (Fq(fq1_val), Fq(fq2_val))
}

/// Convert a U256 to 32-byte little-endian representation.
fn u256_to_le_bytes(val: &U256) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0..8].copy_from_slice(&val.0[0].to_le_bytes());
    out[8..16].copy_from_slice(&val.0[1].to_le_bytes());
    out[16..24].copy_from_slice(&val.0[2].to_le_bytes());
    out[24..32].copy_from_slice(&val.0[3].to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_to_fq_pair_zeros() {
        let hash = [0u8; 32];
        let (fq1, fq2) = hash_to_fq_pair(&hash);
        assert_eq!(fq1, Fq::ZERO);
        assert_eq!(fq2, Fq::ZERO);
    }

    #[test]
    fn test_hash_to_fq_pair_known() {
        let mut hash = [0u8; 32];
        hash[0] = 0x01;
        let (fq1, _fq2) = hash_to_fq_pair(&hash);
        assert_eq!(fq1, Fq(U256::from_u64(1)));

        let mut hash2 = [0u8; 32];
        hash2[16] = 0x02;
        let (_fq1, fq2) = hash_to_fq_pair(&hash2);
        assert_eq!(fq2, Fq(U256::from_u64(2)));
    }

    #[test]
    fn test_hash_to_fq_pair_multi_byte() {
        let mut hash = [0u8; 32];
        hash[0] = 0xFF;
        hash[1] = 0x01;
        let (fq1, _) = hash_to_fq_pair(&hash);
        assert_eq!(fq1, Fq(U256::from_u64(511)));
    }

    #[test]
    fn test_hash_to_fq_pair_limb_boundary() {
        let mut hash = [0u8; 32];
        hash[8] = 0x01;
        let (fq1, _) = hash_to_fq_pair(&hash);
        let expected = U256([0, 1, 0, 0]);
        assert_eq!(fq1.0, expected);
    }

    #[test]
    fn test_u256_to_le_bytes_zero() {
        let val = U256::ZERO;
        let bytes = u256_to_le_bytes(&val);
        assert_eq!(bytes, [0u8; 32]);
    }

    #[test]
    fn test_u256_to_le_bytes_one() {
        let val = U256::from_u64(1);
        let bytes = u256_to_le_bytes(&val);
        let mut expected = [0u8; 32];
        expected[0] = 1;
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_u256_to_le_bytes_large() {
        let val = U256([0x0102030405060708, 0, 0, 0]);
        let bytes = u256_to_le_bytes(&val);
        assert_eq!(bytes[0], 0x08);
        assert_eq!(bytes[1], 0x07);
        assert_eq!(bytes[7], 0x01);
        for i in 8..32 {
            assert_eq!(bytes[i], 0);
        }
    }

    #[test]
    fn test_u256_to_le_bytes_roundtrip() {
        let val = U256([0xDEADBEEFCAFEBABE, 0x1234567890ABCDEF, 0, 0]);
        let le_bytes = u256_to_le_bytes(&val);
        let limb0 = u64::from_le_bytes(le_bytes[0..8].try_into().unwrap());
        let limb1 = u64::from_le_bytes(le_bytes[8..16].try_into().unwrap());
        let limb2 = u64::from_le_bytes(le_bytes[16..24].try_into().unwrap());
        let limb3 = u64::from_le_bytes(le_bytes[24..32].try_into().unwrap());
        let reconstructed = U256([limb0, limb1, limb2, limb3]);
        assert_eq!(val, reconstructed);
    }

    #[test]
    fn test_u256_to_le_is_be_reversed() {
        let val = U256([
            0x0102030405060708,
            0x090A0B0C0D0E0F10,
            0x1112131415161718,
            0x191A1B1C1D1E1F20,
        ]);
        let le = u256_to_le_bytes(&val);
        let be = val.to_be_bytes();
        for i in 0..32 {
            assert_eq!(le[i], be[31 - i], "mismatch at byte index {}", i);
        }
    }

    #[test]
    fn test_le_bytes_to_u256_empty_range() {
        let bytes = [0u8; 16];
        let val = le_bytes_to_u256(&bytes);
        assert_eq!(val, U256::ZERO);
    }

    #[test]
    fn test_le_bytes_to_u256_small() {
        let bytes = [42u8];
        let val = le_bytes_to_u256(&bytes);
        assert_eq!(val, U256::from_u64(42));
    }

    #[test]
    fn test_sha256_hash_chain_empty() {
        let (fq1, fq2) = sha256_hash_chain(&[]);
        let _ = fq1;
        let _ = fq2;
    }

    #[test]
    fn test_sha256_hash_chain_single() {
        let elements = [U256::ZERO];
        let (fq1, fq2) = sha256_hash_chain(&elements);
        let _ = fq1;
        let _ = fq2;
    }

    #[test]
    fn test_setup_transcript_no_panic() {
        let circuit_hash = [0xABu8; 32];
        let pub_inputs: &[Fq] = &[];
        let ec_coords: &[U256] = &[];
        let sponge = setup_transcript(&circuit_hash, pub_inputs, ec_coords);
        let _ = sponge;
    }

    #[test]
    fn test_setup_transcript_with_inputs() {
        let circuit_hash = [0x42u8; 32];
        let pub_inputs = [Fq::from_u64(100), Fq::from_u64(200)];
        let ec_coords = [
            U256::from_u64(1),
            U256::from_u64(2),
            U256::from_u64(3),
            U256::from_u64(4),
        ];
        let sponge = setup_transcript(&circuit_hash, &pub_inputs, &ec_coords);
        let _ = sponge;
    }

    #[test]
    #[should_panic(expected = "input_commit_coords must have even length")]
    fn test_setup_transcript_odd_coords_panics() {
        let circuit_hash = [0u8; 32];
        let pub_inputs: &[Fq] = &[];
        let ec_coords = [U256::from_u64(1), U256::from_u64(2), U256::from_u64(3)];
        let _ = setup_transcript(&circuit_hash, pub_inputs, &ec_coords);
    }
}
