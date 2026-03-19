//! Generate EC witness JSON for the Groth16 EC verification circuit.
//!
//! Runs the GKR+Hyrax proof on the DAG circuit, replays the ECTranscript
//! step-by-step, and captures every EC operation (ecMul, ecAdd, MSM) with
//! inputs and expected outputs for gnark Groth16 verification.
//!
//! The output includes:
//!   - All EC operations performed during verification
//!   - Transcript digest (final sponge squeeze, matches Stylus hybrid mode)
//!   - Fr outputs (rlcBeta, zDotJStar, lTensor, zDotR, mleEval)
//!   - Pedersen generator points
//!   - Operation statistics
//!
//! Usage:
//!   cargo run --release --bin gen_ec_groth16_witness
//!   cargo run --release --bin gen_ec_groth16_witness -- --trees 3 --depth 4 --features 4

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
use shared_types::curves::PrimeOrderCurve;
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::{ECTranscript, ECTranscriptTrait};
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
    perform_function_under_prover_config, perform_function_under_verifier_config, Bn256Point, Fq,
    Fr,
};
use std::collections::HashMap;
use std::ops::Neg;


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

fn fq_to_hex(val: &Fq) -> String {
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
// EC point serialization
// ============================================================================

fn point_to_json(p: &Bn256Point) -> serde_json::Value {
    match p.affine_coordinates() {
        Some((x, y)) => json!({"x": fq_to_hex(&x), "y": fq_to_hex(&y)}),
        None => {
            let z = "0x0000000000000000000000000000000000000000000000000000000000000000";
            json!({"x": z, "y": z})
        }
    }
}

// ============================================================================
// EC operation collector
// ============================================================================

struct ECCollector {
    ops: Vec<serde_json::Value>,
    total_ec_mul: usize,
    total_ec_add: usize,
    total_msm: usize,
    total_msm_points: usize,
}

impl ECCollector {
    fn new() -> Self {
        Self {
            ops: Vec::new(),
            total_ec_mul: 0,
            total_ec_add: 0,
            total_msm: 0,
            total_msm_points: 0,
        }
    }

    fn record_mul(&mut self, point: &Bn256Point, scalar: &Fr, result: &Bn256Point) {
        self.ops.push(json!({
            "type": "mul",
            "point": point_to_json(point),
            "scalar": fr_to_hex(scalar),
            "result": point_to_json(result),
        }));
        self.total_ec_mul += 1;
    }

    fn record_add(&mut self, point1: &Bn256Point, point2: &Bn256Point, result: &Bn256Point) {
        self.ops.push(json!({
            "type": "add",
            "point1": point_to_json(point1),
            "point2": point_to_json(point2),
            "result": point_to_json(result),
        }));
        self.total_ec_add += 1;
    }

    fn record_msm(&mut self, points: &[Bn256Point], scalars: &[Fr], result: &Bn256Point) {
        self.ops.push(json!({
            "type": "msm",
            "points": points.iter().map(point_to_json).collect::<Vec<_>>(),
            "scalars": scalars.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "result": point_to_json(result),
        }));
        self.total_msm += 1;
        self.total_msm_points += points.len();
    }
}

/// ec_mul with recording
fn ec_mul_t(ec: &mut ECCollector, point: &Bn256Point, scalar: &Fr) -> Bn256Point {
    let result = *point * *scalar;
    ec.record_mul(point, scalar, &result);
    result
}

/// ec_add with recording
fn ec_add_t(ec: &mut ECCollector, p1: &Bn256Point, p2: &Bn256Point) -> Bn256Point {
    let result = *p1 + *p2;
    ec.record_add(p1, p2, &result);
    result
}

/// MSM with recording
fn msm_t(ec: &mut ECCollector, points: &[Bn256Point], scalars: &[Fr]) -> Bn256Point {
    let result = points
        .iter()
        .zip(scalars.iter())
        .fold(Bn256Point::zero(), |acc, (p, s)| acc + *p * *s);
    ec.record_msm(points, scalars, &result);
    result
}

/// MSM over Pedersen generators (truncated to match z_vector length)
fn msm_gens_t(
    ec: &mut ECCollector,
    gens: &[Bn256Point],
    scalars: &[Fr],
) -> Bn256Point {
    let n = scalars.len().min(gens.len());
    msm_t(ec, &gens[..n], &scalars[..n])
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
    claim_points: Vec<Vec<Fr>>,
    // Store claim evaluation points (G1 commitments) for EC ops
    claim_evaluations: Vec<Bn256Point>,
}

struct InputGroupExtract {
    rlc_coeffs: Vec<Fr>,
    podp_challenge: Fr,
    podp_z_vector: Vec<Fr>,
    podp_z_delta: Fr,
    podp_z_beta: Fr,
    l_half_bindings: Vec<Fr>,
    r_half_bindings: Vec<Fr>,
    _num_rows: usize,
}

// ============================================================================
// Point template resolution (same as gen_dag_groth16_witness.rs)
// ============================================================================

fn resolve_point_template(
    claim_point: &[Fr],
    source_bindings: &[Fr],
    _claim_points: &[Vec<Fr>],
) -> Vec<String> {
    let mut template = Vec::new();
    for val in claim_point.iter() {
        let mut found = false;
        for (bi, b) in source_bindings.iter().enumerate() {
            if val == b {
                template.push(format!("B{}", bi));
                found = true;
                break;
            }
        }
        if !found {
            if *val == Fr::zero() {
                template.push("F0".to_string());
            } else if *val == Fr::one() {
                template.push("F1".to_string());
            } else {
                template.push(format!("?{}", fr_to_hex(val)));
            }
        }
    }
    template
}

fn parse_template_entry(entry: &str) -> u64 {
    if let Some(rest) = entry.strip_prefix('B') {
        rest.parse::<u64>()
            .unwrap_or_else(|e| panic!("invalid binding index '{}': {}", entry, e))
    } else if let Some(rest) = entry.strip_prefix('F') {
        20000
            + rest
                .parse::<u64>()
                .unwrap_or_else(|e| panic!("invalid fixed index '{}': {}", entry, e))
    } else {
        panic!("Unknown template entry: {}", entry);
    }
}

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
// Sorting/grouping
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
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &idx in sorted_indices {
        for group in &mut groups {
            let first = group[0];
            if r_half_equals(&claim_points[first], &claim_points[idx], log_n_cols) {
                group.push(idx);
                break;
            }
        }
        groups.push(vec![idx]);
    }
    groups
}

// ============================================================================
// EC verification pass: compute and record all EC operations
// ============================================================================

/// Record PODP EC operations (Check 1 + Check 2)
fn record_podp_ec_ops(
    ec: &mut ECCollector,
    com_x: &Bn256Point,      // alpha (for compute) or MSM result (for input)
    com_y: &Bn256Point,      // dot_product (for compute) or com_eval (for input)
    challenge: &Fr,           // PODP challenge c
    commit_d: &Bn256Point,
    commit_d_dot_a: &Bn256Point,
    z_vector: &[Fr],
    z_delta: &Fr,
    z_beta: &Fr,
    z_dot_a: &Fr,            // inner_product(z_vector, a_vector) — Fr, computed outside
    message_gens: &[Bn256Point],
    scalar_gen: &Bn256Point,
    blinding_gen: &Bn256Point,
) {
    // Check 1: c * com_x + commit_d == MSM(gens, z_vector) + z_delta * h
    // LHS
    let c_com_x = ec_mul_t(ec, com_x, challenge);
    let lhs1 = ec_add_t(ec, &c_com_x, commit_d);
    // RHS
    let msm_result = msm_gens_t(ec, message_gens, z_vector);
    let z_delta_h = ec_mul_t(ec, blinding_gen, z_delta);
    let rhs1 = ec_add_t(ec, &msm_result, &z_delta_h);
    debug_assert_eq!(lhs1, rhs1, "PODP check 1 failed");

    // Check 2: c * com_y + commit_d_dot_a == z_dot_a * g + z_beta * h
    let c_com_y = ec_mul_t(ec, com_y, challenge);
    let lhs2 = ec_add_t(ec, &c_com_y, commit_d_dot_a);
    let z_dot_a_g = ec_mul_t(ec, scalar_gen, z_dot_a);
    let z_beta_h = ec_mul_t(ec, blinding_gen, z_beta);
    let rhs2 = ec_add_t(ec, &z_dot_a_g, &z_beta_h);
    debug_assert_eq!(lhs2, rhs2, "PODP check 2 failed");
}

/// Record PoP EC operations (3 checks per product triple)
fn record_pop_ec_ops(
    ec: &mut ECCollector,
    com_x: &Bn256Point,
    com_y: &Bn256Point,
    com_z: &Bn256Point,
    pop_challenge: &Fr,
    pop_alpha: &Bn256Point,
    pop_beta: &Bn256Point,
    pop_delta: &Bn256Point,
    z1: &Fr,
    z2: &Fr,
    z3: &Fr,
    z4: &Fr,
    z5: &Fr,
    scalar_gen: &Bn256Point,
    blinding_gen: &Bn256Point,
) {
    // Check 1: alpha + c * com_x == z1 * g + z2 * h
    let c_com_x = ec_mul_t(ec, com_x, pop_challenge);
    let lhs1 = ec_add_t(ec, pop_alpha, &c_com_x);
    let z1_g = ec_mul_t(ec, scalar_gen, z1);
    let z2_h = ec_mul_t(ec, blinding_gen, z2);
    let rhs1 = ec_add_t(ec, &z1_g, &z2_h);
    debug_assert_eq!(lhs1, rhs1, "PoP check 1 failed");

    // Check 2: beta + c * com_y == z3 * g + z4 * h
    let c_com_y = ec_mul_t(ec, com_y, pop_challenge);
    let lhs2 = ec_add_t(ec, pop_beta, &c_com_y);
    let z3_g = ec_mul_t(ec, scalar_gen, z3);
    let z4_h = ec_mul_t(ec, blinding_gen, z4);
    let rhs2 = ec_add_t(ec, &z3_g, &z4_h);
    debug_assert_eq!(lhs2, rhs2, "PoP check 2 failed");

    // Check 3: delta + c * com_z == z3 * com_x + z5 * h
    let c_com_z = ec_mul_t(ec, com_z, pop_challenge);
    let lhs3 = ec_add_t(ec, pop_delta, &c_com_z);
    let z3_com_x = ec_mul_t(ec, com_x, z3);
    let z5_h = ec_mul_t(ec, blinding_gen, z5);
    let rhs3 = ec_add_t(ec, &z3_com_x, &z5_h);
    debug_assert_eq!(lhs3, rhs3, "PoP check 3 failed");
}

// ============================================================================
// Main
// ============================================================================

fn main() -> Result<()> {
    let num_trees = parse_arg_usize("--trees", 4);
    let depth = parse_arg_usize("--depth", 2);
    let num_features = parse_arg_usize("--features", 5);

    eprintln!(
        "EC witness extractor: trees={}, depth={}, features={}",
        num_trees, depth, num_features
    );

    // ================================================================
    // Build XGBoost circuit and generate proof
    // ================================================================

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
        .expect("failed to generate Hyrax provable circuit");
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
                    .expect("public input MLE must be present")
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

    // === Per-layer transcript replay ===
    let mut layer_extracts: Vec<LayerExtract> = Vec::new();
    let mut all_atom_targets: Vec<Vec<usize>> = Vec::new();
    let mut all_point_templates: Vec<Vec<Vec<u64>>> = Vec::new();

    // Store PSL data for EC operations pass
    struct LayerPSLData {
        psl: PostSumcheckLayer<Fr, Bn256Point>,
        product_triples: Vec<(Bn256Point, Bn256Point, Bn256Point)>,
    }
    let mut layer_psl_data: Vec<LayerPSLData> = Vec::new();

    // Store PoP witness data
    struct PopWitness {
        alpha: Bn256Point,
        beta: Bn256Point,
        delta: Bn256Point,
        z1: Fr,
        z2: Fr,
        z3: Fr,
        z4: Fr,
        z5: Fr,
    }
    let mut layer_pop_witnesses: Vec<Vec<PopWitness>> = Vec::new();

    // Store PODP commit points
    struct PodpCommits {
        commit_d: Bn256Point,
        commit_d_dot_a: Bn256Point,
    }
    let mut layer_podp_commits: Vec<PodpCommits> = Vec::new();

    for (proof_idx, (layer_id, layer_proof)) in
        proof.circuit_proof.layer_proofs.iter().enumerate()
    {
        let layer_desc = desc
            .intermediate_layers
            .iter()
            .find(|ld| ld.layer_id() == *layer_id)
            .unwrap_or_else(|| panic!("layer {:?} not found", layer_id));
        let layer_claims_vec = claim_tracker.remove(layer_id).unwrap_or_default();
        let num_rounds = layer_desc.sumcheck_round_indices().len();
        let degree = layer_desc.max_degree();

        eprintln!(
            "  Layer {} (rounds={}, degree={}, claims={})",
            proof_idx, num_rounds, degree, layer_claims_vec.len()
        );

        // Store claim evaluation commitments (G1 points)
        let claim_evaluations: Vec<Bn256Point> =
            layer_claims_vec.iter().map(|c| c.evaluation).collect();

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

        // Build PSL for claim propagation and oracle eval
        let claim_points: Vec<Vec<Fr>> = layer_claims_vec.iter().map(|c| c.point.clone()).collect();
        let claim_points_refs: Vec<&[Fr]> = claim_points.iter().map(|v| v.as_slice()).collect();
        let psl_desc =
            layer_desc.get_post_sumcheck_layer(&bindings, &claim_points_refs, &random_coefficients);
        let psl: PostSumcheckLayer<Fr, Bn256Point> =
            new_with_values(&psl_desc, &layer_proof.commitments);

        // PODP — extract private fields via JSON serialization
        let podp_json = serde_json::to_value(&layer_proof.proof_of_sumcheck.podp)
            .expect("failed to serialize PODP");
        let podp_commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
        let podp_commit_d_dot_a: Bn256Point = parse_point_from_json(&podp_json["commit_d_dot_a"]);
        let podp_z_vector: Vec<Fr> = podp_json["z_vector"]
            .as_array()
            .expect("PODP z_vector must be array")
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

        // PoP — replicate transcript ops and store witness data
        let product_triples: Vec<(Bn256Point, Bn256Point, Bn256Point)> = psl
            .0
            .iter()
            .filter_map(|p| p.get_product_triples())
            .flatten()
            .collect();
        let mut pop_challenges: Vec<Fr> = Vec::new();
        let mut pop_witnesses: Vec<PopWitness> = Vec::new();
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

            pop_witnesses.push(PopWitness {
                alpha: pop.alpha,
                beta: pop.beta,
                delta: pop.delta,
                z1: pop.z1,
                z2: pop.z2,
                z3: pop.z3,
                z4: pop.z4,
                z5: pop.z5,
            });
        }

        // Extract claims for routing
        let new_claims: Vec<HyraxClaim<Fr, Bn256Point>> =
            psl.0.iter().flat_map(get_claims_from_product).collect();
        let mut atom_targets = Vec::new();
        let mut point_templates = Vec::new();
        for claim in &new_claims {
            let target_idx = *layer_id_to_idx
                .get(&claim.to_layer_id)
                .expect("target layer not found");
            atom_targets.push(target_idx);
            let template_strs = resolve_point_template(&claim.point, &bindings, &claim_points);
            let template: Vec<u64> = template_strs
                .iter()
                .map(|s| parse_template_entry(s))
                .collect();
            point_templates.push(template);
            claim_tracker
                .entry(claim.to_layer_id)
                .or_default()
                .push(claim.clone());
        }

        all_atom_targets.push(atom_targets);
        all_point_templates.push(point_templates);

        layer_podp_commits.push(PodpCommits {
            commit_d: podp_commit_d,
            commit_d_dot_a: podp_commit_d_dot_a,
        });
        layer_psl_data.push(LayerPSLData {
            psl,
            product_triples,
        });
        layer_pop_witnesses.push(pop_witnesses);

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
            claim_points,
            claim_evaluations,
        });
    }

    eprintln!("Compute layer transcript replay complete.");

    // ================================================================
    // Input layer transcript replay
    // ================================================================

    let mut input_group_extracts: Vec<InputGroupExtract> = Vec::new();
    let mut public_claim_points: Vec<Vec<Fr>> = Vec::new();

    // Store input group PODP commits and commitment rows for EC pass
    let mut input_podp_commits: Vec<PodpCommits> = Vec::new();
    let mut input_commitment_rows: Vec<Vec<Bn256Point>> = Vec::new();
    let mut input_com_evals: Vec<Bn256Point> = Vec::new();

    let mut dag_input_proof_idx = 0usize;
    for (j, il) in desc.input_layers.iter().enumerate() {
        let is_committed = private_input_ids.contains(&il.layer_id);
        let _claims = claim_tracker.remove(&il.layer_id).unwrap_or_default();
        eprintln!(
            "  Input layer {} (committed={}, claims={})",
            j,
            is_committed,
            _claims.len()
        );

        if is_committed {
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

            let sorted_indices = sort_claims_lexicographic(&resolved_claim_points);
            let input_proof = &proof.hyrax_input_proofs[dag_input_proof_idx];
            let num_rows = input_proof.input_commitment.len();
            let n = resolved_claim_points[0].len();
            let l_half_len = (num_rows as f64).log2() as usize;
            let log_n_cols = n - l_half_len;

            let groups =
                group_claims_by_r_half(&resolved_claim_points, &sorted_indices, log_n_cols);
            eprintln!(
                "    {} groups, numRows={}, lHalfLen={}",
                groups.len(),
                num_rows,
                l_half_len
            );

            let input_eval_json = serde_json::to_value(&input_proof.evaluation_proofs)
                .expect("failed to serialize input evaluation proofs");

            for (g, group) in groups.iter().enumerate() {
                let group_rlc_coeffs =
                    t.get_scalar_field_challenges("Input claim RLC coefficients", group.len());
                let com_eval = input_proof.evaluation_proofs[g].commitment_to_evaluation;
                t.append_ec_point("Commitment to evaluation", com_eval);

                let eval_json = &input_eval_json[g];
                let podp_json = &eval_json["podp_evaluation_proof"];
                let commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
                let commit_d_dot_a: Bn256Point =
                    parse_point_from_json(&podp_json["commit_d_dot_a"]);
                let z_vector: Vec<Fr> = podp_json["z_vector"]
                    .as_array()
                    .expect("input PODP z_vector must be array")
                    .iter()
                    .map(parse_fr_from_json)
                    .collect();
                let z_delta: Fr = parse_fr_from_json(&podp_json["z_delta"]);
                let z_beta: Fr = parse_fr_from_json(&podp_json["z_beta"]);

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

                let first_claim_point = &resolved_claim_points[group[0]];
                let l_half = first_claim_point[..l_half_len].to_vec();
                let r_half = first_claim_point[l_half_len..].to_vec();

                input_podp_commits.push(PodpCommits {
                    commit_d,
                    commit_d_dot_a,
                });
                input_commitment_rows.push(input_proof.input_commitment.clone());
                input_com_evals.push(com_eval);

                input_group_extracts.push(InputGroupExtract {
                    rlc_coeffs: group_rlc_coeffs,
                    podp_challenge: input_podp_c,
                    podp_z_vector: z_vector,
                    podp_z_delta: z_delta,
                    podp_z_beta: z_beta,
                    l_half_bindings: l_half,
                    r_half_bindings: r_half,
                    _num_rows: num_rows,
                });
            }

            dag_input_proof_idx += 1;
        } else {
            // Public input layer — collect claim data for EC verification
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

    // ================================================================
    // Compute transcript digest (final sponge squeeze)
    // ================================================================

    let transcript_digest: Fr = t.get_scalar_field_challenge("hybrid transcript digest");
    eprintln!("Transcript digest: {}", fr_to_hex(&transcript_digest));

    eprintln!("Transcript replay complete.");
    eprintln!(
        "  Input groups: {}, Public claims: {}",
        input_group_extracts.len(),
        public_claim_points.len()
    );

    // ================================================================
    // Compute Fr outputs (all in Fr)
    // ================================================================

    let mut rlc_betas: Vec<Fr> = Vec::new();
    let mut z_dot_jstars: Vec<Fr> = Vec::new();

    for (i, le) in layer_extracts.iter().enumerate() {
        let rlc_beta: Fr = le
            .claim_points
            .iter()
            .zip(le.rlc_coefficients.iter())
            .fold(Fr::zero(), |acc, (cp, rc)| {
                let beta = compute_beta_fr(&le.bindings, cp);
                acc + beta * rc
            });
        rlc_betas.push(rlc_beta);

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

    let mut l_tensor_flat: Vec<Fr> = Vec::new();
    let mut l_tensor_offsets: Vec<usize> = Vec::new();
    let mut z_dot_rs: Vec<Fr> = Vec::new();

    for (g, ige) in input_group_extracts.iter().enumerate() {
        let l_per_shred = compute_tensor_product_fr(&ige.l_half_bindings);
        l_tensor_offsets.push(l_tensor_flat.len());
        for coeff in ige.rlc_coeffs.iter() {
            for t_val in &l_per_shred {
                l_tensor_flat.push(*coeff * *t_val);
            }
        }

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

    let mut mle_evals: Vec<Fr> = Vec::new();
    for (ci, claim_point) in public_claim_points.iter().enumerate() {
        let mle_val = evaluate_mle_fr(&pub_values, claim_point);
        mle_evals.push(mle_val);
        eprintln!("  Public claim {}: mleEval={}", ci, fr_to_hex(&mle_val));
    }

    // ================================================================
    // EC operations pass: compute and record all EC operations
    // ================================================================

    eprintln!("Computing EC operations...");
    let mut ec = ECCollector::new();

    let message_gens = &verifier_committer.generators;
    let scalar_gen = verifier_committer.scalar_commit_generator();
    let blinding_gen = verifier_committer.blinding_generator;

    // --- Per compute layer ---
    for (layer_idx, ((_layer_id, layer_proof), le)) in proof
        .circuit_proof
        .layer_proofs
        .iter()
        .zip(layer_extracts.iter())
        .enumerate()
    {
        let psl_data = &layer_psl_data[layer_idx];
        let pop_witnesses = &layer_pop_witnesses[layer_idx];
        let podp_commits = &layer_podp_commits[layer_idx];

        // 1. RLC eval: rlc_eval = sum(claim.evaluation * rlc_coeff)
        let mut rlc_eval = Bn256Point::zero();
        for (ci, (eval_pt, coeff)) in le
            .claim_evaluations
            .iter()
            .zip(le.rlc_coefficients.iter())
            .enumerate()
        {
            let scaled = ec_mul_t(&mut ec, eval_pt, coeff);
            if ci == 0 {
                rlc_eval = scaled;
            } else {
                rlc_eval = ec_add_t(&mut ec, &rlc_eval, &scaled);
            }
        }

        // 2. Oracle eval: sum(product.coefficient * product.get_result())
        let mut oracle_eval = Bn256Point::zero();
        for (pi, product) in psl_data.psl.0.iter().enumerate() {
            let result = product.get_result();
            let scaled = ec_mul_t(&mut ec, &result, &product.coefficient);
            if pi == 0 {
                oracle_eval = scaled;
            } else {
                oracle_eval = ec_add_t(&mut ec, &oracle_eval, &scaled);
            }
        }

        // 3. Alpha = MSM(messages, gammas)
        let msgs = &layer_proof.proof_of_sumcheck.messages;
        let alpha = msm_t(&mut ec, msgs, &le.gammas);

        // 4. Dot product = sum * rhos[0] + oracle_eval * (-rhos[n])
        let sum_scaled = ec_mul_t(&mut ec, &layer_proof.proof_of_sumcheck.sum, &le.rhos[0]);
        let neg_rho_n = le.rhos[le.rhos.len() - 1].neg();
        let oracle_scaled = ec_mul_t(&mut ec, &oracle_eval, &neg_rho_n);
        let dot_product = ec_add_t(&mut ec, &sum_scaled, &oracle_scaled);

        // 5. PODP verification (com_x = alpha, com_y = dot_product)
        let z_dot_a: Fr = le
            .podp_z_vector
            .iter()
            .zip(le.j_star.iter())
            .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);

        record_podp_ec_ops(
            &mut ec,
            &alpha,
            &dot_product,
            &le.podp_challenge,
            &podp_commits.commit_d,
            &podp_commits.commit_d_dot_a,
            &le.podp_z_vector,
            &le.podp_z_delta,
            &le.podp_z_beta,
            &z_dot_a,
            message_gens,
            &scalar_gen,
            &blinding_gen,
        );

        // 6. PoP verification (per product triple)
        for (ti, ((com_x, com_y, com_z), pop_w)) in psl_data
            .product_triples
            .iter()
            .zip(pop_witnesses.iter())
            .enumerate()
        {
            record_pop_ec_ops(
                &mut ec,
                com_x,
                com_y,
                com_z,
                &le.pop_challenges[ti],
                &pop_w.alpha,
                &pop_w.beta,
                &pop_w.delta,
                &pop_w.z1,
                &pop_w.z2,
                &pop_w.z3,
                &pop_w.z4,
                &pop_w.z5,
                &scalar_gen,
                &blinding_gen,
            );
        }

        if layer_idx % 10 == 0 {
            eprintln!(
                "  Layer {}/{}: {} ops so far",
                layer_idx,
                num_compute,
                ec.ops.len()
            );
        }
    }

    eprintln!(
        "Compute layers done: {} EC ops (mul={}, add={}, msm={})",
        ec.ops.len(),
        ec.total_ec_mul,
        ec.total_ec_add,
        ec.total_msm
    );

    // --- Per input eval group ---
    for (g, ige) in input_group_extracts.iter().enumerate() {
        let commitment_rows = &input_commitment_rows[g];
        let com_eval = &input_com_evals[g];
        let podp_commits = &input_podp_commits[g];

        // MSM(commitment_rows, l_tensor) = com_x for PODP
        let l_tensor = compute_tensor_product_fr(&ige.l_half_bindings);
        // Scale l_tensor by RLC coefficients
        let mut l_coeffs: Vec<Fr> = Vec::new();
        for coeff in ige.rlc_coeffs.iter() {
            for t_val in &l_tensor {
                l_coeffs.push(*coeff * *t_val);
            }
        }
        let com_x = msm_t(&mut ec, commitment_rows, &l_coeffs[..commitment_rows.len()]);

        // R-tensor for a_vector
        let r_tensor = compute_tensor_product_fr(&ige.r_half_bindings);
        let z_dot_r: Fr = ige
            .podp_z_vector
            .iter()
            .zip(r_tensor.iter())
            .fold(Fr::zero(), |acc, (z, r)| acc + *z * *r);

        // PODP (com_x = MSM result, com_y = com_eval)
        record_podp_ec_ops(
            &mut ec,
            &com_x,
            com_eval,
            &ige.podp_challenge,
            &podp_commits.commit_d,
            &podp_commits.commit_d_dot_a,
            &ige.podp_z_vector,
            &ige.podp_z_delta,
            &ige.podp_z_beta,
            &z_dot_r,
            message_gens,
            &scalar_gen,
            &blinding_gen,
        );
    }

    eprintln!(
        "Input layers done: {} EC ops (mul={}, add={}, msm={})",
        ec.ops.len(),
        ec.total_ec_mul,
        ec.total_ec_add,
        ec.total_msm
    );

    // --- Public claims: Pedersen opening check ---
    // Note: public claim data (commitment, value, blinding) comes from the proof's
    // public_value_claims. We need to access the actual HyraxProof claims.
    // For now, public claims are verified via MLE evaluation only (no EC ops in Stylus)
    // since the Stylus hybrid mode skips Pedersen opening checks for public inputs.
    // The MLE evaluation is a pure Fr operation already captured in mle_evals.

    eprintln!(
        "Total EC operations: {} (mul={}, add={}, msm={}, msm_points={})",
        ec.ops.len(),
        ec.total_ec_mul,
        ec.total_ec_add,
        ec.total_msm,
        ec.total_msm_points
    );

    // ================================================================
    // Extract generator points
    // ================================================================

    let gen_points: Vec<serde_json::Value> = message_gens
        .iter()
        .map(point_to_json)
        .collect();

    // ================================================================
    // Compute circuit hash
    // ================================================================

    use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
    use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
    let hash_elems = get_circuit_description_hash_as_field_elems(
        desc,
        global_verifier_circuit_description_hash_type(),
    );
    let circuit_hash_0_fr = {
        let mut repr = <Fr as PrimeField>::Repr::default();
        repr.as_mut()
            .copy_from_slice(hash_elems[0].to_repr().as_ref());
        Fr::from_repr(repr).expect("circuit hash element 0 must be valid Fr")
    };
    let circuit_hash_1_fr = {
        let mut repr = <Fr as PrimeField>::Repr::default();
        repr.as_mut()
            .copy_from_slice(hash_elems[1].to_repr().as_ref());
        Fr::from_repr(repr).expect("circuit hash element 1 must be valid Fr")
    };

    // ================================================================
    // Build JSON output
    // ================================================================

    let output = json!({
        "transcriptDigest": fr_to_hex(&transcript_digest),
        "circuitHash": [fr_to_hex(&circuit_hash_0_fr), fr_to_hex(&circuit_hash_1_fr)],
        "ecOperations": ec.ops,
        "frOutputs": {
            "rlcBetas": rlc_betas.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "zDotJStars": z_dot_jstars.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "lTensorFlat": l_tensor_flat.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "lTensorOffsets": l_tensor_offsets,
            "zDotRs": z_dot_rs.iter().map(fr_to_hex).collect::<Vec<_>>(),
            "mleEvals": mle_evals.iter().map(fr_to_hex).collect::<Vec<_>>(),
        },
        "generatorPoints": {
            "messageGens": gen_points,
            "scalarGen": point_to_json(&scalar_gen),
            "blindingGen": point_to_json(&blinding_gen),
        },
        "stats": {
            "totalEcMul": ec.total_ec_mul,
            "totalEcAdd": ec.total_ec_add,
            "totalMsm": ec.total_msm,
            "totalMsmPoints": ec.total_msm_points,
            "numComputeLayers": num_compute,
            "numInputGroups": input_group_extracts.len(),
            "numPublicClaims": public_claim_points.len(),
        },
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
    #[ignore] // Slow test (~10s), run with: cargo test --bin gen_ec_groth16_witness -- --nocapture --ignored
    fn test_ec_witness_round_trip() {
        // Use sample_model for deterministic results
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

        let (proof, proof_config) = perform_function_under_prover_config!(
            |w, x, y, z| provable.prove(w, x, y, z),
            &config,
            &committer,
            &mut rng,
            &mut vander,
            &mut transcript
        );

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
        let _private_input_ids: std::collections::HashSet<_> = verifiable
            .get_private_input_layer_ids()
            .into_iter()
            .collect();

        let mut layer_id_to_idx: HashMap<LayerId, usize> = HashMap::new();
        for (proof_idx, (layer_id, _)) in proof.circuit_proof.layer_proofs.iter().enumerate() {
            layer_id_to_idx.insert(*layer_id, proof_idx);
        }
        for (j, il) in desc.input_layers.iter().enumerate() {
            layer_id_to_idx.insert(il.layer_id, num_compute + j);
        }

        // Replay transcript to get challenges
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

        // Per-layer replay + EC ops
        let mut ec = ECCollector::new();
        let message_gens = &verifier_committer.generators;
        let scalar_gen = verifier_committer.scalar_commit_generator();
        let blinding_gen = verifier_committer.blinding_generator;

        let mut all_atom_targets: Vec<Vec<usize>> = Vec::new();
        let mut all_point_templates: Vec<Vec<Vec<u64>>> = Vec::new();
        let mut all_bindings: Vec<Vec<Fr>> = Vec::new();
        let mut all_rlc_betas: Vec<Fr> = Vec::new();

        for (_proof_idx, (layer_id, layer_proof)) in
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

            let claim_evaluations: Vec<Bn256Point> =
                layer_claims.iter().map(|c| c.evaluation).collect();

            let random_coefficients = match global_claim_agg_strategy() {
                ClaimAggregationStrategy::RLC => t
                    .get_scalar_field_challenges("RLC Claim Agg Coefficients", layer_claims.len()),
                _ => vec![Fr::one()],
            };

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
            t.append_ec_points(
                "Commitments to all the layer's leaf values and intermediates",
                &layer_proof.commitments,
            );

            let rhos = t.get_scalar_field_challenges(
                "Proof of sumcheck RLC coefficients for batching rows",
                n + 1,
            );
            let gammas = t.get_scalar_field_challenges(
                "Proof of sumcheck RLC coefficients for batching columns",
                n,
            );
            let j_star = ProofOfSumcheck::<Bn256Point>::calculate_j_star(
                &bindings, &rhos, &gammas, degree,
            );

            let claim_points: Vec<Vec<Fr>> = layer_claims.iter().map(|c| c.point.clone()).collect();
            let claim_points_refs: Vec<&[Fr]> =
                claim_points.iter().map(|v| v.as_slice()).collect();
            let psl_desc = layer_desc.get_post_sumcheck_layer(
                &bindings,
                &claim_points_refs,
                &random_coefficients,
            );
            let psl: PostSumcheckLayer<Fr, Bn256Point> =
                new_with_values(&psl_desc, &layer_proof.commitments);

            // EC: RLC eval
            let mut rlc_eval = Bn256Point::zero();
            for (ci, (eval_pt, coeff)) in claim_evaluations
                .iter()
                .zip(random_coefficients.iter())
                .enumerate()
            {
                let scaled = ec_mul_t(&mut ec, eval_pt, coeff);
                if ci == 0 {
                    rlc_eval = scaled;
                } else {
                    rlc_eval = ec_add_t(&mut ec, &rlc_eval, &scaled);
                }
            }

            // EC: Oracle eval
            let mut oracle_eval = Bn256Point::zero();
            for (pi, product) in psl.0.iter().enumerate() {
                let result = product.get_result();
                let scaled = ec_mul_t(&mut ec, &result, &product.coefficient);
                if pi == 0 {
                    oracle_eval = scaled;
                } else {
                    oracle_eval = ec_add_t(&mut ec, &oracle_eval, &scaled);
                }
            }

            // EC: Alpha MSM
            let alpha = msm_t(&mut ec, msgs, &gammas);

            // EC: Dot product
            let sum_scaled = ec_mul_t(&mut ec, &layer_proof.proof_of_sumcheck.sum, &rhos[0]);
            let neg_rho_n = rhos[rhos.len() - 1].neg();
            let oracle_scaled = ec_mul_t(&mut ec, &oracle_eval, &neg_rho_n);
            let dot_product = ec_add_t(&mut ec, &sum_scaled, &oracle_scaled);

            // PODP
            let podp_json =
                serde_json::to_value(&layer_proof.proof_of_sumcheck.podp).unwrap();
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
            t.append_scalar_field_elem(
                "Blinding factor for blinded inner product",
                podp_z_beta,
            );

            let z_dot_a: Fr = podp_z_vector
                .iter()
                .zip(j_star.iter())
                .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);

            record_podp_ec_ops(
                &mut ec,
                &alpha,
                &dot_product,
                &podp_c,
                &podp_commit_d,
                &podp_commit_d_dot_a,
                &podp_z_vector,
                &podp_z_delta,
                &podp_z_beta,
                &z_dot_a,
                message_gens,
                &scalar_gen,
                &blinding_gen,
            );

            // PoP
            let product_triples: Vec<_> = psl
                .0
                .iter()
                .filter_map(|p| p.get_product_triples())
                .flatten()
                .collect();
            for (ti, ((_com_x_pop, _com_y_pop, _com_z_pop), pop)) in product_triples
                .iter()
                .zip(layer_proof.proofs_of_product.iter())
                .enumerate()
            {
                t.append_ec_point("Commitment to random values 1", pop.alpha);
                t.append_ec_point("Commitment to random values 2", pop.beta);
                t.append_ec_point("Commitment to random values 3", pop.delta);
                let pop_c = t.get_scalar_field_challenge("PoP c");
                t.append_scalar_field_elem("Blinded response 1", pop.z1);
                t.append_scalar_field_elem("Blinded response 2", pop.z2);
                t.append_scalar_field_elem("Blinded response 3", pop.z3);
                t.append_scalar_field_elem("Blinded response 4", pop.z4);
                t.append_scalar_field_elem("Blinded response 5", pop.z5);

                let (com_x, com_y, com_z) = &product_triples[ti];
                record_pop_ec_ops(
                    &mut ec,
                    com_x,
                    com_y,
                    com_z,
                    &pop_c,
                    &pop.alpha,
                    &pop.beta,
                    &pop.delta,
                    &pop.z1,
                    &pop.z2,
                    &pop.z3,
                    &pop.z4,
                    &pop.z5,
                    &scalar_gen,
                    &blinding_gen,
                );
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
                let template_strs =
                    resolve_point_template(&claim.point, &bindings, &claim_points);
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

        // Validate
        assert_eq!(all_rlc_betas.len(), num_compute);
        for i in 0..num_compute {
            assert!(
                all_rlc_betas[i] != Fr::zero(),
                "rlcBeta[{}] should be non-zero",
                i
            );
        }

        // Validate EC op counts
        assert!(ec.total_ec_mul > 0, "Should have ec_mul operations");
        assert!(ec.total_ec_add > 0, "Should have ec_add operations");
        assert!(ec.total_msm > 0, "Should have MSM operations");

        eprintln!("EC witness test PASSED:");
        eprintln!("  {} compute layers", num_compute);
        eprintln!(
            "  {} EC ops (mul={}, add={}, msm={}, msm_pts={})",
            ec.ops.len(),
            ec.total_ec_mul,
            ec.total_ec_add,
            ec.total_msm,
            ec.total_msm_points
        );
    }

    #[test]
    fn test_ec_ops_basic() {
        // Test basic EC operation recording
        let mut ec = ECCollector::new();

        let g = Bn256Point::zero();
        let s = Fr::one();

        // Record a trivial mul (0 * 1 = 0)
        let result = ec_mul_t(&mut ec, &g, &s);
        assert_eq!(result, Bn256Point::zero());
        assert_eq!(ec.total_ec_mul, 1);
        assert_eq!(ec.ops.len(), 1);
        assert_eq!(ec.ops[0]["type"], "mul");

        // Record a trivial add (0 + 0 = 0)
        let result = ec_add_t(&mut ec, &g, &g);
        assert_eq!(result, Bn256Point::zero());
        assert_eq!(ec.total_ec_add, 1);
        assert_eq!(ec.ops.len(), 2);
        assert_eq!(ec.ops[1]["type"], "add");

        // Record a trivial MSM
        let result = msm_t(&mut ec, &[g, g], &[Fr::one(), Fr::one()]);
        assert_eq!(result, Bn256Point::zero());
        assert_eq!(ec.total_msm, 1);
        assert_eq!(ec.total_msm_points, 2);
    }

    #[test]
    fn test_point_serialization() {
        // Test point_to_json with identity
        let p = Bn256Point::zero();
        let j = point_to_json(&p);
        assert!(j["x"].is_string());
        assert!(j["y"].is_string());
    }
}
