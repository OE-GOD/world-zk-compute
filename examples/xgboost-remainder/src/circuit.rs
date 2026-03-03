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
use frontend::abstract_expr::AbstractExpression;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility, NodeRef};
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

/// All prepared circuit inputs for the full inference circuit.
struct CircuitInputs {
    flat_path_bits: Vec<bool>,
    features_quantized: Vec<i64>,
    decomp_bits_padded: Vec<bool>,
    leaf_values_padded: Vec<i64>,
    expected_sum: i64,
    thresholds_padded: Vec<i64>,
    is_real_padded: Vec<i64>,
    fi_padded: Vec<usize>,
    num_trees_padded: usize,
    max_depth: usize,
    num_features_padded: usize,
    decomp_k: usize,
}

/// Prepare all circuit inputs from model and features.
fn prepare_circuit_inputs(model: &XgboostModel, features: &[f64]) -> CircuitInputs {
    let (all_leaf_values, max_depth) = model::collect_all_leaf_values(model);
    let (path_bits_2d, _) = model::compute_path_bits(model, features);

    let num_trees = model.trees.len();
    let num_trees_padded = num_trees.next_power_of_two();
    let leaves_per_tree = 1usize << max_depth;
    let num_features_padded = model.num_features.next_power_of_two();
    let decomp_k = model::DEFAULT_DECOMP_K;

    let vt_count = max_depth * num_trees_padded;
    let vt_padded = vt_count.next_power_of_two();
    let num_positions = 1usize << max_depth;

    // Path bits (committed)
    let mut flat_path_bits: Vec<bool> = Vec::new();
    for bits in &path_bits_2d {
        flat_path_bits.extend(bits);
    }
    for _ in num_trees..num_trees_padded {
        flat_path_bits.extend(vec![false; max_depth]);
    }
    let pb_nv = next_log2(num_trees_padded * max_depth);
    while flat_path_bits.len() < (1 << pb_nv) {
        flat_path_bits.push(false);
    }

    // Quantized features (committed)
    let mut features_quantized: Vec<i64> = features.iter().map(|f| model::quantize(*f)).collect();
    features_quantized.resize(num_features_padded, 0);

    // Decomp bits witness (committed)
    let decomp_bits_raw = model::compute_comparison_witness(model, features, max_depth, decomp_k);
    let decomp_nv = next_log2(vt_padded * decomp_k);
    let mut decomp_bits_padded = vec![false; 1 << decomp_nv];
    for t in 0..num_trees {
        for k in 0..max_depth {
            let src_pos = t * max_depth + k;
            let dst_v = k * num_trees_padded + t;
            for bit_i in 0..decomp_k {
                decomp_bits_padded[dst_v * decomp_k + bit_i] =
                    decomp_bits_raw[src_pos * decomp_k + bit_i];
            }
        }
    }

    // Leaf values (public)
    let mut leaf_values_padded = all_leaf_values;
    for _ in num_trees..num_trees_padded {
        leaf_values_padded.extend(vec![0i64; leaves_per_tree]);
    }

    // Expected sum (public)
    let expected_sum =
        model::compute_leaf_sum(model, features) - model::quantize(model.base_score);

    // Comparison tables (public)
    let (thresholds_raw, feature_indices_raw, is_real_raw) =
        model::compute_comparison_tables(model, max_depth);
    let thresh_table_nv = vt_padded.trailing_zeros() as usize + max_depth;
    let thresh_table_size = 1usize << thresh_table_nv;
    let mut thresholds_padded = vec![0i64; thresh_table_size];
    let mut is_real_padded = vec![0i64; thresh_table_size];
    let mut fi_padded = vec![0usize; vt_padded * num_positions];
    for k in 0..max_depth {
        for t in 0..num_trees {
            for j in 0..num_positions {
                let src = k * num_trees * num_positions + t * num_positions + j;
                let dst_v = k * num_trees_padded + t;
                let dst = dst_v * num_positions + j;
                thresholds_padded[dst] = thresholds_raw[src];
                is_real_padded[dst] = is_real_raw[src];
                fi_padded[dst] = feature_indices_raw[src];
            }
        }
    }

    CircuitInputs {
        flat_path_bits,
        features_quantized,
        decomp_bits_padded,
        leaf_values_padded,
        expected_sum,
        thresholds_padded,
        is_real_padded,
        fi_padded,
        num_trees_padded,
        max_depth,
        num_features_padded,
        decomp_k,
    }
}

/// Build the circuit, set inputs, generate and verify a Hyrax proof.
/// Returns Ok on successful verification, Err on failure.
fn prove_and_verify(inputs: &CircuitInputs) -> Result<()> {
    let base_circuit = build_full_inference_circuit(
        inputs.num_trees_padded,
        inputs.max_depth,
        inputs.num_features_padded,
        &inputs.fi_padded,
        inputs.decomp_k,
    );

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    prover_circuit.set_input("path_bits", inputs.flat_path_bits.clone().into());
    prover_circuit.set_input("features", inputs.features_quantized.clone().into());
    prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.clone().into());
    prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.clone().into());
    prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
    prover_circuit.set_input("thresholds", inputs.thresholds_padded.clone().into());
    prover_circuit.set_input("is_real", inputs.is_real_padded.clone().into());

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

    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| hyrax_provable.prove(w, x, y, z),
        &hyrax_prover_config,
        &pedersen_committer,
        &mut blinding_rng,
        &mut vandermonde,
        &mut prover_transcript
    );

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

    Ok(())
}

/// Build a GKR circuit for XGBoost inference, generate a Hyrax proof, and verify it.
///
/// Returns (serialized_proof, circuit_hash, public_inputs_bytes).
pub fn build_and_prove(
    model: &XgboostModel,
    features: &[f64],
    predicted_class: u32,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let inputs = prepare_circuit_inputs(model, features);

    println!(
        "  Circuit: {} trees (padded to {}), depth {}, {} virtual trees",
        model.trees.len(),
        inputs.num_trees_padded,
        inputs.max_depth,
        (inputs.max_depth * inputs.num_trees_padded).next_power_of_two()
    );
    println!("  Expected leaf sum: {}", inputs.expected_sum);
    println!("  Predicted class: {}", predicted_class);

    // Build circuit
    let base_circuit = build_full_inference_circuit(
        inputs.num_trees_padded,
        inputs.max_depth,
        inputs.num_features_padded,
        &inputs.fi_padded,
        inputs.decomp_k,
    );

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    // Set circuit inputs
    prover_circuit.set_input("path_bits", inputs.flat_path_bits.into());
    prover_circuit.set_input("features", inputs.features_quantized.into());
    prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.into());
    prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.into());
    prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
    prover_circuit.set_input("thresholds", inputs.thresholds_padded.into());
    prover_circuit.set_input("is_real", inputs.is_real_padded.into());

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

/// Fold a table of VT * 2^d entries down to VT entries using path bits.
///
/// At each fold level k (0..max_depth), for each virtual tree v:
///   - Splits v's 2^(d-k) entries into first-half and second-half
///   - Selects using path_bits[bit_map(v) * max_depth + k]
///   - new[v] = first + bit * (second - first)
///
/// bit_map: maps virtual tree index → actual tree index for path bit lookup.
fn fold_table_with_bits(
    builder: &mut CircuitBuilder<Fr>,
    table: &NodeRef<Fr>,
    path_bits: &NodeRef<Fr>,
    vt: usize,
    max_depth: usize,
    bit_map: &dyn Fn(usize) -> usize,
) -> NodeRef<Fr> {
    let vt_nv = vt.trailing_zeros() as usize;
    let mut current = table.clone();

    for k in 0..max_depth {
        let full = 1usize << (max_depth - k);
        let half = full / 2;
        let new_nv = vt_nv + max_depth - k - 1;

        let mut first_gates = Vec::new();
        let mut second_gates = Vec::new();
        let mut expand_gates = Vec::new();
        for v in 0..vt {
            let t = bit_map(v);
            for i in 0..half {
                first_gates.push(((v * half + i) as u32, (v * full + i) as u32));
                second_gates.push(((v * half + i) as u32, (v * full + half + i) as u32));
                expand_gates.push(((v * half + i) as u32, (t * max_depth + k) as u32));
            }
        }

        let first = builder.add_identity_gate_node(&current, first_gates, new_nv, None);
        let second = builder.add_identity_gate_node(&current, second_gates, new_nv, None);
        let b_exp = builder.add_identity_gate_node(path_bits, expand_gates, new_nv, None);

        let diff = builder.add_sector(second.expr() - first.expr());
        let scaled = builder.add_sector(b_exp.expr() * diff.expr());
        current = builder.add_sector(first.expr() + scaled.expr());
    }

    current
}

/// Sum VT values down to 1 using pairwise tree reduction.
fn aggregate_sum(
    builder: &mut CircuitBuilder<Fr>,
    values: &NodeRef<Fr>,
    nv: usize,
) -> NodeRef<Fr> {
    let mut agg = values.clone();
    for level in (0..nv).rev() {
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
    agg
}

/// Build a full GKR circuit for XGBoost inference with comparison constraints (Phase 1a+1b).
///
/// Phase 1a verifies: path bits are binary, leaf fold selects correct leaves, sum matches.
/// Phase 1b verifies: feature-threshold comparisons match path bits via bit decomposition.
///
/// Committed inputs: path_bits, features, decomp_bits
/// Public inputs: leaf_values, expected_sum, thresholds, is_real
///
/// feature_indices_padded: VT_pad * 2^d entries mapping each position to a feature index.
///   Layout: for virtual tree v = k * num_trees_padded + t, position j:
///     feature_indices_padded[v * 2^d + j] = index into features array.
fn build_full_inference_circuit(
    num_trees_padded: usize,
    max_depth: usize,
    num_features_padded: usize,
    feature_indices_padded: &[usize],
    decomp_k: usize,
) -> Circuit<Fr> {
    assert!(num_trees_padded.is_power_of_two());
    assert!(num_features_padded.is_power_of_two());
    assert!(max_depth > 0);
    assert!(decomp_k > 0);

    let tree_nv = num_trees_padded.trailing_zeros() as usize;
    let pb_count = num_trees_padded * max_depth;
    let pb_nv = model::next_log2(pb_count);
    let lv_nv = tree_nv + max_depth;

    // Virtual trees for comparison: d * T_pad, padded to power of 2
    let vt_count = max_depth * num_trees_padded;
    let vt_padded = vt_count.next_power_of_two();
    let vt_nv = vt_padded.trailing_zeros() as usize;

    let feat_nv = num_features_padded.trailing_zeros() as usize;
    let decomp_total = vt_padded * decomp_k;
    let decomp_nv = model::next_log2(decomp_total);
    let thresh_table_nv = vt_nv + max_depth;

    assert_eq!(
        feature_indices_padded.len(),
        vt_padded * (1 << max_depth),
        "feature_indices_padded wrong size"
    );

    let mut builder = CircuitBuilder::<Fr>::new();

    // === Input layers ===
    let committed = builder.add_input_layer("committed", LayerVisibility::Committed);
    let public = builder.add_input_layer("public", LayerVisibility::Public);

    let path_bits = builder.add_input_shred("path_bits", pb_nv, &committed);
    let features = builder.add_input_shred("features", feat_nv, &committed);
    let decomp_bits_input = builder.add_input_shred("decomp_bits", decomp_nv, &committed);

    let leaf_values = builder.add_input_shred("leaf_values", lv_nv, &public);
    let expected_sum = builder.add_input_shred("expected_sum", 0, &public);
    let thresh_table = builder.add_input_shred("thresholds", thresh_table_nv, &public);
    let is_real_table = builder.add_input_shred("is_real", thresh_table_nv, &public);

    // ========== PHASE 1a: Leaf Selection ==========

    // Binary check on path bits
    let b_sq = builder.add_sector(path_bits.expr() * path_bits.expr());
    let path_bc = builder.add_sector(b_sq.expr() - path_bits.expr());

    // Leaf fold: T_pad trees, direct bit mapping
    let leaf_selected =
        fold_table_with_bits(&mut builder, &leaf_values, &path_bits, num_trees_padded, max_depth, &|v| v);

    // Aggregation
    let leaf_sum = aggregate_sum(&mut builder, &leaf_selected, tree_nv);

    // Sum residual
    let sum_residual = builder.add_sector(leaf_sum.expr() - expected_sum.expr());

    // ========== PHASE 1b: Comparison Verification ==========

    // --- Feature routing: identity gates from features → feature_table ---
    let num_positions = 1usize << max_depth;
    let ft_total = vt_padded * num_positions;
    let mut feat_route_gates = Vec::new();
    for idx in 0..ft_total {
        let src = feature_indices_padded[idx];
        feat_route_gates.push((idx as u32, src as u32));
    }
    let feature_table =
        builder.add_identity_gate_node(&features, feat_route_gates, thresh_table_nv, None);

    // --- Fold thresholds, features, is_real with virtual tree path bits ---
    let vt_bit_map = |v: usize| -> usize { v % num_trees_padded };

    let selected_thresh = fold_table_with_bits(
        &mut builder, &thresh_table, &path_bits, vt_padded, max_depth, &vt_bit_map,
    );
    let selected_feat = fold_table_with_bits(
        &mut builder, &feature_table, &path_bits, vt_padded, max_depth, &vt_bit_map,
    );
    let selected_is_real = fold_table_with_bits(
        &mut builder, &is_real_table, &path_bits, vt_padded, max_depth, &vt_bit_map,
    );

    // --- Decomp binary check: bit^2 - bit ---
    let db_sq = builder.add_sector(decomp_bits_input.expr() * decomp_bits_input.expr());
    let decomp_bc = builder.add_sector(db_sq.expr() - decomp_bits_input.expr());

    // --- Reconstruction: weighted sum of bits == feature - threshold + offset ---
    // Route each bit position and compute weighted sum using scaled expressions
    // decomp_bits layout: [vt0_bit0, vt0_bit1, ..., vt0_bitK-1, vt1_bit0, ...]
    let mut weighted_sum_expr: AbstractExpression<Fr> =
        AbstractExpression::constant(Fr::from(0u64));
    let mut bit_routes: Vec<NodeRef<Fr>> = Vec::new();

    for bit_i in 0..decomp_k {
        let mut route_gates = Vec::new();
        for v in 0..vt_padded {
            route_gates.push((v as u32, (v * decomp_k + bit_i) as u32));
        }
        let bit_routed =
            builder.add_identity_gate_node(&decomp_bits_input, route_gates, vt_nv, None);
        let scale = Fr::from(1u64 << bit_i);
        weighted_sum_expr =
            weighted_sum_expr + AbstractExpression::scaled(bit_routed.expr(), scale);

        if bit_i == decomp_k - 1 {
            bit_routes.push(bit_routed); // save top bit for sign check
        }
    }
    let weighted_sum = builder.add_sector(weighted_sum_expr);

    // shifted_diff = selected_feature - selected_threshold + offset
    let offset = Fr::from(1u64 << (decomp_k - 1));
    let shifted_diff = builder.add_sector(
        selected_feat.expr() - selected_thresh.expr() + AbstractExpression::constant(offset),
    );

    // Reconstruction residual: (weighted_sum - shifted_diff) * is_real
    let recon_diff = builder.add_sector(weighted_sum.expr() - shifted_diff.expr());
    let masked_recon = builder.add_sector(recon_diff.expr() * selected_is_real.expr());

    // --- Sign consistency: (top_bit - path_bit) * is_real ---
    let top_bit = &bit_routes[0]; // bit K-1, the sign bit

    // Route path bits to VT_pad layout: for virtual tree v = k*T+t, need path_bits[t*d+k]
    let mut sign_pb_gates = Vec::new();
    for v in 0..vt_padded {
        let t = v % num_trees_padded;
        let k = v / num_trees_padded;
        let src = if k < max_depth {
            t * max_depth + k
        } else {
            0 // padding
        };
        sign_pb_gates.push((v as u32, src as u32));
    }
    let sign_path_bits =
        builder.add_identity_gate_node(&path_bits, sign_pb_gates, vt_nv, None);

    let sign_diff = builder.add_sector(top_bit.expr() - sign_path_bits.expr());
    let masked_sign = builder.add_sector(sign_diff.expr() * selected_is_real.expr());

    // ========== OUTPUT: combine all zero checks ==========
    // [0]: sum_residual
    // [1..pb_count+1]: path_bits binary check
    // [pb_count+1..pb_count+1+decomp_total]: decomp_bits binary check
    // [pb_count+1+decomp_total..+vt_padded]: masked reconstruction
    // [+vt_padded..+2*vt_padded]: masked sign
    let total_checks = 1 + pb_count + decomp_total + vt_padded + vt_padded;
    let output_nv = model::next_log2(total_checks);

    let mut offset_pos = 0usize;

    // sum_residual → position 0
    let sum_r = builder.add_identity_gate_node(
        &sum_residual,
        vec![(offset_pos as u32, 0)],
        output_nv,
        None,
    );
    offset_pos += 1;

    // path binary check → positions 1..pb_count+1
    let mut pbc_gates = Vec::new();
    for i in 0..pb_count {
        pbc_gates.push(((offset_pos + i) as u32, i as u32));
    }
    let pbc_r = builder.add_identity_gate_node(&path_bc, pbc_gates, output_nv, None);
    offset_pos += pb_count;

    // decomp binary check → next decomp_total positions
    let mut dbc_gates = Vec::new();
    for i in 0..decomp_total {
        dbc_gates.push(((offset_pos + i) as u32, i as u32));
    }
    let dbc_r = builder.add_identity_gate_node(&decomp_bc, dbc_gates, output_nv, None);
    offset_pos += decomp_total;

    // masked reconstruction → next vt_padded positions
    let mut mr_gates = Vec::new();
    for i in 0..vt_padded {
        mr_gates.push(((offset_pos + i) as u32, i as u32));
    }
    let mr_r = builder.add_identity_gate_node(&masked_recon, mr_gates, output_nv, None);
    offset_pos += vt_padded;

    // masked sign → next vt_padded positions
    let mut ms_gates = Vec::new();
    for i in 0..vt_padded {
        ms_gates.push(((offset_pos + i) as u32, i as u32));
    }
    let ms_r = builder.add_identity_gate_node(&masked_sign, ms_gates, output_nv, None);

    // Combine all (disjoint positions, no cancellation)
    let combined = builder.add_sector(
        sum_r.expr() + pbc_r.expr() + dbc_r.expr() + mr_r.expr() + ms_r.expr(),
    );

    builder.set_output(&combined);
    builder.build().expect("Failed to build full inference circuit")
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

    /// Helper: prepare inputs, apply tampering, and try to prove.
    /// Returns true if proof succeeds, false if it panics/fails.
    fn try_prove_tampered(
        model: &XgboostModel,
        features: &[f64],
        tamper: impl FnOnce(&mut CircuitInputs) + std::panic::UnwindSafe,
    ) -> bool {
        let mut inputs = prepare_circuit_inputs(model, features);
        tamper(&mut inputs);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prove_and_verify(&inputs).is_ok()
        }))
        .unwrap_or(false)
    }

    /// Negative test: wrong expected sum should fail verification.
    #[test]
    fn test_reject_wrong_expected_sum() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let result = try_prove_tampered(&model, &features, |inputs| {
            inputs.expected_sum += 1000; // tamper
        });
        assert!(!result, "Should reject wrong expected_sum");
    }

    /// Negative test: flipped path bit should fail verification.
    #[test]
    fn test_reject_flipped_path_bit() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let result = try_prove_tampered(&model, &features, |inputs| {
            // Flip the first path bit
            inputs.flat_path_bits[0] = !inputs.flat_path_bits[0];
        });
        assert!(!result, "Should reject flipped path bit");
    }

    /// Negative test: tampered feature value should fail verification.
    #[test]
    fn test_reject_tampered_feature() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let result = try_prove_tampered(&model, &features, |inputs| {
            // Change first feature significantly
            inputs.features_quantized[0] = model::quantize(0.1); // was 0.6
        });
        assert!(!result, "Should reject tampered feature");
    }

    /// Negative test: flipped decomp bit should fail verification.
    #[test]
    fn test_reject_flipped_decomp_bit() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let result = try_prove_tampered(&model, &features, |inputs| {
            // Flip a decomp bit (bit 5 of first virtual tree)
            inputs.decomp_bits_padded[5] = !inputs.decomp_bits_padded[5];
        });
        assert!(!result, "Should reject flipped decomp bit");
    }

    /// Diagnostic: inspect the proof structure of the Phase 1a circuit.
    /// Run with: cargo test test_inspect_phase1a_proof_structure -- --nocapture
    #[test]
    fn test_inspect_phase1a_proof_structure() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let (all_leaf_values, max_depth) = model::collect_all_leaf_values(&model);
        let (path_bits_2d, _) = model::compute_path_bits(&model, &features);
        let num_trees = model.trees.len();
        let num_trees_padded = num_trees.next_power_of_two();
        let tree_nv = num_trees_padded.trailing_zeros() as usize;
        let pb_count = num_trees_padded * max_depth;
        let pb_nv = next_log2(pb_count);
        let leaves_per_tree = 1usize << max_depth;

        let base_circuit = build_tree_inference_circuit(num_trees_padded, max_depth);

        let mut prover_circuit = base_circuit.clone();
        let verifier_circuit = base_circuit;

        // Prepare path bits
        let mut flat_path_bits: Vec<bool> = Vec::new();
        for bits in &path_bits_2d {
            flat_path_bits.extend(bits);
        }
        for _ in num_trees..num_trees_padded {
            flat_path_bits.extend(vec![false; max_depth]);
        }
        while flat_path_bits.len() < (1 << pb_nv) {
            flat_path_bits.push(false);
        }

        // Prepare leaf values
        let mut leaf_values_padded = all_leaf_values;
        for _ in num_trees..num_trees_padded {
            leaf_values_padded.extend(vec![0i64; leaves_per_tree]);
        }

        // Expected sum
        let expected_sum =
            model::compute_leaf_sum(&model, &features) - model::quantize(model.base_score);

        prover_circuit.set_input("path_bits", flat_path_bits.into());
        prover_circuit.set_input("leaf_values", leaf_values_padded.into());
        prover_circuit.set_input("expected_sum", vec![expected_sum].into());

        let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();

        let mut provable = prover_circuit
            .gen_hyrax_provable_circuit()
            .expect("gen_hyrax_provable_circuit");

        let committer =
            PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
        let mut rng = thread_rng();
        let mut vander = VandermondeInverse::new();
        let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");

        let (proof, _proof_config) = perform_function_under_prover_config!(
            |w, x, y, z| provable.prove(w, x, y, z),
            &config,
            &committer,
            &mut rng,
            &mut vander,
            &mut transcript
        );

        let verifiable: hyrax::verifiable_circuit::HyraxVerifiableCircuit<Bn256Point> = verifier_circuit
            .gen_hyrax_verifiable_circuit()
            .expect("gen_hyrax_verifiable_circuit");
        let desc = verifiable.get_gkr_circuit_description_ref();

        eprintln!("\n=== Phase 1a Circuit Structure ===");
        eprintln!("Output layers: {}", desc.output_layers.len());
        eprintln!("Intermediate layers: {}", desc.intermediate_layers.len());

        use remainder::layer::LayerDescription;

        // Build layer index map: LayerId → proof order index
        let mut layer_id_to_proof_idx: std::collections::HashMap<remainder::layer::LayerId, usize> = std::collections::HashMap::new();
        for (i, (layer_id, _)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
            layer_id_to_proof_idx.insert(*layer_id, i);
        }
        // Map input layer IDs
        for (i, il) in desc.input_layers.iter().enumerate() {
            eprintln!("  Input layer {}: id={:?}", i, il.layer_id);
        }

        eprintln!("\n=== DAG Topology (intermediate layers) ===");
        for (i, layer) in desc.intermediate_layers.iter().enumerate() {
            let mles = layer.get_circuit_mles();
            let targets: Vec<String> = mles.iter().map(|m| format!("{:?}", m.layer_id())).collect();
            eprintln!(
                "  Layer {:2}: id={:?}, degree={}, rounds={}, mle_targets=[{}]",
                i,
                layer.layer_id(),
                layer.max_degree(),
                layer.sumcheck_round_indices().len(),
                targets.join(", "),
            );
        }

        eprintln!("\n=== Output layers ===");
        for (i, ol) in desc.output_layers.iter().enumerate() {
            eprintln!("  Output {:2}: layer_id={:?}", i, ol.layer_id());
        }

        eprintln!("\n=== FS Challenges ===");
        for (i, fs) in desc.fiat_shamir_challenges.iter().enumerate() {
            eprintln!("  FS {:2}: layer_id={:?}, num_bits={}", i, fs.layer_id(), fs.num_bits);
        }

        eprintln!("\n=== Proof Structure ===");
        eprintln!("Layer proofs: {}", proof.circuit_proof.layer_proofs.len());
        for (i, (_layer_id, layer_proof)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
            let has_claim_agg = layer_proof.maybe_proof_of_claim_agg.is_some();
            eprintln!(
                "  Layer {:2}: id={:?}, msgs={}, commits={}, pops={}, claim_agg={}",
                i,
                _layer_id,
                layer_proof.proof_of_sumcheck.messages.len(),
                layer_proof.commitments.len(),
                layer_proof.proofs_of_product.len(),
                has_claim_agg,
            );
        }

        eprintln!("\nInput proofs:");
        for (i, ip) in proof.hyrax_input_proofs.iter().enumerate() {
            eprintln!(
                "  Input {}: layer_id={:?}, commitments={}, eval_proofs={}",
                i,
                ip.layer_id,
                ip.input_commitment.len(),
                ip.evaluation_proofs.len(),
            );
        }

        eprintln!("\nFS claims:");
        for (i, fc) in proof.circuit_proof.fiat_shamir_claims.iter().enumerate() {
            eprintln!(
                "  FS claim {}: to={:?}, point_len={}",
                i,
                fc.to_layer_id,
                fc.point.len(),
            );
        }
        eprintln!("=== End ===\n");
    }

    /// Diagnostic: inspect the proof structure of the Phase 1b circuit.
    /// Run with: cargo test test_inspect_proof_structure -- --nocapture
    #[test]
    fn test_inspect_proof_structure() {
        let model = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let inputs = prepare_circuit_inputs(&model, &features);

        let base_circuit = build_full_inference_circuit(
            inputs.num_trees_padded,
            inputs.max_depth,
            inputs.num_features_padded,
            &inputs.fi_padded,
            inputs.decomp_k,
        );

        let mut prover_circuit = base_circuit.clone();
        let verifier_circuit = base_circuit;

        prover_circuit.set_input("path_bits", inputs.flat_path_bits.into());
        prover_circuit.set_input("features", inputs.features_quantized.into());
        prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.into());
        prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.into());
        prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
        prover_circuit.set_input("thresholds", inputs.thresholds_padded.into());
        prover_circuit.set_input("is_real", inputs.is_real_padded.into());

        let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();

        let mut provable = prover_circuit
            .gen_hyrax_provable_circuit()
            .expect("gen_hyrax_provable_circuit");

        let committer =
            PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
        let mut rng = thread_rng();
        let mut vander = VandermondeInverse::new();
        let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");

        let (proof, _proof_config) = perform_function_under_prover_config!(
            |w, x, y, z| provable.prove(w, x, y, z),
            &config,
            &committer,
            &mut rng,
            &mut vander,
            &mut transcript
        );

        // Inspect the verifiable circuit description
        let verifiable: hyrax::verifiable_circuit::HyraxVerifiableCircuit<Bn256Point> = verifier_circuit
            .gen_hyrax_verifiable_circuit()
            .expect("gen_hyrax_verifiable_circuit");
        let desc = verifiable.get_gkr_circuit_description_ref();

        eprintln!("\n=== Phase 1b Circuit Structure ===");
        eprintln!("Output layers: {}", desc.output_layers.len());
        eprintln!("Intermediate layers: {}", desc.intermediate_layers.len());
        eprintln!("Fiat-Shamir challenges: {}", desc.fiat_shamir_challenges.len());

        for (i, layer) in desc.intermediate_layers.iter().enumerate() {
            use remainder::layer::LayerDescription;
            eprintln!(
                "  Layer {:2}: id={:?}, degree={}, sumcheck_rounds={}",
                i,
                layer.layer_id(),
                layer.max_degree(),
                layer.sumcheck_round_indices().len(),
            );
        }

        eprintln!("\n=== Proof Structure ===");
        eprintln!("Layer proofs: {}", proof.circuit_proof.layer_proofs.len());
        eprintln!("Output layer proofs: {}", proof.circuit_proof.output_layer_proofs.len());
        eprintln!("Hyrax input proofs: {}", proof.hyrax_input_proofs.len());
        eprintln!("Public inputs: {}", proof.public_inputs.len());

        for (i, (_layer_id, layer_proof)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
            eprintln!(
                "  Layer {:2}: id={:?}, msgs={}, commits={}, pops={}",
                i,
                _layer_id,
                layer_proof.proof_of_sumcheck.messages.len(),
                layer_proof.commitments.len(),
                layer_proof.proofs_of_product.len(),
            );
        }

        eprintln!("\nInput proofs:");
        for (i, ip) in proof.hyrax_input_proofs.iter().enumerate() {
            eprintln!(
                "  Input {}: commitments={}, eval_proofs={}",
                i,
                ip.input_commitment.len(),
                ip.evaluation_proofs.len(),
            );
        }
        eprintln!("=== End ===\n");
    }

    /// Test that the full inference circuit (Phase 1a+1b) builds without panicking.
    #[test]
    fn test_build_full_inference_circuit() {
        let model = model::sample_model();
        let (_, max_depth) = model::collect_all_leaf_values(&model);
        let num_trees = model.trees.len();
        let num_trees_padded = num_trees.next_power_of_two();
        let num_features_padded = model.num_features.next_power_of_two();
        let decomp_k = model::DEFAULT_DECOMP_K;

        // Build comparison tables
        let (_, feature_indices, _) = model::compute_comparison_tables(&model, max_depth);

        // Pad feature_indices for padded virtual trees
        let vt_count = max_depth * num_trees_padded;
        let vt_padded = vt_count.next_power_of_two();
        let num_positions = 1usize << max_depth;
        let mut fi_padded = vec![0usize; vt_padded * num_positions];
        for k in 0..max_depth {
            for t in 0..num_trees {
                for j in 0..num_positions {
                    let src_idx = k * num_trees * num_positions + t * num_positions + j;
                    let dst_idx = (k * num_trees_padded + t) * num_positions + j;
                    fi_padded[dst_idx] = feature_indices[src_idx];
                }
            }
        }

        let _circuit = build_full_inference_circuit(
            num_trees_padded,
            max_depth,
            num_features_padded,
            &fi_padded,
            decomp_k,
        );
    }
}
