//! Dump the HyraxProof JSON structure for a small test circuit.
//!
//! This helps us understand how serde_json serializes HyraxProof
//! so that the ABI encoder can correctly walk the JSON tree.
//!
//! Usage: cargo run --release --bin dump_proof_json > /tmp/hyrax_proof.json

use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use shared_types::config::{GKRCircuitProverConfig, GKRCircuitVerifierConfig};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::ECTranscript;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
    perform_function_under_prover_config, perform_function_under_verifier_config, Bn256Point, Fq,
    Fr,
};

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

fn main() {
    let num_vars = 1; // 2 elements — smallest circuit
    let base_circuit = build_circuit(num_vars);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    // Inputs: a=[3,5], b=[2,4], expected=[6,20]
    prover_circuit.set_input("a", vec![3u64, 5].into());
    prover_circuit.set_input("b", vec![2u64, 4].into());
    prover_circuit.set_input("expected", vec![6u64, 20].into());

    let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);

    let mut provable = prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("Failed to gen Hyrax-provable circuit");

    let committer = PedersenCommitter::new(512, "dump-proof-json Pedersen committer seed string for generating bases", None);
    let mut rng = thread_rng();
    let mut vander = VandermondeInverse::new();
    let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("dump-proof transcript");

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
        .expect("Failed to gen Hyrax-verifiable circuit");
    let verifier_committer = PedersenCommitter::new(512, "dump-proof-json Pedersen committer seed string for generating bases", None);
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("dump-proof transcript");
    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &verifier_config,
        &proof,
        &verifiable,
        &verifier_committer,
        &mut verifier_transcript,
        &proof_config
    );
    eprintln!("Proof verified!");

    // Dump proof as JSON
    let json = serde_json::to_value(&proof).expect("Failed to serialize proof to JSON");
    let pretty = serde_json::to_string_pretty(&json).expect("Failed to pretty-print");
    println!("{}", pretty);

    // Also print a summary of the structure
    eprintln!("\n=== Proof JSON Structure Summary ===");
    print_structure(&json, "", 0);
}

fn print_structure(val: &serde_json::Value, path: &str, depth: usize) {
    let indent = "  ".repeat(depth);
    match val {
        serde_json::Value::Object(map) => {
            for (key, v) in map {
                let p = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                match v {
                    serde_json::Value::Array(arr) => {
                        eprintln!("{}  {}: Array[{}]", indent, p, arr.len());
                        if !arr.is_empty() {
                            print_structure(&arr[0], &format!("{}[0]", p), depth + 1);
                        }
                    }
                    serde_json::Value::Object(_) => {
                        eprintln!("{}  {}: Object", indent, p);
                        print_structure(v, &p, depth + 1);
                    }
                    serde_json::Value::String(s) => {
                        let preview = if s.len() > 20 {
                            format!("{}...", &s[..20])
                        } else {
                            s.clone()
                        };
                        eprintln!("{}  {}: String(\"{}\")", indent, p, preview);
                    }
                    serde_json::Value::Number(n) => {
                        eprintln!("{}  {}: Number({})", indent, p, n);
                    }
                    serde_json::Value::Bool(b) => {
                        eprintln!("{}  {}: Bool({})", indent, p, b);
                    }
                    serde_json::Value::Null => {
                        eprintln!("{}  {}: Null", indent, p);
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            eprintln!("{}  Array[{}]", indent, arr.len());
            if !arr.is_empty() {
                print_structure(&arr[0], &format!("{}[0]", path), depth + 1);
            }
        }
        _ => {}
    }
}
