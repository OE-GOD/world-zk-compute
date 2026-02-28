//! Generate E2E test fixtures for Solidity Fiat-Shamir bridge and proof decoding.
//!
//! Generates:
//! 1. ABI-encoded proof bytes (for HyraxProofDecoder testing)
//! 2. Exact Fq field element values absorbed into the Poseidon sponge
//! 3. Resulting squeeze challenges (for Fiat-Shamir validation)
//!
//! All values come from a SINGLE proof run, ensuring consistency for E2E testing.
//!
//! Usage: cargo run --release --bin gen_transcript_trace

use anyhow::Result;
use ff::PrimeField;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use serde_json::json;
use shared_types::config::{GKRCircuitProverConfig, GKRCircuitVerifierConfig};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::ECTranscript;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::transcript::TranscriptSponge;
use shared_types::{
    curves::PrimeOrderCurve,
    perform_function_under_prover_config, perform_function_under_verifier_config, Bn256Point, Fq,
    Fr,
};

#[path = "../abi_encode.rs"]
#[allow(clippy::all)]
mod abi_encode;

fn build_circuit(num_vars: usize) -> Circuit<Fr> {
    let mut builder = CircuitBuilder::<Fr>::new();
    let private = builder.add_input_layer("private", LayerVisibility::Committed);
    let public = builder.add_input_layer("public", LayerVisibility::Public);
    let a = builder.add_input_shred("a", num_vars, &private);
    let b = builder.add_input_shred("b", num_vars, &private);
    let expected = builder.add_input_shred("expected", num_vars, &public);
    let product = builder.add_sector(a * b);
    let diff = builder.add_sector(product - expected);
    builder.set_output(&diff);
    builder.build().expect("Failed to build circuit")
}

/// Convert Fq to big-endian hex string (for Solidity uint256)
fn fq_to_hex(val: &Fq) -> String {
    let repr = val.to_repr();
    let bytes: &[u8] = repr.as_ref();
    let mut be = bytes.to_vec();
    be.reverse();
    format!("0x{}", hex::encode(&be))
}

/// SHA-256 hash chain (1000 iterations) on field elements.
/// Matches Remainder_CE's sha256_hash_chain_on_field_elems.
fn sha256_hash_chain(elems: &[Fq]) -> (Fq, Fq) {
    use sha2::{Digest, Sha256};

    // Convert field elements to bytes (LE representation)
    let elems_bytes: Vec<u8> = elems
        .iter()
        .flat_map(|elem| elem.to_repr().as_ref().to_vec())
        .collect();

    // Initial SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&elems_bytes);
    let mut hash_result = hasher.finalize();

    // Iterate SHA-256 1000 times
    for _ in 0..1000 {
        let mut hasher = Sha256::new();
        hasher.update(&hash_result);
        hash_result = hasher.finalize();
    }

    // Split 32-byte result into two 16-byte halves → field elements
    let mut first_half = [0u8; 32];
    let mut second_half = [0u8; 32];
    first_half[..16].copy_from_slice(&hash_result[..16]);
    second_half[..16].copy_from_slice(&hash_result[16..]);

    let fq1 = Fq::from_repr(first_half).unwrap();
    let fq2 = Fq::from_repr(second_half).unwrap();
    (fq1, fq2)
}

fn main() -> Result<()> {
    let num_vars = 1;
    let base_circuit = build_circuit(num_vars);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    prover_circuit.set_input("a", vec![3u64, 5].into());
    prover_circuit.set_input("b", vec![2u64, 4].into());
    prover_circuit.set_input("expected", vec![6u64, 20].into());

    let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);

    let mut provable = prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("gen_hyrax_provable_circuit");

    let committer = PedersenCommitter::new(
        512,
        "gen-test-proof Pedersen committer seed string for generating bases",
        None,
    );
    let mut rng = thread_rng();
    let mut vander = VandermondeInverse::new();
    let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("gen-test-proof transcript");

    eprintln!("Generating Hyrax proof...");
    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| provable.prove(w, x, y, z),
        &config,
        &committer,
        &mut rng,
        &mut vander,
        &mut transcript
    );

    // Verify with debug transcript to capture ALL operations
    let verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("gen_hyrax_verifiable_circuit");
    let verifier_committer = PedersenCommitter::new(
        512,
        "gen-test-proof Pedersen committer seed string for generating bases",
        None,
    );
    let mut debug_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new_with_debug("gen-test-proof transcript");

    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &verifier_config,
        &proof,
        &verifiable,
        &verifier_committer,
        &mut debug_transcript,
        &proof_config
    );
    eprintln!("Proof verified with debug transcript!");

    // === Manual transcript replay with raw PoseidonSponge ===
    // This captures ALL absorbed values for Solidity testing.

    // The circuit description hash values (observed from debug output above).
    // These are the SHA3-256 hash of the GKRCircuitDescription JSON,
    // split into two 16-byte halves and interpreted as Fq field elements (LE).
    //
    // From the debug output:
    //   Circuit desc hash part 1: 0x0000...5a9fc1776ad5b87f01d493983001d78f
    //   Circuit desc hash part 2: 0x0000...5daa4b26c457652c4f9715758acff1bc
    //
    // These are in the Fq Debug format (big-endian hex of the field element value).
    // For the sponge, we need the actual Fq values.

    // Parse from the known hex strings (observed from debug output)
    // Debug format is big-endian hex. The SHA3-256 hash is 32 bytes,
    // split into two 16-byte LE chunks. The debug output shows the value
    // in BE hex: 0x0000...{16 hex bytes for value}
    //
    // Circuit desc hash 1: 0x000000000000000000000000000000005a9fc1776ad5b87f01d493983001d78f
    // The value part is: 5a9fc1776ad5b87f01d493983001d78f (BE)
    // As LE bytes: 8f d7 01 30 98 93 d4 01 7f b8 d5 6a 77 c1 9f 5a (pad to 32 with zeros)
    let circuit_hash_1 = {
        let be_hex = "5a9fc1776ad5b87f01d493983001d78f";
        let be_bytes = hex::decode(be_hex)?;
        let mut le = [0u8; 32];
        for i in 0..16 {
            le[i] = be_bytes[15 - i];
        }
        Fq::from_repr(le).unwrap()
    };
    // Circuit desc hash 2: 0x000000000000000000000000000000005daa4b26c457652c4f9715758acff1bc
    let circuit_hash_2 = {
        let be_hex = "5daa4b26c457652c4f9715758acff1bc";
        let be_bytes = hex::decode(be_hex)?;
        let mut le = [0u8; 32];
        for i in 0..16 {
            le[i] = be_bytes[15 - i];
        }
        Fq::from_repr(le).unwrap()
    };

    // Verify our parsed values match the debug output
    eprintln!("\n=== Manually parsed circuit hash values ===");
    eprintln!("  hash_1: {}", fq_to_hex(&circuit_hash_1));
    eprintln!("  hash_2: {}", fq_to_hex(&circuit_hash_2));

    // Public input values (Fr → Fq via byte reinterpretation)
    // Fr(6) and Fr(20) become Fq(6) and Fq(20) since both fit in field
    let pub_val_1 = Fq::from(6u64);
    let pub_val_2 = Fq::from(20u64);

    // SHA-256 hash chain of [Fq(6), Fq(20)]
    let (pub_hash_1, pub_hash_2) = sha256_hash_chain(&[pub_val_1, pub_val_2]);

    eprintln!("\n=== Public input hash chain ===");
    eprintln!("  pub_val_1: {}", fq_to_hex(&pub_val_1));
    eprintln!("  pub_val_2: {}", fq_to_hex(&pub_val_2));
    eprintln!("  hash_chain_1: {}", fq_to_hex(&pub_hash_1));
    eprintln!("  hash_chain_2: {}", fq_to_hex(&pub_hash_2));

    // Hyrax input commitment EC points
    // Extract from proof data (sorted by LayerId, which matches proof order)
    let input_commits: Vec<(Fq, Fq)> = proof
        .hyrax_input_proofs
        .iter()
        .flat_map(|ip| {
            ip.input_commitment.iter().map(|pt| {
                use shared_types::curves::PrimeOrderCurve;
                pt.affine_coordinates().unwrap()
            })
        })
        .collect();

    eprintln!("\n=== Hyrax input commitment EC points ({}) ===", input_commits.len());
    let mut ec_fq_values: Vec<Fq> = Vec::new();
    for (i, (x, y)) in input_commits.iter().enumerate() {
        eprintln!("  point[{}].x: {}", i, fq_to_hex(x));
        eprintln!("  point[{}].y: {}", i, fq_to_hex(y));
        ec_fq_values.push(*x);
        ec_fq_values.push(*y);
    }

    // SHA-256 hash chain of EC point coordinates
    let (ec_hash_1, ec_hash_2) = sha256_hash_chain(&ec_fq_values);
    eprintln!("  ec_hash_chain_1: {}", fq_to_hex(&ec_hash_1));
    eprintln!("  ec_hash_chain_2: {}", fq_to_hex(&ec_hash_2));

    // === Full sponge replay ===
    let mut sponge = PoseidonSponge::<Fq>::default();

    // Step 1: Absorb circuit description hash (2 Fq elements)
    sponge.absorb(circuit_hash_1);
    sponge.absorb(circuit_hash_2);

    // Step 2: Absorb public input values + hash chain (4 Fq elements)
    sponge.absorb(pub_val_1);
    sponge.absorb(pub_val_2);
    sponge.absorb(pub_hash_1);
    sponge.absorb(pub_hash_2);

    // Intermediate test: squeeze after 6 elements (circuit hash + public inputs)
    let mut sponge_after_6 = sponge.clone();
    let challenge_after_6 = sponge_after_6.squeeze();
    eprintln!("\n=== Intermediate squeeze (after 6 absorbs, before EC points) ===");
    eprintln!("  squeeze = {}", fq_to_hex(&challenge_after_6));

    // Step 3: Absorb Hyrax input commitment EC points + hash chain
    for fq in &ec_fq_values {
        sponge.absorb(*fq);
    }
    sponge.absorb(ec_hash_1);
    sponge.absorb(ec_hash_2);

    // Step 4: Squeeze first challenge (should match "Challenge for claim on output")
    let first_challenge = sponge.squeeze();
    eprintln!("\n=== First challenge (after all initial absorbs) ===");
    eprintln!("  squeeze = {}", fq_to_hex(&first_challenge));
    eprintln!("  expected: 0x0ea43f7f1df793d4fbdf3bff6de4941404c0a3483f048b5065a4902629d64671");

    let matches = fq_to_hex(&first_challenge)
        == "0x0ea43f7f1df793d4fbdf3bff6de4941404c0a3483f048b5065a4902629d64671";
    eprintln!("  MATCH: {}", matches);

    // === ABI-encode the proof ===
    // Reconstruct raw 32-byte circuit hash from the two Fq halves
    let circuit_hash_raw: [u8; 32] = {
        let repr1 = circuit_hash_1.to_repr();
        let repr2 = circuit_hash_2.to_repr();
        let mut hash = [0u8; 32];
        hash[..16].copy_from_slice(&repr1.as_ref()[..16]);
        hash[16..].copy_from_slice(&repr2.as_ref()[..16]);
        hash
    };
    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash_raw)?;
    eprintln!(
        "\n=== ABI-encoded proof: {} bytes ({} uint256 slots) ===",
        abi_bytes.len(),
        (abi_bytes.len() - 4) / 32
    );

    // === ABI-encode the Pedersen generators ===
    let gens_bytes = abi_encode::encode_pedersen_gens(&verifier_committer)?;
    eprintln!(
        "ABI-encoded generators: {} bytes ({} uint256 slots)",
        gens_bytes.len(),
        gens_bytes.len() / 32
    );

    // Build the output JSON with all values
    let ec_points_hex: Vec<serde_json::Value> = input_commits
        .iter()
        .map(|(x, y)| json!({"x": fq_to_hex(x), "y": fq_to_hex(y)}))
        .collect();

    let output = json!({
        "proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
        "proof_size_bytes": abi_bytes.len(),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "gens_size_bytes": gens_bytes.len(),
        "circuit_hash_raw": format!("0x{}", hex::encode(&circuit_hash_raw)),
        "transcript_trace": {
            "circuit_hash_fq_1": fq_to_hex(&circuit_hash_1),
            "circuit_hash_fq_2": fq_to_hex(&circuit_hash_2),
            "public_input_fq_1": fq_to_hex(&pub_val_1),
            "public_input_fq_2": fq_to_hex(&pub_val_2),
            "public_input_hash_chain_1": fq_to_hex(&pub_hash_1),
            "public_input_hash_chain_2": fq_to_hex(&pub_hash_2),
            "input_commitment_points": ec_points_hex,
            "input_commitment_hash_chain_1": fq_to_hex(&ec_hash_1),
            "input_commitment_hash_chain_2": fq_to_hex(&ec_hash_2),
        },
        "challenges": {
            "after_circuit_hash_and_public_inputs": fq_to_hex(&challenge_after_6),
            "first_challenge_for_output_claim": fq_to_hex(&first_challenge),
            "first_challenge_matches_debug": matches,
        },
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}
