//! GKR Circuit Builder for XGBoost Decision Tree Inference (Phase 1a)
//!
//! Uses Remainder_CE's CircuitBuilder API to encode XGBoost inference as a GKR circuit.
//!
//! Circuit structure (tree inference verification):
//!   - Input layer (Committed): path bits — binary indicators for tree traversal (private)
//!   - Public input layer: all leaf values from model + expected aggregate sum
//!   - Binary check: verifies each path bit is 0 or 1
//!   - Leaf fold: MLE evaluation selects the correct leaf per tree using path bits
//!   - Aggregation: sums selected leaves across all trees
//!   - Output layer: binary check values + (sum - expected) must all be zero
//!
//! The circuit proves that valid binary paths select specific leaves from the public
//! model, and those leaves aggregate to the claimed prediction.

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
    // === Collect model data for circuit ===
    let (all_leaf_values, max_depth) = model::collect_all_leaf_values(model);
    let (path_bits_2d, _) = model::compute_path_bits(model, features);

    let num_trees = model.trees.len();
    let num_trees_padded = num_trees.next_power_of_two();
    let leaves_per_tree = 1usize << max_depth;

    // Flatten path bits: [tree0_b0, tree0_b1, ..., tree1_b0, ...]
    let mut flat_path_bits: Vec<bool> = Vec::new();
    for bits in &path_bits_2d {
        flat_path_bits.extend(bits);
    }
    // Pad for extra trees (padding trees get all-false = left)
    for _ in num_trees..num_trees_padded {
        flat_path_bits.extend(vec![false; max_depth]);
    }
    // Pad to 2^pb_nv
    let pb_nv = next_log2(num_trees_padded * max_depth);
    while flat_path_bits.len() < (1 << pb_nv) {
        flat_path_bits.push(false);
    }

    // Leaf values: pad for extra trees (padding trees get all-zero leaves)
    let mut leaf_values_padded = all_leaf_values;
    for _ in num_trees..num_trees_padded {
        leaf_values_padded.extend(vec![0i64; leaves_per_tree]);
    }

    // Expected sum = sum of selected leaves only (base_score handled externally)
    let expected_sum =
        model::compute_leaf_sum(model, features) - model::quantize(model.base_score);

    println!(
        "  Circuit: {} trees (padded to {}), depth {}",
        num_trees, num_trees_padded, max_depth
    );
    println!("  Leaf values: {} entries", leaf_values_padded.len());
    println!("  Expected leaf sum: {}", expected_sum);
    println!("  Predicted class: {}", predicted_class);

    // === Build the GKR circuit ===
    let base_circuit = build_tree_inference_circuit(num_trees_padded, max_depth);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    // Set circuit inputs (From<Vec<i64>> handles negatives via field negation,
    // From<Vec<bool>> converts true→1, false→0)
    prover_circuit.set_input("path_bits", flat_path_bits.into());
    prover_circuit.set_input("leaf_values", leaf_values_padded.into());
    prover_circuit.set_input("expected_sum", vec![expected_sum].into());

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

/// Build a GKR circuit for XGBoost tree inference verification (Phase 1a).
///
/// The circuit verifies:
/// 1. Path bits are binary (each bit is 0 or 1)
/// 2. Correct leaf is selected for each tree via MLE fold
/// 3. Sum of selected leaves equals expected_sum
///
/// Inputs:
/// - path_bits (committed): T * d binary indicators (0=left, 1=right)
/// - leaf_values (public): T * 2^d quantized leaf values (big-endian path ordering)
/// - expected_sum (public): expected aggregate of selected leaves (scalar)
///
/// Leaf ordering is big-endian: leaf index = b_0 * 2^(d-1) + b_1 * 2^(d-2) + ... + b_{d-1}.
/// The fold processes b_0 first (root decision), splitting each tree's leaves into
/// first-half (left subtree) and second-half (right subtree).
fn build_tree_inference_circuit(
    num_trees_padded: usize,
    max_depth: usize,
) -> Circuit<Fr> {
    assert!(num_trees_padded.is_power_of_two());
    assert!(max_depth > 0);

    // For a power-of-2 count, trailing_zeros gives exact log2
    let tree_nv = num_trees_padded.trailing_zeros() as usize;
    let pb_count = num_trees_padded * max_depth;
    let pb_nv = model::next_log2(pb_count);
    let lv_nv = tree_nv + max_depth; // log2(T * 2^d)

    let mut builder = CircuitBuilder::<Fr>::new();

    // Input layers
    let committed = builder.add_input_layer("committed", LayerVisibility::Committed);
    let public = builder.add_input_layer("public", LayerVisibility::Public);

    // Input shreds
    let path_bits = builder.add_input_shred("path_bits", pb_nv, &committed);
    let leaf_values = builder.add_input_shred("leaf_values", lv_nv, &public);
    let expected_sum = builder.add_input_shred("expected_sum", 0, &public);

    // === Binary check: b^2 - b = b*(b-1), must be zero for b in {0,1} ===
    let b_squared = builder.add_sector(path_bits.expr() * path_bits.expr());
    let bc = builder.add_sector(b_squared.expr() - path_bits.expr());

    // === Leaf fold: d iterations, MSB-first (b_0 = root decision) ===
    // At each level k, we fold using path bit b_k:
    //   Split current into first-half (left subtree) and second-half (right subtree),
    //   then compute: new[i] = first[i] + b_k * (second[i] - first[i])
    let mut current = leaf_values;
    for k in 0..max_depth {
        let full = 1usize << (max_depth - k); // elements per tree before fold
        let half = full / 2; // elements per tree after fold
        let new_nv = tree_nv + max_depth - k - 1;

        // Route first-half elements: current[t*full + i] for i in 0..half
        let mut first_gates = Vec::new();
        for t in 0..num_trees_padded {
            for i in 0..half {
                first_gates.push(((t * half + i) as u32, (t * full + i) as u32));
            }
        }
        let first = builder.add_identity_gate_node(&current, first_gates, new_nv, None);

        // Route second-half elements: current[t*full + half + i] for i in 0..half
        let mut second_gates = Vec::new();
        for t in 0..num_trees_padded {
            for i in 0..half {
                second_gates.push(((t * half + i) as u32, (t * full + half + i) as u32));
            }
        }
        let second = builder.add_identity_gate_node(&current, second_gates, new_nv, None);

        // Expand path bit b_k: replicate each tree's bit to half positions
        // path_bits layout: [t0_b0, t0_b1, ..., t0_b{d-1}, t1_b0, ...]
        let mut expand_gates = Vec::new();
        for t in 0..num_trees_padded {
            for i in 0..half {
                expand_gates.push(((t * half + i) as u32, (t * max_depth + k) as u32));
            }
        }
        let b_exp = builder.add_identity_gate_node(&path_bits, expand_gates, new_nv, None);

        // Fold: first + b_k * (second - first)
        let diff = builder.add_sector(second.expr() - first.expr());
        let scaled = builder.add_sector(b_exp.expr() * diff.expr());
        current = builder.add_sector(first.expr() + scaled.expr());
    }
    // current now has T values (one selected leaf per tree), num_vars = tree_nv

    // === Aggregation: sum T values down to 1 ===
    let mut agg = current;
    for level in (0..tree_nv).rev() {
        let half_n = 1usize << level;
        let mut even_gates = Vec::new();
        let mut odd_gates = Vec::new();
        for i in 0..half_n {
            even_gates.push((i as u32, (2 * i) as u32));
            odd_gates.push((i as u32, (2 * i + 1) as u32));
        }
        let even = builder.add_identity_gate_node(&agg, even_gates, level, None);
        let odd = builder.add_identity_gate_node(&agg, odd_gates, level, None);
        agg = builder.add_sector(even.expr() + odd.expr());
    }
    // agg is a scalar (num_vars = 0): sum of all selected leaves

    // === Sum residual: leaf_sum - expected_sum must be zero ===
    let sum_residual = builder.add_sector(agg.expr() - expected_sum.expr());

    // === Output: combine binary check and sum residual ===
    // Output[0] = sum_residual, Output[1..bc_count+1] = binary check values
    // All must be zero for a valid proof.
    let output_nv = model::next_log2(pb_count + 1);

    // Route sum residual to position 0
    let sum_routed =
        builder.add_identity_gate_node(&sum_residual, vec![(0, 0)], output_nv, None);

    // Route binary check values to positions 1..bc_count+1
    let mut bc_gates = Vec::new();
    for i in 0..pb_count {
        bc_gates.push(((i + 1) as u32, i as u32));
    }
    let bc_routed = builder.add_identity_gate_node(&bc, bc_gates, output_nv, None);

    // Combine (no cancellation since they occupy disjoint positions)
    let output = builder.add_sector(sum_routed.expr() + bc_routed.expr());

    builder.set_output(&output);
    builder.build().expect("Failed to build tree inference circuit")
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
    use crate::model::{self, DecisionTree, TreeNode, XgboostModel};

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

    /// Test that the tree inference circuit builds without panicking.
    #[test]
    fn test_build_tree_inference_circuit() {
        // 2 trees, depth 2
        let _circuit = build_tree_inference_circuit(2, 2);
    }

    /// Test that the tree inference circuit builds for a single tree.
    #[test]
    fn test_build_tree_inference_circuit_single_tree() {
        // 1 tree, depth 1
        let _circuit = build_tree_inference_circuit(1, 1);
    }

    /// Test that the tree inference circuit builds for 4 trees, depth 3.
    #[test]
    fn test_build_tree_inference_circuit_4_trees() {
        let _circuit = build_tree_inference_circuit(4, 3);
    }

    /// Full prove-and-verify with the sample model (2 trees, depth 2).
    #[test]
    fn test_build_and_prove_tree_inference() {
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

    /// Test with a minimal 1-tree depth-1 model.
    #[test]
    fn test_build_and_prove_single_tree_depth1() {
        let model = XgboostModel {
            num_features: 1,
            num_classes: 2,
            max_depth: 1,
            base_score: 0.0,
            trees: vec![DecisionTree {
                nodes: vec![
                    TreeNode {
                        feature_index: 0,
                        threshold: 0.5,
                        left_child: 1,
                        right_child: 2,
                        leaf_value: 0.0,
                        is_leaf: false,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: -1.0,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 1.0,
                        is_leaf: true,
                    },
                ],
            }],
        };
        // Feature 0.3 < 0.5 → left → leaf value -1.0
        let features = vec![0.3];
        let predicted = model::predict(&model, &features);
        assert_eq!(predicted, 0); // score = -1.0 → class 0
        let result = build_and_prove(&model, &features, predicted);
        assert!(result.is_ok(), "Single tree depth 1 failed: {:?}", result.err());
    }

    /// Test with features going right at all decisions.
    #[test]
    fn test_build_and_prove_all_right() {
        let model = model::sample_model();
        // All features high → go right at every node
        let features = vec![0.9, 0.9, 0.9, 0.9, 0.9];
        let predicted = model::predict(&model, &features);
        let result = build_and_prove(&model, &features, predicted);
        assert!(result.is_ok(), "All-right path failed: {:?}", result.err());
    }

    /// Test with features going left at all decisions.
    #[test]
    fn test_build_and_prove_all_left() {
        let model = model::sample_model();
        // All features low → go left at every node
        let features = vec![0.1, 0.1, 0.1, 0.1, 0.1];
        let predicted = model::predict(&model, &features);
        let result = build_and_prove(&model, &features, predicted);
        assert!(result.is_ok(), "All-left path failed: {:?}", result.err());
    }
}
