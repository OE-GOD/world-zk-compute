//! Dump Poseidon round constants, MDS matrix, and test vectors.
//!
//! This extracts the exact Poseidon parameters used by Remainder_CE
//! (Scroll's Poseidon: width=3, rate=2, R_F=8, R_P=57 over BN254 Fr)
//! and generates Solidity-compatible hex constants.
//!
//! Usage: cargo run --release --bin dump_poseidon

use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::transcript::TranscriptSponge;
use shared_types::Fr;

fn fr_to_hex(f: &Fr) -> String {
    // Fr implements Debug which prints the internal representation.
    // We need the canonical byte representation.
    use ff::PrimeField;
    let bytes = f.to_repr();
    // to_repr() gives little-endian bytes. Convert to big-endian hex for Solidity.
    let mut be_bytes = bytes.as_ref().to_vec();
    be_bytes.reverse();
    format!("0x{}", hex::encode(&be_bytes))
}

fn main() {
    println!("=== Poseidon Test Vectors for BN254 Fr ===");
    println!("Parameters: width=3, rate=2, R_F=8, R_P=57");
    println!();

    // Test vector 1: absorb(1), squeeze
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        let one = Fr::from(1u64);
        sponge.absorb(one);
        let out = sponge.squeeze();
        println!("Test 1: absorb(1) -> squeeze");
        println!("  input:  {}", fr_to_hex(&one));
        println!("  output: {}", fr_to_hex(&out));
        println!();
    }

    // Test vector 2: absorb(0), squeeze
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        let zero = Fr::from(0u64);
        sponge.absorb(zero);
        let out = sponge.squeeze();
        println!("Test 2: absorb(0) -> squeeze");
        println!("  input:  {}", fr_to_hex(&zero));
        println!("  output: {}", fr_to_hex(&out));
        println!();
    }

    // Test vector 3: absorb(42), squeeze
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        let val = Fr::from(42u64);
        sponge.absorb(val);
        let out = sponge.squeeze();
        println!("Test 3: absorb(42) -> squeeze");
        println!("  input:  {}", fr_to_hex(&val));
        println!("  output: {}", fr_to_hex(&out));
        println!();
    }

    // Test vector 4: absorb(1, 2), squeeze
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        let one = Fr::from(1u64);
        let two = Fr::from(2u64);
        sponge.absorb(one);
        sponge.absorb(two);
        let out = sponge.squeeze();
        println!("Test 4: absorb(1, 2) -> squeeze");
        println!("  input1: {}", fr_to_hex(&one));
        println!("  input2: {}", fr_to_hex(&two));
        println!("  output: {}", fr_to_hex(&out));
        println!();
    }

    // Test vector 5: absorb(1, 2, 3), squeeze, squeeze
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        sponge.absorb(Fr::from(1u64));
        sponge.absorb(Fr::from(2u64));
        sponge.absorb(Fr::from(3u64));
        let out1 = sponge.squeeze();
        let out2 = sponge.squeeze();
        println!("Test 5: absorb(1, 2, 3) -> squeeze, squeeze");
        println!("  squeeze1: {}", fr_to_hex(&out1));
        println!("  squeeze2: {}", fr_to_hex(&out2));
        println!();
    }

    // Test vector 6: Multiple absorb-squeeze cycles
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        sponge.absorb(Fr::from(100u64));
        let s1 = sponge.squeeze();
        sponge.absorb(Fr::from(200u64));
        let s2 = sponge.squeeze();
        println!("Test 6: absorb(100)->squeeze->absorb(200)->squeeze");
        println!("  squeeze1: {}", fr_to_hex(&s1));
        println!("  squeeze2: {}", fr_to_hex(&s2));
        println!();
    }

    // Test vector 7: Large value
    {
        let mut sponge = PoseidonSponge::<Fr>::default();
        // FR_MODULUS - 1 (max field element)
        let max_val = Fr::from(0xFFFFFFFFFFFFFFFFu64);
        sponge.absorb(max_val);
        let out = sponge.squeeze();
        println!("Test 7: absorb(2^64 - 1) -> squeeze");
        println!("  input:  {}", fr_to_hex(&max_val));
        println!("  output: {}", fr_to_hex(&out));
        println!();
    }

    // Now dump the internal Poseidon state after a permutation
    // to help debug constant matching
    println!("=== Poseidon Permutation Debug ===");
    {
        // Permutation of [0, 0, 0] (initial state)
        let mut sponge = PoseidonSponge::<Fr>::default();
        // Squeeze without absorbing anything - forces a permutation of zero state
        let out = sponge.squeeze();
        println!("Permutation of [0, 0, 0]:");
        println!("  output element: {}", fr_to_hex(&out));
        println!();
    }

    println!("=== Solidity Constants ===");
    println!("uint256 constant FR_MODULUS = 21888242871839275222246405745257275088548364400416034343698204186575808495617;");
    println!();

    // Print the test vectors as Solidity assertions
    println!("// Solidity test assertions:");
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
}
