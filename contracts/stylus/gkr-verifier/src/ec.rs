/// BN254 EC operations via EVM precompiles.
/// On Stylus, these are called via RawCall to precompile addresses.

use alloc::vec::Vec;

use crate::field::{Fr, U256};

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
#[cfg(all(target_arch = "wasm32", feature = "stylus"))]
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

/// Native/non-stylus implementation using pure Rust
#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
fn call_precompile(addr: u64, input: &[u8], _output_len: usize) -> Vec<u8> {
    match addr {
        0x02 => sha256_native(input).to_vec(),
        0x06 => ec_add_native(input),
        0x07 => ec_mul_native(input),
        _ => panic!("unknown precompile"),
    }
}

// ============================================================
// Native SHA-256 implementation (FIPS 180-4)
// ============================================================

#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
fn sha256_native(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    // Padding: append 1 bit, zeros, then 64-bit big-endian length
    let bit_len = (data.len() as u64) * 8;
    let mut padded = alloc::vec::Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit block
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

// ============================================================
// Native BN254 EC arithmetic (Jacobian coordinates for performance)
// ============================================================

#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
use crate::field::Fq;

/// Jacobian point (X, Y, Z) represents affine (X/Z², Y/Z³). Infinity: Z=0.
#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
struct JacPoint {
    x: Fq,
    y: Fq,
    z: Fq,
}

#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
impl JacPoint {
    fn infinity() -> Self {
        JacPoint { x: Fq::ONE, y: Fq::ONE, z: Fq::ZERO }
    }

    fn is_infinity(&self) -> bool {
        self.z.0.is_zero()
    }

    fn from_affine(x: Fq, y: Fq) -> Self {
        if x.0.is_zero() && y.0.is_zero() {
            return Self::infinity();
        }
        JacPoint { x, y, z: Fq::ONE }
    }

    fn to_affine(&self) -> (Fq, Fq) {
        if self.is_infinity() {
            return (Fq::ZERO, Fq::ZERO);
        }
        let z_inv = self.z.inv();
        let z_inv2 = z_inv.square();
        let z_inv3 = z_inv2.mul(&z_inv);
        (self.x.mul(&z_inv2), self.y.mul(&z_inv3))
    }

    /// Point doubling for BN254 (a=0). ~4M + 4S
    fn double(&self) -> Self {
        if self.is_infinity() || self.y.0.is_zero() {
            return Self::infinity();
        }
        let a = self.x.square();
        let b = self.y.square();
        let c = b.square();
        let xpb = self.x.add(&b);
        let d = xpb.square().sub(&a).sub(&c);
        let d = d.add(&d); // 2 * ((X+B)² - A - C)
        let e = a.add(&a).add(&a); // 3*X²
        let f = e.square();
        let x3 = f.sub(&d).sub(&d);
        let c8 = c.add(&c);
        let c8 = c8.add(&c8);
        let c8 = c8.add(&c8); // 8*C
        let y3 = e.mul(&d.sub(&x3)).sub(&c8);
        let z3 = self.y.add(&self.y).mul(&self.z);
        JacPoint { x: x3, y: y3, z: z3 }
    }

    /// Mixed addition: self + affine point (px, py). ~8M + 3S
    fn add_affine(&self, px: Fq, py: Fq) -> Self {
        if px.0.is_zero() && py.0.is_zero() {
            return JacPoint { x: self.x, y: self.y, z: self.z };
        }
        if self.is_infinity() {
            return Self::from_affine(px, py);
        }
        let z1z1 = self.z.square();
        let u2 = px.mul(&z1z1);
        let s2 = py.mul(&self.z).mul(&z1z1);
        let h = u2.sub(&self.x);
        let r = s2.sub(&self.y);
        if h.0.is_zero() {
            if r.0.is_zero() {
                return self.double();
            }
            return Self::infinity();
        }
        let hh = h.square();
        let hhh = h.mul(&hh);
        let x1hh = self.x.mul(&hh);
        let x3 = r.square().sub(&hhh).sub(&x1hh).sub(&x1hh);
        let y3 = r.mul(&x1hh.sub(&x3)).sub(&self.y.mul(&hhh));
        let z3 = h.mul(&self.z);
        JacPoint { x: x3, y: y3, z: z3 }
    }
}

#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
fn ec_add_native(input: &[u8]) -> Vec<u8> {
    use alloc::vec;
    assert!(input.len() >= 128, "ecadd: need 128 bytes");

    let x1 = Fq::from_be_bytes(input[0..32].try_into().unwrap());
    let y1 = Fq::from_be_bytes(input[32..64].try_into().unwrap());
    let x2 = Fq::from_be_bytes(input[64..96].try_into().unwrap());
    let y2 = Fq::from_be_bytes(input[96..128].try_into().unwrap());

    let p1 = JacPoint::from_affine(x1, y1);
    let result = p1.add_affine(x2, y2);
    let (rx, ry) = result.to_affine();

    let mut out = vec![0u8; 64];
    out[0..32].copy_from_slice(&rx.0.to_be_bytes());
    out[32..64].copy_from_slice(&ry.0.to_be_bytes());
    out
}

#[cfg(not(all(target_arch = "wasm32", feature = "stylus")))]
fn ec_mul_native(input: &[u8]) -> Vec<u8> {
    use alloc::vec;
    assert!(input.len() >= 96, "ecmul: need 96 bytes");

    let px = Fq::from_be_bytes(input[0..32].try_into().unwrap());
    let py = Fq::from_be_bytes(input[32..64].try_into().unwrap());
    let scalar = U256::from_be_bytes(input[64..96].try_into().unwrap());

    if (px.0.is_zero() && py.0.is_zero()) || scalar.is_zero() {
        return vec![0u8; 64];
    }

    // Double-and-add in Jacobian coordinates (one inversion at the end)
    let mut acc = JacPoint::infinity();
    let mut started = false;

    for limb_idx in (0..4).rev() {
        let limb = scalar.0[limb_idx];
        for bit_idx in (0..64).rev() {
            if started {
                acc = acc.double();
            }
            if (limb >> bit_idx) & 1 == 1 {
                if !started {
                    acc = JacPoint::from_affine(px, py);
                    started = true;
                } else {
                    acc = acc.add_affine(px, py);
                }
            }
        }
    }

    let (rx, ry) = acc.to_affine();
    let mut out = vec![0u8; 64];
    out[0..32].copy_from_slice(&rx.0.to_be_bytes());
    out[32..64].copy_from_slice(&ry.0.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Fq;

    // BN254 G1 generator
    fn g1() -> G1Point {
        G1Point {
            x: U256::from_u64(1),
            y: U256::from_u64(2),
        }
    }

    #[test]
    fn test_generator_on_curve() {
        // y² = x³ + 3 for BN254
        let g = g1();
        let x = Fq::new(g.x);
        let y = Fq::new(g.y);
        let lhs = y.mul(&y);
        let rhs = x.mul(&x).mul(&x).add(&Fq::from_u64(3));
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn test_ec_add_identity() {
        let g = g1();
        let zero = G1Point::INFINITY;
        let r = ec_add(&g, &zero);
        assert_eq!(r, g);
        let r2 = ec_add(&zero, &g);
        assert_eq!(r2, g);
    }

    #[test]
    fn test_ec_add_self_is_double() {
        let g = g1();
        let double_via_add = ec_add(&g, &g);
        let double_via_mul = ec_mul(&g, &U256::from_u64(2));
        assert_eq!(double_via_add, double_via_mul);
    }

    #[test]
    fn test_ec_double_on_curve() {
        let g = g1();
        let g2 = ec_add(&g, &g);
        // Verify 2G is on the curve: y² = x³ + 3
        let x = Fq::new(g2.x);
        let y = Fq::new(g2.y);
        let lhs = y.mul(&y);
        let rhs = x.mul(&x).mul(&x).add(&Fq::from_u64(3));
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn test_ec_mul_by_one() {
        let g = g1();
        let r = ec_mul(&g, &U256::ONE);
        assert_eq!(r, g);
    }

    #[test]
    fn test_ec_mul_by_zero() {
        let g = g1();
        let r = ec_mul(&g, &U256::ZERO);
        assert!(r.is_infinity());
    }

    #[test]
    fn test_ec_mul_3g() {
        let g = g1();
        let g3_add = ec_add(&ec_add(&g, &g), &g);
        let g3_mul = ec_mul(&g, &U256::from_u64(3));
        assert_eq!(g3_add, g3_mul);
    }

    #[test]
    fn test_ec_add_inverse() {
        let g = g1();
        let neg_g = negate(&g);
        let r = ec_add(&g, &neg_g);
        assert!(r.is_infinity());
    }

    #[test]
    fn test_ec_mul_result_on_curve() {
        let g = g1();
        let r = ec_mul(&g, &U256::from_u64(42));
        if !r.is_infinity() {
            let x = Fq::new(r.x);
            let y = Fq::new(r.y);
            let lhs = y.mul(&y);
            let rhs = x.mul(&x).mul(&x).add(&Fq::from_u64(3));
            assert_eq!(lhs, rhs, "42*G should be on curve");
        }
    }

    #[test]
    fn test_ec_mul_associative() {
        // 2 * (3 * G) == 6 * G
        let g = g1();
        let g3 = ec_mul(&g, &U256::from_u64(3));
        let g6_a = ec_mul(&g3, &U256::from_u64(2));
        let g6_b = ec_mul(&g, &U256::from_u64(6));
        assert_eq!(g6_a, g6_b);
    }

    #[test]
    fn test_msm_basic() {
        let g = g1();
        let g2 = ec_mul(&g, &U256::from_u64(2));
        // 3*G + 5*(2G) = 3G + 10G = 13G
        let r = multi_scalar_mul(&[g, g2], &[U256::from_u64(3), U256::from_u64(5)]);
        let expected = ec_mul(&g, &U256::from_u64(13));
        assert_eq!(r, expected);
    }

    #[test]
    fn test_negate_double_negate() {
        let g = g1();
        let neg_g = negate(&g);
        let neg_neg_g = negate(&neg_g);
        assert_eq!(neg_neg_g, g);
    }

    #[test]
    fn test_sha256_empty() {
        // SHA-256 of empty string = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = sha256(&[]);
        assert_eq!(hash[0], 0xe3);
        assert_eq!(hash[1], 0xb0);
        assert_eq!(hash[31], 0x55);
    }

    #[test]
    fn test_sha256_abc() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let hash = sha256(b"abc");
        assert_eq!(hash[0], 0xba);
        assert_eq!(hash[1], 0x78);
        assert_eq!(hash[31], 0xad);
    }

    #[test]
    fn test_inner_product_basic() {
        let a = [U256::from_u64(2), U256::from_u64(3)];
        let b = [U256::from_u64(4), U256::from_u64(5)];
        let result = inner_product(&a, &b);
        // 2*4 + 3*5 = 8 + 15 = 23
        assert_eq!(result, Fr::from_u64(23));
    }
}
