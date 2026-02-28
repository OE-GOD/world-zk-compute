//! Generate a test proof fixture for Foundry Solidity tests.
//!
//! Outputs a JSON file with ABI-encoded proof data that can be loaded
//! by RemainderVerifier.t.sol for on-chain verification testing.
//!
//! Usage: cargo run --release --bin gen_test_proof > /tmp/test_proof.json

use anyhow::Result;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use sha2::{Digest, Sha256};
use serde_json::json;
use shared_types::config::{GKRCircuitProverConfig, GKRCircuitVerifierConfig};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::ECTranscript;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
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

fn main() -> Result<()> {
    let num_vars = 1;
    let base_circuit = build_circuit(num_vars);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    // Simple test: a=[3,5], b=[2,4], expected=[6,20]
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

    // Verify
    let verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("gen_hyrax_verifiable_circuit");
    let verifier_committer = PedersenCommitter::new(
        512,
        "gen-test-proof Pedersen committer seed string for generating bases",
        None,
    );
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("gen-test-proof transcript");
    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &verifier_config,
        &proof,
        &verifiable,
        &verifier_committer,
        &mut verifier_transcript,
        &proof_config
    );
    eprintln!("Proof verified in Rust!");

    // Circuit hash
    let circuit_hash: [u8; 32] = {
        let mut hasher = Sha256::new();
        hasher.update(b"REMAINDER_XGBOOST_TEST_V1");
        hasher.finalize().into()
    };

    // ABI-encode
    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash)?;
    eprintln!("ABI-encoded proof: {} bytes ({} uint256 slots)", abi_bytes.len(), (abi_bytes.len() - 4) / 32);

    // ABI-encode Pedersen generators
    let gens_bytes = abi_encode::encode_pedersen_gens(&committer)?;
    eprintln!("ABI-encoded generators: {} bytes ({} uint256 slots)", gens_bytes.len(), gens_bytes.len() / 32);

    // Also serialize the proof JSON for debugging
    let proof_json = serde_json::to_value(&proof)?;

    // Count proof elements for the Solidity decoder
    let circuit_proof = &proof_json["circuit_proof"];
    let num_layer_proofs = circuit_proof["layer_proofs"].as_array().map(|a| a.len()).unwrap_or(0);
    let num_output_proofs = circuit_proof["output_layer_proofs"].as_array().map(|a| a.len()).unwrap_or(0);
    let num_fs_claims = circuit_proof["fiat_shamir_claims"].as_array().map(|a| a.len()).unwrap_or(0);
    let num_pub_claims = proof_json["claims_on_public_values"].as_array().map(|a| a.len()).unwrap_or(0);
    let num_input_proofs = proof_json["hyrax_input_proofs"].as_array().map(|a| a.len()).unwrap_or(0);

    let output = json!({
        "proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "circuit_hash": format!("0x{}", hex::encode(&circuit_hash)),
        "proof_size_bytes": abi_bytes.len(),
        "gens_size_bytes": gens_bytes.len(),
        "structure": {
            "num_public_inputs": proof_json["public_inputs"].as_array().map(|a| a.len()).unwrap_or(0),
            "num_output_layer_proofs": num_output_proofs,
            "num_layer_proofs": num_layer_proofs,
            "num_fiat_shamir_claims": num_fs_claims,
            "num_public_value_claims": num_pub_claims,
            "num_hyrax_input_proofs": num_input_proofs,
        },
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}
