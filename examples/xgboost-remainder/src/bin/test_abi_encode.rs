//! Test the ABI encoder against a real HyraxProof.
//!
//! Generates a small proof, encodes it to ABI format, and prints
//! the result along with decoded structure info.
//!
//! Usage: cargo run --release --bin test_abi_encode

use anyhow::Result;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use sha2::{Digest, Sha256};
use shared_types::config::{GKRCircuitProverConfig, GKRCircuitVerifierConfig};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::ECTranscript;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
    perform_function_under_prover_config, perform_function_under_verifier_config, Bn256Point, Fq,
    Fr,
};

// Import abi_encode from the main crate
// Since this is a [[bin]], we need to include it via path attribute
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

fn main() -> Result<()> {
    let num_vars = 1; // 2 elements
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
        "test-abi-encode Pedersen committer seed string for generating bases",
        None,
    );
    let mut rng = thread_rng();
    let mut vander = VandermondeInverse::new();
    let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("test-abi-encode transcript");

    println!("Generating Hyrax proof...");
    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| provable.prove(w, x, y, z),
        &config,
        &committer,
        &mut rng,
        &mut vander,
        &mut transcript
    );

    // Verify in Rust
    let verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("gen_hyrax_verifiable_circuit");
    let verifier_committer = PedersenCommitter::new(
        512,
        "test-abi-encode Pedersen committer seed string for generating bases",
        None,
    );
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("test-abi-encode transcript");
    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &verifier_config,
        &proof,
        &verifiable,
        &verifier_committer,
        &mut verifier_transcript,
        &proof_config
    );
    println!("Proof verified in Rust!");

    // Compute circuit hash
    let circuit_hash: [u8; 32] = {
        let mut hasher = Sha256::new();
        hasher.update(b"TEST_CIRCUIT_V1");
        hasher.finalize().into()
    };

    // ABI-encode
    println!("\nABI-encoding proof...");
    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash)?;

    println!("ABI-encoded proof size: {} bytes", abi_bytes.len());
    println!("  Selector: {:?}", std::str::from_utf8(&abi_bytes[..4]));
    println!("  Circuit hash: 0x{}", hex::encode(&abi_bytes[4..36]));

    // Count the approximate number of uint256 slots
    let data_slots = (abi_bytes.len() - 4) / 32; // subtract selector
    println!("  Data slots (uint256): {}", data_slots);
    println!("  Total proof hex: 0x{}", hex::encode(&abi_bytes));

    // Also dump the bincode size for comparison
    let bincode_bytes = bincode::serialize(&proof)?;
    println!("\nComparison:");
    println!("  Bincode size:    {} bytes", bincode_bytes.len());
    println!("  ABI-encoded size: {} bytes", abi_bytes.len());

    Ok(())
}
