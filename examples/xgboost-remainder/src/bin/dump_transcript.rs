//! Dump the GKR proof transcript structure for a small circuit.
//!
//! Usage: cargo run --release --bin dump_transcript

use frontend::layouter::builder::{CircuitBuilder, LayerVisibility};
use remainder::prover::helpers::verify_circuit_with_proof_config;
use shared_types::circuit_hash::CircuitHashType;
use shared_types::config::GKRCircuitProverConfig;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::transcript::{TranscriptReader, TranscriptWriter};
use shared_types::{perform_function_under_prover_config, Fr};

/// Our own prove function that captures the Transcript<Fr>.
fn prove_and_capture_transcript(
    provable: &remainder::provable_circuit::ProvableCircuit<Fr>,
) -> (
    shared_types::config::ProofConfig,
    shared_types::transcript::Transcript<Fr>,
) {
    let config = GKRCircuitProverConfig::runtime_optimized_default();

    let (proof_config, transcript) = perform_function_under_prover_config!(
        |p: &remainder::provable_circuit::ProvableCircuit<Fr>| {
            let mut writer =
                TranscriptWriter::<Fr, PoseidonSponge<Fr>>::new("GKR Prover Transcript");
            let proof_config = p
                .prove(CircuitHashType::Sha3_256, &mut writer)
                .expect("Proof failed");
            let transcript = writer.get_transcript();
            (proof_config, transcript)
        },
        &config,
        provable
    );

    (proof_config, transcript)
}

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    println!("=== GKR Proof Transcript Dump ===");
    println!();

    let num_vars = 2;
    let mut builder = CircuitBuilder::<Fr>::new();

    let private_layer = builder.add_input_layer("private inputs", LayerVisibility::Committed);
    let public_layer = builder.add_input_layer("public outputs", LayerVisibility::Public);

    let lhs = builder.add_input_shred("lhs", num_vars, &private_layer);
    let rhs = builder.add_input_shred("rhs", num_vars, &private_layer);
    let expected = builder.add_input_shred("expected", num_vars, &public_layer);

    let product = builder.add_sector(lhs * rhs);
    let diff = builder.add_sector(product - expected);

    builder.set_output(&diff);
    let base_circuit = builder.build().expect("Failed to build circuit");

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit;

    prover_circuit.set_input("lhs", vec![1u64, 2, 3, 4].into());
    prover_circuit.set_input("rhs", vec![5u64, 6, 7, 8].into());
    prover_circuit.set_input("expected", vec![5u64, 12, 21, 32].into());

    println!("Input: lhs=[1,2,3,4], rhs=[5,6,7,8], expected=[5,12,21,32]");
    println!();

    let provable = prover_circuit
        .gen_provable_circuit()
        .expect("Failed to generate provable circuit");

    // Generate proof and capture transcript
    println!("Generating GKR proof...");
    let (proof_config, transcript) = prove_and_capture_transcript(&provable);

    // Serialize transcript to JSON
    let transcript_json =
        serde_json::to_string_pretty(&transcript).expect("Failed to serialize transcript");

    // Write full JSON to file
    std::fs::write("/tmp/gkr_transcript.json", &transcript_json)
        .expect("Failed to write transcript JSON");
    println!("Full transcript JSON written to /tmp/gkr_transcript.json");
    println!("Transcript JSON size: {} bytes", transcript_json.len());
    println!();

    // Show summary
    println!("=== Transcript Summary ===");
    println!("{}", transcript);

    // Show first part of JSON
    println!("=== Transcript JSON (first 3000 chars) ===");
    let display_len = transcript_json.len().min(3000);
    println!("{}", &transcript_json[..display_len]);
    if transcript_json.len() > 3000 {
        println!("... ({} more chars)", transcript_json.len() - 3000);
    }
    println!();

    // Verify
    println!("Verifying proof...");
    let reader = TranscriptReader::<Fr, PoseidonSponge<Fr>>::new(transcript);
    let verifiable = verifier_circuit
        .gen_verifiable_circuit()
        .expect("Failed to generate verifiable circuit");
    verify_circuit_with_proof_config::<Fr, PoseidonSponge<Fr>>(&verifiable, &proof_config, reader);
    println!("Proof verified successfully!");
}
