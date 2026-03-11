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
pub struct CircuitInputs {
    pub flat_path_bits: Vec<bool>,
    pub features_quantized: Vec<i64>,
    pub decomp_bits_padded: Vec<bool>,
    pub leaf_values_padded: Vec<i64>,
    pub expected_sum: i64,
    pub thresholds_padded: Vec<i64>,
    pub is_real_padded: Vec<i64>,
    pub fi_padded: Vec<usize>,
    pub num_trees_padded: usize,
    pub max_depth: usize,
    pub num_features_padded: usize,
    pub decomp_k: usize,
}

/// Prepare all circuit inputs from model and features.
pub fn prepare_circuit_inputs(model: &XgboostModel, features: &[f64]) -> CircuitInputs {
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
    let expected_sum = model::compute_leaf_sum(model, features) - model::quantize(model.base_score);

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
#[cfg(test)]
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

    let hyrax_prover_config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
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
    let hyrax_prover_config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
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

// ========================================================================
// Cached (warm) prover: pre-builds circuit and generators for reuse
// ========================================================================

/// Pre-built circuit and generator state that can be reused across multiple
/// proof requests. Building the circuit and Pedersen generators is expensive,
/// so caching them across requests saves significant time per proof.
pub struct CachedProver {
    /// The model (owned, for preparing per-request inputs).
    pub model: XgboostModel,
    /// Pre-built base circuit (cloned per request for prover/verifier).
    pub base_circuit: Circuit<Fr>,
    /// Cached circuit hash (depends only on the model).
    pub circuit_hash: [u8; 32],
    /// Pre-built prover config.
    pub prover_config: GKRCircuitProverConfig,
    /// Pre-built verifier config.
    pub verifier_config: GKRCircuitVerifierConfig,
    /// Cached Pedersen committer (generator points, expensive to create).
    pub pedersen_committer: PedersenCommitter<Bn256Point>,
    /// Cached model parameters for input preparation.
    #[allow(dead_code)]
    pub fi_padded: Vec<usize>,
    #[allow(dead_code)]
    pub num_trees_padded: usize,
    pub max_depth: usize,
    #[allow(dead_code)]
    pub num_features_padded: usize,
    #[allow(dead_code)]
    pub decomp_k: usize,
}

impl CachedProver {
    /// Create a new CachedProver for the given model.
    ///
    /// This performs all expensive one-time setup:
    /// - Builds the GKR circuit from the model structure
    /// - Computes the circuit hash
    /// - Initializes Pedersen commitment generators
    /// - Pre-computes prover/verifier configs
    pub fn new(model: XgboostModel) -> Self {
        // Compute model-dependent parameters
        let max_depth = model.trees.iter().map(model::tree_depth).max().unwrap_or(0);
        let num_trees_padded = model.trees.len().next_power_of_two();
        let num_features_padded = model.num_features.next_power_of_two();
        let decomp_k = model::DEFAULT_DECOMP_K;

        // Compute feature index table (model-dependent, not feature-dependent)
        let num_positions = 1usize << max_depth;
        let vt_count = max_depth * num_trees_padded;
        let vt_padded = vt_count.next_power_of_two();
        let (_, feature_indices_raw, _) = model::compute_comparison_tables(&model, max_depth);
        let mut fi_padded = vec![0usize; vt_padded * num_positions];
        let num_trees = model.trees.len();
        for k in 0..max_depth {
            for t in 0..num_trees {
                for j in 0..num_positions {
                    let src = k * num_trees * num_positions + t * num_positions + j;
                    let dst_v = k * num_trees_padded + t;
                    let dst = dst_v * num_positions + j;
                    fi_padded[dst] = feature_indices_raw[src];
                }
            }
        }

        // Build circuit (expensive)
        let base_circuit = build_full_inference_circuit(
            num_trees_padded,
            max_depth,
            num_features_padded,
            &fi_padded,
            decomp_k,
        );

        // Compute circuit hash
        let circuit_hash: [u8; 32] = {
            use sha2::{Digest, Sha256};
            let circuit_desc = build_circuit_description(&model);
            let mut hasher = Sha256::new();
            hasher.update(&circuit_desc);
            hasher.finalize().into()
        };

        // Build configs
        let prover_config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
        let verifier_config =
            GKRCircuitVerifierConfig::new_from_prover_config(&prover_config, false);

        // Initialize Pedersen generators (expensive)
        let pedersen_committer =
            PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);

        CachedProver {
            model,
            base_circuit,
            circuit_hash,
            prover_config,
            verifier_config,
            pedersen_committer,
            fi_padded,
            num_trees_padded,
            max_depth,
            num_features_padded,
            decomp_k,
        }
    }

    /// Generate and verify a proof for the given features.
    ///
    /// Reuses the cached circuit and generators — only witness-dependent
    /// computation is done per call.
    pub fn prove(
        &self,
        features: &[f64],
        predicted_class: u32,
    ) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let inputs = prepare_circuit_inputs(&self.model, features);

        // Clone the cached base circuit for this request
        let mut prover_circuit = self.base_circuit.clone();
        let verifier_circuit = self.base_circuit.clone();

        // Set witness-specific inputs
        prover_circuit.set_input("path_bits", inputs.flat_path_bits.into());
        prover_circuit.set_input("features", inputs.features_quantized.into());
        prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.into());
        prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.into());
        prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
        prover_circuit.set_input("thresholds", inputs.thresholds_padded.into());
        prover_circuit.set_input("is_real", inputs.is_real_padded.into());

        // Generate Hyrax proof
        let mut hyrax_provable = prover_circuit
            .gen_hyrax_provable_circuit()
            .expect("Failed to generate Hyrax-provable circuit");

        let mut blinding_rng = thread_rng();
        let mut vandermonde = VandermondeInverse::new();
        let mut prover_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");

        let (proof, proof_config) = perform_function_under_prover_config!(
            |w, x, y, z| hyrax_provable.prove(w, x, y, z),
            &self.prover_config,
            &self.pedersen_committer,
            &mut blinding_rng,
            &mut vandermonde,
            &mut prover_transcript
        );

        // Verify the proof
        let hyrax_verifiable = verifier_circuit
            .gen_hyrax_verifiable_circuit()
            .expect("Failed to generate Hyrax-verifiable circuit");

        let verifier_pedersen_committer =
            PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
        let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder verifier transcript");

        perform_function_under_verifier_config!(
            verify_hyrax_proof,
            &self.verifier_config,
            &proof,
            &hyrax_verifiable,
            &verifier_pedersen_committer,
            &mut verifier_transcript,
            &proof_config
        );

        // ABI-encode the proof
        let proof_bytes = crate::abi_encode::encode_hyrax_proof(&proof, &self.circuit_hash)?;
        let public_inputs = predicted_class.to_be_bytes().to_vec();

        Ok((proof_bytes, self.circuit_hash.to_vec(), public_inputs))
    }
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
#[allow(dead_code)]
pub fn build_tree_inference_circuit(num_trees_padded: usize, max_depth: usize) -> Circuit<Fr> {
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
    let sum_routed = builder.add_identity_gate_node(&sum_residual, vec![(0, 0)], output_nv, None);

    // Route binary check values to positions 1..bc_count+1
    let mut bc_gates = Vec::new();
    for i in 0..pb_count {
        bc_gates.push(((i + 1) as u32, i as u32));
    }
    let bc_routed = builder.add_identity_gate_node(&bc, bc_gates, output_nv, None);

    // Combine (no cancellation since they occupy disjoint positions)
    let output = builder.add_sector(sum_routed.expr() + bc_routed.expr());

    builder.set_output(&output);
    builder
        .build()
        .expect("Failed to build tree inference circuit")
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
fn aggregate_sum(builder: &mut CircuitBuilder<Fr>, values: &NodeRef<Fr>, nv: usize) -> NodeRef<Fr> {
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
pub fn build_full_inference_circuit(
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
    let leaf_selected = fold_table_with_bits(
        &mut builder,
        &leaf_values,
        &path_bits,
        num_trees_padded,
        max_depth,
        &|v| v,
    );

    // Aggregation
    let leaf_sum = aggregate_sum(&mut builder, &leaf_selected, tree_nv);

    // Sum residual
    let sum_residual = builder.add_sector(leaf_sum.expr() - expected_sum.expr());

    // ========== PHASE 1b: Comparison Verification ==========

    // --- Feature routing: identity gates from features → feature_table ---
    let num_positions = 1usize << max_depth;
    let ft_total = vt_padded * num_positions;
    let mut feat_route_gates = Vec::new();
    for (idx, &src) in feature_indices_padded.iter().enumerate().take(ft_total) {
        feat_route_gates.push((idx as u32, src as u32));
    }
    let feature_table =
        builder.add_identity_gate_node(&features, feat_route_gates, thresh_table_nv, None);

    // --- Fold thresholds, features, is_real with virtual tree path bits ---
    let vt_bit_map = |v: usize| -> usize { v % num_trees_padded };

    let selected_thresh = fold_table_with_bits(
        &mut builder,
        &thresh_table,
        &path_bits,
        vt_padded,
        max_depth,
        &vt_bit_map,
    );
    let selected_feat = fold_table_with_bits(
        &mut builder,
        &feature_table,
        &path_bits,
        vt_padded,
        max_depth,
        &vt_bit_map,
    );
    let selected_is_real = fold_table_with_bits(
        &mut builder,
        &is_real_table,
        &path_bits,
        vt_padded,
        max_depth,
        &vt_bit_map,
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
        weighted_sum_expr += AbstractExpression::scaled(bit_routed.expr(), scale);

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
    let sign_path_bits = builder.add_identity_gate_node(&path_bits, sign_pb_gates, vt_nv, None);

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
    let combined =
        builder.add_sector(sum_r.expr() + pbc_r.expr() + dbc_r.expr() + mr_r.expr() + ms_r.expr());

    builder.set_output(&combined);
    builder
        .build()
        .expect("Failed to build full inference circuit")
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
            desc.extend_from_slice(&node.feature_index.to_be_bytes());
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
    use ff::PrimeField;
    use shared_types::curves::PrimeOrderCurve;

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
        assert!(
            result.is_ok(),
            "Single tree depth 1 failed: {:?}",
            result.err()
        );
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

        let committer = PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
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

        let verifiable: hyrax::verifiable_circuit::HyraxVerifiableCircuit<Bn256Point> =
            verifier_circuit
                .gen_hyrax_verifiable_circuit()
                .expect("gen_hyrax_verifiable_circuit");
        let desc = verifiable.get_gkr_circuit_description_ref();

        eprintln!("\n=== Phase 1a Circuit Structure ===");
        eprintln!("Output layers: {}", desc.output_layers.len());
        eprintln!("Intermediate layers: {}", desc.intermediate_layers.len());

        use remainder::layer::LayerDescription;

        // Build layer index map: LayerId → proof order index
        let mut layer_id_to_proof_idx: std::collections::HashMap<remainder::layer::LayerId, usize> =
            std::collections::HashMap::new();
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
            eprintln!(
                "  FS {:2}: layer_id={:?}, num_bits={}",
                i,
                fs.layer_id(),
                fs.num_bits
            );
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

        let committer = PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
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
        let verifiable: hyrax::verifiable_circuit::HyraxVerifiableCircuit<Bn256Point> =
            verifier_circuit
                .gen_hyrax_verifiable_circuit()
                .expect("gen_hyrax_verifiable_circuit");
        let desc = verifiable.get_gkr_circuit_description_ref();

        eprintln!("\n=== Phase 1b Circuit Structure ===");
        eprintln!("Output layers: {}", desc.output_layers.len());
        eprintln!("Intermediate layers: {}", desc.intermediate_layers.len());
        eprintln!(
            "Fiat-Shamir challenges: {}",
            desc.fiat_shamir_challenges.len()
        );

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
        eprintln!(
            "Output layer proofs: {}",
            proof.circuit_proof.output_layer_proofs.len()
        );
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

    // ========== Scale tests ==========

    /// Helper: generate a model, run prove_and_verify, return elapsed time.
    fn scale_prove_verify(
        num_trees: usize,
        depth: usize,
        num_features: usize,
    ) -> std::time::Duration {
        let model = model::generate_model(num_trees, depth, num_features);
        let features = model::generate_features(num_features, 42);

        // Verify model is well-formed
        let predicted = model::predict(&model, &features);
        eprintln!(
            "  Scale test: {} trees, depth {}, {} features → class {}",
            num_trees, depth, num_features, predicted
        );

        let inputs = prepare_circuit_inputs(&model, &features);
        let start = std::time::Instant::now();
        prove_and_verify(&inputs).expect("prove_and_verify failed");
        let elapsed = start.elapsed();
        eprintln!("  Proved + verified in {:.2}s", elapsed.as_secs_f64());
        elapsed
    }

    /// 4 trees, depth 2, 8 features
    #[test]
    fn test_scale_4_trees_depth2() {
        scale_prove_verify(4, 2, 8);
    }

    /// 4 trees, depth 3, 8 features
    #[test]
    fn test_scale_4_trees_depth3() {
        scale_prove_verify(4, 3, 8);
    }

    /// 8 trees, depth 3, 16 features
    #[test]
    fn test_scale_8_trees_depth3() {
        scale_prove_verify(8, 3, 16);
    }

    /// Test full prove-and-verify with a JSON-parsed XGBoost model
    #[test]
    fn test_prove_verify_from_json() {
        let json = r#"{
            "version": [2, 0, 0],
            "learner": {
                "learner_model_param": {
                    "num_feature": "4",
                    "num_class": "0",
                    "base_score": "5E-1"
                },
                "gradient_booster": {
                    "name": "gbtree",
                    "model": {
                        "trees": [
                            {
                                "left_children": [1, 3, 5, -1, -1, -1, -1],
                                "right_children": [2, 4, 6, -1, -1, -1, -1],
                                "split_indices": [2, 0, 3, 0, 0, 0, 0],
                                "split_conditions": [2.45, 1.5, 1.75, 0.0, 0.0, 0.0, 0.0],
                                "base_weights": [0.0, 0.0, 0.0, -0.2, 0.5, -0.3, 0.8]
                            },
                            {
                                "left_children": [1, -1, -1],
                                "right_children": [2, -1, -1],
                                "split_indices": [1, 0, 0],
                                "split_conditions": [3.0, 0.0, 0.0],
                                "base_weights": [0.0, -0.4, 0.6]
                            }
                        ],
                        "tree_info": [0, 0]
                    }
                },
                "objective": { "name": "binary:logistic" }
            }
        }"#;

        let model = model::parse_xgboost_json(json).unwrap();
        let features = vec![1.0, 2.0, 3.0, 1.0];

        let inputs = prepare_circuit_inputs(&model, &features);
        prove_and_verify(&inputs).expect("prove_and_verify from JSON model failed");
    }

    // ========== Phase 1a Fixture Generator ==========
    /// Convert Fr to big-endian hex string
    fn fr_to_hex(val: &Fr) -> String {
        let repr = <Fr as PrimeField>::to_repr(&val);
        let bytes: &[u8] = repr.as_ref();
        let mut be = bytes.to_vec();
        be.reverse();
        format!("0x{}", hex::encode(&be))
    }

    /// Convert Fq to big-endian hex string
    fn fq_to_hex(val: &Fq) -> String {
        let repr = val.to_repr();
        let bytes: &[u8] = repr.as_ref();
        let mut be = bytes.to_vec();
        be.reverse();
        format!("0x{}", hex::encode(&be))
    }

    /// Compute point template by analyzing how claim.point relates to bindings and claim_points.
    fn compute_point_template(
        point: &[Fr],
        bindings: &[Fr],
        claim_points: &[Vec<Fr>],
    ) -> Vec<String> {
        let mut template = Vec::new();
        for val in point.iter() {
            let mut found = false;
            // Check bindings
            for (bi, bv) in bindings.iter().enumerate() {
                if val == bv {
                    template.push(format!("B{}", bi));
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }
            // Check claim point coordinates
            for (ci, cp) in claim_points.iter().enumerate() {
                for (cj, cv) in cp.iter().enumerate() {
                    if val == cv {
                        template.push(format!("C{}.{}", ci, cj));
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            if found {
                continue;
            }
            // Fixed values
            if *val == Fr::zero() {
                template.push("F0".to_string());
            } else if *val == Fr::one() {
                template.push("F1".to_string());
            } else {
                template.push(format!("RAW:{}", fr_to_hex(val)));
            }
        }
        template
    }

    /// Parse template entry into u64 for Solidity encoding.
    /// BINDING_REF_LIMIT=1000, CLAIM_REF_BASE=10000, FIXED_REF_BASE=20000.
    fn parse_template_entry(entry: &str) -> u64 {
        if let Some(rest) = entry.strip_prefix('B') {
            rest.parse::<u64>().unwrap()
        } else if let Some(rest) = entry.strip_prefix('C') {
            let parts: Vec<&str> = rest.split('.').collect();
            let coord_idx: u64 = parts[1].parse().unwrap();
            10000 + coord_idx
        } else if let Some(rest) = entry.strip_prefix('F') {
            let v: u64 = rest.parse().unwrap();
            20000 + v
        } else {
            eprintln!("WARNING: Unknown template entry: {}", entry);
            20000 + 999
        }
    }

    /// Generate Phase 1a+1b fixture and write to contracts/test/fixtures/phase1a_dag_fixture.json.
    /// Run with: cargo test gen_phase1a_fixture -- --nocapture --ignored
    #[test]
    #[ignore] // Only run explicitly (takes ~2s, writes file)
    fn gen_phase1a_fixture() {
        use hyrax::gkr::layer::{get_claims_from_product, HyraxClaim};
        use remainder::layer::product::{new_with_values, PostSumcheckLayer};
        use remainder::layer::LayerDescription;
        use serde_json::json;
        use shared_types::config::{
            global_config::global_claim_agg_strategy, ClaimAggregationStrategy,
        };
        use shared_types::transcript::ec_transcript::ECTranscriptTrait;

        let mdl = model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let inputs = prepare_circuit_inputs(&mdl, &features);

        // Build full Phase 1a+1b circuit
        let base_circuit = build_full_inference_circuit(
            inputs.num_trees_padded,
            inputs.max_depth,
            inputs.num_features_padded,
            &inputs.fi_padded,
            inputs.decomp_k,
        );
        let mut prover_circuit = base_circuit.clone();
        let verifier_circuit = base_circuit;
        prover_circuit.set_input("path_bits", inputs.flat_path_bits.clone().into());
        prover_circuit.set_input("features", inputs.features_quantized.clone().into());
        prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.clone().into());
        prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.clone().into());
        prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
        prover_circuit.set_input("thresholds", inputs.thresholds_padded.clone().into());
        prover_circuit.set_input("is_real", inputs.is_real_padded.clone().into());

        let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
        let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);
        let mut provable = prover_circuit.gen_hyrax_provable_circuit().unwrap();
        let committer = PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
        let mut rng = thread_rng();
        let mut vander = VandermondeInverse::new();
        let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");

        eprintln!("Generating Phase 1a proof...");
        let (proof, proof_config) = perform_function_under_prover_config!(
            |w, x, y, z| provable.prove(w, x, y, z),
            &config,
            &committer,
            &mut rng,
            &mut vander,
            &mut transcript
        );

        // Verify in Rust
        let verifiable = verifier_circuit.gen_hyrax_verifiable_circuit().unwrap();
        let verifier_committer =
            PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
        let mut vtx: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder verifier transcript");
        perform_function_under_verifier_config!(
            verify_hyrax_proof,
            &verifier_config,
            &proof,
            &verifiable,
            &verifier_committer,
            &mut vtx,
            &proof_config
        );
        eprintln!("Proof verified in Rust!");

        let desc = verifiable.get_gkr_circuit_description_ref();
        eprintln!("FS challenges count: {}", desc.fiat_shamir_challenges.len());
        for (i, fs) in desc.fiat_shamir_challenges.iter().enumerate() {
            use remainder::layer::LayerDescription;
            eprintln!(
                "  FS {}: layer_id={:?}, num_bits={}",
                i,
                fs.layer_id(),
                fs.num_bits
            );
        }

        // Build layer ID → proof-order index mapping
        // Compute layers are indexed 0..N-1 in proof order, input layers are N, N+1, ...
        let mut layer_id_to_idx: std::collections::HashMap<remainder::layer::LayerId, usize> =
            std::collections::HashMap::new();
        let num_compute = desc.intermediate_layers.len();
        for (proof_idx, (layer_id, _)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
            layer_id_to_idx.insert(*layer_id, proof_idx);
        }
        for (j, il) in desc.input_layers.iter().enumerate() {
            layer_id_to_idx.insert(il.layer_id, num_compute + j);
        }

        // === Trace verification ===
        let mut custom_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");
        {
            use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
            use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
            let hash_elems = get_circuit_description_hash_as_field_elems(
                desc,
                global_verifier_circuit_description_hash_type(),
            );
            custom_transcript.append_scalar_field_elems("Circuit description hash", &hash_elems);
            proof.public_inputs.iter().for_each(|(_, mle)| {
                custom_transcript.append_input_scalar_field_elems(
                    "Public input",
                    &mle.as_ref().unwrap().f.iter().collect::<Vec<_>>(),
                );
            });
            proof.hyrax_input_proofs.iter().for_each(|ip| {
                custom_transcript
                    .append_input_ec_points("Hyrax input commitment", ip.input_commitment.clone());
            });
            for fs_desc in &desc.fiat_shamir_challenges {
                custom_transcript
                    .get_scalar_field_challenges("Verifier challenges", 1 << fs_desc.num_bits);
            }
        }

        // Output layer
        let mut claim_tracker: std::collections::HashMap<
            remainder::layer::LayerId,
            Vec<HyraxClaim<Fr, Bn256Point>>,
        > = std::collections::HashMap::new();
        for (_, _, olp) in &proof.circuit_proof.output_layer_proofs {
            let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
                olp,
                &desc.output_layers[0],
                &mut custom_transcript,
            );
            claim_tracker.insert(claim.to_layer_id, vec![claim]);
        }

        // Process intermediate layers
        let mut dag_layers: Vec<serde_json::Value> = Vec::new();
        let mut all_atom_targets: Vec<Vec<usize>> = Vec::new();
        let mut all_point_templates: Vec<Vec<Vec<String>>> = Vec::new();
        let mut all_claim_data: Vec<serde_json::Value> = Vec::new();
        let mut all_oracle_result_idxs: Vec<Vec<usize>> = Vec::new();
        let mut all_oracle_expr_coeffs: Vec<Vec<String>> = Vec::new();
        let mut all_atom_commit_idxs: Vec<Vec<usize>> = Vec::new();

        for (proof_idx, (layer_id, layer_proof)) in
            proof.circuit_proof.layer_proofs.iter().enumerate()
        {
            let layer_desc = desc
                .intermediate_layers
                .iter()
                .find(|ld| ld.layer_id() == *layer_id)
                .unwrap();
            let layer_claims = claim_tracker.remove(layer_id).unwrap_or_default();
            let num_rounds = layer_desc.sumcheck_round_indices().len();
            let degree = layer_desc.max_degree();
            let layer_type: u8 = if degree == 3 { 1 } else { 0 };

            eprintln!(
                "  Layer {} (id={:?}, type={}, rounds={}, claims={})",
                proof_idx,
                layer_id,
                layer_type,
                num_rounds,
                layer_claims.len()
            );

            // RLC coefficients
            let random_coefficients = match global_claim_agg_strategy() {
                ClaimAggregationStrategy::RLC => custom_transcript
                    .get_scalar_field_challenges("RLC Claim Agg Coefficients", layer_claims.len()),
                _ => vec![Fr::one()],
            };

            // Absorb messages, derive bindings
            let mut bindings: Vec<Fr> = Vec::new();
            if num_rounds > 0 {
                custom_transcript.append_ec_point("msg", layer_proof.proof_of_sumcheck.messages[0]);
            }
            for msg in layer_proof.proof_of_sumcheck.messages.iter().skip(1) {
                bindings.push(custom_transcript.get_scalar_field_challenge("binding"));
                custom_transcript.append_ec_point("msg", *msg);
            }
            if num_rounds > 0 {
                bindings.push(custom_transcript.get_scalar_field_challenge("binding"));
            }

            // Absorb commitments
            custom_transcript.append_ec_points("commits", &layer_proof.commitments);

            // Build PSL
            let claim_points: Vec<Vec<Fr>> = layer_claims.iter().map(|c| c.point.clone()).collect();
            let claim_points_refs: Vec<&[Fr]> = claim_points.iter().map(|v| v.as_slice()).collect();
            let psl_desc = layer_desc.get_post_sumcheck_layer(
                &bindings,
                &claim_points_refs,
                &random_coefficients,
            );
            let psl: PostSumcheckLayer<Fr, Bn256Point> =
                new_with_values(&psl_desc, &layer_proof.commitments);

            // Verify
            let rlc_eval = layer_claims
                .iter()
                .zip(random_coefficients.iter())
                .fold(Bn256Point::zero(), |acc, (elem, rc)| {
                    acc + elem.evaluation * *rc
                });
            layer_proof.proof_of_sumcheck.verify(
                &rlc_eval,
                degree,
                &psl,
                &bindings,
                &verifier_committer,
                &mut custom_transcript,
            );

            let product_triples: Vec<_> = psl
                .0
                .iter()
                .filter_map(|p| p.get_product_triples())
                .flatten()
                .collect();
            for ((x, y, z), pop) in product_triples
                .iter()
                .zip(layer_proof.proofs_of_product.iter())
            {
                pop.verify(*x, *y, *z, &verifier_committer, &mut custom_transcript);
            }

            // Extract oracle eval formula: for each product, (result_commit_idx, expr_coeff)
            // product.coefficient = rlcBeta * expr_coeff
            // Compute rlcBeta to extract expr_coeff
            let rlc_beta: Fr = claim_points.iter().zip(random_coefficients.iter()).fold(
                Fr::zero(),
                |acc, (cp, rc)| {
                    let beta = bindings
                        .iter()
                        .zip(cp.iter())
                        .fold(Fr::one(), |b, (ri, ci)| {
                            let term = *ri * *ci + (Fr::one() - *ri) * (Fr::one() - *ci);
                            b * term
                        });
                    acc + beta * rc
                },
            );
            let rlc_beta_inv =
                Option::<Fr>::from(rlc_beta.invert()).expect("rlcBeta should be non-zero");
            let mut oracle_result_idxs: Vec<usize> = Vec::new();
            let mut oracle_expr_coeffs: Vec<Fr> = Vec::new();
            let mut flat_commit_idx = 0usize;
            for prod in &psl.0 {
                let result_idx = flat_commit_idx + prod.intermediates.len() - 1;
                let expr_coeff = prod.coefficient * rlc_beta_inv;
                oracle_result_idxs.push(result_idx);
                oracle_expr_coeffs.push(expr_coeff);
                flat_commit_idx += prod.intermediates.len();
            }

            // Extract atom-to-commitment index mapping
            // Use get_claims_from_product to determine actual atom count per product
            let mut atom_commit_idxs: Vec<usize> = Vec::new();
            {
                let mut fidx = 0usize;
                for prod in &psl.0 {
                    let num_atoms = get_claims_from_product(prod).len();
                    for i in 0..num_atoms {
                        atom_commit_idxs.push(fidx + i);
                    }
                    fidx += prod.intermediates.len();
                }
            }

            // Extract claims (atom routing)
            let new_claims: Vec<HyraxClaim<Fr, Bn256Point>> = psl
                .0
                .iter()
                .flat_map(|p| get_claims_from_product(p))
                .collect();
            let mut atom_targets = Vec::new();
            let mut point_templates = Vec::new();
            let mut claim_data_atoms = Vec::new();

            for claim in &new_claims {
                let target_idx = *layer_id_to_idx
                    .get(&claim.to_layer_id)
                    .expect("target not found");
                atom_targets.push(target_idx);
                let template = compute_point_template(&claim.point, &bindings, &claim_points);
                point_templates.push(template.clone());
                claim_data_atoms.push(json!({
                    "target_layer": target_idx,
                    "target_layer_id": format!("{:?}", claim.to_layer_id),
                    "point": claim.point.iter().map(fr_to_hex).collect::<Vec<_>>(),
                    "point_template": template,
                }));
                claim_tracker
                    .entry(claim.to_layer_id)
                    .or_insert_with(Vec::new)
                    .push(claim.clone());
            }

            all_atom_targets.push(atom_targets);
            all_point_templates.push(point_templates);
            all_oracle_result_idxs.push(oracle_result_idxs);
            all_oracle_expr_coeffs.push(oracle_expr_coeffs.iter().map(fr_to_hex).collect());
            all_atom_commit_idxs.push(atom_commit_idxs);
            dag_layers.push(json!({
                "proof_idx": proof_idx,
                "layer_type": layer_type,
                "num_rounds": num_rounds,
                "degree": degree,
                "num_claims": layer_claims.len(),
                "num_commitments": layer_proof.commitments.len(),
                "num_pops": layer_proof.proofs_of_product.len(),
                "num_atoms": new_claims.len(),
            }));
            all_claim_data.push(json!({ "atoms": claim_data_atoms }));
        }

        // Input layer claim data
        let private_input_ids: std::collections::HashSet<_> = verifiable
            .get_private_input_layer_ids()
            .into_iter()
            .collect();
        let mut input_layers_data = Vec::new();
        for (j, il) in desc.input_layers.iter().enumerate() {
            let claims = claim_tracker.remove(&il.layer_id).unwrap_or_default();
            let is_committed = private_input_ids.contains(&il.layer_id);
            let claim_points_hex: Vec<Vec<String>> = claims
                .iter()
                .map(|c| c.point.iter().map(fr_to_hex).collect())
                .collect();
            input_layers_data.push(json!({
                "input_idx": j,
                "is_committed": is_committed,
                "num_claims": claims.len(),
                "claim_points": claim_points_hex,
            }));
            eprintln!(
                "  Input {}: committed={}, claims={}",
                j,
                is_committed,
                claims.len()
            );
        }

        // === ABI-encode ===
        let circuit_hash_raw: [u8; 32] = {
            use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
            use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
            let hash_elems = get_circuit_description_hash_as_field_elems(
                desc,
                global_verifier_circuit_description_hash_type(),
            );
            let repr1 = hash_elems[0].to_repr();
            let repr2 = hash_elems[1].to_repr();
            let mut hash = [0u8; 32];
            hash[..16].copy_from_slice(&repr1.as_ref()[..16]);
            hash[16..].copy_from_slice(&repr2.as_ref()[..16]);
            hash
        };

        let abi_bytes = crate::abi_encode::encode_hyrax_proof(&proof, &circuit_hash_raw).unwrap();
        let gens_bytes = crate::abi_encode::encode_pedersen_gens(&verifier_committer).unwrap();

        // Public inputs bytes — all public shreds: leaf_values, expected_sum, thresholds, is_real
        let mut pub_input_bytes = Vec::new();
        let encode_i64_as_fr = |v: i64| -> [u8; 32] {
            let fr_val = if v >= 0 {
                Fr::from(v as u64)
            } else {
                -Fr::from((-v) as u64)
            };
            let repr = <Fr as PrimeField>::to_repr(&fr_val);
            let mut be = [0u8; 32];
            be.copy_from_slice(repr.as_ref());
            be.reverse();
            be
        };
        for v in &inputs.leaf_values_padded {
            pub_input_bytes.extend_from_slice(&encode_i64_as_fr(*v));
        }
        pub_input_bytes.extend_from_slice(&encode_i64_as_fr(inputs.expected_sum));
        for v in &inputs.thresholds_padded {
            pub_input_bytes.extend_from_slice(&encode_i64_as_fr(*v));
        }
        for v in &inputs.is_real_padded {
            pub_input_bytes.extend_from_slice(&encode_i64_as_fr(*v));
        }

        // Flatten DAG metadata
        let mut atom_offsets: Vec<usize> = Vec::new();
        let mut flat_atom_targets: Vec<usize> = Vec::new();
        let mut pt_offsets: Vec<usize> = Vec::new();
        let mut flat_pt_data: Vec<u64> = Vec::new();
        let mut atom_offset = 0usize;
        let mut pt_offset = 0usize;
        for (targets, templates) in all_atom_targets.iter().zip(all_point_templates.iter()) {
            atom_offsets.push(atom_offset);
            for (target, template) in targets.iter().zip(templates.iter()) {
                flat_atom_targets.push(*target);
                pt_offsets.push(pt_offset);
                for entry in template {
                    flat_pt_data.push(parse_template_entry(entry));
                    pt_offset += 1;
                }
            }
            atom_offset += targets.len();
        }
        atom_offsets.push(atom_offset);
        pt_offsets.push(pt_offset);

        let layer_types_ordered: Vec<u8> = dag_layers
            .iter()
            .map(|l| l["layer_type"].as_u64().unwrap() as u8)
            .collect();
        let num_rounds_ordered: Vec<usize> = dag_layers
            .iter()
            .map(|l| l["num_rounds"].as_u64().unwrap() as usize)
            .collect();

        // Flatten oracle eval formula data
        let mut oracle_product_offsets: Vec<usize> = Vec::new();
        let mut flat_oracle_result_idxs: Vec<usize> = Vec::new();
        let mut flat_oracle_expr_coeffs: Vec<String> = Vec::new();
        let mut oracle_offset = 0usize;
        for (result_idxs, coeffs) in all_oracle_result_idxs
            .iter()
            .zip(all_oracle_expr_coeffs.iter())
        {
            oracle_product_offsets.push(oracle_offset);
            for (idx, coeff) in result_idxs.iter().zip(coeffs.iter()) {
                flat_oracle_result_idxs.push(*idx);
                flat_oracle_expr_coeffs.push(coeff.clone());
            }
            oracle_offset += result_idxs.len();
        }
        oracle_product_offsets.push(oracle_offset);

        // Flatten atom-to-commitment index mapping
        let flat_atom_commit_idxs: Vec<usize> =
            all_atom_commit_idxs.into_iter().flatten().collect();

        // Flatten incoming claims per layer (inverse of atom routing)
        let total_layers = num_compute + desc.input_layers.len();
        let mut incoming_counts: Vec<usize> = vec![0; total_layers];
        for &target in &flat_atom_targets {
            incoming_counts[target] += 1;
        }
        let mut incoming_offsets: Vec<usize> = Vec::new();
        let mut flat_incoming_atom_idx: Vec<usize> = vec![0; flat_atom_targets.len()];
        let mut write_pos: Vec<usize> = vec![0; total_layers];
        let mut offset = 0usize;
        for i in 0..total_layers {
            incoming_offsets.push(offset);
            offset += incoming_counts[i];
        }
        incoming_offsets.push(offset);
        // Build inverse mapping
        for (atom_idx, &target) in flat_atom_targets.iter().enumerate() {
            let pos = incoming_offsets[target] + write_pos[target];
            flat_incoming_atom_idx[pos] = atom_idx;
            write_pos[target] += 1;
        }

        let fixture = json!({
            "proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
            "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
            "circuit_hash_raw": format!("0x{}", hex::encode(&circuit_hash_raw)),
            "public_inputs_hex": format!("0x{}", hex::encode(&pub_input_bytes)),
            "dag_circuit_description": {
                "numComputeLayers": num_compute,
                "numInputLayers": desc.input_layers.len(),
                "layerTypes": layer_types_ordered,
                "numSumcheckRounds": num_rounds_ordered,
                "atomOffsets": atom_offsets,
                "atomTargetLayers": flat_atom_targets,
                "ptOffsets": pt_offsets,
                "ptData": flat_pt_data,
                "inputIsCommitted": desc.input_layers.iter()
                    .map(|il| private_input_ids.contains(&il.layer_id))
                    .collect::<Vec<_>>(),
                "oracleProductOffsets": oracle_product_offsets,
                "oracleResultIdxs": flat_oracle_result_idxs,
                "oracleExprCoeffs": flat_oracle_expr_coeffs,
                "atomCommitIdxs": flat_atom_commit_idxs,
                "incomingOffsets": incoming_offsets,
                "incomingAtomIdx": flat_incoming_atom_idx,
            },
            "dag_layers": dag_layers,
            "claim_routing": all_claim_data,
            "input_layers": input_layers_data,
        });

        // Write to fixture file
        let fixture_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("contracts/test/fixtures/phase1a_dag_fixture.json");
        std::fs::write(
            &fixture_path,
            serde_json::to_string_pretty(&fixture).unwrap(),
        )
        .unwrap();
        eprintln!("Fixture written to: {}", fixture_path.display());
    }
}
