//! Generate a complete PoseidonSponge.sol Solidity file with hardcoded constants.
//!
//! Uses the unoptimized Poseidon permutation for clarity and correctness:
//!   65 rounds of: AddConstants → S-box → MulMDS
//!
//! Parameters: width=3, rate=2, R_F=8 (4+4), R_P=57, x^5 S-box, BN254 Fq
//!
//! Usage: cargo run --release --bin gen_poseidon_sol > /tmp/PoseidonSponge.sol

use ff::{Field, FromUniformBytes, PrimeField};
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::transcript::TranscriptSponge;
use shared_types::Fq;

// ── Grain LFSR ────────────────────────────────────────────────────────────────

struct Grain {
    state: Vec<bool>,
}

impl Grain {
    fn new(field_size: u32, t: usize, r_f: usize, r_p: usize) -> Self {
        let mut state = Vec::with_capacity(80);
        append_bits(&mut state, 2, 1u8);
        append_bits(&mut state, 4, 0u8);
        append_bits(&mut state, 12, field_size);
        append_bits(&mut state, 12, t as u32);
        append_bits(&mut state, 10, r_f as u16);
        append_bits(&mut state, 10, r_p as u16);
        append_bits(&mut state, 30, 0x3FFFFFFFu32);
        assert_eq!(state.len(), 80);
        let mut grain = Grain { state };
        for _ in 0..160 {
            grain.new_bit();
        }
        grain
    }

    fn new_bit(&mut self) -> bool {
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

    fn next_bit(&mut self) -> bool {
        loop {
            if self.new_bit() {
                return self.new_bit();
            }
            self.new_bit();
        }
    }

    fn next_field_element(&mut self) -> Fq {
        let num_bits = Fq::NUM_BITS as usize;
        loop {
            let mut repr = <Fq as PrimeField>::Repr::default();
            let view = repr.as_mut();
            for i in 0..num_bits {
                let bit = self.next_bit();
                let idx = num_bits - 1 - i;
                if bit {
                    view[idx / 8] |= 1 << (idx % 8);
                }
            }
            if let Some(f) = Fq::from_repr(repr).into() {
                return f;
            }
        }
    }

    fn next_field_element_no_rejection(&mut self) -> Fq {
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

fn fq_to_hex(f: &Fq) -> String {
    let bytes = f.to_repr();
    let mut be = bytes.as_ref().to_vec();
    be.reverse();
    format!("0x{}", hex::encode(&be))
}

fn main() {
    let field_size = Fq::NUM_BITS;
    let t = 3usize;
    let r_f = 8usize;
    let r_p = 57usize;
    let num_rounds = r_f + r_p;

    // Generate raw round constants
    let mut grain = Grain::new(field_size, t, r_f, r_p);
    let mut rc: Vec<[Fq; 3]> = Vec::new();
    for _ in 0..num_rounds {
        rc.push([
            grain.next_field_element(),
            grain.next_field_element(),
            grain.next_field_element(),
        ]);
    }

    // Generate MDS matrix
    let mut xs = [Fq::ZERO; 3];
    let mut ys = [Fq::ZERO; 3];
    for x in xs.iter_mut() {
        *x = grain.next_field_element_no_rejection();
    }
    for y in ys.iter_mut() {
        *y = grain.next_field_element_no_rejection();
    }

    let mut mds = [[Fq::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            mds[i][j] = (xs[i] + ys[j]).invert().unwrap();
        }
    }

    // Generate test vectors using PoseidonSponge<Fq>
    let tv1 = {
        let mut s = PoseidonSponge::<Fq>::default();
        s.absorb(Fq::from(1u64));
        s.squeeze()
    };
    let tv0 = {
        let mut s = PoseidonSponge::<Fq>::default();
        s.absorb(Fq::from(0u64));
        s.squeeze()
    };
    let tv42 = {
        let mut s = PoseidonSponge::<Fq>::default();
        s.absorb(Fq::from(42u64));
        s.squeeze()
    };
    let tv12 = {
        let mut s = PoseidonSponge::<Fq>::default();
        s.absorb(Fq::from(1u64));
        s.absorb(Fq::from(2u64));
        s.squeeze()
    };

    // Now emit the Solidity file
    println!(
        r#"// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title PoseidonSponge
/// @notice Poseidon hash function sponge for BN254 Fq (base field).
/// @dev Parameters: width=3, rate=2, R_F=8, R_P=57, x^5 S-box
///      Uses the PSE/Scroll Poseidon implementation's constants.
///      Constants generated via Grain LFSR (Poseidon paper Section F).
///      Initial state: [2^64, 0, 0] (capacity domain separator).
library PoseidonSponge {{

    /// BN254 base field modulus
    uint256 internal constant FQ_MOD =
        21888242871839275222246405745257275088696311157297823662689037894645226208583;

    /// Number of full rounds (split 4+4)
    uint256 internal constant R_F = 8;
    /// Number of partial rounds
    uint256 internal constant R_P = 57;

    struct Sponge {{
        uint256[3] state;
        uint256 absorbing; // number of elements absorbed since last permutation
    }}

    /// @notice Initialize a new Poseidon sponge
    function init() internal pure returns (Sponge memory s) {{
        // Capacity value = 2^64 (domain separator)
        s.state[0] = 1 << 64;
        s.state[1] = 0;
        s.state[2] = 0;
        s.absorbing = 0;
    }}

    /// @notice Absorb a single field element
    function absorb(Sponge memory s, uint256 value) internal pure {{
        // Add to rate position (state[1] or state[2])
        s.state[s.absorbing + 1] = addmod(s.state[s.absorbing + 1], value, FQ_MOD);
        s.absorbing++;

        if (s.absorbing == 2) {{
            // Rate is full, permute
            permute(s.state);
            s.absorbing = 0;
        }}
    }}

    /// @notice Absorb multiple field elements
    function absorbMany(Sponge memory s, uint256[] memory values) internal pure {{
        for (uint256 i = 0; i < values.length; i++) {{
            absorb(s, values[i]);
        }}
    }}

    /// @notice Squeeze a single challenge from the sponge
    function squeeze(Sponge memory s) internal pure returns (uint256) {{
        // Pad: add 1 to next rate position
        s.state[s.absorbing + 1] = addmod(s.state[s.absorbing + 1], 1, FQ_MOD);
        // Permute
        permute(s.state);
        s.absorbing = 0;
        // Return state[1] (second element = result)
        return s.state[1];
    }}

    /// @notice Poseidon permutation (unoptimized: AddRC → S-box → MDS)
    function permute(uint256[3] memory state) internal pure {{
        uint256 t0; uint256 t1; uint256 t2;
        uint256 new0; uint256 new1; uint256 new2;
"#
    );

    // Emit all 65 rounds inline
    let r_f_half = r_f / 2;
    for round in 0..num_rounds {
        let is_full = round < r_f_half || round >= r_f_half + r_p;
        let rc = &rc[round];

        println!();
        if round == 0 {
            println!("        // === First half full rounds (rounds 0-3) ===");
        } else if round == r_f_half {
            println!("        // === Partial rounds (rounds 4-60) ===");
        } else if round == (r_f_half + r_p) {
            println!("        // === Second half full rounds (rounds 61-64) ===");
        }
        println!("        // Round {}", round);

        // AddConstants
        println!(
            "        state[0] = addmod(state[0], {}, FQ_MOD);",
            fq_to_hex(&rc[0])
        );
        println!(
            "        state[1] = addmod(state[1], {}, FQ_MOD);",
            fq_to_hex(&rc[1])
        );
        println!(
            "        state[2] = addmod(state[2], {}, FQ_MOD);",
            fq_to_hex(&rc[2])
        );

        // S-box
        if is_full {
            // Full S-box: x^5 on all elements
            for i in 0..3 {
                println!("        t{i} = mulmod(state[{i}], state[{i}], FQ_MOD);");
                println!("        t{i} = mulmod(t{i}, t{i}, FQ_MOD);");
                println!("        state[{i}] = mulmod(state[{i}], t{i}, FQ_MOD);");
            }
        } else {
            // Partial S-box: x^5 on first element only
            println!("        t0 = mulmod(state[0], state[0], FQ_MOD);");
            println!("        t0 = mulmod(t0, t0, FQ_MOD);");
            println!("        state[0] = mulmod(state[0], t0, FQ_MOD);");
        }

        // MDS multiply
        for i in 0..3 {
            let var = ["new0", "new1", "new2"][i];
            println!(
                "        {var} = mulmod(state[0], {}, FQ_MOD);",
                fq_to_hex(&mds[i][0])
            );
            println!(
                "        {var} = addmod({var}, mulmod(state[1], {}, FQ_MOD), FQ_MOD);",
                fq_to_hex(&mds[i][1])
            );
            println!(
                "        {var} = addmod({var}, mulmod(state[2], {}, FQ_MOD), FQ_MOD);",
                fq_to_hex(&mds[i][2])
            );
        }
        println!("        state[0] = new0; state[1] = new1; state[2] = new2;");
    }

    // Close permute function and library
    println!(
        r#"    }}
"#
    );

    // Emit test vectors as view functions
    println!(
        r#"    /// @notice Validate Poseidon implementation against known test vectors.
    /// @return true if all test vectors pass
    function selfTest() internal pure returns (bool) {{
        Sponge memory s;

        // Test 1: absorb(1) -> squeeze
        s = init();
        absorb(s, 1);
        if (squeeze(s) != {tv1}) return false;

        // Test 2: absorb(0) -> squeeze
        s = init();
        absorb(s, 0);
        if (squeeze(s) != {tv0}) return false;

        // Test 3: absorb(42) -> squeeze
        s = init();
        absorb(s, 42);
        if (squeeze(s) != {tv42}) return false;

        // Test 4: absorb(1, 2) -> squeeze
        s = init();
        absorb(s, 1);
        absorb(s, 2);
        if (squeeze(s) != {tv12}) return false;

        return true;
    }}"#,
        tv1 = fq_to_hex(&tv1),
        tv0 = fq_to_hex(&tv0),
        tv42 = fq_to_hex(&tv42),
        tv12 = fq_to_hex(&tv12),
    );

    println!("}}");
}
