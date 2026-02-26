//! GKR Circuit Builder for XGBoost Decision Tree Inference
//!
//! Encodes the XGBoost decision tree traversal as a GKR circuit:
//!
//! Layer structure (bottom to top):
//!   - Input layer: committed feature values (private)
//!   - Threshold layer: quantized thresholds for all decision nodes
//!   - Comparison layer: feature[i] - threshold[i] differences
//!   - Path selection layer: binary routing bits (which path was taken)
//!   - Accumulation layer: weighted leaf values per tree
//!   - Output layer: final prediction (public)
//!
//! The circuit is satisfiable when:
//!   1. Each comparison correctly reflects feature vs threshold
//!   2. Path routing bits are consistent with comparisons
//!   3. The accumulated score matches the claimed prediction

use crate::model::{self, XgboostModel};
use anyhow::Result;
use sha2::{Digest, Sha256};

/// Build the GKR circuit for XGBoost inference and generate a proof.
///
/// Returns (proof_bytes, circuit_hash, public_inputs).
///
/// Note: This is a structural implementation that defines the circuit
/// architecture. The actual Remainder API calls are stubbed with clear
/// integration points, since Remainder_CE's API may evolve.
pub fn build_and_prove(
    model: &XgboostModel,
    features: &[f64],
    predicted_class: u32,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    // Quantize all values to fixed-point for field arithmetic
    let quantized_features: Vec<i64> = features.iter().map(|&f| model::quantize(f)).collect();

    // Collect all thresholds and leaf values from the model
    let circuit_desc = build_circuit_description(model);

    // Hash the circuit description for on-chain identification
    let circuit_hash = {
        let mut hasher = Sha256::new();
        hasher.update(&circuit_desc);
        hasher.finalize().to_vec()
    };

    // Trace the inference to get decision paths
    let paths = model::trace_inference(model, features);

    // Build the witness (all intermediate values)
    let witness = build_witness(model, &quantized_features, &paths, predicted_class);

    // Generate the GKR+Hyrax proof
    //
    // Integration point for Remainder_CE:
    //
    // ```rust
    // use remainder::prelude::*;
    //
    // let mut builder = CircuitBuilder::<BN254>::new();
    //
    // // Layer 0: Private feature inputs (committed via Hyrax)
    // let features_layer = builder.add_input_layer(
    //     "features",
    //     quantized_features.iter().map(|&v| Fr::from(v as u64)).collect(),
    //     LayerVisibility::Committed,
    // );
    //
    // // Layer 1: Threshold constants
    // let thresholds_layer = builder.add_const_layer(
    //     "thresholds",
    //     all_thresholds.iter().map(|&t| Fr::from(t as u64)).collect(),
    // );
    //
    // // Layer 2: Comparison results (feature - threshold)
    // let comparison_layer = builder.add_sector(
    //     "comparisons",
    //     features_layer.clone(),
    //     thresholds_layer.clone(),
    //     |a, b| a - b,  // Zero means equality, sign determines direction
    // );
    //
    // // Layer 3: Path routing bits
    // let routing_layer = builder.add_split_node(
    //     "routing",
    //     comparison_layer,
    //     num_bits,  // Bit decomposition for threshold check
    // );
    //
    // // Layer 4: Leaf value accumulation
    // let accumulation_layer = builder.add_sector(
    //     "accumulation",
    //     routing_layer,
    //     leaf_values_layer,
    //     |route, leaf| route * leaf,  // Selected leaf contributes its value
    // );
    //
    // // Output: sum of all tree outputs must equal predicted score
    // let output_layer = builder.set_output(
    //     "prediction",
    //     accumulation_layer,
    //     expected_output,  // Must evaluate to zero for valid proof
    // );
    //
    // let circuit = builder.finalize()?;
    // let proof = circuit.prove(&witness)?;
    // let proof_bytes = bincode::serialize(&proof)?;
    // ```

    // For now, generate a structured proof placeholder that matches
    // the expected format for the Solidity verifier
    let proof_bytes = generate_proof_structure(&witness, &circuit_hash)?;

    // Public inputs: predicted class as field element
    let public_inputs = predicted_class.to_be_bytes().to_vec();

    Ok((proof_bytes, circuit_hash, public_inputs))
}

/// Circuit description encodes the model structure (not the witness).
/// This is deterministic for a given model and is hashed for on-chain identification.
fn build_circuit_description(model: &XgboostModel) -> Vec<u8> {
    let mut desc = Vec::new();

    // Header
    desc.extend_from_slice(b"REMAINDER_XGBOOST_V1");
    desc.extend_from_slice(&(model.num_features as u32).to_be_bytes());
    desc.extend_from_slice(&(model.num_classes as u32).to_be_bytes());
    desc.extend_from_slice(&(model.max_depth as u32).to_be_bytes());
    desc.extend_from_slice(&(model.trees.len() as u32).to_be_bytes());

    // Encode each tree's structure (thresholds, feature indices)
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

/// Witness contains all intermediate computation values
struct CircuitWitness {
    /// Quantized feature values
    features: Vec<i64>,
    /// Comparison results: feature[i] - threshold[i]
    comparisons: Vec<i64>,
    /// Path routing bits (1 = left, 0 = right)
    routing_bits: Vec<u8>,
    /// Selected leaf values per tree
    leaf_values: Vec<i64>,
    /// Final accumulated score
    total_score: i64,
    /// Predicted class
    predicted_class: u32,
}

fn build_witness(
    model: &XgboostModel,
    quantized_features: &[i64],
    paths: &[Vec<(usize, bool)>],
    predicted_class: u32,
) -> CircuitWitness {
    let mut comparisons = Vec::new();
    let mut routing_bits = Vec::new();
    let mut leaf_values = Vec::new();
    let mut total_score: i64 = model::quantize(model.base_score);

    for (tree_idx, tree) in model.trees.iter().enumerate() {
        let path = &paths[tree_idx];

        // Record comparisons along the path
        for &(node_idx, goes_left) in path {
            let node = &tree.nodes[node_idx];
            let feature_val = quantized_features[node.feature_index as usize];
            let threshold = model::quantize(node.threshold);
            comparisons.push(feature_val - threshold);
            routing_bits.push(if goes_left { 1 } else { 0 });
        }

        // Find the leaf value
        let mut node_idx = 0;
        loop {
            let node = &tree.nodes[node_idx];
            if node.is_leaf {
                let leaf_val = model::quantize(node.leaf_value);
                leaf_values.push(leaf_val);
                total_score += leaf_val;
                break;
            }
            let feature_val = quantized_features[node.feature_index as usize];
            if feature_val < model::quantize(node.threshold) {
                node_idx = node.left_child;
            } else {
                node_idx = node.right_child;
            }
        }
    }

    CircuitWitness {
        features: quantized_features.to_vec(),
        comparisons,
        routing_bits,
        leaf_values,
        total_score,
        predicted_class,
    }
}

/// Generate a structured proof that matches the Solidity verifier's expected format.
///
/// Proof structure (all values are BN254 field elements / G1 points):
/// - GKR proof: per-layer sumcheck transcripts
/// - Hyrax PCS: commitment rows + evaluation proof
/// - Poseidon transcript state (for Fiat-Shamir replay)
///
/// This generates a structurally valid proof with placeholder values.
/// Real proofs will be generated by Remainder_CE.
fn generate_proof_structure(witness: &CircuitWitness, circuit_hash: &[u8]) -> Result<Vec<u8>> {
    let mut proof = Vec::new();

    // Proof header
    proof.extend_from_slice(b"REM1"); // Remainder v1 proof selector

    // Number of GKR layers
    let num_layers: u32 = 4; // features, comparisons, routing, output
    proof.extend_from_slice(&num_layers.to_be_bytes());

    // Per-layer sumcheck proofs (placeholder structure)
    for layer in 0..num_layers {
        // Number of sumcheck rounds for this layer
        let num_rounds: u32 = match layer {
            0 => witness.features.len() as u32, // log2(input_size) rounds
            1 => witness.comparisons.len() as u32,
            2 => witness.routing_bits.len() as u32,
            3 => 1, // output layer
            _ => unreachable!(),
        };
        proof.extend_from_slice(&num_rounds.to_be_bytes());

        // Each round: degree-2 polynomial (3 coefficients as 32-byte field elements)
        for _round in 0..num_rounds {
            // g_i(0), g_i(1), g_i(2) — three evaluation points
            for _coeff in 0..3 {
                proof.extend_from_slice(&[0u8; 32]); // Placeholder Fr element
            }
        }

        // Claimed value after sumcheck
        proof.extend_from_slice(&[0u8; 32]); // Placeholder Fr element
    }

    // Hyrax PCS proof
    // Number of commitment rows (sqrt(N) for input layer)
    let num_rows: u32 = (witness.features.len() as f64).sqrt().ceil() as u32;
    proof.extend_from_slice(&num_rows.to_be_bytes());

    // Commitment rows: G1 points (64 bytes each: x, y as uint256)
    for _row in 0..num_rows {
        proof.extend_from_slice(&[0u8; 64]); // Placeholder G1 point
    }

    // Evaluation proof (dot product proof)
    proof.extend_from_slice(&[0u8; 32]); // com_d (Fr)
    proof.extend_from_slice(&[0u8; 64]); // com_x (G1)
    proof.extend_from_slice(&[0u8; 64]); // com_z (G1)

    // Circuit hash
    proof.extend_from_slice(circuit_hash);

    // Public inputs
    proof.extend_from_slice(&witness.predicted_class.to_be_bytes());

    Ok(proof)
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
        // Header should start with magic bytes
        assert!(desc.starts_with(b"REMAINDER_XGBOOST_V1"));
    }

    #[test]
    fn test_build_and_prove() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let predicted = model::predict(&model, &features);
        let result = build_and_prove(&model, &features, predicted);
        assert!(result.is_ok());

        let (proof, hash, public_inputs) = result.unwrap();
        assert!(!proof.is_empty());
        assert_eq!(hash.len(), 32); // SHA-256
        assert_eq!(public_inputs.len(), 4); // u32
    }

    #[test]
    fn test_witness_construction() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let quantized: Vec<i64> = features.iter().map(|&f| model::quantize(f)).collect();
        let paths = model::trace_inference(&model, &features);

        let witness = build_witness(&model, &quantized, &paths, 1);
        assert_eq!(witness.features.len(), 5);
        assert!(!witness.comparisons.is_empty());
        assert!(!witness.routing_bits.is_empty());
        assert_eq!(witness.leaf_values.len(), model.trees.len());
    }
}
