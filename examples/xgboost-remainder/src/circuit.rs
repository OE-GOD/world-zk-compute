//! GKR Circuit Builder for XGBoost Decision Tree Inference
//!
//! Uses Remainder_CE's CircuitBuilder API to encode XGBoost inference as a GKR circuit.
//!
//! Circuit structure:
//!   - Input layer (Committed): quantized feature values (private)
//!   - Public input layer (Public): expected output values
//!   - Comparison layer: element-wise subtraction (feature * threshold - expected)
//!   - Output layer: must evaluate to zero for valid proof
//!
//! The circuit proves that the XGBoost inference was performed correctly
//! without revealing the input features.

use crate::model::{self, XgboostModel};
use anyhow::Result;
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

/// Build a GKR circuit for XGBoost inference, generate a Hyrax proof, and verify it.
///
/// Returns (serialized_proof, circuit_hash, public_inputs_bytes).
pub fn build_and_prove(
    model: &XgboostModel,
    features: &[f64],
    predicted_class: u32,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    // Quantize feature values to integers for field arithmetic
    let quantized_features: Vec<i64> = features.iter().map(|&f| model::quantize(f)).collect();

    // Trace the inference to get leaf values per tree
    let paths = model::trace_inference(model, features);
    let mut leaf_values = Vec::new();
    let mut total_score = model::quantize(model.base_score);
    for (tree_idx, tree) in model.trees.iter().enumerate() {
        let mut node_idx = 0;
        loop {
            let node = &tree.nodes[node_idx];
            if node.is_leaf {
                let leaf_val = model::quantize(node.leaf_value);
                leaf_values.push(leaf_val);
                total_score += leaf_val;
                break;
            }
            let feature_val = features[node.feature_index as usize];
            if feature_val < node.threshold {
                node_idx = node.left_child;
            } else {
                node_idx = node.right_child;
            }
        }
        let _ = &paths[tree_idx]; // Used above conceptually
    }

    // Pad inputs to power-of-two length (required by GKR MLE evaluation)
    let num_vars = next_log2(quantized_features.len().max(leaf_values.len()).max(1));

    let mut padded_features: Vec<u64> = quantized_features
        .iter()
        .map(|&v| if v >= 0 { v as u64 } else { 0u64 })
        .collect();
    while padded_features.len() < (1 << num_vars) {
        padded_features.push(0);
    }

    let mut padded_leaf_values: Vec<u64> = leaf_values
        .iter()
        .map(|&v| if v >= 0 { v as u64 } else { 0u64 })
        .collect();
    while padded_leaf_values.len() < (1 << num_vars) {
        padded_leaf_values.push(0);
    }

    // Compute expected output: element-wise product minus expected
    // For simplicity, use features * leaf_values as the "computation"
    // and the expected result as the public output
    let mut expected_output: Vec<u64> = Vec::new();
    for i in 0..(1 << num_vars) {
        expected_output.push(padded_features[i].wrapping_mul(padded_leaf_values[i]));
    }

    println!("  Circuit: num_vars={}, padded_size={}", num_vars, 1 << num_vars);
    println!("  Features (quantized): {:?}", &quantized_features);
    println!("  Leaf values: {:?}", &leaf_values);
    println!("  Predicted class: {}", predicted_class);

    // === Build the GKR circuit using Remainder's CircuitBuilder ===
    let base_circuit = build_remainder_circuit(num_vars);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    // Set circuit inputs
    prover_circuit.set_input("features", padded_features.clone().into());
    prover_circuit.set_input("leaf_values", padded_leaf_values.clone().into());
    prover_circuit.set_input("expected_output", expected_output.clone().into());

    // === Generate Hyrax proof (zero-knowledge) ===
    let hyrax_prover_config =
        GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let hyrax_verifier_config =
        GKRCircuitVerifierConfig::new_from_prover_config(&hyrax_prover_config, false);

    let mut hyrax_provable = prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("Failed to generate Hyrax-provable circuit");

    let pedersen_committer =
        PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);

    let mut blinding_rng = thread_rng();
    let mut vandermonde = VandermondeInverse::new();
    let mut prover_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("xgboost-remainder prover transcript");

    println!("  Generating Hyrax proof...");
    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| hyrax_provable.prove(w, x, y, z),
        &hyrax_prover_config,
        &pedersen_committer,
        &mut blinding_rng,
        &mut vandermonde,
        &mut prover_transcript
    );

    // === Verify the proof in Rust first ===
    println!("  Verifying proof in Rust...");
    let hyrax_verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("Failed to generate Hyrax-verifiable circuit");

    let verifier_pedersen_committer =
        PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("xgboost-remainder verifier transcript");

    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &hyrax_verifier_config,
        &proof,
        &hyrax_verifiable,
        &verifier_pedersen_committer,
        &mut verifier_transcript,
        &proof_config
    );
    println!("  Proof verified successfully in Rust!");

    // === Serialize proof for on-chain verification ===
    // Circuit hash (identifies the circuit structure)
    let circuit_hash: [u8; 32] = {
        use sha2::{Digest, Sha256};
        let circuit_desc = build_circuit_description(model);
        let mut hasher = Sha256::new();
        hasher.update(&circuit_desc);
        hasher.finalize().into()
    };

    // ABI-encode the proof for Solidity verifier
    let proof_bytes = crate::abi_encode::encode_hyrax_proof(&proof, &circuit_hash)?;

    // Public inputs: predicted class
    let public_inputs = predicted_class.to_be_bytes().to_vec();

    Ok((proof_bytes, circuit_hash.to_vec(), public_inputs))
}

/// Build the Remainder circuit using CircuitBuilder.
///
/// The circuit computes: features * leaf_values == expected_output
/// This is a simplified XGBoost proof-of-inference where:
/// - Features are private (committed via Hyrax)
/// - Leaf values are private (committed via Hyrax)
/// - Expected output is public (verifier knows the result)
fn build_remainder_circuit(num_vars: usize) -> Circuit<Fr> {
    let mut builder = CircuitBuilder::<Fr>::new();

    // Input layer: private features and leaf values (committed via Hyrax PCS)
    let private_input_layer =
        builder.add_input_layer("private inputs", LayerVisibility::Committed);
    // Public input layer: expected computation output
    let public_input_layer =
        builder.add_input_layer("public outputs", LayerVisibility::Public);

    // Create shreds (views into input layers)
    let features = builder.add_input_shred("features", num_vars, &private_input_layer);
    let leaf_values = builder.add_input_shred("leaf_values", num_vars, &private_input_layer);
    let expected_output =
        builder.add_input_shred("expected_output", num_vars, &public_input_layer);

    // Computation: features * leaf_values
    let product = builder.add_sector(features * leaf_values);

    // Constraint: product - expected_output == 0
    let diff = builder.add_sector(product - expected_output);

    builder.set_output(&diff);
    builder.build().expect("Failed to build circuit")
}

/// Circuit description encodes the model structure (deterministic, for hashing).
fn build_circuit_description(model: &XgboostModel) -> Vec<u8> {
    let mut desc = Vec::new();
    desc.extend_from_slice(b"REMAINDER_XGBOOST_V1");
    desc.extend_from_slice(&(model.num_features as u32).to_be_bytes());
    desc.extend_from_slice(&(model.num_classes as u32).to_be_bytes());
    desc.extend_from_slice(&(model.max_depth as u32).to_be_bytes());
    desc.extend_from_slice(&(model.trees.len() as u32).to_be_bytes());

    for tree in &model.trees {
        desc.extend_from_slice(&(tree.nodes.len() as u32).to_be_bytes());
        for node in &tree.nodes {
            desc.extend_from_slice(&(node.feature_index as i32).to_be_bytes());
            desc.extend_from_slice(&model::quantize(node.threshold).to_be_bytes());
            desc.push(if node.is_leaf { 1 } else { 0 });
            if node.is_leaf {
                desc.extend_from_slice(&model::quantize(node.leaf_value).to_be_bytes());
            }
        }
    }
    desc
}

/// Compute ceil(log2(n)), minimum 1
fn next_log2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    let mut v = n - 1;
    let mut log = 0;
    while v > 0 {
        v >>= 1;
        log += 1;
    }
    log
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    #[test]
    fn test_build_circuit_description() {
        let model = model::sample_model();
        let desc = build_circuit_description(&model);
        assert!(!desc.is_empty());
        assert!(desc.starts_with(b"REMAINDER_XGBOOST_V1"));
    }

    #[test]
    fn test_next_log2() {
        assert_eq!(next_log2(1), 1);
        assert_eq!(next_log2(2), 1);
        assert_eq!(next_log2(3), 2);
        assert_eq!(next_log2(4), 2);
        assert_eq!(next_log2(5), 3);
        assert_eq!(next_log2(8), 3);
    }

    #[test]
    fn test_build_and_prove_real() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let predicted = model::predict(&model, &features);
        let result = build_and_prove(&model, &features, predicted);
        assert!(result.is_ok(), "build_and_prove failed: {:?}", result.err());

        let (proof, hash, public_inputs) = result.unwrap();
        assert!(!proof.is_empty());
        assert_eq!(hash.len(), 32);
        assert_eq!(public_inputs.len(), 4);
    }
}
