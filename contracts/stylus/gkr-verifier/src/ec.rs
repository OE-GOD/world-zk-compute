/// BN254 EC operations via EVM precompiles.
/// On Stylus, these are called via RawCall to precompile addresses.

use alloc::vec::Vec;

use crate::field::{Fq, Fr, U256};

/// A G1 affine point on BN254
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct G1Point {
    pub x: U256,
    pub y: U256,
}

impl G1Point {
    pub const INFINITY: G1Point = G1Point {
        x: U256::ZERO,
        y: U256::ZERO,
    };

    pub fn is_infinity(&self) -> bool {
        self.x.is_zero() && self.y.is_zero()
    }

    /// Decode from 64 bytes (x: 32 BE bytes, y: 32 BE bytes)
    pub fn from_be_bytes(data: &[u8]) -> Self {
        assert!(data.len() >= 64);
        let mut x_bytes = [0u8; 32];
        let mut y_bytes = [0u8; 32];
        x_bytes.copy_from_slice(&data[0..32]);
        y_bytes.copy_from_slice(&data[32..64]);
        G1Point {
            x: U256::from_be_bytes(&x_bytes),
            y: U256::from_be_bytes(&y_bytes),
        }
    }

    /// Encode to 64 bytes (x: 32 BE bytes, y: 32 BE bytes)
    pub fn to_be_bytes(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[0..32].copy_from_slice(&self.x.to_be_bytes());
        out[32..64].copy_from_slice(&self.y.to_be_bytes());
        out
    }
}

/// Pedersen generator set
pub struct PedersenGens {
    pub message_gens: Vec<G1Point>,
    pub scalar_gen: G1Point,
    pub blinding_gen: G1Point,
}

/// PODP proof (Proof of Dot Product)
pub struct PODPProof {
    pub commit_d: G1Point,
    pub commit_d_dot_a: G1Point,
    pub z_vector: Vec<U256>,
    pub z_delta: U256,
    pub z_beta: U256,
}

/// ProofOfProduct
pub struct ProofOfProduct {
    pub alpha: G1Point,
    pub beta: G1Point,
    pub delta: G1Point,
    pub z1: U256,
    pub z2: U256,
    pub z3: U256,
    pub z4: U256,
    pub z5: U256,
}

// ============================================================
// EC precompile operations
// ============================================================

/// EC point addition using precompile at address 0x06.
/// Input: 128 bytes (P1.x, P1.y, P2.x, P2.y) each 32 bytes BE
/// Output: 64 bytes (R.x, R.y) each 32 bytes BE
pub fn ec_add(p1: &G1Point, p2: &G1Point) -> G1Point {
    if p1.is_infinity() {
        return *p2;
    }
    if p2.is_infinity() {
        return *p1;
    }

    let mut input = [0u8; 128];
    input[0..32].copy_from_slice(&p1.x.to_be_bytes());
    input[32..64].copy_from_slice(&p1.y.to_be_bytes());
    input[64..96].copy_from_slice(&p2.x.to_be_bytes());
    input[96..128].copy_from_slice(&p2.y.to_be_bytes());

    let output = call_precompile(0x06, &input, 64);
    G1Point {
        x: U256::from_be_bytes(&output[0..32].try_into().unwrap()),
        y: U256::from_be_bytes(&output[32..64].try_into().unwrap()),
    }
}

/// EC scalar multiplication using precompile at address 0x07.
/// Input: 96 bytes (P.x, P.y, scalar) each 32 bytes BE
/// Output: 64 bytes (R.x, R.y) each 32 bytes BE
pub fn ec_mul(p: &G1Point, s: &U256) -> G1Point {
    if p.is_infinity() || s.is_zero() {
        return G1Point::INFINITY;
    }

    let mut input = [0u8; 96];
    input[0..32].copy_from_slice(&p.x.to_be_bytes());
    input[32..64].copy_from_slice(&p.y.to_be_bytes());
    input[64..96].copy_from_slice(&s.to_be_bytes());

    let output = call_precompile(0x07, &input, 64);
    G1Point {
        x: U256::from_be_bytes(&output[0..32].try_into().unwrap()),
        y: U256::from_be_bytes(&output[32..64].try_into().unwrap()),
    }
}

/// Multi-scalar multiplication: sum(s_i * P_i)
pub fn multi_scalar_mul(points: &[G1Point], scalars: &[U256]) -> G1Point {
    assert_eq!(points.len(), scalars.len());
    if points.is_empty() {
        return G1Point::INFINITY;
    }

    let mut result = ec_mul(&points[0], &scalars[0]);
    for i in 1..points.len() {
        let term = ec_mul(&points[i], &scalars[i]);
        result = ec_add(&result, &term);
    }
    result
}

/// MSM with truncated generators (use first n generators)
pub fn msm_with_truncated_gens(all_gens: &[G1Point], scalars: &[U256]) -> G1Point {
    let n = scalars.len();
    assert!(n <= all_gens.len());
    multi_scalar_mul(&all_gens[..n], scalars)
}

/// Negate a G1 point (reflect over x-axis)
pub fn negate(p: &G1Point) -> G1Point {
    if p.is_infinity() {
        return *p;
    }
    let fq_mod = U256(crate::field::FQ_MOD);
    G1Point {
        x: p.x,
        y: fq_mod.sub_no_mod(&p.y),
    }
}

/// Inner product <a, b> mod Fr
pub fn inner_product(a: &[U256], b: &[U256]) -> Fr {
    assert_eq!(a.len(), b.len());
    let mut result = Fr::ZERO;
    for i in 0..a.len() {
        let ai = Fr::new(a[i]);
        let bi = Fr::new(b[i]);
        result = result.add(&ai.mul(&bi));
    }
    result
}

/// SHA-256 via precompile at address 0x02.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let output = call_precompile(0x02, data, 32);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&output[..32]);
    hash
}

// ============================================================
// Precompile call implementation
// ============================================================

/// Call an EVM precompile. On Stylus, uses RawCall::new_static().
/// For native testing, uses a mock/stub.
#[cfg(target_arch = "wasm32")]
fn call_precompile(addr: u64, input: &[u8], output_len: usize) -> Vec<u8> {
    use stylus_sdk::call::RawCall;
    use alloy_primitives::Address;

    let address = Address::from_word(alloy_primitives::B256::from({
        let mut bytes = [0u8; 32];
        bytes[31] = addr as u8;
        bytes
    }));

    let result = unsafe {
        RawCall::new_static()
            .limit_return_data(0, output_len)
            .call(address, input)
    };

    match result {
        Ok(data) => data,
        Err(_) => panic!("precompile call failed"),
    }
}

/// Native test implementation using pure Rust (for unit tests)
#[cfg(not(target_arch = "wasm32"))]
fn call_precompile(addr: u64, input: &[u8], output_len: usize) -> Vec<u8> {
    match addr {
        0x02 => {
            // SHA-256
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            // For testing, we'll use a simple placeholder
            // In real tests, we should use the sha2 crate
            // For now, return zeros — tests should use known vectors
            let mut out = vec![0u8; 32];
            // Simple hash for testing (NOT cryptographic)
            let mut h: u64 = 0xcbf29ce484222325;
            for &b in input {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            out[0..8].copy_from_slice(&h.to_le_bytes());
            out
        }
        0x06 => {
            // EC Add — for testing, use the field math directly
            // This is a simplified stub; real tests should validate against known vectors
            ec_add_native(input)
        }
        0x07 => {
            // EC Mul — for testing, simplified stub
            ec_mul_native(input)
        }
        _ => panic!("unknown precompile {}", addr),
    }
}

/// Native EC add for testing (placeholder — returns identity for now)
/// TODO: Replace with actual BN254 point addition for integration tests
#[cfg(not(target_arch = "wasm32"))]
fn ec_add_native(input: &[u8]) -> Vec<u8> {
    // Parse points
    let mut x1_bytes = [0u8; 32];
    let mut y1_bytes = [0u8; 32];
    let mut x2_bytes = [0u8; 32];
    let mut y2_bytes = [0u8; 32];
    x1_bytes.copy_from_slice(&input[0..32]);
    y1_bytes.copy_from_slice(&input[32..64]);
    x2_bytes.copy_from_slice(&input[64..96]);
    y2_bytes.copy_from_slice(&input[96..128]);

    let p1_x = U256::from_be_bytes(&x1_bytes);
    let p1_y = U256::from_be_bytes(&y1_bytes);
    let p2_x = U256::from_be_bytes(&x2_bytes);
    let p2_y = U256::from_be_bytes(&y2_bytes);

    // If either point is identity, return the other
    if p1_x.is_zero() && p1_y.is_zero() {
        let mut out = vec![0u8; 64];
        out[0..32].copy_from_slice(&p2_x.to_be_bytes());
        out[32..64].copy_from_slice(&p2_y.to_be_bytes());
        return out;
    }
    if p2_x.is_zero() && p2_y.is_zero() {
        let mut out = vec![0u8; 64];
        out[0..32].copy_from_slice(&p1_x.to_be_bytes());
        out[32..64].copy_from_slice(&p1_y.to_be_bytes());
        return out;
    }

    // For now, return a placeholder. Real testing requires full BN254 arithmetic.
    // Integration tests will use the actual Stylus devnode.
    vec![0u8; 64]
}

/// Native EC mul for testing (placeholder)
/// TODO: Replace with actual BN254 scalar multiplication for integration tests
#[cfg(not(target_arch = "wasm32"))]
fn ec_mul_native(input: &[u8]) -> Vec<u8> {
    let mut x_bytes = [0u8; 32];
    let mut y_bytes = [0u8; 32];
    x_bytes.copy_from_slice(&input[0..32]);
    y_bytes.copy_from_slice(&input[32..64]);

    let p_x = U256::from_be_bytes(&x_bytes);
    let p_y = U256::from_be_bytes(&y_bytes);

    // If point is identity, return identity
    if p_x.is_zero() && p_y.is_zero() {
        return vec![0u8; 64];
    }

    // For now, return a placeholder
    vec![0u8; 64]
}
