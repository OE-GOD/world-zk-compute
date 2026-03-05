/// BN254 field arithmetic for Fq (base field) and Fr (scalar field).
/// Uses simple [u64; 4] limb representation with direct modular arithmetic.
/// No Montgomery form — keeps it simple and avoids conversion overhead.

/// BN254 base field modulus Fq
/// 21888242871839275222246405745257275088696311157297823662689037894645226208583
pub const FQ_MOD: [u64; 4] = [
    0x3c208c16d87cfd47,
    0x97816a916871ca8d,
    0xb85045b68181585d,
    0x30644e72e131a029,
];

/// BN254 scalar field modulus Fr
/// 21888242871839275222246405745257275088548364400416034343698204186575808495617
pub const FR_MOD: [u64; 4] = [
    0x43e1f593f0000001,
    0x2833e84879b97091,
    0xb85045b68181585d,
    0x30644e72e131a029,
];

/// A 256-bit unsigned integer stored as 4 little-endian u64 limbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct U256(pub [u64; 4]);

impl U256 {
    pub const ZERO: U256 = U256([0, 0, 0, 0]);
    pub const ONE: U256 = U256([1, 0, 0, 0]);

    /// Create from a single u64 value
    #[inline(always)]
    pub const fn from_u64(v: u64) -> Self {
        U256([v, 0, 0, 0])
    }

    /// Create from big-endian bytes (32 bytes)
    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        let mut limbs = [0u64; 4];
        // bytes[0..8] = most significant
        limbs[3] = u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        limbs[2] = u64::from_be_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        limbs[1] = u64::from_be_bytes([
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]);
        limbs[0] = u64::from_be_bytes([
            bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29], bytes[30], bytes[31],
        ]);
        U256(limbs)
    }

    /// Convert to big-endian bytes (32 bytes)
    pub fn to_be_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        let b3 = self.0[3].to_be_bytes();
        let b2 = self.0[2].to_be_bytes();
        let b1 = self.0[1].to_be_bytes();
        let b0 = self.0[0].to_be_bytes();
        out[0..8].copy_from_slice(&b3);
        out[8..16].copy_from_slice(&b2);
        out[16..24].copy_from_slice(&b1);
        out[24..32].copy_from_slice(&b0);
        out
    }

    /// Check if zero
    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    /// Compare: returns true if self >= other
    #[inline]
    pub fn gte(&self, other: &U256) -> bool {
        for i in (0..4).rev() {
            if self.0[i] > other.0[i] {
                return true;
            }
            if self.0[i] < other.0[i] {
                return false;
            }
        }
        true // equal
    }

    /// Subtraction: self - other (assumes self >= other)
    #[inline]
    pub fn sub_no_mod(&self, other: &U256) -> U256 {
        let mut result = [0u64; 4];
        let mut borrow: u64 = 0;
        for i in 0..4 {
            let (diff, b1) = self.0[i].overflowing_sub(other.0[i]);
            let (diff2, b2) = diff.overflowing_sub(borrow);
            result[i] = diff2;
            borrow = (b1 as u64) + (b2 as u64);
        }
        U256(result)
    }

    /// Addition without reduction
    #[inline]
    fn add_no_mod(&self, other: &U256) -> (U256, bool) {
        let mut result = [0u64; 4];
        let mut carry: u64 = 0;
        for i in 0..4 {
            let sum = (self.0[i] as u128) + (other.0[i] as u128) + (carry as u128);
            result[i] = sum as u64;
            carry = (sum >> 64) as u64;
        }
        (U256(result), carry != 0)
    }
}

/// Field element with associated modulus
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fq(pub U256);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fr(pub U256);

// ============================================================
// Fq operations (base field)
// ============================================================

impl Fq {
    pub const ZERO: Fq = Fq(U256::ZERO);
    pub const ONE: Fq = Fq(U256::ONE);
    pub const MODULUS: U256 = U256(FQ_MOD);

    #[inline(always)]
    pub fn new(v: U256) -> Self {
        Fq(v)
    }

    #[inline(always)]
    pub fn from_u64(v: u64) -> Self {
        Fq(U256::from_u64(v))
    }

    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        let v = U256::from_be_bytes(bytes);
        // Reduce mod p
        let modulus = U256(FQ_MOD);
        if v.gte(&modulus) {
            Fq(v.sub_no_mod(&modulus))
        } else {
            Fq(v)
        }
    }

    #[inline]
    pub fn add(&self, other: &Fq) -> Fq {
        let (sum, carry) = self.0.add_no_mod(&other.0);
        let modulus = U256(FQ_MOD);
        if carry || sum.gte(&modulus) {
            Fq(sum.sub_no_mod(&modulus))
        } else {
            Fq(sum)
        }
    }

    #[inline]
    pub fn sub(&self, other: &Fq) -> Fq {
        if self.0.gte(&other.0) {
            Fq(self.0.sub_no_mod(&other.0))
        } else {
            // self < other: result = modulus - (other - self)
            let diff = other.0.sub_no_mod(&self.0);
            Fq(U256(FQ_MOD).sub_no_mod(&diff))
        }
    }

    #[inline]
    pub fn neg(&self) -> Fq {
        if self.0.is_zero() {
            *self
        } else {
            Fq(U256(FQ_MOD).sub_no_mod(&self.0))
        }
    }

    /// Multiplication using schoolbook 512-bit multiply + Barrett-like reduction
    pub fn mul(&self, other: &Fq) -> Fq {
        let product = mul_wide(&self.0, &other.0);
        Fq(reduce_512(&product, &FQ_MOD))
    }

    /// Squaring
    #[inline]
    pub fn square(&self) -> Fq {
        self.mul(self)
    }

    /// x^5 (used in Poseidon S-box)
    #[inline]
    pub fn pow5(&self) -> Fq {
        let x2 = self.square();
        let x4 = x2.square();
        x4.mul(self)
    }

    /// Modular inverse via Fermat's little theorem: a^(p-2) mod p
    pub fn inv(&self) -> Fq {
        // p - 2
        let mut exp = U256(FQ_MOD);
        // Subtract 2
        exp.0[0] = exp.0[0].wrapping_sub(2);
        self.pow(&exp)
    }

    /// Modular exponentiation via square-and-multiply
    pub fn pow(&self, exp: &U256) -> Fq {
        let mut result = Fq::ONE;
        let mut base = *self;
        for i in 0..4 {
            let mut e = exp.0[i];
            for _ in 0..64 {
                if e & 1 == 1 {
                    result = result.mul(&base);
                }
                base = base.square();
                e >>= 1;
            }
        }
        result
    }
}

// ============================================================
// Fr operations (scalar field)
// ============================================================

impl Fr {
    pub const ZERO: Fr = Fr(U256::ZERO);
    pub const ONE: Fr = Fr(U256::ONE);
    pub const MODULUS: U256 = U256(FR_MOD);

    #[inline(always)]
    pub fn new(v: U256) -> Self {
        Fr(v)
    }

    #[inline(always)]
    pub fn from_u64(v: u64) -> Self {
        Fr(U256::from_u64(v))
    }

    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        let v = U256::from_be_bytes(bytes);
        let modulus = U256(FR_MOD);
        if v.gte(&modulus) {
            Fr(v.sub_no_mod(&modulus))
        } else {
            Fr(v)
        }
    }

    /// Reduce an Fq value modulo Fr (for squeeze % FR_MODULUS conversion)
    pub fn from_fq(fq: &Fq) -> Fr {
        let modulus = U256(FR_MOD);
        if fq.0.gte(&modulus) {
            Fr(fq.0.sub_no_mod(&modulus))
        } else {
            Fr(fq.0)
        }
    }

    #[inline]
    pub fn add(&self, other: &Fr) -> Fr {
        let (sum, carry) = self.0.add_no_mod(&other.0);
        let modulus = U256(FR_MOD);
        if carry || sum.gte(&modulus) {
            Fr(sum.sub_no_mod(&modulus))
        } else {
            Fr(sum)
        }
    }

    #[inline]
    pub fn sub(&self, other: &Fr) -> Fr {
        if self.0.gte(&other.0) {
            Fr(self.0.sub_no_mod(&other.0))
        } else {
            let diff = other.0.sub_no_mod(&self.0);
            Fr(U256(FR_MOD).sub_no_mod(&diff))
        }
    }

    #[inline]
    pub fn neg(&self) -> Fr {
        if self.0.is_zero() {
            *self
        } else {
            Fr(U256(FR_MOD).sub_no_mod(&self.0))
        }
    }

    pub fn mul(&self, other: &Fr) -> Fr {
        let product = mul_wide(&self.0, &other.0);
        Fr(reduce_512(&product, &FR_MOD))
    }

    #[inline]
    pub fn square(&self) -> Fr {
        self.mul(self)
    }

    /// Modular inverse via Fermat's little theorem
    pub fn inv(&self) -> Fr {
        let mut exp = U256(FR_MOD);
        exp.0[0] = exp.0[0].wrapping_sub(2);
        self.pow(&exp)
    }

    pub fn pow(&self, exp: &U256) -> Fr {
        let mut result = Fr::ONE;
        let mut base = *self;
        for i in 0..4 {
            let mut e = exp.0[i];
            for _ in 0..64 {
                if e & 1 == 1 {
                    result = result.mul(&base);
                }
                base = base.square();
                e >>= 1;
            }
        }
        result
    }
}

// ============================================================
// Wide multiplication and reduction
// ============================================================

/// 256x256 -> 512 bit multiplication (schoolbook)
fn mul_wide(a: &U256, b: &U256) -> [u64; 8] {
    let mut result = [0u64; 8];
    for i in 0..4 {
        let mut carry: u64 = 0;
        for j in 0..4 {
            let product = (a.0[i] as u128) * (b.0[j] as u128)
                + (result[i + j] as u128)
                + (carry as u128);
            result[i + j] = product as u64;
            carry = (product >> 64) as u64;
        }
        result[i + 4] = carry;
    }
    result
}

/// Reduce a 512-bit value modulo a 256-bit modulus using repeated subtraction
/// of shifted modulus. This is a simple approach suitable for WASM.
fn reduce_512(val: &[u64; 8], modulus: &[u64; 4]) -> U256 {
    // We do division by trial subtraction.
    // For our use case, a*b where a,b < p, the quotient fits in 256 bits.
    // Use the standard approach: iterate from MSB down, trial subtract.

    // Simple approach: convert to double-width and do modular reduction
    // by dividing the 512-bit number by the 256-bit modulus.

    // We use a limb-by-limb long division approach.
    // But a simpler approach for 256-bit moduli: use the fact that
    // the result fits in 256 bits after reduction.

    // Barrett reduction approximation:
    // q_hat = val / modulus (approx)
    // r = val - q_hat * modulus
    // Adjust r if needed

    // For simplicity, use iterative subtraction with shifts.
    // Since we're in WASM, this is efficient enough for our needs.

    // Copy to mutable working space
    let mut rem = [0u64; 9]; // extra limb for safety
    rem[..8].copy_from_slice(val);

    // Reduce: while rem >= modulus * 2^(64*i), subtract
    // Process from high limb down
    for shift in (0..5).rev() {
        // Shifted modulus occupies limbs [shift..shift+4]
        loop {
            // Check if rem[shift..shift+5] >= modulus shifted
            let mut can_sub = false;
            // Compare rem[shift+4] (high limb of shifted region) first
            if shift + 4 < 9 && rem[shift + 4] > 0 {
                can_sub = true;
            } else if shift + 4 < 9 && rem[shift + 4] == 0 {
                // Compare lower limbs
                let mut ge = true;
                for k in (0..4).rev() {
                    if rem[shift + k] > modulus[k] {
                        break;
                    }
                    if rem[shift + k] < modulus[k] {
                        ge = false;
                        break;
                    }
                }
                can_sub = ge;
            }

            if !can_sub {
                break;
            }

            // Subtract modulus from rem[shift..]
            let mut borrow: u64 = 0;
            for k in 0..4 {
                let (diff, b1) = rem[shift + k].overflowing_sub(modulus[k]);
                let (diff2, b2) = diff.overflowing_sub(borrow);
                rem[shift + k] = diff2;
                borrow = (b1 as u64) + (b2 as u64);
            }
            if shift + 4 < 9 {
                rem[shift + 4] = rem[shift + 4].wrapping_sub(borrow);
            }
        }
    }

    U256([rem[0], rem[1], rem[2], rem[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fq_add() {
        let a = Fq::from_u64(10);
        let b = Fq::from_u64(20);
        let c = a.add(&b);
        assert_eq!(c, Fq::from_u64(30));
    }

    #[test]
    fn test_fq_sub() {
        let a = Fq::from_u64(10);
        let b = Fq::from_u64(20);
        let c = b.sub(&a);
        assert_eq!(c, Fq::from_u64(10));
    }

    #[test]
    fn test_fq_sub_wrap() {
        let a = Fq::from_u64(10);
        let b = Fq::from_u64(20);
        let c = a.sub(&b); // should wrap around
        // c = Fq_MOD - 10
        let expected = Fq(U256(FQ_MOD)).sub(&Fq::from_u64(10));
        assert_eq!(c, expected);
    }

    #[test]
    fn test_fq_mul() {
        let a = Fq::from_u64(7);
        let b = Fq::from_u64(11);
        let c = a.mul(&b);
        assert_eq!(c, Fq::from_u64(77));
    }

    #[test]
    fn test_fq_pow5() {
        let a = Fq::from_u64(3);
        let c = a.pow5();
        assert_eq!(c, Fq::from_u64(243)); // 3^5
    }

    #[test]
    fn test_fq_inv() {
        let a = Fq::from_u64(7);
        let inv = a.inv();
        let product = a.mul(&inv);
        assert_eq!(product, Fq::ONE);
    }

    #[test]
    fn test_fr_mul() {
        let a = Fr::from_u64(100);
        let b = Fr::from_u64(200);
        let c = a.mul(&b);
        assert_eq!(c, Fr::from_u64(20000));
    }

    #[test]
    fn test_fr_inv() {
        let a = Fr::from_u64(13);
        let inv = a.inv();
        let product = a.mul(&inv);
        assert_eq!(product, Fr::ONE);
    }

    #[test]
    fn test_u256_from_be_bytes() {
        let mut bytes = [0u8; 32];
        bytes[31] = 42;
        let v = U256::from_be_bytes(&bytes);
        assert_eq!(v, U256::from_u64(42));
    }

    #[test]
    fn test_fq_large_mul() {
        // Test that multiplication works with large values near the modulus
        let a = Fq(U256(FQ_MOD)).sub(&Fq::ONE); // p - 1
        let b = Fq(U256(FQ_MOD)).sub(&Fq::ONE); // p - 1
        let c = a.mul(&b); // (p-1)^2 mod p = 1
        assert_eq!(c, Fq::ONE);
    }

    #[test]
    fn test_fr_neg() {
        let a = Fr::from_u64(5);
        let neg_a = a.neg();
        let sum = a.add(&neg_a);
        assert_eq!(sum, Fr::ZERO);
    }

    #[test]
    fn test_fr_from_fq() {
        // When Fq value is less than Fr modulus, it should pass through unchanged
        let fq = Fq::from_u64(42);
        let fr = Fr::from_fq(&fq);
        assert_eq!(fr, Fr::from_u64(42));
    }
}
