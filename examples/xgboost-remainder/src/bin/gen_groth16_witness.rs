//! Generate witness JSON for the Groth16 wrapper circuit.
//!
//! Runs the GKR+Hyrax proof, replays the ECTranscript step-by-step to extract
//! ALL Fiat-Shamir challenges and proof elements, and outputs JSON that the
//! gnark `prove` command can consume.
//!
//! All challenge values are output as Fr (scalar field) hex strings. The gnark
//! circuit operates over Fr, and the Solidity verifier reduces Fq→Fr via
//! `% FR_MODULUS`.
//!
//! Usage: cargo run --release --bin gen_groth16_witness

use anyhow::Result;
use ff::PrimeField;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::layer::HyraxClaim;
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

fn main() -> Result<()> {
    let num_vars = 1;
    let base_circuit = build_circuit(num_vars);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    prover_circuit.set_input("a", vec![3u64, 5].into());
    prover_circuit.set_input("b", vec![2u64, 4].into());
    prover_circuit.set_input("expected", vec![6u64, 20].into());

    let config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let verifier_config = GKRCircuitVerifierConfig::new_from_prover_config(&config, false);

    let mut provable = prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("gen_hyrax_provable_circuit");

    let committer = PedersenCommitter::new(
        512,
        "gen-test-proof Pedersen committer seed string for generating bases",
        None,
    );
    let mut rng = thread_rng();
    let mut vander = VandermondeInverse::new();
    let mut transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("gen-test-proof transcript");

    eprintln!("Generating Hyrax proof...");
    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| provable.prove(w, x, y, z),
        &config,
        &committer,
        &mut rng,
        &mut vander,
        &mut transcript
    );

    // Verify proof (full verification, ensures proof is valid)
    let verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("gen_hyrax_verifiable_circuit");
    let verifier_committer = PedersenCommitter::new(
        512,
        "gen-test-proof Pedersen committer seed string for generating bases",
        None,
    );
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("gen-test-proof transcript");
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
    // Replay ECTranscript to extract all challenges as Fr values.
    // We manually replicate the verification transcript operations
    // WITHOUT calling verify() (which would assert EC equations we
    // don't need to re-check). This keeps the transcript state in
    // sync for correct challenge derivation.
    // ================================================================

    let verifiable_ref = verifiable.get_gkr_circuit_description_ref();

    // === Initial transcript setup ===
    let mut t: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("gen-test-proof transcript");
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
            t.append_input_ec_points(
                "Hyrax input layer commitment",
                ip.input_commitment.clone(),
            );
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
        podp_z_vector: Vec<Fr>,
        podp_z_delta: Fr,
        podp_z_beta: Fr,
        j_star: Vec<Fr>,
        random_coeff: Fr,
        pop_data: Vec<serde_json::Value>,
    }

    let mut layer_extracts: Vec<LayerExtract> = Vec::new();
    let _num_layers = proof.circuit_proof.layer_proofs.len();

    for (layer_idx, (_layer_id, layer_proof)) in
        proof.circuit_proof.layer_proofs.iter().enumerate()
    {
        let layer_desc = verifiable_ref
            .intermediate_layers
            .iter()
            .find(|ld| ld.layer_id() == *_layer_id)
            .unwrap();
        let layer_claims_vec = claim_tracker.remove(&layer_desc.layer_id()).unwrap();

        // RLC coefficients (from transcript squeeze)
        let random_coefficients = match global_claim_agg_strategy() {
            ClaimAggregationStrategy::RLC => t.get_scalar_field_challenges(
                "RLC Claim Agg Coefficients",
                layer_claims_vec.len(),
            ),
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
        let claim_points: Vec<Vec<Fr>> =
            layer_claims_vec.iter().map(|c| c.point.clone()).collect();
        let claim_points_refs: Vec<&[Fr]> = claim_points.iter().map(|v| v.as_slice()).collect();
        let psl_desc = layer_desc.get_post_sumcheck_layer(
            &bindings,
            &claim_points_refs,
            &random_coefficients,
        );
        let psl: PostSumcheckLayer<Fr, Bn256Point> =
            new_with_values(&psl_desc, &layer_proof.commitments);

        // PODP — extract private fields via JSON serialization
        let podp_json = serde_json::to_value(&layer_proof.proof_of_sumcheck.podp).unwrap();
        let podp_commit_d: Bn256Point = parse_point_from_json(&podp_json["commit_d"]);
        let podp_commit_d_dot_a: Bn256Point =
            parse_point_from_json(&podp_json["commit_d_dot_a"]);
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
        let _podp_c = t.get_scalar_field_challenge("challenge c");
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
        for (_triple, pop) in product_triples
            .iter()
            .zip(layer_proof.proofs_of_product.iter())
        {
            t.append_ec_point("Commitment to random values 1", pop.alpha);
            t.append_ec_point("Commitment to random values 2", pop.beta);
            t.append_ec_point("Commitment to random values 3", pop.delta);
            let _pop_c = t.get_scalar_field_challenge("PoP c");
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
            podp_z_vector,
            podp_z_delta,
            podp_z_beta,
            j_star,
            random_coeff,
            pop_data,
        });
    }

    // === Input layer ===
    // Squeeze input RLC coefficients (2 claims → 2 squeezes)
    let input_rlc_coeffs =
        t.get_scalar_field_challenges("Input claim RLC coefficients", 2);
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
    let _input_podp_c = t.get_scalar_field_challenge("challenge c");
    t.append_scalar_field_elems("Blinded private vector", &input_podp_z_vector);
    t.append_scalar_field_elem(
        "Blinding factor for blinded vector commitment",
        input_podp_z_delta,
    );
    t.append_scalar_field_elem("Blinding factor for blinded inner product", input_podp_z_beta);

    eprintln!("Transcript replay complete.");

    // ================================================================
    // Compute public outputs (all in Fr)
    // ================================================================

    let l0 = &layer_extracts[0];
    let l1 = &layer_extracts[1];

    // For the gnark circuit, the "output_challenge" is the claim point for layer 0.
    // The claim point comes from the output layer verification.
    // We get it from the layer_claims that flowed into layer 0.
    // Since we already processed layers, we can reconstruct:
    // Layer 0 claim point = [output_challenge] (from output layer verify)
    // Layer 1 claim point = [l0_binding] (from layer 0 bindings)
    //
    // For the gnark public inputs, the challenges are the Fr values
    // (what Solidity will compute as Fq_squeeze % FR_MODULUS).

    // rlc_beta_0 = beta(l0_bindings, [output_challenge]) * claim_agg_coeff
    // The output_challenge is whatever the output layer used as claim point for layer 0.
    // In the ECTranscript, this was squeezed during output layer verify.
    // We captured it as layer 0's random_coeff = claim_agg_coeff.
    // Actually, the claim point for layer 0 is the output challenge, and
    // random_coeff is the RLC aggregation coefficient.
    //
    // From the circuit design:
    //   rlc_beta_0 = beta(l0_bindings, claimPoint_0) * randomCoeff_0
    //   rlc_beta_1 = beta(l1_bindings, claimPoint_1) * randomCoeff_1
    // where claimPoint_0 = [outputChallenge], claimPoint_1 = l0_bindings
    //
    // The output challenge was squeezed during output layer verify and used as
    // the claim point. The random_coeff was squeezed at the start of layer processing.
    // Since these are internal to the Rust verification, we need to extract them.

    // The claim point for layer 0 comes from the output layer's binding.
    // For the test circuit, the output has 1 indexed variable, so 1 challenge.
    // This is the point stored in the claim that flows to layer 0.
    // We can get it from the proof structure.
    let _output_claim_point = &proof.circuit_proof.output_layer_proofs[0].1;
    // Actually output_layer_proofs[0].1 is the MLE indices, not the claim point.
    // The claim point was generated during output layer verify from the transcript.
    // Since we called HyraxOutputLayerProof::verify on our transcript, the claim
    // was computed and inserted into claim_tracker. But we already consumed those claims.
    //
    // For the gnark circuit, we use a simpler approach: the output challenge is the
    // claim point for layer 0, and the inter-layer coeff is the random coeff for layer 1.
    // From the transcript replay, these map to:
    // - Layer 0: random_coeff = the RLC coefficient squeezed before layer 0
    // - Layer 1: random_coeff = the RLC coefficient squeezed before layer 1
    //
    // The output challenge (claim point for layer 0) was absorbed into claim.point
    // during output layer verify. We need to recover it.
    //
    // WORKAROUND: Re-run output layer verify on a fresh transcript to capture the claim point.
    let output_challenge = {
        let mut t2: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
            ECTranscript::new("gen-test-proof transcript");
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
        claim.point[0] // This is the output challenge
    };

    eprintln!("output_challenge: {}", fr_to_hex(&output_challenge));
    eprintln!("claim_agg_coeff (l0 random_coeff): {}", fr_to_hex(&l0.random_coeff));
    eprintln!("inter_layer_coeff (l1 random_coeff): {}", fr_to_hex(&l1.random_coeff));

    // rlc_beta_0 = beta(l0_bindings, [output_challenge]) * claim_agg_coeff
    let rlc_beta_0 = compute_beta_fr(&l0.bindings, &[output_challenge]) * l0.random_coeff;

    // rlc_beta_1 = beta(l1_bindings, l0_bindings) * inter_layer_coeff
    let rlc_beta_1 = compute_beta_fr(&l1.bindings, &l0.bindings) * l1.random_coeff;

    // z_dot_jstar for each layer
    let z_dot_jstar_0: Fr = l0
        .podp_z_vector
        .iter()
        .zip(l0.j_star.iter())
        .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);
    let z_dot_jstar_1: Fr = l1
        .podp_z_vector
        .iter()
        .zip(l1.j_star.iter())
        .fold(Fr::zero(), |acc, (a, b)| acc + *a * *b);

    // L-tensor = [inputRLCCoeff0, inputRLCCoeff1]
    let l_tensor_0 = input_rlc_0;
    let l_tensor_1 = input_rlc_1;

    // R-tensor: [(1-l1_binding), l1_binding]
    let r0 = Fr::one() - l1.bindings[0];
    let r1 = l1.bindings[0];

    // z_dot_r = <input_z, R_tensor>
    let z_dot_r = input_podp_z_vector[0] * r0 + input_podp_z_vector[1] * r1;

    // MLE eval: (1-x)*pub0 + x*pub1, where x = l0_binding
    let pub0 = Fr::from(6u64);
    let pub1 = Fr::from(20u64);
    let mle_eval = (Fr::one() - l0.bindings[0]) * pub0 + l0.bindings[0] * pub1;

    eprintln!("rlc_beta_0: {}", fr_to_hex(&rlc_beta_0));
    eprintln!("rlc_beta_1: {}", fr_to_hex(&rlc_beta_1));
    eprintln!("z_dot_jstar_0: {}", fr_to_hex(&z_dot_jstar_0));
    eprintln!("z_dot_jstar_1: {}", fr_to_hex(&z_dot_jstar_1));
    eprintln!("z_dot_r: {}", fr_to_hex(&z_dot_r));
    eprintln!("mle_eval: {}", fr_to_hex(&mle_eval));

    // ================================================================
    // ABI-encode inner proof and generators for on-chain verification
    // ================================================================

    // Extract the actual circuit description hash (the one used in the transcript).
    // This is a pair of Fq values. We reconstruct the 32-byte hash from them.
    let circuit_hash: [u8; 32] = {
        use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
        use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
        let hash_elems = get_circuit_description_hash_as_field_elems(
            verifiable_ref,
            global_verifier_circuit_description_hash_type(),
        );
        // hash_elems are two Fq values, each from a 16-byte LE half of the SHA-256 hash.
        // Reconstruct the original 32-byte hash: first 16 LE bytes from hash_elems[0],
        // next 16 LE bytes from hash_elems[1].
        let repr0 = hash_elems[0].to_repr();
        let repr1 = hash_elems[1].to_repr();
        let mut hash = [0u8; 32];
        hash[..16].copy_from_slice(&repr0.as_ref()[..16]);
        hash[16..].copy_from_slice(&repr1.as_ref()[..16]);
        hash
    };

    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash)?;
    eprintln!("ABI-encoded proof: {} bytes ({} uint256 slots)", abi_bytes.len(), (abi_bytes.len() - 4) / 32);

    let gens_bytes = abi_encode::encode_pedersen_gens(&committer)?;
    eprintln!("ABI-encoded generators: {} bytes ({} uint256 slots)", gens_bytes.len(), gens_bytes.len() / 32);

    // Encode public input values as flat big-endian bytes
    let pub_values_bytes = {
        let mut buf = Vec::new();
        for val in &[pub0, pub1] {
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
    // Build JSON output (all values as Fr hex strings)
    // ================================================================
    let circuit_hash_0 = fr_to_hex(&Fr::from(12345u64)); // placeholder
    let circuit_hash_1 = fr_to_hex(&Fr::from(67890u64)); // placeholder

    let output = json!({
        "public_inputs": {
            "circuit_hash": [circuit_hash_0, circuit_hash_1],
            "public_values": [fr_to_hex(&pub0), fr_to_hex(&pub1)],
            "output_challenge": fr_to_hex(&output_challenge),
            "claim_agg_coeff": fr_to_hex(&l0.random_coeff),
            "inter_layer_coeff": fr_to_hex(&l1.random_coeff),
            "layer_0": {
                "bindings": l0.bindings.iter().map(|b| fr_to_hex(b)).collect::<Vec<_>>(),
                "rhos": l0.rhos.iter().map(|r| fr_to_hex(r)).collect::<Vec<_>>(),
                "gammas": l0.gammas.iter().map(|g| fr_to_hex(g)).collect::<Vec<_>>(),
                "podp_challenge": "0x0", // PODP challenge used only for EC checks on-chain
            },
            "layer_1": {
                "bindings": l1.bindings.iter().map(|b| fr_to_hex(b)).collect::<Vec<_>>(),
                "rhos": l1.rhos.iter().map(|r| fr_to_hex(r)).collect::<Vec<_>>(),
                "gammas": l1.gammas.iter().map(|g| fr_to_hex(g)).collect::<Vec<_>>(),
                "podp_challenge": "0x0",
                "pop_challenge": "0x0",
            },
            "input_rlc_coeffs": [fr_to_hex(&input_rlc_0), fr_to_hex(&input_rlc_1)],
            "input_podp_challenge": "0x0",
        },
        "public_outputs": {
            "rlc_beta_0": fr_to_hex(&rlc_beta_0),
            "rlc_beta_1": fr_to_hex(&rlc_beta_1),
            "z_dot_jstar_0": fr_to_hex(&z_dot_jstar_0),
            "z_dot_jstar_1": fr_to_hex(&z_dot_jstar_1),
            "l_tensor_0": fr_to_hex(&l_tensor_0),
            "l_tensor_1": fr_to_hex(&l_tensor_1),
            "z_dot_r": fr_to_hex(&z_dot_r),
            "mle_eval": fr_to_hex(&mle_eval),
        },
        "witness": {
            "layer_0_podp": {
                "z_vector": l0.podp_z_vector.iter().map(|z| fr_to_hex(z)).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&l0.podp_z_delta),
                "z_beta": fr_to_hex(&l0.podp_z_beta),
            },
            "layer_1_podp": {
                "z_vector": l1.podp_z_vector.iter().map(|z| fr_to_hex(z)).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&l1.podp_z_delta),
                "z_beta": fr_to_hex(&l1.podp_z_beta),
            },
            "layer_1_pop": if l1.pop_data.is_empty() {
                json!({"z1": "0x0", "z2": "0x0", "z3": "0x0", "z4": "0x0", "z5": "0x0"})
            } else {
                l1.pop_data[0].clone()
            },
            "input_podp": {
                "z_vector": input_podp_z_vector.iter().map(|z| fr_to_hex(z)).collect::<Vec<_>>(),
                "z_delta": fr_to_hex(&input_podp_z_delta),
                "z_beta": fr_to_hex(&input_podp_z_beta),
            },
        },
        // Inner proof and generators for on-chain hybrid verification
        "inner_proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "circuit_hash_raw": format!("0x{}", hex::encode(&circuit_hash)),
        "public_values_abi": format!("0x{}", hex::encode(&pub_values_bytes)),
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}
