/// BN254 field arithmetic for Fq (base field) and Fr (scalar field).
/// Uses simple [u64; 4] limb representation with direct modular arithmetic.
/// Fq/Fr share common implementations via free functions to minimize code size.

/// BN254 base field modulus Fq
pub const FQ_MOD: [u64; 4] = [
    0x3c208c16d87cfd47,
    0x97816a916871ca8d,
    0xb85045b68181585d,
    0x30644e72e131a029,
];

/// BN254 scalar field modulus Fr
pub const FR_MOD: [u64; 4] = [
    0x43e1f593f0000001,
    0x2833e84879b97091,
    0xb85045b68181585d,
    0x30644e72e131a029,
];

/// A 256-bit unsigned integer stored as 4 little-endian u64 limbs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct U256(pub [u64; 4]);

impl core::fmt::Debug for U256 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "U256({:#x}, {:#x}, {:#x}, {:#x})",
            self.0[3], self.0[2], self.0[1], self.0[0]
        )
    }
}

impl U256 {
    pub const ZERO: U256 = U256([0, 0, 0, 0]);
    pub const ONE: U256 = U256([1, 0, 0, 0]);

    #[inline(always)]
    pub const fn from_u64(v: u64) -> Self {
        U256([v, 0, 0, 0])
    }

    /// Like `from_be_bytes` but takes a slice (must be >= 32 bytes).
    #[inline]
    pub fn from_be_slice(bytes: &[u8]) -> Self {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes[..32]);
        Self::from_be_bytes(&arr)
    }

    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        let mut limbs = [0u64; 4];
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

    #[inline(always)]
    pub fn is_zero(&self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

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
        true
    }

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

// ============================================================
// Shared field operations (compiled once, used by both Fq and Fr)
// ============================================================

#[inline(never)]
fn field_add(a: &U256, b: &U256, modulus: &[u64; 4]) -> U256 {
    let (sum, carry) = a.add_no_mod(b);
    let m = U256(*modulus);
    if carry || sum.gte(&m) {
        sum.sub_no_mod(&m)
    } else {
        sum
    }
}

#[inline(never)]
fn field_sub(a: &U256, b: &U256, modulus: &[u64; 4]) -> U256 {
    if a.gte(b) {
        a.sub_no_mod(b)
    } else {
        let diff = b.sub_no_mod(a);
        U256(*modulus).sub_no_mod(&diff)
    }
}

#[inline(never)]
fn field_neg(a: &U256, modulus: &[u64; 4]) -> U256 {
    if a.is_zero() {
        *a
    } else {
        U256(*modulus).sub_no_mod(a)
    }
}

#[inline(never)]
fn field_mul(a: &U256, b: &U256, modulus: &[u64; 4]) -> U256 {
    let product = mul_wide(a, b);
    reduce_512(&product, modulus)
}

#[inline(never)]
fn field_pow(base: &U256, exp: &U256, modulus: &[u64; 4]) -> U256 {
    let mut result = U256::ONE;
    let mut b = *base;
    for i in 0..4 {
        let mut e = exp.0[i];
        for _ in 0..64 {
            if e & 1 == 1 {
                result = field_mul(&result, &b, modulus);
            }
            b = field_mul(&b, &b, modulus);
            e >>= 1;
        }
    }
    result
}

#[inline(never)]
fn field_inv(a: &U256, modulus: &[u64; 4]) -> U256 {
    let mut exp = U256(*modulus);
    exp.0[0] = exp.0[0].wrapping_sub(2);
    field_pow(a, &exp, modulus)
}

fn field_from_be_bytes(bytes: &[u8; 32], modulus: &[u64; 4]) -> U256 {
    let v = U256::from_be_bytes(bytes);
    let m = U256(*modulus);
    if v.gte(&m) {
        v.sub_no_mod(&m)
    } else {
        v
    }
}

// ============================================================
// Fq (base field) -- thin wrapper
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Fq(pub U256);

impl core::fmt::Debug for Fq {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Fq({:?})", self.0)
    }
}

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
        Fq(field_from_be_bytes(bytes, &FQ_MOD))
    }

    #[inline]
    pub fn add(&self, other: &Fq) -> Fq {
        Fq(field_add(&self.0, &other.0, &FQ_MOD))
    }
    #[inline]
    pub fn sub(&self, other: &Fq) -> Fq {
        Fq(field_sub(&self.0, &other.0, &FQ_MOD))
    }
    #[inline]
    pub fn neg(&self) -> Fq {
        Fq(field_neg(&self.0, &FQ_MOD))
    }
    #[inline]
    pub fn mul(&self, other: &Fq) -> Fq {
        Fq(field_mul(&self.0, &other.0, &FQ_MOD))
    }
    #[inline]
    pub fn square(&self) -> Fq {
        self.mul(self)
    }
    pub fn inv(&self) -> Fq {
        Fq(field_inv(&self.0, &FQ_MOD))
    }
    pub fn pow(&self, exp: &U256) -> Fq {
        Fq(field_pow(&self.0, exp, &FQ_MOD))
    }

    #[inline]
    pub fn pow5(&self) -> Fq {
        let x2 = self.square();
        let x4 = x2.square();
        x4.mul(self)
    }
}

// ============================================================
// Fr (scalar field) -- thin wrapper
// ============================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Fr(pub U256);

impl core::fmt::Debug for Fr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Fr({:?})", self.0)
    }
}

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
        Fr(field_from_be_bytes(bytes, &FR_MOD))
    }

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
        Fr(field_add(&self.0, &other.0, &FR_MOD))
    }
    #[inline]
    pub fn sub(&self, other: &Fr) -> Fr {
        Fr(field_sub(&self.0, &other.0, &FR_MOD))
    }
    #[inline]
    pub fn neg(&self) -> Fr {
        Fr(field_neg(&self.0, &FR_MOD))
    }
    pub fn mul(&self, other: &Fr) -> Fr {
        Fr(field_mul(&self.0, &other.0, &FR_MOD))
    }
    #[inline]
    pub fn square(&self) -> Fr {
        self.mul(self)
    }
    pub fn inv(&self) -> Fr {
        Fr(field_inv(&self.0, &FR_MOD))
    }
    pub fn pow(&self, exp: &U256) -> Fr {
        Fr(field_pow(&self.0, exp, &FR_MOD))
    }
}

// ============================================================
// Wide multiplication and reduction
// ============================================================

fn mul_wide(a: &U256, b: &U256) -> [u64; 8] {
    let mut result = [0u64; 8];
    for i in 0..4 {
        let mut carry: u64 = 0;
        for j in 0..4 {
            let product =
                (a.0[i] as u128) * (b.0[j] as u128) + (result[i + j] as u128) + (carry as u128);
            result[i + j] = product as u64;
            carry = (product >> 64) as u64;
        }
        result[i + 4] = carry;
    }
    result
}

/// Reduce a 512-bit value modulo a 256-bit modulus using Knuth's Algorithm D.
fn reduce_512(val: &[u64; 8], modulus: &[u64; 4]) -> U256 {
    let n = 4usize;
    let s = modulus[n - 1].leading_zeros();

    let vn: [u64; 4] = if s > 0 {
        [
            modulus[0] << s,
            (modulus[1] << s) | (modulus[0] >> (64 - s)),
            (modulus[2] << s) | (modulus[1] >> (64 - s)),
            (modulus[3] << s) | (modulus[2] >> (64 - s)),
        ]
    } else {
        [modulus[0], modulus[1], modulus[2], modulus[3]]
    };

    let mut un = [0u64; 9];
    if s > 0 {
        un[8] = val[7] >> (64 - s);
        for i in (1..8).rev() {
            un[i] = (val[i] << s) | (val[i - 1] >> (64 - s));
        }
        un[0] = val[0] << s;
    } else {
        un[..8].copy_from_slice(val);
        un[8] = 0;
    }

    for j in (0..5).rev() {
        let dividend_top = ((un[j + n] as u128) << 64) | (un[j + n - 1] as u128);
        let mut q_hat = dividend_top / (vn[n - 1] as u128);
        let mut r_hat = dividend_top % (vn[n - 1] as u128);

        loop {
            if q_hat >= (1u128 << 64)
                || q_hat * (vn[n - 2] as u128) > ((r_hat << 64) | (un[j + n - 2] as u128))
            {
                q_hat -= 1;
                r_hat += vn[n - 1] as u128;
                if r_hat < (1u128 << 64) {
                    continue;
                }
            }
            break;
        }

        let mut borrow: i128 = 0;
        for i in 0..n {
            let prod = q_hat * (vn[i] as u128);
            let t = (un[j + i] as i128) - borrow - ((prod as u64) as i128);
            un[j + i] = t as u64;
            borrow = ((prod >> 64) as i128) - (t >> 64);
        }
        let t = (un[j + n] as i128) - borrow;
        un[j + n] = t as u64;

        if t < 0 {
            let mut carry: u64 = 0;
            for i in 0..n {
                let sum = (un[j + i] as u128) + (vn[i] as u128) + (carry as u128);
                un[j + i] = sum as u64;
                carry = (sum >> 64) as u64;
            }
            un[j + n] = un[j + n].wrapping_add(carry);
        }
    }

    if s > 0 {
        U256([
            (un[0] >> s) | (un[1] << (64 - s)),
            (un[1] >> s) | (un[2] << (64 - s)),
            (un[2] >> s) | (un[3] << (64 - s)),
            un[3] >> s,
        ])
    } else {
        U256([un[0], un[1], un[2], un[3]])
    }
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
        let c = a.sub(&b);
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
        assert_eq!(c, Fq::from_u64(243));
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
        let a = Fq(U256(FQ_MOD)).sub(&Fq::ONE);
        let b = Fq(U256(FQ_MOD)).sub(&Fq::ONE);
        let c = a.mul(&b);
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
        let fq = Fq::from_u64(42);
        let fr = Fr::from_fq(&fq);
        assert_eq!(fr, Fr::from_u64(42));
    }
}
