//! Dump all Poseidon round constants and MDS matrix for Solidity.
//!
//! Reimplements the Grain LFSR (from the Poseidon paper, Section F) to generate
//! the EXACT same constants as the PSE/Scroll Poseidon crate used by Remainder_CE.
//!
//! Parameters: width=3, rate=2, R_F=8, R_P=57, field=BN254 Fq (base field)
//!
//! Usage: cargo run --release --bin dump_poseidon_constants > /tmp/poseidon_constants.txt

use ff::{Field, FromUniformBytes, PrimeField};
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::transcript::TranscriptSponge;
use shared_types::{Fq, Fr};

// ── Grain LFSR implementation ─────────────────────────────────────────────────
// Reproduces the constant generation from the PSE poseidon crate exactly.
// See: https://eprint.iacr.org/2019/458.pdf Supplementary Material Section F

struct Grain {
    state: Vec<bool>,
}

impl Grain {
    fn new(field_size: u32, t: usize, r_f: usize, r_p: usize) -> Self {
        let mut state = Vec::with_capacity(80);

        // Field type: 1 (prime field) in 2 bits
        append_bits(&mut state, 2, 1u8);
        // S-box type: 0 (x^alpha) in 4 bits
        append_bits(&mut state, 4, 0u8);
        // Field size in 12 bits
        append_bits(&mut state, 12, field_size);
        // T (width) in 12 bits
        append_bits(&mut state, 12, t as u32);
        // R_F in 10 bits
        append_bits(&mut state, 10, r_f as u16);
        // R_P in 10 bits
        append_bits(&mut state, 10, r_p as u16);
        // Padding: 30 bits of ones
        append_bits(&mut state, 30, 0x3FFFFFFFu32);

        assert_eq!(state.len(), 80);

        let mut grain = Grain { state };

        // Clock 160 times to warm up
        for _ in 0..160 {
            grain.new_bit();
        }

        grain
    }

    fn new_bit(&mut self) -> bool {
        // Feedback taps: positions 0, 13, 23, 38, 51, 62
        let new = self.state[0]
            ^ self.state[13]
            ^ self.state[23]
            ^ self.state[38]
            ^ self.state[51]
            ^ self.state[62];
        self.state.remove(0);
        self.state.push(new);
        new
    }

    /// Get next usable bit (with validity filtering as per PSE impl).
    /// The PSE implementation's Iterator skips bits where new_bit() returns false,
    /// then returns the NEXT new_bit().
    fn next_bit(&mut self) -> bool {
        loop {
            if self.new_bit() {
                return self.new_bit();
            }
            self.new_bit();
        }
    }

    /// Generate next field element with rejection sampling (for round constants).
    fn next_field_element_fq(&mut self) -> Fq {
        let num_bits = Fq::NUM_BITS as usize; // 254
        loop {
            let mut repr = <Fq as PrimeField>::Repr::default();
            let view = repr.as_mut();

            for i in 0..num_bits {
                let bit = self.next_bit();
                // MSB-first ordering (matching PSE reference implementation)
                let idx = num_bits - 1 - i;
                if bit {
                    view[idx / 8] |= 1 << (idx % 8);
                }
            }

            if let Some(f) = Fq::from_repr(repr).into() {
                return f;
            }
            // Rejection: value >= Fq modulus, try again
        }
    }

    /// Generate next field element WITHOUT rejection sampling (for MDS matrix).
    /// Uses from_uniform_bytes with a 64-byte buffer.
    fn next_field_element_fq_no_rejection(&mut self) -> Fq {
        let num_bits = Fq::NUM_BITS as usize;
        let mut bytes = [0u8; 64];

        for i in 0..num_bits {
            let bit = self.next_bit();
            let idx = num_bits - 1 - i;
            if bit {
                bytes[idx / 8] |= 1 << (idx % 8);
            }
        }

        Fq::from_uniform_bytes(&bytes)
    }
}

fn append_bits<T: Into<u128>>(vec: &mut Vec<bool>, n: usize, from: T) {
    let val = from.into();
    for i in (0..n).rev() {
        vec.push((val >> i) & 1 != 0);
    }
}

// ── MDS Matrix computation ────────────────────────────────────────────────────

type Matrix3 = [[Fq; 3]; 3];

fn cauchy_mds(xs: &[Fq; 3], ys: &[Fq; 3]) -> Matrix3 {
    let mut m = [[Fq::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let sum = xs[i] + ys[j];
            m[i][j] = sum.invert().unwrap();
        }
    }
    m
}

fn mat_mul(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut result = [[Fq::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                result[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    result
}

fn mat_mul_vec(m: &Matrix3, v: &[Fq; 3]) -> [Fq; 3] {
    let mut result = [Fq::ZERO; 3];
    for i in 0..3 {
        for j in 0..3 {
            result[i] += m[i][j] * v[j];
        }
    }
    result
}

fn mat_transpose(m: &Matrix3) -> Matrix3 {
    let mut result = [[Fq::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            result[i][j] = m[j][i];
        }
    }
    result
}

fn mat_invert(m: &Matrix3) -> Matrix3 {
    // Gaussian elimination for 3x3
    let mut aug = [[Fq::ZERO; 6]; 3];
    for i in 0..3 {
        for j in 0..3 {
            aug[i][j] = m[i][j];
        }
        aug[i][i + 3] = Fq::ONE;
    }

    for col in 0..3 {
        // Find pivot
        let mut pivot = col;
        for row in col + 1..3 {
            if aug[row][col] != Fq::ZERO && aug[pivot][col] == Fq::ZERO {
                pivot = row;
            }
        }
        aug.swap(col, pivot);

        let inv = aug[col][col].invert().unwrap();
        for j in 0..6 {
            aug[col][j] *= inv;
        }

        for row in 0..3 {
            if row != col {
                let factor = aug[row][col];
                for j in 0..6 {
                    aug[row][j] -= factor * aug[col][j];
                }
            }
        }
    }

    let mut result = [[Fq::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            result[i][j] = aug[i][j + 3];
        }
    }
    result
}

fn fq_to_hex(f: &Fq) -> String {
    let bytes = f.to_repr();
    let mut be_bytes = bytes.as_ref().to_vec();
    be_bytes.reverse();
    format!("0x{}", hex::encode(&be_bytes))
}

fn fr_to_hex(f: &Fr) -> String {
    let bytes = f.to_repr();
    let mut be_bytes = bytes.as_ref().to_vec();
    be_bytes.reverse();
    format!("0x{}", hex::encode(&be_bytes))
}

fn main() {
    let field_size = Fq::NUM_BITS; // 254
    let t = 3usize;
    let r_f = 8usize;
    let r_p = 57usize;

    // ── Generate raw round constants via Grain LFSR ────────────────────────
    let mut grain = Grain::new(field_size, t, r_f, r_p);

    let num_rounds = r_f + r_p; // 65
    let mut raw_constants: Vec<[Fq; 3]> = Vec::new();
    for _ in 0..num_rounds {
        let rc = [
            grain.next_field_element_fq(),
            grain.next_field_element_fq(),
            grain.next_field_element_fq(),
        ];
        raw_constants.push(rc);
    }

    // ── Generate MDS matrix ────────────────────────────────────────────────
    let mut xs = [Fq::ZERO; 3];
    let mut ys = [Fq::ZERO; 3];
    for x in xs.iter_mut() {
        *x = grain.next_field_element_fq_no_rejection();
    }
    for y in ys.iter_mut() {
        *y = grain.next_field_element_fq_no_rejection();
    }
    let mds = cauchy_mds(&xs, &ys);

    // ── Compute optimized constants (matching PSE implementation) ──────────
    let inv_mds = mat_invert(&mds);
    let r_f_half = r_f / 2; // 4

    // Start constants: first r_f/2 rounds
    // constants_start[0] = raw_constants[0]
    // constants_start[1..r_f_half] = inv_mds * raw_constants[1..r_f_half]
    let mut constants_start: Vec<[Fq; 3]> = vec![[Fq::ZERO; 3]; r_f_half];
    constants_start[0] = raw_constants[0];
    for i in 1..r_f_half {
        constants_start[i] = mat_mul_vec(&inv_mds, &raw_constants[i]);
    }

    // Partial round constants (computed in reverse)
    let mut acc = raw_constants[r_f_half + r_p]; // constants[r_f/2 + r_p]
    let mut constants_partial = vec![Fq::ZERO; r_p];
    for i in (0..r_p).rev() {
        let raw_idx = r_f_half + i;
        let mut tmp = mat_mul_vec(&inv_mds, &acc);
        constants_partial[i] = tmp[0];
        tmp[0] = Fq::ZERO;
        for j in 0..3 {
            acc[j] = tmp[j] + raw_constants[raw_idx][j];
        }
    }
    // Push the accumulated value as an extra start constant
    constants_start.push(mat_mul_vec(&inv_mds, &acc));

    // End constants: last r_f/2 - 1 rounds
    let mut constants_end: Vec<[Fq; 3]> = Vec::new();
    for i in (r_f_half + r_p + 1)..num_rounds {
        constants_end.push(mat_mul_vec(&inv_mds, &raw_constants[i]));
    }

    // ── Compute sparse matrices ────────────────────────────────────────────
    let mds_t = mat_transpose(&mds);
    let mut acc_m = mds_t;
    let mut sparse_matrices: Vec<([Fq; 3], [Fq; 2])> = Vec::new();

    for _ in 0..r_p {
        // Factorize acc_m into m_prime and m_prime_prime (sparse)
        let (m_prime, sparse) = factorize_mds(&acc_m);
        acc_m = mat_mul(&mds_t, &m_prime);
        sparse_matrices.push(sparse);
    }
    sparse_matrices.reverse();
    let pre_sparse_mds = mat_transpose(&acc_m);

    // ═══════════════════════════════════════════════════════════════════════
    // Output Solidity constants
    // ═══════════════════════════════════════════════════════════════════════

    println!("// SPDX-License-Identifier: Apache-2.0");
    println!("// Auto-generated Poseidon constants for BN254 Fq (base field)");
    println!("// Parameters: width=3, rate=2, R_F=8, R_P=57 (PSE/Scroll Poseidon)");
    println!("// Field: BN254 Fq = 21888242871839275222246405745257275088696311157297823662689037894645226208583");
    println!("pragma solidity ^0.8.20;");
    println!();

    println!("// ═══ Full MDS Matrix (3x3, Cauchy) ═══");
    for i in 0..3 {
        for j in 0..3 {
            println!("uint256 constant MDS_{}_{} = {};", i, j, fq_to_hex(&mds[i][j]));
        }
    }
    println!();

    println!("// ═══ Pre-sparse MDS Matrix (3x3) ═══");
    for i in 0..3 {
        for j in 0..3 {
            println!("uint256 constant PRE_SPARSE_{}_{} = {};", i, j, fq_to_hex(&pre_sparse_mds[i][j]));
        }
    }
    println!();

    println!("// ═══ Start constants (first half full rounds: {} sets) ═══", constants_start.len());
    for (r, rc) in constants_start.iter().enumerate() {
        for (i, c) in rc.iter().enumerate() {
            println!("uint256 constant S_{}_{}  = {};", r, i, fq_to_hex(c));
        }
    }
    println!();

    println!("// ═══ Partial round constants ({} constants) ═══", constants_partial.len());
    for (r, c) in constants_partial.iter().enumerate() {
        println!("uint256 constant P_{}  = {};", r, fq_to_hex(c));
    }
    println!();

    println!("// ═══ End constants (second half full rounds: {} sets) ═══", constants_end.len());
    for (r, rc) in constants_end.iter().enumerate() {
        for (i, c) in rc.iter().enumerate() {
            println!("uint256 constant E_{}_{}  = {};", r, i, fq_to_hex(c));
        }
    }
    println!();

    println!("// ═══ Sparse matrices ({} matrices: row[3] + col_hat[2]) ═══", sparse_matrices.len());
    for (r, (row, col_hat)) in sparse_matrices.iter().enumerate() {
        for (i, v) in row.iter().enumerate() {
            println!("uint256 constant SM_{}_R{} = {};", r, i, fq_to_hex(v));
        }
        for (i, v) in col_hat.iter().enumerate() {
            println!("uint256 constant SM_{}_C{} = {};", r, i, fq_to_hex(v));
        }
    }
    println!();

    // ═══ Test vectors ═══
    println!("// ═══════════════════════════════════════════════════════════════");
    println!("// TEST VECTORS");
    println!("// ═══════════════════════════════════════════════════════════════");
    println!();

    // PoseidonSponge<Fr> test vectors (these we already have from dump_poseidon.rs)
    println!("// PoseidonSponge<Fr> (scalar field - used in plain GKR mode):");
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        sponge.absorb(Fr::from(1u64));
        let out = sponge.squeeze();
        println!("// absorb(1) -> squeeze = {}", fr_to_hex(&out));
    }
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        sponge.absorb(Fr::from(0u64));
        let out = sponge.squeeze();
        println!("// absorb(0) -> squeeze = {}", fr_to_hex(&out));
    }
    println!();

    // Raw round constants (first few, for cross-validation)
    println!("// First 3 raw (unoptimized) round constants for cross-validation:");
    for i in 0..3 {
        println!("// RC[{}] = [{}, {}, {}]", i,
            fq_to_hex(&raw_constants[i][0]),
            fq_to_hex(&raw_constants[i][1]),
            fq_to_hex(&raw_constants[i][2]));
    }
    println!();

    // Summary
    let total = constants_start.len() * 3 + constants_partial.len() + constants_end.len() * 3
        + 9 + 9 + sparse_matrices.len() * 5;
    println!("// Total constants: {} field elements ({} bytes)", total, total * 32);
    println!("// Start:   {} x 3 = {}", constants_start.len(), constants_start.len() * 3);
    println!("// Partial: {}", constants_partial.len());
    println!("// End:     {} x 3 = {}", constants_end.len(), constants_end.len() * 3);
    println!("// MDS:     9 + 9 (full + pre-sparse)");
    println!("// Sparse:  {} x 5 = {}", sparse_matrices.len(), sparse_matrices.len() * 5);
}

/// Factorize MDS matrix into M' (dense) and M'' (sparse).
/// Returns (M', (row, col_hat)) where M'' is in sparse form.
///
/// See Section B in https://eprint.iacr.org/2019/458.pdf
fn factorize_mds(m: &Matrix3) -> (Matrix3, ([Fq; 3], [Fq; 2])) {
    // Extract the (T-1) x (T-1) sub-matrix (rows 1..3, cols 1..3)
    let mut m_hat = [[Fq::ZERO; 2]; 2];
    for i in 0..2 {
        for j in 0..2 {
            m_hat[i][j] = m[i + 1][j + 1];
        }
    }

    // Invert 2x2 sub-matrix
    let det = m_hat[0][0] * m_hat[1][1] - m_hat[0][1] * m_hat[1][0];
    let det_inv = det.invert().unwrap();
    let m_hat_inv = [
        [m_hat[1][1] * det_inv, -m_hat[0][1] * det_inv],
        [-m_hat[1][0] * det_inv, m_hat[0][0] * det_inv],
    ];

    // w vector: first column, rows 1..3
    let w = [m[1][0], m[2][0]];

    // w_hat = m_hat_inv * w
    let w_hat = [
        m_hat_inv[0][0] * w[0] + m_hat_inv[0][1] * w[1],
        m_hat_inv[1][0] * w[0] + m_hat_inv[1][1] * w[1],
    ];

    // M' = [[1, 0, 0], [0, m_hat]]
    let mut m_prime = [[Fq::ZERO; 3]; 3];
    m_prime[0][0] = Fq::ONE;
    for i in 0..2 {
        for j in 0..2 {
            m_prime[i + 1][j + 1] = m_hat[i][j];
        }
    }

    // M'' (before transpose) = [[m[0][0], m[0][1], m[0][2]], [w_hat[0], 1, 0], [w_hat[1], 0, 1]]
    // The sparse representation stores the TRANSPOSED form:
    // row = first row of M''^T = [m[0][0], w_hat[0], w_hat[1]]
    // Wait, need to be more careful. PSE impl transposes then factorizes.
    // The sparse matrix stores: row (first row) and col_hat (first column minus diagonal)

    // M'' = [[m[0][0], m[0][1], m[0][2]], [w_hat[0], 1, 0], [w_hat[1], 0, 1]]
    // row = [m[0][0], m[0][1], m[0][2]]
    // col_hat = [w_hat[0], w_hat[1]]
    let row = [m[0][0], m[0][1], m[0][2]];
    let col_hat = [w_hat[0], w_hat[1]];

    (m_prime, (row, col_hat))
}
