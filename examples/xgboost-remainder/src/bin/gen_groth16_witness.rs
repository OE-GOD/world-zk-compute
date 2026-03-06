//! Generate witness JSON for the Groth16 wrapper circuit.
//!
//! Runs the GKR+Hyrax proof, replays the ECTranscript step-by-step to extract
//! ALL Fiat-Shamir challenges and proof elements, and outputs JSON that the
//! gnark `prove` command can consume.
//!
//! All challenge values are output as Fr (scalar field) hex strings. The gnark
//! circuit operates over Fr, and the Solidity verifier reduces Fq->Fr via
//! `% FR_MODULUS`.
//!
//! Usage:
//!   cargo run --release --bin gen_groth16_witness               # num_vars=1 (default)
//!   cargo run --release --bin gen_groth16_witness -- --num-vars 4  # num_vars=4

use anyhow::Result;
use ff::PrimeField;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::layer::{get_claims_from_product, HyraxClaim};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::primitives::proof_of_sumcheck::ProofOfSumcheck;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use remainder::layer::product::{new_with_values, PostSumcheckLayer};
use remainder::layer::LayerDescription;
use serde_json::json;
use shared_types::config::{
    global_config::global_claim_agg_strategy, ClaimAggregationStrategy, GKRCircuitProverConfig,
    GKRCircuitVerifierConfig,
};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::{ECTranscript, ECTranscriptTrait};
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
    curves::PrimeOrderCurve, perform_function_under_prover_config,
    perform_function_under_verifier_config, Bn256Point, Fq, Fr,
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

/// Convert Fr to big-endian hex string
fn fr_to_hex(val: &Fr) -> String {
    let repr = val.to_repr();
    let bytes: &[u8] = repr.as_ref();
    let mut be = bytes.to_vec();
    be.reverse();
    format!("0x{}", hex::encode(&be))
}

/// Parse a Bn256Point from its serde JSON representation (compressed hex)
fn parse_point_from_json(val: &serde_json::Value) -> Bn256Point {
    serde_json::from_value(val.clone()).unwrap()
}

/// Parse an Fr scalar from its serde JSON representation (LE hex string)
fn parse_fr_from_json(val: &serde_json::Value) -> Fr {
    serde_json::from_value(val.clone()).unwrap()
}

/// Compute beta(r, c) = prod_i (r_i * c_i + (1 - r_i) * (1 - c_i)) over Fr
fn compute_beta_fr(bindings: &[Fr], claim_point: &[Fr]) -> Fr {
    let n = bindings.len().min(claim_point.len());
    let mut beta = Fr::one();
    for i in 0..n {
        let rc = bindings[i] * claim_point[i];
        let one_minus_r = Fr::one() - bindings[i];
        let one_minus_c = Fr::one() - claim_point[i];
        let term = rc + one_minus_r * one_minus_c;
        beta = beta * term;
    }
    beta
}

/// Compute tensor product of bindings: tensor([b0, b1, ...]) -> 2^n elements
fn compute_tensor_product_fr(bindings: &[Fr]) -> Vec<Fr> {
    let mut result = vec![Fr::one()];
    for b in bindings {
        let one_minus_b = Fr::one() - b;
        let new_result: Vec<Fr> = result
            .iter()
            .flat_map(|r| vec![*r * one_minus_b, *r * *b])
            .collect();
        result = new_result;
    }
    result
}

/// Evaluate multilinear extension: MLE(values, point) = sum_i values[i] * tensor(point)[i]
fn evaluate_mle_fr(values: &[Fr], point: &[Fr]) -> Fr {
    let basis = compute_tensor_product_fr(point);
    values
        .iter()
        .zip(basis.iter())
        .fold(Fr::zero(), |acc, (v, b)| acc + *v * *b)
}

/// CLI argument parsing helpers
fn parse_arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for i in 1..args.len() {
        if args[i] == name && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

fn parse_arg_usize(name: &str, default: usize) -> usize {
    parse_arg(name)
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("invalid {} value", name))
        })
        .unwrap_or(default)
}

/// Holds the circuit + metadata needed for witness generation.
struct CircuitSetup {
    prover_circuit: Circuit<Fr>,
    verifier_circuit: Circuit<Fr>,
    /// Public input values (for MLE eval in the gnark circuit)
    pub_values: Vec<Fr>,
    transcript_label: &'static str,
    pedersen_seed: &'static str,
}

fn setup_toy_circuit() -> CircuitSetup {
    let num_vars = parse_arg_usize("--num-vars", 1);
    let size = 1usize << num_vars;
    eprintln!("Toy circuit: num_vars={}, size={}", num_vars, size);

    let base_circuit = build_circuit(num_vars);
    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    let a_vals: Vec<u64> = (1..=size as u64).map(|i| i * 3).collect();
    let b_vals: Vec<u64> = (1..=size as u64).map(|i| i * 2).collect();
    let expected_vals: Vec<u64> = a_vals
        .iter()
        .zip(b_vals.iter())
        .map(|(a, b)| a * b)
        .collect();

    prover_circuit.set_input("a", a_vals.into());
    prover_circuit.set_input("b", b_vals.into());
    prover_circuit.set_input("expected", expected_vals.clone().into());

    let pub_values: Vec<Fr> = expected_vals.iter().map(|v| Fr::from(*v)).collect();

    CircuitSetup {
        prover_circuit,
        verifier_circuit,
        pub_values,
        transcript_label: "gen-test-proof transcript",
        pedersen_seed: "gen-test-proof Pedersen committer seed string for generating bases",
    }
}

fn setup_xgboost_circuit() -> CircuitSetup {
    let num_trees = parse_arg_usize("--trees", 1);
    let depth = parse_arg_usize("--depth", 1);
    let num_features = parse_arg_usize("--features", 2);
    let num_trees_padded = num_trees.next_power_of_two();

    eprintln!(
        "XGBoost circuit: trees={} (padded={}), depth={}, features={}",
        num_trees, num_trees_padded, depth, num_features
    );

    // Generate a deterministic test model
    let model = xgboost_remainder::model::generate_model(num_trees, depth, num_features);
    let features = xgboost_remainder::model::generate_features(num_features, 123);

    let inputs = xgboost_remainder::circuit::prepare_circuit_inputs(&model, &features);

    let base_circuit = xgboost_remainder::circuit::build_full_inference_circuit(
        inputs.num_trees_padded,
        inputs.max_depth,
        inputs.num_features_padded,
        &inputs.fi_padded,
        inputs.decomp_k,
    );
    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    prover_circuit.set_input("path_bits", inputs.flat_path_bits.into());
    prover_circuit.set_input("features", inputs.features_quantized.into());
    prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.into());
    prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.clone().into());
    prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
    prover_circuit.set_input("thresholds", inputs.thresholds_padded.into());
    prover_circuit.set_input("is_real", inputs.is_real_padded.into());

    // Public values = leaf_values + expected_sum (the full public input layer)
    // NOTE: The actual public input order depends on the circuit builder's shred ordering.
    // We'll extract the correct values from the proof's public_inputs field later.
    let pub_values_placeholder: Vec<Fr> = Vec::new(); // filled from proof

    CircuitSetup {
        prover_circuit,
        verifier_circuit,
        pub_values: pub_values_placeholder,
        transcript_label: "gen-test-proof transcript",
        pedersen_seed: "gen-test-proof Pedersen committer seed string for generating bases",
    }
}

// NOTE: DAG circuit witness generation is in gen_dag_groth16_witness.rs
// The generate_dag_witness stub was removed because it had unresolvable type errors.

fn main() -> Result<()> {
    let circuit_type = parse_arg("--circuit").unwrap_or_else(|| "toy".to_string());

    let mut setup = match circuit_type.as_str() {
        "toy" => setup_toy_circuit(),
        "xgboost" => setup_xgboost_circuit(),
        other => {
            eprintln!("Unknown circuit type: {}. Use 'toy' or 'xgboost'.", other);
            std::process::exit(1);
        }
    };

    let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);

    let mut provable = setup
        .prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("gen_hyrax_provable_circuit");

    let committer = PedersenCommitter::new(512, setup.pedersen_seed, None);
    let mut rng = thread_rng();
    let mut vander = VandermondeInverse::new();
    let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new(setup.transcript_label);

    eprintln!("Generating Hyrax proof...");
    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| provable.prove(w, x, y, z),
        &config,
        &committer,
        &mut rng,
        &mut vander,
        &mut transcript
    );

    // Extract public input values from the proof (correct for any circuit type)
    if setup.pub_values.is_empty() {
        // For XGBoost: extract public values from the proof's public_inputs field
        for (_layer_id, mle_opt) in &proof.public_inputs {
            if let Some(mle) = mle_opt {
                setup.pub_values = mle.f.iter().collect();
                break;
            }
        }
    }
    let pub_values = &setup.pub_values;

    // Verify proof (full verification, ensures proof is valid)
    let verifiable = setup
        .verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("gen_hyrax_verifiable_circuit");
    let verifier_committer = PedersenCommitter::new(512, setup.pedersen_seed, None);
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new(setup.transcript_label);
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

    // ================================================================
    // DAG circuits (xgboost) use gen_dag_groth16_witness binary
    // ================================================================
    if circuit_type == "xgboost" {
        anyhow::bail!("XGBoost/DAG witness generation moved to gen_dag_groth16_witness binary");
    }

    // ================================================================
    // Replay ECTranscript to extract all challenges as Fr values.
    // (Linear topology for toy circuits)
    // ================================================================

    let verifiable_ref = verifiable.get_gkr_circuit_description_ref();

    // === Initial transcript setup ===
    let mut t: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new(setup.transcript_label);
    {
        use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
        use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
        let hash_elems = get_circuit_description_hash_as_field_elems(
            verifiable_ref,
            global_verifier_circuit_description_hash_type(),
        );
        t.append_scalar_field_elems("Circuit description hash", &hash_elems);
        proof.public_inputs.iter().for_each(|(_, mle)| {
            t.append_input_scalar_field_elems(
                "Public input layer values",
                &mle.as_ref().unwrap().f.iter().collect::<Vec<_>>(),
            );
        });
        proof.hyrax_input_proofs.iter().for_each(|ip| {
            t.append_input_ec_points("Hyrax input layer commitment", ip.input_commitment.clone());
        });
        for fs_desc in &verifiable_ref.fiat_shamir_challenges {
            let num_evals = 1 << fs_desc.num_bits;
            t.get_scalar_field_challenges("Verifier challenges", num_evals);
        }
    }

    // === Output layer ===
    let mut claim_tracker: std::collections::HashMap<
        remainder::layer::LayerId,
        Vec<HyraxClaim<Fr, Bn256Point>>,
    > = std::collections::HashMap::new();
    for (_, _, olp) in &proof.circuit_proof.output_layer_proofs {
        let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
            olp,
            &verifiable_ref.output_layers[0],
            &mut t,
        );
        claim_tracker.insert(claim.to_layer_id, vec![claim]);
    }

    // === Per-layer transcript replay ===
    struct LayerExtract {
        bindings: Vec<Fr>,
        rhos: Vec<Fr>,
        gammas: Vec<Fr>,
        podp_challenge: Fr,
        pop_challenge: Option<Fr>,
        podp_z_vector: Vec<Fr>,
        podp_z_delta: Fr,
        podp_z_beta: Fr,
        j_star: Vec<Fr>,
        random_coeff: Fr,
        pop_data: Vec<serde_json::Value>,
    }

    let mut layer_extracts: Vec<LayerExtract> = Vec::new();
    let mut layer_num_vars: Vec<usize> = Vec::new();
    let mut layer_degrees: Vec<usize> = Vec::new();

    for (layer_idx, (_layer_id, layer_proof)) in proof.circuit_proof.layer_proofs.iter().enumerate()
    {
        let layer_desc = verifiable_ref
            .intermediate_layers
            .iter()
            .find(|ld| ld.layer_id() == *_layer_id)
            .unwrap();
        let layer_claims_vec = claim_tracker.remove(&layer_desc.layer_id()).unwrap();

        // RLC coefficients (from transcript squeeze)
        let random_coefficients = match global_claim_agg_strategy() {
            ClaimAggregationStrategy::RLC => {
                t.get_scalar_field_challenges("RLC Claim Agg Coefficients", layer_claims_vec.len())
            }
            _ => vec![Fr::one()],
        };
        let random_coeff = random_coefficients[0];

        let _rlc_eval = layer_claims_vec
            .iter()
            .zip(random_coefficients.iter())
            .fold(Bn256Point::zero(), |acc, (elem, rc)| {
                acc + elem.evaluation * *rc
            });

        // Absorb sumcheck messages, squeeze bindings
        let msgs = &layer_proof.proof_of_sumcheck.messages;
        let n = msgs.len();
        let num_rounds = layer_desc.sumcheck_round_indices().len();

        layer_num_vars.push(num_rounds);
        layer_degrees.push(layer_desc.max_degree());

        if num_rounds > 0 {
            t.append_ec_point("Commitment to sumcheck message", msgs[0]);
        }
        let mut bindings: Vec<Fr> = vec![];
        for msg in msgs.iter().skip(1) {
            bindings.push(t.get_scalar_field_challenge("sumcheck round challenge"));
            t.append_ec_point("Commitment to sumcheck message", *msg);
        }
        if num_rounds > 0 {
            bindings.push(t.get_scalar_field_challenge("sumcheck round challenge"));
        }

        // Absorb post-sumcheck commitments
        t.append_ec_points(
            "Commitments to all the layer's leaf values and intermediates",
            &layer_proof.commitments,
        );

        // Squeeze rhos and gammas
        let rhos = t.get_scalar_field_challenges(
            "Proof of sumcheck RLC coefficients for batching rows",
            n + 1,
        );
        let gammas = t.get_scalar_field_challenges(
            "Proof of sumcheck RLC coefficients for batching columns",
            n,
        );

        // Compute j_star
        let j_star = ProofOfSumcheck::<Bn256Point>::calculate_j_star(
            &bindings,
            &rhos,
            &gammas,
            layer_desc.max_degree(),
        );

        // Build PostSumcheckLayer for claim tracking
        let claim_points: Vec<Vec<Fr>> = layer_claims_vec.iter().map(|c| c.point.clone()).collect();
        let claim_points_refs: Vec<&[Fr]> = claim_points.iter().map(|v| v.as_slice()).collect();
        let psl_desc =
            layer_desc.get_post_sumcheck_layer(&bindings, &claim_points_refs, &random_coefficients);
        let psl: PostSumcheckLayer<Fr, Bn256Point> =
            new_with_values(&psl_desc, &layer_proof.commitments);

        // PODP — extract private fields via JSON serialization
        let podp_json = serde_json::to_value(&layer_proof.proof_of_sumcheck.podp).unwrap();
        let podp_commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
        let podp_commit_d_dot_a: Bn256Point = parse_point_from_json(&podp_json["commit_d_dot_a"]);
        let podp_z_vector: Vec<Fr> = podp_json["z_vector"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| parse_fr_from_json(v))
            .collect();
        let podp_z_delta: Fr = parse_fr_from_json(&podp_json["z_delta"]);
        let podp_z_beta: Fr = parse_fr_from_json(&podp_json["z_beta"]);

        // Replicate PODP transcript ops (matches proof_of_dot_prod.rs verify())
        t.append_ec_point("Commitment to random vector", podp_commit_d);
        t.append_ec_point(
            "Commitment to inner product of random vector and public vector",
            podp_commit_d_dot_a,
        );
        let podp_c = t.get_scalar_field_challenge("challenge c");
        t.append_scalar_field_elems("Blinded private vector", &podp_z_vector);
        t.append_scalar_field_elem(
            "Blinding factor for blinded vector commitment",
            podp_z_delta,
        );
        t.append_scalar_field_elem("Blinding factor for blinded inner product", podp_z_beta);

        // PoP — replicate transcript ops (matches proof_of_product.rs verify())
        let product_triples: Vec<(Bn256Point, Bn256Point, Bn256Point)> = psl
            .0
            .iter()
            .filter_map(|p| p.get_product_triples())
            .flatten()
            .collect();
        let mut pop_data: Vec<serde_json::Value> = Vec::new();
        let mut pop_challenge: Option<Fr> = None;
        for (_triple, pop) in product_triples
            .iter()
            .zip(layer_proof.proofs_of_product.iter())
        {
            t.append_ec_point("Commitment to random values 1", pop.alpha);
            t.append_ec_point("Commitment to random values 2", pop.beta);
            t.append_ec_point("Commitment to random values 3", pop.delta);
            let pop_c = t.get_scalar_field_challenge("PoP c");
            pop_challenge = Some(pop_c);
            t.append_scalar_field_elem("Blinded response 1", pop.z1);
            t.append_scalar_field_elem("Blinded response 2", pop.z2);
            t.append_scalar_field_elem("Blinded response 3", pop.z3);
            t.append_scalar_field_elem("Blinded response 4", pop.z4);
            t.append_scalar_field_elem("Blinded response 5", pop.z5);

            pop_data.push(json!({
                "z1": fr_to_hex(&pop.z1), "z2": fr_to_hex(&pop.z2),
                "z3": fr_to_hex(&pop.z3), "z4": fr_to_hex(&pop.z4),
                "z5": fr_to_hex(&pop.z5),
            }));
        }

        // Extract claims for next layer
        let new_claims: Vec<HyraxClaim<Fr, Bn256Point>> = psl
            .0
            .iter()
            .flat_map(|p| hyrax::gkr::layer::get_claims_from_product(p))
            .collect();
        for claim in new_claims {
            claim_tracker
                .entry(claim.to_layer_id)
                .or_insert_with(Vec::new)
                .push(claim);
        }

        eprintln!(
            "Layer {} ({:?}): degree={}, bindings={:?}, j_star_len={}, z_vec_len={}, pops={}",
            layer_idx,
            _layer_id,
            layer_desc.max_degree(),
            bindings.iter().map(|b| fr_to_hex(b)).collect::<Vec<_>>(),
            j_star.len(),
            podp_z_vector.len(),
            pop_data.len(),
        );

        layer_extracts.push(LayerExtract {
            bindings,
            rhos,
            gammas,
            podp_challenge: podp_c,
            pop_challenge,
            podp_z_vector,
            podp_z_delta,
            podp_z_beta,
            j_star,
            random_coeff,
            pop_data,
        });
    }

    // === Input layer ===
    // Squeeze input RLC coefficients (2 claims -> 2 squeezes)
    let input_rlc_coeffs = t.get_scalar_field_challenges("Input claim RLC coefficients", 2);
    let input_rlc_0 = input_rlc_coeffs[0];
    let input_rlc_1 = input_rlc_coeffs[1];

    // Absorb comEval
    let input_proof = &proof.hyrax_input_proofs[0];
    let com_eval = input_proof.evaluation_proofs[0].commitment_to_evaluation;
    t.append_ec_point("Commitment to evaluation", com_eval);

    // Input PODP — extract private fields via JSON
    let input_eval_json = serde_json::to_value(&input_proof.evaluation_proofs[0]).unwrap();
    let input_podp_json = &input_eval_json["podp_evaluation_proof"];
    let input_podp_commit_d: Bn256Point = parse_point_from_json(&input_podp_json["commit_d"]);
    let input_podp_commit_d_dot_a: Bn256Point =
        parse_point_from_json(&input_podp_json["commit_d_dot_a"]);
    let input_podp_z_vector: Vec<Fr> = input_podp_json["z_vector"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| parse_fr_from_json(v))
        .collect();
    let input_podp_z_delta: Fr = parse_fr_from_json(&input_podp_json["z_delta"]);
    let input_podp_z_beta: Fr = parse_fr_from_json(&input_podp_json["z_beta"]);

    // Replicate input PODP transcript ops
    t.append_ec_point("Commitment to random vector", input_podp_commit_d);
    t.append_ec_point(
        "Commitment to inner product of random vector and public vector",
        input_podp_commit_d_dot_a,
    );
    let input_podp_c = t.get_scalar_field_challenge("challenge c");
    t.append_scalar_field_elems("Blinded private vector", &input_podp_z_vector);
    t.append_scalar_field_elem(
        "Blinding factor for blinded vector commitment",
        input_podp_z_delta,
    );
    t.append_scalar_field_elem(
        "Blinding factor for blinded inner product",
        input_podp_z_beta,
    );

    eprintln!("Transcript replay complete.");

    // ================================================================
    // Compute public outputs (all in Fr)
    // ================================================================

    let num_layers = layer_extracts.len();
    eprintln!("num_layers={}", num_layers);

    // Extract output challenges (claim point for layer 0) by re-running output layer verify.
    let output_challenges: Vec<Fr> = {
        let mut t2: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new(setup.transcript_label);
        {
            use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
            use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
            let hash_elems = get_circuit_description_hash_as_field_elems(
                verifiable_ref,
                global_verifier_circuit_description_hash_type(),
            );
            t2.append_scalar_field_elems("Circuit description hash", &hash_elems);
            proof.public_inputs.iter().for_each(|(_, mle)| {
                t2.append_input_scalar_field_elems(
                    "Public input layer values",
                    &mle.as_ref().unwrap().f.iter().collect::<Vec<_>>(),
                );
            });
            proof.hyrax_input_proofs.iter().for_each(|ip| {
                t2.append_input_ec_points(
                    "Hyrax input layer commitment",
                    ip.input_commitment.clone(),
                );
            });
            for fs_desc in &verifiable_ref.fiat_shamir_challenges {
                let num_evals = 1 << fs_desc.num_bits;
                t2.get_scalar_field_challenges("Verifier challenges", num_evals);
            }
        }
        let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
            &proof.circuit_proof.output_layer_proofs[0].2,
            &verifiable_ref.output_layers[0],
            &mut t2,
        );
        claim.point.clone() // Vec<Fr> with num_vars elements
    };

    eprintln!(
        "output_challenges: {:?}",
        output_challenges
            .iter()
            .map(|c| fr_to_hex(c))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "claim_agg_coeff: {}",
        fr_to_hex(&layer_extracts[0].random_coeff)
    );

    // Per-layer: rlc_beta and z_dot_jstar
    let mut rlc_betas: Vec<Fr> = Vec::new();
    let mut z_dot_jstars: Vec<Fr> = Vec::new();
    for i in 0..num_layers {
        let le = &layer_extracts[i];
        // rlc_beta: beta(bindings, claim_point) * coeff
        let (claim_point, coeff) = if i == 0 {
            (output_challenges.as_slice(), le.random_coeff)
        } else {
            (layer_extracts[i - 1].bindings.as_slice(), le.random_coeff)
        };
        let rlc_beta = compute_beta_fr(&le.bindings, claim_point) * coeff;
        rlc_betas.push(rlc_beta);

        // z_dot_jstar = <z_vector, j_star>
        let z_dot_jstar: Fr = le
            .podp_z_vector
            .iter()
            .zip(le.j_star.iter())
            .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);
        z_dot_jstars.push(z_dot_jstar);

        eprintln!(
            "  layer {}: rlc_beta={}, z_dot_jstar={}",
            i,
            fr_to_hex(&rlc_beta),
            fr_to_hex(&z_dot_jstar)
        );
    }

    // Inter-layer coefficients (L-1 values: layer_extracts[1..].random_coeff)
    let inter_layer_coeffs: Vec<Fr> = layer_extracts[1..]
        .iter()
        .map(|le| le.random_coeff)
        .collect();
    for (i, c) in inter_layer_coeffs.iter().enumerate() {
        eprintln!("  inter_layer_coeff[{}]: {}", i, fr_to_hex(c));
    }

    // Input claim point: the Hyrax PODP evaluation uses the per-shred dimensions.
    // For linear circuits, this equals the last compute layer's bindings.
    // Note: the total input layer claim may include a shred selector bit,
    // but the L/R tensor split and PODP use only the per-shred evaluation point.
    let input_claim_point: Vec<Fr> = layer_extracts[num_layers - 1].bindings.clone();
    let input_num_vars = input_claim_point.len();
    let left_dims = input_num_vars / 2;
    eprintln!(
        "input_claim_point (len={}): {:?}",
        input_num_vars,
        input_claim_point
            .iter()
            .map(|p| fr_to_hex(p))
            .collect::<Vec<_>>()
    );

    // L-tensor: for each shred, scale tensor(left_bindings) by the RLC coefficient
    let left_bindings = &input_claim_point[..left_dims];
    let l_per_shred = compute_tensor_product_fr(left_bindings); // 2^left_dims elements
    let mut l_tensor: Vec<Fr> = Vec::new();
    for coeff in &[input_rlc_0, input_rlc_1] {
        for t in &l_per_shred {
            l_tensor.push(*coeff * *t);
        }
    }

    // R-tensor: tensor(right_bindings) -> 2^right_dims elements
    let right_bindings = &input_claim_point[left_dims..];
    let r_tensor = compute_tensor_product_fr(right_bindings);

    // z_dot_r = <input_z, R_tensor>
    let z_dot_r: Fr = input_podp_z_vector
        .iter()
        .zip(r_tensor.iter())
        .fold(Fr::zero(), |acc, (z, r)| acc + *z * *r);

    // MLE eval: extract the actual claim point for the public input layer from claim_tracker.
    // After all intermediate layers, claim_tracker has claims for input layers.
    // The public input claim's point is the MLE eval point.
    let mle_eval_point: Vec<Fr> = {
        let mut found_point: Option<Vec<Fr>> = None;
        for (_lid, claims) in &claim_tracker {
            for claim in claims {
                // Look for a claim whose point length matches log2(pub_input_count)
                let expected_len = (pub_values.len() as f64).log2() as usize;
                if claim.point.len() == expected_len {
                    found_point = Some(claim.point.clone());
                    break;
                }
            }
            if found_point.is_some() {
                break;
            }
        }
        // Fallback: use first layer's bindings (for toy circuits where they match)
        found_point.unwrap_or_else(|| layer_extracts[0].bindings.clone())
    };
    eprintln!(
        "mle_eval_point (len={}): {:?}",
        mle_eval_point.len(),
        mle_eval_point
            .iter()
            .map(|p| fr_to_hex(p))
            .collect::<Vec<_>>()
    );
    let mle_eval = evaluate_mle_fr(pub_values, &mle_eval_point);

    eprintln!("z_dot_r: {}", fr_to_hex(&z_dot_r));
    eprintln!("mle_eval: {}", fr_to_hex(&mle_eval));

    // ================================================================
    // ABI-encode inner proof and generators for on-chain verification
    // ================================================================

    use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
    use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
    let hash_elems = get_circuit_description_hash_as_field_elems(
        verifiable_ref,
        global_verifier_circuit_description_hash_type(),
    );
    let circuit_hash: [u8; 32] = {
        let repr0 = hash_elems[0].to_repr();
        let repr1 = hash_elems[1].to_repr();
        let mut hash = [0u8; 32];
        hash[..16].copy_from_slice(&repr0.as_ref()[..16]);
        hash[16..].copy_from_slice(&repr1.as_ref()[..16]);
        hash
    };

    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash)?;
    eprintln!(
        "ABI-encoded proof: {} bytes ({} uint256 slots)",
        abi_bytes.len(),
        (abi_bytes.len() - 4) / 32
    );

    let gens_bytes = abi_encode::encode_pedersen_gens(&committer)?;
    eprintln!(
        "ABI-encoded generators: {} bytes ({} uint256 slots)",
        gens_bytes.len(),
        gens_bytes.len() / 32
    );

    // Encode public input values as flat big-endian bytes
    let pub_values_bytes = {
        let mut buf = Vec::new();
        for val in pub_values {
            let repr = val.to_repr();
            let bytes: &[u8] = repr.as_ref();
            let mut be = [0u8; 32];
            be.copy_from_slice(bytes);
            be.reverse();
            buf.extend_from_slice(&be);
        }
        buf
    };

    // ================================================================
    // Build JSON output (new parameterized format)
    // ================================================================
    let circuit_hash_0_fr = {
        let mut repr = <Fr as PrimeField>::Repr::default();
        repr.as_mut()
            .copy_from_slice(hash_elems[0].to_repr().as_ref());
        Fr::from_repr(repr).unwrap()
    };
    let circuit_hash_1_fr = {
        let mut repr = <Fr as PrimeField>::Repr::default();
        repr.as_mut()
            .copy_from_slice(hash_elems[1].to_repr().as_ref());
        Fr::from_repr(repr).unwrap()
    };
    let circuit_hash_0 = fr_to_hex(&circuit_hash_0_fr);
    let circuit_hash_1 = fr_to_hex(&circuit_hash_1_fr);

    // Build layers array
    let layers: Vec<serde_json::Value> = layer_extracts
        .iter()
        .map(|le| {
            let mut layer = json!({
                "bindings": le.bindings.iter().map(|b| fr_to_hex(b)).collect::<Vec<_>>(),
                "rhos": le.rhos.iter().map(|r| fr_to_hex(r)).collect::<Vec<_>>(),
                "gammas": le.gammas.iter().map(|g| fr_to_hex(g)).collect::<Vec<_>>(),
                "podp_challenge": fr_to_hex(&le.podp_challenge),
            });
            // Include pop_challenge for layers that have PoP data (degree > 2)
            if let Some(ref pop_c) = le.pop_challenge {
                layer["pop_challenge"] = json!(fr_to_hex(pop_c));
            }
            layer
        })
        .collect();

    // Build layer_podps array
    let layer_podps: Vec<serde_json::Value> = layer_extracts
        .iter()
        .map(|le| {
            json!({
                "z_vector": le.podp_z_vector.iter().map(|z| fr_to_hex(z)).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&le.podp_z_delta),
                "z_beta": fr_to_hex(&le.podp_z_beta),
            })
        })
        .collect();

    // Build layer_pops array (only layers with PoP data)
    let layer_pops: Vec<serde_json::Value> = layer_extracts
        .iter()
        .flat_map(|le| {
            if le.pop_data.is_empty() {
                vec![json!({"z1": "0x0", "z2": "0x0", "z3": "0x0", "z4": "0x0", "z5": "0x0"})]
            } else {
                le.pop_data.clone()
            }
        })
        .collect();

    let output = json!({
        "config": {
            "num_vars": if layer_num_vars.iter().all(|&nv| nv == layer_num_vars[0]) { layer_num_vars[0] } else { 0 },
            "num_layers": num_layers,
            "layer_num_vars": layer_num_vars,
            "layer_degrees": layer_degrees,
            "output_num_vars": output_challenges.len(),
            "pub_input_count": pub_values.len(),
            "mle_eval_num_vars": mle_eval_point.len(),
            "input_num_vars": input_num_vars,
        },
        "public_inputs": {
            "circuit_hash": [circuit_hash_0, circuit_hash_1],
            "public_values": pub_values.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "output_challenges": output_challenges.iter().map(|c| fr_to_hex(c)).collect::<Vec<_>>(),
            "claim_agg_coeff": fr_to_hex(&layer_extracts[0].random_coeff),
            "inter_layer_coeffs": inter_layer_coeffs.iter().map(|c| fr_to_hex(c)).collect::<Vec<_>>(),
            // Backward compat: single inter_layer_coeff for 2-layer case
            "inter_layer_coeff": if num_layers >= 2 { fr_to_hex(&layer_extracts[1].random_coeff) } else { "0x0".to_string() },
            "layers": layers,
            "input_rlc_coeffs": [fr_to_hex(&input_rlc_0), fr_to_hex(&input_rlc_1)],
            "input_podp_challenge": fr_to_hex(&input_podp_c),
            "mle_eval_point": mle_eval_point.iter().map(|p| fr_to_hex(p)).collect::<Vec<_>>(),
            "input_claim_point": input_claim_point.iter().map(|p| fr_to_hex(p)).collect::<Vec<_>>(),
        },
        "public_outputs": {
            "rlc_betas": rlc_betas.iter().map(|v| fr_to_hex(v)).collect::<Vec<_>>(),
            "z_dot_jstars": z_dot_jstars.iter().map(|v| fr_to_hex(v)).collect::<Vec<_>>(),
            "l_tensor": l_tensor.iter().map(|v| fr_to_hex(v)).collect::<Vec<_>>(),
            "z_dot_r": fr_to_hex(&z_dot_r),
            "mle_eval": fr_to_hex(&mle_eval),
        },
        "witness": {
            "layer_podps": layer_podps,
            "layer_pops": layer_pops,
            "input_podp": {
                "z_vector": input_podp_z_vector.iter().map(|z| fr_to_hex(z)).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&input_podp_z_delta),
                "z_beta": fr_to_hex(&input_podp_z_beta),
            },
        },
        // Extra fields for on-chain hybrid verification
        "inner_proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "circuit_hash_raw": format!("0x{}", hex::encode(&circuit_hash)),
        "public_values_abi": format!("0x{}", hex::encode(&pub_values_bytes)),
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}
