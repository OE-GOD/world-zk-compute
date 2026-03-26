/// BN254 EC operations using pure Rust arithmetic.
/// Standalone port of the Stylus GKR verifier's EC module.
use crate::field::{Fq, Fr, FQ_MOD, U256};

/// A G1 affine point on BN254
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
        x: U256::from_be_slice(&output[0..32]),
        y: U256::from_be_slice(&output[32..64]),
    }
}

/// EC scalar multiplication using precompile at address 0x07.
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
        x: U256::from_be_slice(&output[0..32]),
        y: U256::from_be_slice(&output[32..64]),
    }
}

/// Multi-scalar multiplication: sum(s_i * P_i)
pub fn multi_scalar_mul(points: &[G1Point], scalars: &[U256]) -> G1Point {
    assert!(points.len() == scalars.len());
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
    let fq_mod = U256(FQ_MOD);
    G1Point {
        x: p.x,
        y: fq_mod.sub_no_mod(&p.y),
    }
}

/// Inner product <a, b> mod Fr
pub fn inner_product(a: &[U256], b: &[U256]) -> Fr {
    assert!(a.len() == b.len());
    let mut result = Fr::ZERO;
    for i in 0..a.len() {
        let ai = Fr::new(a[i]);
        let bi = Fr::new(b[i]);
        result = result.add(&ai.mul(&bi));
    }
    result
}

// ============================================================
// Precompile call implementation (native pure Rust)
// ============================================================

fn call_precompile(addr: u64, input: &[u8], _output_len: usize) -> Vec<u8> {
    match addr {
        0x06 => ec_add_native(input),
        0x07 => ec_mul_native(input),
        _ => {
            panic!("unknown precompile");
        }
    }
}

// ============================================================
// Native BN254 EC arithmetic (Jacobian coordinates for performance)
// ============================================================

/// Jacobian point (X, Y, Z) represents affine (X/Z^2, Y/Z^3). Infinity: Z=0.
struct JacPoint {
    x: Fq,
    y: Fq,
    z: Fq,
}

impl JacPoint {
    fn infinity() -> Self {
        JacPoint {
            x: Fq::ONE,
            y: Fq::ONE,
            z: Fq::ZERO,
        }
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
        let d = d.add(&d); // 2 * ((X+B)^2 - A - C)
        let e = a.add(&a).add(&a); // 3*X^2
        let f = e.square();
        let x3 = f.sub(&d).sub(&d);
        let c8 = c.add(&c);
        let c8 = c8.add(&c8);
        let c8 = c8.add(&c8); // 8*C
        let y3 = e.mul(&d.sub(&x3)).sub(&c8);
        let z3 = self.y.add(&self.y).mul(&self.z);
        JacPoint {
            x: x3,
            y: y3,
            z: z3,
        }
    }

    /// Mixed addition: self + affine point (px, py). ~8M + 3S
    fn add_affine(&self, px: Fq, py: Fq) -> Self {
        if px.0.is_zero() && py.0.is_zero() {
            return JacPoint {
                x: self.x,
                y: self.y,
                z: self.z,
            };
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
        JacPoint {
            x: x3,
            y: y3,
            z: z3,
        }
    }
}

fn ec_add_native(input: &[u8]) -> Vec<u8> {
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

fn ec_mul_native(input: &[u8]) -> Vec<u8> {
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

    // BN254 G1 generator
    fn g1() -> G1Point {
        G1Point {
            x: U256::from_u64(1),
            y: U256::from_u64(2),
        }
    }

    #[test]
    fn test_generator_on_curve() {
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
    fn test_inner_product_basic() {
        let a = [U256::from_u64(2), U256::from_u64(3)];
        let b = [U256::from_u64(4), U256::from_u64(5)];
        let result = inner_product(&a, &b);
        assert_eq!(result, Fr::from_u64(23));
    }
}
