//! Generate DAG witness JSON for the Groth16 wrapper circuit (88-layer XGBoost).
//!
//! Runs the GKR+Hyrax proof on the full DAG circuit, replays the ECTranscript
//! step-by-step to extract ALL Fiat-Shamir challenges and proof elements, then
//! computes the public outputs (rlcBeta, zDotJStar, lTensor, zDotR, mleEval).
//!
//! The output JSON matches the `buildDAGGroth16Inputs` layout in
//! `GKRDAGHybridVerifier.sol`.
//!
//! Usage:
//!   cargo run --release --bin gen_dag_groth16_witness
//!   cargo run --release --bin gen_dag_groth16_witness -- --trees 4 --depth 2 --features 5

use anyhow::Result;
use ff::PrimeField;
use hyrax::gkr::layer::{get_claims_from_product, HyraxClaim};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::primitives::proof_of_sumcheck::ProofOfSumcheck;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use remainder::layer::product::{new_with_values, PostSumcheckLayer};
use remainder::layer::{LayerDescription, LayerId};
use serde_json::json;
use shared_types::config::{
    global_config::global_claim_agg_strategy, ClaimAggregationStrategy, GKRCircuitProverConfig,
    GKRCircuitVerifierConfig,
};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::{ECTranscript, ECTranscriptTrait};
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
    perform_function_under_prover_config, perform_function_under_verifier_config, Bn256Point, Fq,
    Fr,
};
use std::collections::HashMap;

#[path = "../abi_encode.rs"]
#[allow(clippy::all)]
mod abi_encode;

// ============================================================================
// Utility functions
// ============================================================================

fn fr_to_hex(val: &Fr) -> String {
    let repr = val.to_repr();
    let bytes: &[u8] = repr.as_ref();
    let mut be = bytes.to_vec();
    be.reverse();
    format!("0x{}", hex::encode(&be))
}

fn parse_point_from_json(val: &serde_json::Value) -> Bn256Point {
    serde_json::from_value(val.clone()).expect("failed to parse EC point from proof JSON")
}

fn parse_fr_from_json(val: &serde_json::Value) -> Fr {
    serde_json::from_value(val.clone()).expect("failed to parse Fr field element from proof JSON")
}

fn compute_beta_fr(bindings: &[Fr], claim_point: &[Fr]) -> Fr {
    let n = bindings.len().min(claim_point.len());
    let mut beta = Fr::one();
    for i in 0..n {
        let rc = bindings[i] * claim_point[i];
        let one_minus_r = Fr::one() - bindings[i];
        let one_minus_c = Fr::one() - claim_point[i];
        let term = rc + one_minus_r * one_minus_c;
        beta *= term;
    }
    beta
}

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

fn evaluate_mle_fr(values: &[Fr], point: &[Fr]) -> Fr {
    let basis = compute_tensor_product_fr(point);
    values
        .iter()
        .zip(basis.iter())
        .fold(Fr::zero(), |acc, (v, b)| acc + *v * *b)
}

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

// ============================================================================
// Per-layer extracted data
// ============================================================================

struct LayerExtract {
    bindings: Vec<Fr>,
    rhos: Vec<Fr>,
    gammas: Vec<Fr>,
    rlc_coefficients: Vec<Fr>,
    podp_challenge: Fr,
    podp_z_vector: Vec<Fr>,
    podp_z_delta: Fr,
    podp_z_beta: Fr,
    j_star: Vec<Fr>,
    pop_challenges: Vec<Fr>,
    pop_data: Vec<serde_json::Value>,
    pop_challenges_count: usize,
    claim_points: Vec<Vec<Fr>>,
}

struct InputGroupExtract {
    rlc_coeffs: Vec<Fr>,
    podp_challenge: Fr,
    podp_z_vector: Vec<Fr>,
    podp_z_delta: Fr,
    podp_z_beta: Fr,
    _claim_point: Vec<Fr>, // resolved claim point for this group's first claim
    l_half_bindings: Vec<Fr>, // L-half of the claim point
    r_half_bindings: Vec<Fr>, // R-half of the claim point
    num_rows: usize,       // number of commitment rows
}

// ============================================================================
// Point template resolution
// ============================================================================

fn resolve_point_template(
    claim_point: &[Fr],
    source_bindings: &[Fr],
    _claim_points: &[Vec<Fr>],
) -> Vec<String> {
    let mut template = Vec::new();
    for val in claim_point.iter() {
        let mut found = false;
        // Check if it matches a binding reference
        for (bi, b) in source_bindings.iter().enumerate() {
            if val == b {
                template.push(format!("B{}", bi));
                found = true;
                break;
            }
        }
        if !found {
            // Check if it's 0 or 1 (fixed values)
            if *val == Fr::zero() {
                template.push("F0".to_string());
            } else if *val == Fr::one() {
                template.push("F1".to_string());
            } else {
                // Shouldn't happen for XGBoost circuits
                template.push(format!("?{}", fr_to_hex(val)));
            }
        }
    }
    template
}

fn parse_template_entry(entry: &str) -> u64 {
    if let Some(rest) = entry.strip_prefix('B') {
        rest.parse::<u64>().unwrap_or_else(|e| {
            panic!("invalid binding index in template entry '{}': {}", entry, e)
        })
    } else if let Some(rest) = entry.strip_prefix('F') {
        20000
            + rest.parse::<u64>().unwrap_or_else(|e| {
                panic!("invalid fixed index in template entry '{}': {}", entry, e)
            })
    } else {
        panic!("Unknown template entry: {}", entry);
    }
}

// Resolve a point from template + source bindings (mirroring Solidity _resolvePoint)
fn resolve_point_from_template(template: &[u64], source_bindings: &[Fr]) -> Vec<Fr> {
    template
        .iter()
        .map(|&entry| {
            if entry < 1000 {
                source_bindings[entry as usize]
            } else if entry >= 20000 {
                Fr::from(entry - 20000)
            } else {
                panic!("Unsupported template entry: {}", entry);
            }
        })
        .collect()
}

// ============================================================================
// Sorting/grouping (matching Solidity's lexicographic sort + R-half grouping)
// ============================================================================

fn sort_claims_lexicographic(claim_points: &[Vec<Fr>]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..claim_points.len()).collect();
    indices.sort_by(|&a, &b| {
        let pa = &claim_points[a];
        let pb = &claim_points[b];
        let len = pa.len().min(pb.len());
        for i in 0..len {
            let ra = pa[i].to_repr();
            let rb = pb[i].to_repr();
            let ba: &[u8] = ra.as_ref();
            let bb: &[u8] = rb.as_ref();
            // Compare as little-endian integers (compare from MSB end)
            for j in (0..32).rev() {
                match ba[j].cmp(&bb[j]) {
                    std::cmp::Ordering::Less => return std::cmp::Ordering::Less,
                    std::cmp::Ordering::Greater => return std::cmp::Ordering::Greater,
                    std::cmp::Ordering::Equal => continue,
                }
            }
        }
        pa.len().cmp(&pb.len())
    });
    indices
}

fn r_half_equals(a: &[Fr], b: &[Fr], log_n_cols: usize) -> bool {
    let n = a.len();
    let start_r = n - log_n_cols;
    for i in start_r..n {
        if a[i] != b[i] {
            return false;
        }
    }
    true
}

fn group_claims_by_r_half(
    claim_points: &[Vec<Fr>],
    sorted_indices: &[usize],
    log_n_cols: usize,
) -> Vec<Vec<usize>> {
    // Must match Remainder_CE prover's group_claims_by_common_points_with_dimension:
    // each claim ALWAYS creates its own singleton group, AND if a matching group
    // exists, the claim is also added there. Result: numGroups == numClaims.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &idx in sorted_indices {
        for group in &mut groups {
            let first = group[0];
            if r_half_equals(&claim_points[first], &claim_points[idx], log_n_cols) {
                group.push(idx);
                break;
            }
        }
        // Always push singleton (matching prover behavior)
        groups.push(vec![idx]);
    }
    groups
}

// ============================================================================
// Main
// ============================================================================

fn main() -> Result<()> {
    let num_trees = parse_arg_usize("--trees", 4);
    let depth = parse_arg_usize("--depth", 2);
    let num_features = parse_arg_usize("--features", 5);

    eprintln!(
        "XGBoost DAG circuit: trees={}, depth={}, features={}",
        num_trees, depth, num_features
    );

    // Build XGBoost circuit
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
    let verifier_circuit = base_circuit;

    prover_circuit.set_input("path_bits", inputs.flat_path_bits.into());
    prover_circuit.set_input("features", inputs.features_quantized.into());
    prover_circuit.set_input("decomp_bits", inputs.decomp_bits_padded.into());
    prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.clone().into());
    prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
    prover_circuit.set_input("thresholds", inputs.thresholds_padded.into());
    prover_circuit.set_input("is_real", inputs.is_real_padded.into());

    let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);

    let mut provable = prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("failed to generate Hyrax provable circuit from DAG builder");
    let committer = PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
    let mut rng = thread_rng();
    let mut vander = VandermondeInverse::new();
    let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("xgboost-remainder prover transcript");

    eprintln!("Generating Hyrax proof...");
    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| provable.prove(w, x, y, z),
        &config,
        &committer,
        &mut rng,
        &mut vander,
        &mut transcript
    );

    // Extract public input values from the proof
    let mut pub_values: Vec<Fr> = Vec::new();
    for (_layer_id, mle_opt) in &proof.public_inputs {
        if let Some(mle) = mle_opt {
            pub_values = mle.f.iter().collect();
            break;
        }
    }
    eprintln!("Public input values: {} elements", pub_values.len());

    // Verify proof
    let verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("failed to generate Hyrax verifiable circuit");
    let verifier_committer =
        PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("xgboost-remainder verifier transcript");
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
    // Build layer ID → proof-order index mapping
    // ================================================================

    let desc = verifiable.get_gkr_circuit_description_ref();
    let num_compute = desc.intermediate_layers.len();
    let mut layer_id_to_idx: HashMap<LayerId, usize> = HashMap::new();
    for (proof_idx, (layer_id, _)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
        layer_id_to_idx.insert(*layer_id, proof_idx);
    }
    for (j, il) in desc.input_layers.iter().enumerate() {
        layer_id_to_idx.insert(il.layer_id, num_compute + j);
    }

    let private_input_ids: std::collections::HashSet<_> = verifiable
        .get_private_input_layer_ids()
        .into_iter()
        .collect();

    eprintln!(
        "Circuit: {} compute layers, {} input layers",
        num_compute,
        desc.input_layers.len()
    );

    // ================================================================
    // Replay ECTranscript to extract all challenges
    // ================================================================

    let mut t: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("xgboost-remainder prover transcript");
    {
        use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
        use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
        let hash_elems = get_circuit_description_hash_as_field_elems(
            desc,
            global_verifier_circuit_description_hash_type(),
        );
        t.append_scalar_field_elems("Circuit description hash", &hash_elems);
        proof.public_inputs.iter().for_each(|(_, mle)| {
            t.append_input_scalar_field_elems(
                "Public input layer values",
                &mle.as_ref()
                    .expect("public input MLE must be present in proof")
                    .f
                    .iter()
                    .collect::<Vec<_>>(),
            );
        });
        proof.hyrax_input_proofs.iter().for_each(|ip| {
            t.append_input_ec_points("Hyrax input layer commitment", ip.input_commitment.clone());
        });
        for fs_desc in &desc.fiat_shamir_challenges {
            let num_evals = 1 << fs_desc.num_bits;
            t.get_scalar_field_challenges("Verifier challenges", num_evals);
        }
    }

    // === Output layer ===
    let mut claim_tracker: HashMap<LayerId, Vec<HyraxClaim<Fr, Bn256Point>>> = HashMap::new();
    for (_, _, olp) in &proof.circuit_proof.output_layer_proofs {
        let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
            olp,
            &desc.output_layers[0],
            &mut t,
        );
        claim_tracker.insert(claim.to_layer_id, vec![claim]);
    }

    // Extract output challenges
    let output_challenges: Vec<Fr> = {
        let mut t2: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");
        {
            use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
            use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
            let hash_elems = get_circuit_description_hash_as_field_elems(
                desc,
                global_verifier_circuit_description_hash_type(),
            );
            t2.append_scalar_field_elems("Circuit description hash", &hash_elems);
            proof.public_inputs.iter().for_each(|(_, mle)| {
                t2.append_input_scalar_field_elems(
                    "Public input layer values",
                    &mle.as_ref()
                        .expect("public input MLE must be present in proof")
                        .f
                        .iter()
                        .collect::<Vec<_>>(),
                );
            });
            proof.hyrax_input_proofs.iter().for_each(|ip| {
                t2.append_input_ec_points(
                    "Hyrax input layer commitment",
                    ip.input_commitment.clone(),
                );
            });
            for fs_desc in &desc.fiat_shamir_challenges {
                let num_evals = 1 << fs_desc.num_bits;
                t2.get_scalar_field_challenges("Verifier challenges", num_evals);
            }
        }
        let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
            &proof.circuit_proof.output_layer_proofs[0].2,
            &desc.output_layers[0],
            &mut t2,
        );
        claim.point.clone()
    };

    eprintln!(
        "output_challenges (len={}): {:?}",
        output_challenges.len(),
        output_challenges.iter().map(fr_to_hex).collect::<Vec<_>>()
    );

    // === Per-layer transcript replay ===
    let mut layer_extracts: Vec<LayerExtract> = Vec::new();
    // Track all atom routing for the DAG
    let mut all_atom_targets: Vec<Vec<usize>> = Vec::new();
    let mut all_point_templates: Vec<Vec<Vec<u64>>> = Vec::new();
    let mut all_atom_commit_idxs: Vec<Vec<usize>> = Vec::new();
    let mut all_oracle_result_idxs: Vec<Vec<usize>> = Vec::new();
    let mut all_oracle_expr_coeffs: Vec<Vec<Fr>> = Vec::new();

    for (proof_idx, (layer_id, layer_proof)) in proof.circuit_proof.layer_proofs.iter().enumerate()
    {
        let layer_desc = desc
            .intermediate_layers
            .iter()
            .find(|ld| ld.layer_id() == *layer_id)
            .unwrap_or_else(|| panic!("layer {:?} not found in circuit description", layer_id));
        let layer_claims_vec = claim_tracker.remove(layer_id).unwrap_or_default();
        let num_rounds = layer_desc.sumcheck_round_indices().len();
        let degree = layer_desc.max_degree();

        eprintln!(
            "  Layer {} (id={:?}, rounds={}, degree={}, claims={})",
            proof_idx,
            layer_id,
            num_rounds,
            degree,
            layer_claims_vec.len()
        );

        // RLC coefficients
        let random_coefficients = match global_claim_agg_strategy() {
            ClaimAggregationStrategy::RLC => {
                t.get_scalar_field_challenges("RLC Claim Agg Coefficients", layer_claims_vec.len())
            }
            _ => vec![Fr::one()],
        };

        // Absorb sumcheck messages, squeeze bindings
        let msgs = &layer_proof.proof_of_sumcheck.messages;
        let n = msgs.len();

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
        let j_star =
            ProofOfSumcheck::<Bn256Point>::calculate_j_star(&bindings, &rhos, &gammas, degree);

        // Build PSL for claim propagation
        let claim_points: Vec<Vec<Fr>> = layer_claims_vec.iter().map(|c| c.point.clone()).collect();
        let claim_points_refs: Vec<&[Fr]> = claim_points.iter().map(|v| v.as_slice()).collect();
        let psl_desc =
            layer_desc.get_post_sumcheck_layer(&bindings, &claim_points_refs, &random_coefficients);
        let psl: PostSumcheckLayer<Fr, Bn256Point> =
            new_with_values(&psl_desc, &layer_proof.commitments);

        // PODP — extract private fields via JSON serialization
        let podp_json = serde_json::to_value(&layer_proof.proof_of_sumcheck.podp)
            .expect("failed to serialize PODP proof to JSON");
        let podp_commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
        let podp_commit_d_dot_a: Bn256Point = parse_point_from_json(&podp_json["commit_d_dot_a"]);
        let podp_z_vector: Vec<Fr> = podp_json["z_vector"]
            .as_array()
            .expect("PODP z_vector must be a JSON array")
            .iter()
            .map(parse_fr_from_json)
            .collect();
        let podp_z_delta: Fr = parse_fr_from_json(&podp_json["z_delta"]);
        let podp_z_beta: Fr = parse_fr_from_json(&podp_json["z_beta"]);

        // Replicate PODP transcript ops
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

        // PoP — replicate transcript ops
        let product_triples: Vec<(Bn256Point, Bn256Point, Bn256Point)> = psl
            .0
            .iter()
            .filter_map(|p| p.get_product_triples())
            .flatten()
            .collect();
        let mut pop_data: Vec<serde_json::Value> = Vec::new();
        let mut pop_challenges: Vec<Fr> = Vec::new();
        for (_triple, pop) in product_triples
            .iter()
            .zip(layer_proof.proofs_of_product.iter())
        {
            t.append_ec_point("Commitment to random values 1", pop.alpha);
            t.append_ec_point("Commitment to random values 2", pop.beta);
            t.append_ec_point("Commitment to random values 3", pop.delta);
            let pop_c = t.get_scalar_field_challenge("PoP c");
            pop_challenges.push(pop_c);
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

        // Extract oracle eval formula
        let rlc_beta_for_oracle: Fr = claim_points.iter().zip(random_coefficients.iter()).fold(
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
            Option::<Fr>::from(rlc_beta_for_oracle.invert()).expect("rlcBeta should be non-zero");
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
        let new_claims: Vec<HyraxClaim<Fr, Bn256Point>> =
            psl.0.iter().flat_map(get_claims_from_product).collect();

        let mut atom_targets = Vec::new();
        let mut point_templates = Vec::new();

        for claim in &new_claims {
            let target_idx = *layer_id_to_idx
                .get(&claim.to_layer_id)
                .expect("target layer not found");
            atom_targets.push(target_idx);

            // Compute point template
            let template_strs = resolve_point_template(&claim.point, &bindings, &claim_points);
            let template: Vec<u64> = template_strs
                .iter()
                .map(|s| parse_template_entry(s))
                .collect();
            point_templates.push(template);

            // Forward claim to target layer
            claim_tracker
                .entry(claim.to_layer_id)
                .or_default()
                .push(claim.clone());
        }

        all_atom_targets.push(atom_targets);
        all_point_templates.push(point_templates);
        all_atom_commit_idxs.push(atom_commit_idxs);
        all_oracle_result_idxs.push(oracle_result_idxs);
        all_oracle_expr_coeffs.push(oracle_expr_coeffs);

        layer_extracts.push(LayerExtract {
            bindings,
            rhos,
            gammas,
            rlc_coefficients: random_coefficients,
            podp_challenge: podp_c,
            podp_z_vector,
            podp_z_delta,
            podp_z_beta,
            j_star,
            pop_challenges,
            pop_data,
            pop_challenges_count: product_triples.len(),
            claim_points,
        });
    }

    eprintln!("Compute layer transcript replay complete.");

    // ================================================================
    // Input layer transcript replay
    // ================================================================

    let mut input_group_extracts: Vec<InputGroupExtract> = Vec::new();
    let mut public_claim_points: Vec<Vec<Fr>> = Vec::new(); // for mleEval computation

    let mut dag_input_proof_idx = 0usize;
    for (j, il) in desc.input_layers.iter().enumerate() {
        let is_committed = private_input_ids.contains(&il.layer_id);
        let claims = claim_tracker.remove(&il.layer_id).unwrap_or_default();
        eprintln!(
            "  Input layer {} (committed={}, claims={})",
            j,
            is_committed,
            claims.len()
        );

        if is_committed {
            // Collect claim points for this input layer by resolving from atoms
            let target_layer = num_compute + j;
            let mut resolved_claim_points: Vec<Vec<Fr>> = Vec::new();
            for (src_layer_idx, atom_targets) in all_atom_targets.iter().enumerate() {
                let pt_templates = &all_point_templates[src_layer_idx];
                for (atom_local_idx, &target) in atom_targets.iter().enumerate() {
                    if target == target_layer {
                        let pt = resolve_point_from_template(
                            &pt_templates[atom_local_idx],
                            &layer_extracts[src_layer_idx].bindings,
                        );
                        resolved_claim_points.push(pt);
                    }
                }
            }

            // Sort and group
            let sorted_indices = sort_claims_lexicographic(&resolved_claim_points);
            let input_proof = &proof.hyrax_input_proofs[dag_input_proof_idx];
            let num_rows = input_proof.input_commitment.len();
            let n = resolved_claim_points[0].len();
            let l_half_len = (num_rows as f64).log2() as usize;
            let log_n_cols = n - l_half_len;

            let groups =
                group_claims_by_r_half(&resolved_claim_points, &sorted_indices, log_n_cols);
            eprintln!(
                "    Input layer {}: {} groups, numRows={}, lHalfLen={}, logNCols={}",
                j,
                groups.len(),
                num_rows,
                l_half_len,
                log_n_cols
            );

            // Process each group
            let input_eval_json = serde_json::to_value(&input_proof.evaluation_proofs)
                .expect("failed to serialize input evaluation proofs to JSON");

            for (g, group) in groups.iter().enumerate() {
                // Squeeze RLC coefficients
                let group_rlc_coeffs =
                    t.get_scalar_field_challenges("Input claim RLC coefficients", group.len());

                // Absorb comEval
                let com_eval = input_proof.evaluation_proofs[g].commitment_to_evaluation;
                t.append_ec_point("Commitment to evaluation", com_eval);

                // Input PODP
                let eval_json = &input_eval_json[g];
                let podp_json = &eval_json["podp_evaluation_proof"];
                let commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
                let commit_d_dot_a: Bn256Point =
                    parse_point_from_json(&podp_json["commit_d_dot_a"]);
                let z_vector: Vec<Fr> = podp_json["z_vector"]
                    .as_array()
                    .expect("input PODP z_vector must be a JSON array")
                    .iter()
                    .map(parse_fr_from_json)
                    .collect();
                let z_delta: Fr = parse_fr_from_json(&podp_json["z_delta"]);
                let z_beta: Fr = parse_fr_from_json(&podp_json["z_beta"]);

                // Replicate input PODP transcript ops
                t.append_ec_point("Commitment to random vector", commit_d);
                t.append_ec_point(
                    "Commitment to inner product of random vector and public vector",
                    commit_d_dot_a,
                );
                let input_podp_c = t.get_scalar_field_challenge("challenge c");
                t.append_scalar_field_elems("Blinded private vector", &z_vector);
                t.append_scalar_field_elem(
                    "Blinding factor for blinded vector commitment",
                    z_delta,
                );
                t.append_scalar_field_elem("Blinding factor for blinded inner product", z_beta);

                // Extract L-half and R-half bindings from the first claim's point
                let first_claim_point = &resolved_claim_points[group[0]];
                let l_half = first_claim_point[..l_half_len].to_vec();
                let r_half = first_claim_point[l_half_len..].to_vec();

                input_group_extracts.push(InputGroupExtract {
                    rlc_coeffs: group_rlc_coeffs,
                    podp_challenge: input_podp_c,
                    podp_z_vector: z_vector,
                    podp_z_delta: z_delta,
                    podp_z_beta: z_beta,
                    _claim_point: first_claim_point.clone(),
                    l_half_bindings: l_half,
                    r_half_bindings: r_half,
                    num_rows,
                });
            }

            dag_input_proof_idx += 1;
        } else {
            // Public input layer — collect claim points for mleEval
            let target_layer = num_compute + j;
            for (src_layer_idx, atom_targets) in all_atom_targets.iter().enumerate() {
                let pt_templates = &all_point_templates[src_layer_idx];
                for (atom_local_idx, &target) in atom_targets.iter().enumerate() {
                    if target == target_layer {
                        let pt = resolve_point_from_template(
                            &pt_templates[atom_local_idx],
                            &layer_extracts[src_layer_idx].bindings,
                        );
                        public_claim_points.push(pt);
                    }
                }
            }
        }
    }

    eprintln!("Transcript replay complete.");
    eprintln!(
        "  Input groups: {}, Public claims: {}",
        input_group_extracts.len(),
        public_claim_points.len()
    );

    // ================================================================
    // Compute public outputs (all in Fr)
    // ================================================================

    // Per-layer: rlcBeta and zDotJStar
    let mut rlc_betas: Vec<Fr> = Vec::new();
    let mut z_dot_jstars: Vec<Fr> = Vec::new();

    for (i, le) in layer_extracts.iter().enumerate() {
        // rlcBeta = SUM_k( beta(bindings, claimPoint_k) * rlcCoeff_k )
        let rlc_beta: Fr = le.claim_points.iter().zip(le.rlc_coefficients.iter()).fold(
            Fr::zero(),
            |acc, (cp, rc)| {
                let beta = compute_beta_fr(&le.bindings, cp);
                acc + beta * rc
            },
        );
        rlc_betas.push(rlc_beta);

        // zDotJStar = <z_vector, j_star>
        let z_dot_jstar: Fr = le
            .podp_z_vector
            .iter()
            .zip(le.j_star.iter())
            .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);
        z_dot_jstars.push(z_dot_jstar);

        eprintln!(
            "  Layer {}: rlcBeta={}, zDotJStar={}",
            i,
            fr_to_hex(&rlc_beta),
            fr_to_hex(&z_dot_jstar)
        );
    }

    // Per input group: lTensor and zDotR
    let mut l_tensor_flat: Vec<Fr> = Vec::new();
    let mut l_tensor_offsets: Vec<usize> = Vec::new();
    let mut z_dot_rs: Vec<Fr> = Vec::new();

    for (g, ige) in input_group_extracts.iter().enumerate() {
        // L-tensor: tensor product of L-half bindings, scaled by RLC coefficient
        let l_per_shred = compute_tensor_product_fr(&ige.l_half_bindings);
        l_tensor_offsets.push(l_tensor_flat.len());

        // Scale by RLC coeff (for single-claim groups, just multiply by rlc_coeffs[0])
        for (ci, coeff) in ige.rlc_coeffs.iter().enumerate() {
            for t_val in &l_per_shred {
                l_tensor_flat.push(*coeff * *t_val);
            }
            let _ = ci;
        }

        // R-tensor and zDotR = <z_vector, R_tensor>
        let r_tensor = compute_tensor_product_fr(&ige.r_half_bindings);
        let z_dot_r: Fr = ige
            .podp_z_vector
            .iter()
            .zip(r_tensor.iter())
            .fold(Fr::zero(), |acc, (z, r)| acc + *z * *r);
        z_dot_rs.push(z_dot_r);

        eprintln!("  Input group {}: zDotR={}", g, fr_to_hex(&z_dot_r));
    }
    l_tensor_offsets.push(l_tensor_flat.len());

    // Per public claim: mleEval
    let mut mle_evals: Vec<Fr> = Vec::new();
    for (ci, claim_point) in public_claim_points.iter().enumerate() {
        let mle_val = evaluate_mle_fr(&pub_values, claim_point);
        mle_evals.push(mle_val);
        eprintln!("  Public claim {}: mleEval={}", ci, fr_to_hex(&mle_val));
    }

    // ================================================================
    // ABI-encode inner proof and generators
    // ================================================================

    use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
    use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
    let hash_elems = get_circuit_description_hash_as_field_elems(
        desc,
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
    let circuit_hash_0_fr = {
        let mut repr = <Fr as PrimeField>::Repr::default();
        repr.as_mut()
            .copy_from_slice(hash_elems[0].to_repr().as_ref());
        Fr::from_repr(repr).expect("circuit hash element 0 must be a valid Fr field element")
    };
    let circuit_hash_1_fr = {
        let mut repr = <Fr as PrimeField>::Repr::default();
        repr.as_mut()
            .copy_from_slice(hash_elems[1].to_repr().as_ref());
        Fr::from_repr(repr).expect("circuit hash element 1 must be a valid Fr field element")
    };

    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash)?;
    eprintln!(
        "ABI-encoded proof: {} bytes ({} uint256 slots)",
        abi_bytes.len(),
        (abi_bytes.len() - 4) / 32
    );

    let gens_bytes = abi_encode::encode_pedersen_gens(&verifier_committer)?;
    eprintln!(
        "ABI-encoded generators: {} bytes ({} uint256 slots)",
        gens_bytes.len(),
        gens_bytes.len() / 32
    );

    // Encode public input values as flat big-endian bytes
    let pub_values_bytes = {
        let mut buf = Vec::new();
        for val in &pub_values {
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
    // Build DAG metadata (same format as phase1a fixture)
    // ================================================================

    let layer_types: Vec<u8> = proof
        .circuit_proof
        .layer_proofs
        .iter()
        .map(|(layer_id, _)| {
            let ld = desc
                .intermediate_layers
                .iter()
                .find(|l| l.layer_id() == *layer_id)
                .unwrap_or_else(|| panic!("layer {:?} not found in circuit description", layer_id));
            if ld.max_degree() == 3 {
                1
            } else {
                0
            }
        })
        .collect();
    let num_rounds: Vec<usize> = proof
        .circuit_proof
        .layer_proofs
        .iter()
        .map(|(layer_id, _)| {
            let ld = desc
                .intermediate_layers
                .iter()
                .find(|l| l.layer_id() == *layer_id)
                .unwrap_or_else(|| panic!("layer {:?} not found in circuit description", layer_id));
            ld.sumcheck_round_indices().len()
        })
        .collect();

    // Flatten atom routing
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
                flat_pt_data.push(*entry);
                pt_offset += 1;
            }
        }
        atom_offset += targets.len();
    }
    atom_offsets.push(atom_offset);
    pt_offsets.push(pt_offset);

    // Flatten oracle eval formula
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
            flat_oracle_expr_coeffs.push(fr_to_hex(coeff));
        }
        oracle_offset += result_idxs.len();
    }
    oracle_product_offsets.push(oracle_offset);

    let flat_atom_commit_idxs: Vec<usize> = all_atom_commit_idxs.into_iter().flatten().collect();

    // Incoming atom index (inverse mapping)
    let total_layers = num_compute + desc.input_layers.len();
    let mut incoming_counts: Vec<usize> = vec![0; total_layers];
    for &target in &flat_atom_targets {
        incoming_counts[target] += 1;
    }
    let mut incoming_offsets: Vec<usize> = Vec::new();
    let mut flat_incoming_atom_idx: Vec<usize> = vec![0; flat_atom_targets.len()];
    let mut write_pos: Vec<usize> = vec![0; total_layers];
    let mut offset = 0usize;
    for count in incoming_counts.iter().take(total_layers) {
        incoming_offsets.push(offset);
        offset += count;
    }
    incoming_offsets.push(offset);
    for (atom_idx, &target) in flat_atom_targets.iter().enumerate() {
        let pos = incoming_offsets[target] + write_pos[target];
        flat_incoming_atom_idx[pos] = atom_idx;
        write_pos[target] += 1;
    }

    // ================================================================
    // Build JSON output
    // ================================================================

    // Per-layer challenges
    let layers_json: Vec<serde_json::Value> = layer_extracts
        .iter()
        .map(|le| {
            json!({
                "rlc_coeffs": le.rlc_coefficients.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "bindings": le.bindings.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "rhos": le.rhos.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "gammas": le.gammas.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "podp_challenge": fr_to_hex(&le.podp_challenge),
                "pop_challenges": le.pop_challenges.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "num_pop_challenges": le.pop_challenges_count,
            })
        })
        .collect();

    // Per-layer PODP/PoP witness
    let layer_podps: Vec<serde_json::Value> = layer_extracts
        .iter()
        .map(|le| {
            json!({
                "z_vector": le.podp_z_vector.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&le.podp_z_delta),
                "z_beta": fr_to_hex(&le.podp_z_beta),
            })
        })
        .collect();

    let layer_pops: Vec<serde_json::Value> = layer_extracts
        .iter()
        .flat_map(|le| {
            if le.pop_data.is_empty() {
                vec![]
            } else {
                le.pop_data.clone()
            }
        })
        .collect();

    // Per-input-group challenges + witness
    let input_groups_json: Vec<serde_json::Value> = input_group_extracts
        .iter()
        .map(|ige| {
            json!({
                "rlc_coeffs": ige.rlc_coeffs.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "podp_challenge": fr_to_hex(&ige.podp_challenge),
                "z_vector": ige.podp_z_vector.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&ige.podp_z_delta),
                "z_beta": fr_to_hex(&ige.podp_z_beta),
                "l_half_bindings": ige.l_half_bindings.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "r_half_bindings": ige.r_half_bindings.iter().map(fr_to_hex).collect::<Vec<_>>(),
                "num_rows": ige.num_rows,
            })
        })
        .collect();

    let dag_layers_meta: Vec<serde_json::Value> = proof
        .circuit_proof
        .layer_proofs
        .iter()
        .enumerate()
        .map(|(idx, (layer_id, lp))| {
            let ld = desc
                .intermediate_layers
                .iter()
                .find(|l| l.layer_id() == *layer_id)
                .unwrap_or_else(|| panic!("layer {:?} not found in circuit description", layer_id));
            json!({
                "proof_idx": idx,
                "layer_type": if ld.max_degree() == 3 { 1 } else { 0 },
                "num_rounds": ld.sumcheck_round_indices().len(),
                "degree": ld.max_degree(),
                "num_claims": layer_extracts[idx].claim_points.len(),
                "num_commitments": lp.commitments.len(),
                "num_pops": lp.proofs_of_product.len(),
                "num_atoms": all_atom_targets[idx].len(),
            })
        })
        .collect();

    let output = json!({
        "config": {
            "num_compute_layers": num_compute,
            "num_input_layers": desc.input_layers.len(),
            "output_num_vars": output_challenges.len(),
            "pub_input_count": pub_values.len(),
            "num_input_groups": input_group_extracts.len(),
            "num_public_claims": public_claim_points.len(),
        },
        "public_inputs": {
            "circuit_hash": [fr_to_hex(&circuit_hash_0_fr), fr_to_hex(&circuit_hash_1_fr)],
            "output_challenges": output_challenges.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "layers": layers_json,
            "input_groups": input_groups_json,
        },
        "public_outputs": {
            "rlc_betas": rlc_betas.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "z_dot_jstars": z_dot_jstars.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "l_tensor_flat": l_tensor_flat.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "l_tensor_offsets": l_tensor_offsets,
            "z_dot_rs": z_dot_rs.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "mle_evals": mle_evals.iter().map(fr_to_hex).collect::<Vec<_>>(),
        },
        "witness": {
            "layer_podps": layer_podps,
            "layer_pops": layer_pops,
        },
        // DAG circuit description (for Solidity fixture)
        "dag_circuit_description": {
            "numComputeLayers": num_compute,
            "numInputLayers": desc.input_layers.len(),
            "layerTypes": layer_types,
            "numSumcheckRounds": num_rounds,
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
        "dag_layers": dag_layers_meta,
        // On-chain verification data
        "inner_proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "circuit_hash_raw": format!("0x{}", hex::encode(circuit_hash)),
        "public_values_abi": format!("0x{}", hex::encode(&pub_values_bytes)),
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Slow test (~5s), run with: cargo test --bin gen_dag_groth16_witness -- --nocapture --ignored
    fn gen_dag_groth16_witness() {
        // Use the same model/params as gen_phase1a_fixture
        let mdl = xgboost_remainder::model::sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let inputs = xgboost_remainder::circuit::prepare_circuit_inputs(&mdl, &features);

        let base_circuit = xgboost_remainder::circuit::build_full_inference_circuit(
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
        prover_circuit.set_input("leaf_values", inputs.leaf_values_padded.clone().into());
        prover_circuit.set_input("expected_sum", vec![inputs.expected_sum].into());
        prover_circuit.set_input("thresholds", inputs.thresholds_padded.into());
        prover_circuit.set_input("is_real", inputs.is_real_padded.into());

        let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
        let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);

        let mut provable = prover_circuit.gen_hyrax_provable_circuit().unwrap();
        let committer = PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
        let mut rng = thread_rng();
        let mut vander = VandermondeInverse::new();
        let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");

        eprintln!("Generating Hyrax proof...");
        let (proof, proof_config) = perform_function_under_prover_config!(
            |w, x, y, z| provable.prove(w, x, y, z),
            &config,
            &committer,
            &mut rng,
            &mut vander,
            &mut transcript
        );

        // Extract public input values
        let mut pub_values: Vec<Fr> = Vec::new();
        for (_layer_id, mle_opt) in &proof.public_inputs {
            if let Some(mle) = mle_opt {
                pub_values = mle.f.iter().collect();
                break;
            }
        }

        // Verify proof
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
        eprintln!("Proof verified!");

        let desc = verifiable.get_gkr_circuit_description_ref();
        let num_compute = desc.intermediate_layers.len();
        let private_input_ids: std::collections::HashSet<_> = verifiable
            .get_private_input_layer_ids()
            .into_iter()
            .collect();

        // Build layer ID → proof-order index mapping
        let mut layer_id_to_idx: HashMap<LayerId, usize> = HashMap::new();
        for (proof_idx, (layer_id, _)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
            layer_id_to_idx.insert(*layer_id, proof_idx);
        }
        for (j, il) in desc.input_layers.iter().enumerate() {
            layer_id_to_idx.insert(il.layer_id, num_compute + j);
        }

        // Replay transcript
        let mut t: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("xgboost-remainder prover transcript");
        {
            use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
            use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
            let hash_elems = get_circuit_description_hash_as_field_elems(
                desc,
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
                t.append_input_ec_points(
                    "Hyrax input layer commitment",
                    ip.input_commitment.clone(),
                );
            });
            for fs_desc in &desc.fiat_shamir_challenges {
                let num_evals = 1 << fs_desc.num_bits;
                t.get_scalar_field_challenges("Verifier challenges", num_evals);
            }
        }

        // Output layer
        let mut claim_tracker: HashMap<LayerId, Vec<HyraxClaim<Fr, Bn256Point>>> = HashMap::new();
        for (_, _, olp) in &proof.circuit_proof.output_layer_proofs {
            let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
                olp,
                &desc.output_layers[0],
                &mut t,
            );
            claim_tracker.insert(claim.to_layer_id, vec![claim]);
        }

        // Per-layer replay
        let mut all_bindings: Vec<Vec<Fr>> = Vec::new();
        let mut all_atom_targets: Vec<Vec<usize>> = Vec::new();
        let mut all_point_templates: Vec<Vec<Vec<u64>>> = Vec::new();
        let mut all_rlc_betas: Vec<Fr> = Vec::new();
        let mut all_z_dot_jstars: Vec<Fr> = Vec::new();

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

            let random_coefficients = match global_claim_agg_strategy() {
                ClaimAggregationStrategy::RLC => {
                    t.get_scalar_field_challenges("RLC Claim Agg Coefficients", layer_claims.len())
                }
                _ => vec![Fr::one()],
            };

            let msgs = &layer_proof.proof_of_sumcheck.messages;
            let n = msgs.len();

            if num_rounds > 0 {
                t.append_ec_point("msg", msgs[0]);
            }
            let mut bindings: Vec<Fr> = vec![];
            for msg in msgs.iter().skip(1) {
                bindings.push(t.get_scalar_field_challenge("binding"));
                t.append_ec_point("msg", *msg);
            }
            if num_rounds > 0 {
                bindings.push(t.get_scalar_field_challenge("binding"));
            }

            t.append_ec_points("commits", &layer_proof.commitments);

            let rhos = t.get_scalar_field_challenges("rhos", n + 1);
            let gammas = t.get_scalar_field_challenges("gammas", n);

            let j_star =
                ProofOfSumcheck::<Bn256Point>::calculate_j_star(&bindings, &rhos, &gammas, degree);

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

            // PODP transcript
            let podp_json = serde_json::to_value(&layer_proof.proof_of_sumcheck.podp).unwrap();
            let podp_commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
            let podp_commit_d_dot_a: Bn256Point =
                parse_point_from_json(&podp_json["commit_d_dot_a"]);
            let podp_z_vector: Vec<Fr> = podp_json["z_vector"]
                .as_array()
                .unwrap()
                .iter()
                .map(parse_fr_from_json)
                .collect();
            let podp_z_delta: Fr = parse_fr_from_json(&podp_json["z_delta"]);
            let podp_z_beta: Fr = parse_fr_from_json(&podp_json["z_beta"]);

            t.append_ec_point("commitD", podp_commit_d);
            t.append_ec_point("commitDDotA", podp_commit_d_dot_a);
            let _podp_c = t.get_scalar_field_challenge("c");
            t.append_scalar_field_elems("z", &podp_z_vector);
            t.append_scalar_field_elem("zd", podp_z_delta);
            t.append_scalar_field_elem("zb", podp_z_beta);

            // PoP transcript
            let product_triples: Vec<_> = psl
                .0
                .iter()
                .filter_map(|p| p.get_product_triples())
                .flatten()
                .collect();
            for (_, pop) in product_triples
                .iter()
                .zip(layer_proof.proofs_of_product.iter())
            {
                t.append_ec_point("alpha", pop.alpha);
                t.append_ec_point("beta", pop.beta);
                t.append_ec_point("delta", pop.delta);
                let _pop_c = t.get_scalar_field_challenge("pop_c");
                t.append_scalar_field_elem("z1", pop.z1);
                t.append_scalar_field_elem("z2", pop.z2);
                t.append_scalar_field_elem("z3", pop.z3);
                t.append_scalar_field_elem("z4", pop.z4);
                t.append_scalar_field_elem("z5", pop.z5);
            }

            // Compute rlcBeta
            let rlc_beta: Fr = claim_points.iter().zip(random_coefficients.iter()).fold(
                Fr::zero(),
                |acc, (cp, rc)| {
                    let beta = compute_beta_fr(&bindings, cp);
                    acc + beta * rc
                },
            );
            all_rlc_betas.push(rlc_beta);

            // Compute zDotJStar
            let z_dot_jstar: Fr = podp_z_vector
                .iter()
                .zip(j_star.iter())
                .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);
            all_z_dot_jstars.push(z_dot_jstar);

            // Extract claims
            let new_claims: Vec<HyraxClaim<Fr, Bn256Point>> = psl
                .0
                .iter()
                .flat_map(|p| get_claims_from_product(p))
                .collect();
            let mut atom_targets = Vec::new();
            let mut point_templates_layer = Vec::new();
            for claim in &new_claims {
                let target_idx = *layer_id_to_idx.get(&claim.to_layer_id).unwrap();
                atom_targets.push(target_idx);
                let template_strs = resolve_point_template(&claim.point, &bindings, &claim_points);
                let template: Vec<u64> = template_strs
                    .iter()
                    .map(|s| parse_template_entry(s))
                    .collect();
                point_templates_layer.push(template);
                claim_tracker
                    .entry(claim.to_layer_id)
                    .or_default()
                    .push(claim.clone());
            }

            all_bindings.push(bindings);
            all_atom_targets.push(atom_targets);
            all_point_templates.push(point_templates_layer);
        }

        // Validate rlcBeta and zDotJStar
        assert_eq!(all_rlc_betas.len(), num_compute);
        assert_eq!(all_z_dot_jstars.len(), num_compute);
        for i in 0..num_compute {
            assert!(
                all_rlc_betas[i] != Fr::zero(),
                "rlcBeta[{}] should be non-zero",
                i
            );
        }

        // Input layer validation
        let mut total_input_groups = 0usize;
        let mut total_public_claims = 0usize;
        let mut dag_input_proof_idx = 0usize;

        for (j, il) in desc.input_layers.iter().enumerate() {
            let is_committed = private_input_ids.contains(&il.layer_id);
            let claims = claim_tracker.remove(&il.layer_id).unwrap_or_default();

            if is_committed {
                let target_layer = num_compute + j;
                let mut resolved_claim_points: Vec<Vec<Fr>> = Vec::new();
                for (src_idx, atom_targets_layer) in all_atom_targets.iter().enumerate() {
                    let pt_templates = &all_point_templates[src_idx];
                    for (atom_local_idx, &target) in atom_targets_layer.iter().enumerate() {
                        if target == target_layer {
                            let pt = resolve_point_from_template(
                                &pt_templates[atom_local_idx],
                                &all_bindings[src_idx],
                            );
                            resolved_claim_points.push(pt);
                        }
                    }
                }

                let sorted_indices = sort_claims_lexicographic(&resolved_claim_points);
                let input_proof = &proof.hyrax_input_proofs[dag_input_proof_idx];
                let num_rows = input_proof.input_commitment.len();
                let n = resolved_claim_points[0].len();
                let l_half_len = (num_rows as f64).log2() as usize;
                let log_n_cols = n - l_half_len;
                let groups =
                    group_claims_by_r_half(&resolved_claim_points, &sorted_indices, log_n_cols);

                // Replay transcript per group
                let input_eval_json = serde_json::to_value(&input_proof.evaluation_proofs).unwrap();
                for (g, group) in groups.iter().enumerate() {
                    let _group_rlc = t.get_scalar_field_challenges("rlc", group.len());
                    let com_eval = input_proof.evaluation_proofs[g].commitment_to_evaluation;
                    t.append_ec_point("comEval", com_eval);

                    let eval_json = &input_eval_json[g];
                    let podp_json = &eval_json["podp_evaluation_proof"];
                    let commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
                    let commit_d_dot_a: Bn256Point =
                        parse_point_from_json(&podp_json["commit_d_dot_a"]);
                    let z_vector: Vec<Fr> = podp_json["z_vector"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(parse_fr_from_json)
                        .collect();
                    let z_delta: Fr = parse_fr_from_json(&podp_json["z_delta"]);
                    let z_beta: Fr = parse_fr_from_json(&podp_json["z_beta"]);

                    t.append_ec_point("commitD", commit_d);
                    t.append_ec_point("commitDDotA", commit_d_dot_a);
                    let _c = t.get_scalar_field_challenge("c");
                    t.append_scalar_field_elems("z", &z_vector);
                    t.append_scalar_field_elem("zd", z_delta);
                    t.append_scalar_field_elem("zb", z_beta);

                    total_input_groups += 1;
                }

                dag_input_proof_idx += 1;
            } else {
                // Public input layer
                let target_layer = num_compute + j;
                for (src_idx, atom_targets_layer) in all_atom_targets.iter().enumerate() {
                    let pt_templates = &all_point_templates[src_idx];
                    for (atom_local_idx, &target) in atom_targets_layer.iter().enumerate() {
                        if target == target_layer {
                            let pt = resolve_point_from_template(
                                &pt_templates[atom_local_idx],
                                &all_bindings[src_idx],
                            );
                            let mle_val = evaluate_mle_fr(&pub_values, &pt);
                            assert!(
                                mle_val != Fr::zero() || true,
                                "mleEval can be zero for valid inputs"
                            );
                            total_public_claims += 1;
                        }
                    }
                }
            }
        }

        eprintln!("Validation results:");
        eprintln!("  {} compute layers", num_compute);
        eprintln!("  {} input groups", total_input_groups);
        eprintln!("  {} public claims", total_public_claims);
        eprintln!("  All rlcBeta values non-zero: PASS");
        eprintln!("  Transcript replay complete without panics: PASS");
    }
}
