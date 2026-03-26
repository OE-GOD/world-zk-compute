//! Generate E2E test fixtures for Solidity Fiat-Shamir bridge and proof decoding.
//!
//! Generates:
//! 1. ABI-encoded proof bytes (for HyraxProofDecoder testing)
//! 2. Exact Fq field element values absorbed into the Poseidon sponge
//! 3. Resulting squeeze challenges (for Fiat-Shamir validation)
//!
//! All values come from a SINGLE proof run, ensuring consistency for E2E testing.
//!
//! Usage: cargo run --release --bin gen_transcript_trace

use anyhow::Result;
use ff::PrimeField;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use remainder::layer::LayerDescription;
use serde_json::json;
use shared_types::config::{GKRCircuitProverConfig, GKRCircuitVerifierConfig};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::ECTranscript;
use shared_types::transcript::ec_transcript::ECTranscriptTrait;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::transcript::TranscriptSponge;
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

/// Convert Fq to big-endian hex string (for Solidity uint256)
fn fq_to_hex(val: &Fq) -> String {
    let repr = val.to_repr();
    let bytes: &[u8] = repr.as_ref();
    let mut be = bytes.to_vec();
    be.reverse();
    format!("0x{}", hex::encode(&be))
}

/// SHA-256 hash chain (1000 iterations) on field elements.
/// Matches Remainder_CE's sha256_hash_chain_on_field_elems.
fn sha256_hash_chain(elems: &[Fq]) -> (Fq, Fq) {
    use sha2::{Digest, Sha256};

    // Convert field elements to bytes (LE representation)
    let elems_bytes: Vec<u8> = elems
        .iter()
        .flat_map(|elem| elem.to_repr().as_ref().to_vec())
        .collect();

    // Initial SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&elems_bytes);
    let mut hash_result = hasher.finalize();

    // Iterate SHA-256 1000 times
    for _ in 0..1000 {
        let mut hasher = Sha256::new();
        hasher.update(hash_result);
        hash_result = hasher.finalize();
    }

    // Split 32-byte result into two 16-byte halves → field elements
    let mut first_half = [0u8; 32];
    let mut second_half = [0u8; 32];
    first_half[..16].copy_from_slice(&hash_result[..16]);
    second_half[..16].copy_from_slice(&hash_result[16..]);

    let fq1 = Fq::from_repr(first_half)
        .expect("first 16 bytes of circuit hash must be a valid Fq field element");
    let fq2 = Fq::from_repr(second_half)
        .expect("second 16 bytes of circuit hash must be a valid Fq field element");
    (fq1, fq2)
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

    // Verify with debug transcript to capture ALL operations
    let verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("gen_hyrax_verifiable_circuit");
    let verifier_committer = PedersenCommitter::new(
        512,
        "gen-test-proof Pedersen committer seed string for generating bases",
        None,
    );
    let mut debug_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new_with_debug("gen-test-proof transcript");

    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &verifier_config,
        &proof,
        &verifiable,
        &verifier_committer,
        &mut debug_transcript,
        &proof_config
    );
    eprintln!("Proof verified with debug transcript!");

    // === Manual transcript replay with raw PoseidonSponge ===
    // This captures ALL absorbed values for Solidity testing.

    // The circuit description hash values (observed from debug output above).
    // These are the SHA3-256 hash of the GKRCircuitDescription JSON,
    // split into two 16-byte halves and interpreted as Fq field elements (LE).
    //
    // From the debug output:
    //   Circuit desc hash part 1: 0x0000...5a9fc1776ad5b87f01d493983001d78f
    //   Circuit desc hash part 2: 0x0000...5daa4b26c457652c4f9715758acff1bc
    //
    // These are in the Fq Debug format (big-endian hex of the field element value).
    // For the sponge, we need the actual Fq values.

    // Parse from the known hex strings (observed from debug output)
    // Debug format is big-endian hex. The SHA3-256 hash is 32 bytes,
    // split into two 16-byte LE chunks. The debug output shows the value
    // in BE hex: 0x0000...{16 hex bytes for value}
    //
    // Circuit desc hash 1: 0x000000000000000000000000000000005a9fc1776ad5b87f01d493983001d78f
    // The value part is: 5a9fc1776ad5b87f01d493983001d78f (BE)
    // As LE bytes: 8f d7 01 30 98 93 d4 01 7f b8 d5 6a 77 c1 9f 5a (pad to 32 with zeros)
    let circuit_hash_1 = {
        let be_hex = "5a9fc1776ad5b87f01d493983001d78f";
        let be_bytes = hex::decode(be_hex)?;
        let mut le = [0u8; 32];
        for i in 0..16 {
            le[i] = be_bytes[15 - i];
        }
        Fq::from_repr(le).expect("constant circuit hash hex must produce a valid Fq element")
    };
    // Circuit desc hash 2: 0x000000000000000000000000000000005daa4b26c457652c4f9715758acff1bc
    let circuit_hash_2 = {
        let be_hex = "5daa4b26c457652c4f9715758acff1bc";
        let be_bytes = hex::decode(be_hex)?;
        let mut le = [0u8; 32];
        for i in 0..16 {
            le[i] = be_bytes[15 - i];
        }
        Fq::from_repr(le).expect("constant circuit hash hex must produce a valid Fq element")
    };

    // Verify our parsed values match the debug output
    eprintln!("\n=== Manually parsed circuit hash values ===");
    eprintln!("  hash_1: {}", fq_to_hex(&circuit_hash_1));
    eprintln!("  hash_2: {}", fq_to_hex(&circuit_hash_2));

    // Public input values (Fr → Fq via byte reinterpretation)
    // Fr(6) and Fr(20) become Fq(6) and Fq(20) since both fit in field
    let pub_val_1 = Fq::from(6u64);
    let pub_val_2 = Fq::from(20u64);

    // SHA-256 hash chain of [Fq(6), Fq(20)]
    let (pub_hash_1, pub_hash_2) = sha256_hash_chain(&[pub_val_1, pub_val_2]);

    eprintln!("\n=== Public input hash chain ===");
    eprintln!("  pub_val_1: {}", fq_to_hex(&pub_val_1));
    eprintln!("  pub_val_2: {}", fq_to_hex(&pub_val_2));
    eprintln!("  hash_chain_1: {}", fq_to_hex(&pub_hash_1));
    eprintln!("  hash_chain_2: {}", fq_to_hex(&pub_hash_2));

    // Hyrax input commitment EC points
    // Extract from proof data (sorted by LayerId, which matches proof order)
    let input_commits: Vec<(Fq, Fq)> = proof
        .hyrax_input_proofs
        .iter()
        .flat_map(|ip| {
            ip.input_commitment.iter().map(|pt| {
                use shared_types::curves::PrimeOrderCurve;
                pt.affine_coordinates()
                    .expect("EC point must have valid affine coordinates")
            })
        })
        .collect();

    eprintln!(
        "\n=== Hyrax input commitment EC points ({}) ===",
        input_commits.len()
    );
    let mut ec_fq_values: Vec<Fq> = Vec::new();
    for (i, (x, y)) in input_commits.iter().enumerate() {
        eprintln!("  point[{}].x: {}", i, fq_to_hex(x));
        eprintln!("  point[{}].y: {}", i, fq_to_hex(y));
        ec_fq_values.push(*x);
        ec_fq_values.push(*y);
    }

    // SHA-256 hash chain of EC point coordinates
    let (ec_hash_1, ec_hash_2) = sha256_hash_chain(&ec_fq_values);
    eprintln!("  ec_hash_chain_1: {}", fq_to_hex(&ec_hash_1));
    eprintln!("  ec_hash_chain_2: {}", fq_to_hex(&ec_hash_2));

    // === Full sponge replay ===
    let mut sponge = PoseidonSponge::<Fq>::default();

    // Step 1: Absorb circuit description hash (2 Fq elements)
    sponge.absorb(circuit_hash_1);
    sponge.absorb(circuit_hash_2);

    // Step 2: Absorb public input values + hash chain (4 Fq elements)
    sponge.absorb(pub_val_1);
    sponge.absorb(pub_val_2);
    sponge.absorb(pub_hash_1);
    sponge.absorb(pub_hash_2);

    // Intermediate test: squeeze after 6 elements (circuit hash + public inputs)
    let mut sponge_after_6 = sponge.clone();
    let challenge_after_6 = sponge_after_6.squeeze();
    eprintln!("\n=== Intermediate squeeze (after 6 absorbs, before EC points) ===");
    eprintln!("  squeeze = {}", fq_to_hex(&challenge_after_6));

    // Step 3: Absorb Hyrax input commitment EC points + hash chain
    for fq in &ec_fq_values {
        sponge.absorb(*fq);
    }
    sponge.absorb(ec_hash_1);
    sponge.absorb(ec_hash_2);

    // Step 4: Squeeze first challenge (should match "Challenge for claim on output")
    let first_challenge = sponge.squeeze();
    eprintln!("\n=== First challenge (after all initial absorbs) ===");
    eprintln!("  squeeze = {}", fq_to_hex(&first_challenge));
    eprintln!("  expected: 0x0ea43f7f1df793d4fbdf3bff6de4941404c0a3483f048b5065a4902629d64671");

    let matches = fq_to_hex(&first_challenge)
        == "0x0ea43f7f1df793d4fbdf3bff6de4941404c0a3483f048b5065a4902629d64671";
    eprintln!("  MATCH: {}", matches);

    // === ABI-encode the proof ===
    // Reconstruct raw 32-byte circuit hash from the two Fq halves
    let circuit_hash_raw: [u8; 32] = {
        let repr1 = circuit_hash_1.to_repr();
        let repr2 = circuit_hash_2.to_repr();
        let mut hash = [0u8; 32];
        hash[..16].copy_from_slice(&repr1.as_ref()[..16]);
        hash[16..].copy_from_slice(&repr2.as_ref()[..16]);
        hash
    };
    let abi_bytes = abi_encode::encode_hyrax_proof(&proof, &circuit_hash_raw)?;
    eprintln!(
        "\n=== ABI-encoded proof: {} bytes ({} uint256 slots) ===",
        abi_bytes.len(),
        (abi_bytes.len() - 4) / 32
    );

    // === ABI-encode the Pedersen generators ===
    let gens_bytes = abi_encode::encode_pedersen_gens(&verifier_committer)?;
    eprintln!(
        "ABI-encoded generators: {} bytes ({} uint256 slots)",
        gens_bytes.len(),
        gens_bytes.len() / 32
    );

    // === Verify claim_commitment decompression ===
    // Extract claim_commitment directly from proof struct (ground truth)
    let output_layer_proofs = &proof.circuit_proof.output_layer_proofs;
    let mut claim_commitment_coords: Vec<serde_json::Value> = Vec::new();
    for (layer_id, _mle_indices, olp) in output_layer_proofs {
        use shared_types::curves::PrimeOrderCurve;
        let (cx, cy) = olp
            .claim_commitment
            .affine_coordinates()
            .expect("claim_commitment not at infinity");
        eprintln!(
            "\n=== Output layer {:?} claim_commitment (direct affine) ===",
            layer_id
        );
        eprintln!("  x: {}", fq_to_hex(&cx));
        eprintln!("  y: {}", fq_to_hex(&cy));

        // Also check what serde_json produces (compressed hex)
        let json_val = serde_json::to_value(olp.claim_commitment)
            .expect("failed to serialize claim commitment to JSON");
        let hex_str = json_val
            .as_str()
            .expect("serialized EC point must be a JSON string");
        eprintln!("  compressed hex: {}", hex_str);
        eprintln!(
            "  compressed len: {} bytes",
            hex::decode(hex_str)
                .expect("compressed hex must be valid hex")
                .len()
        );

        // Decompress and compare
        let compressed_bytes = hex::decode(hex_str).expect("compressed hex must be valid hex");
        let (x_be, y_be) =
            abi_encode::decompress_point(&compressed_bytes).expect("failed to decompress EC point");
        let decompressed_x_hex = format!("0x{}", hex::encode(x_be));
        let decompressed_y_hex = format!("0x{}", hex::encode(y_be));
        eprintln!("  decompressed x: {}", decompressed_x_hex);
        eprintln!("  decompressed y: {}", decompressed_y_hex);
        let direct_x_hex = fq_to_hex(&cx);
        let direct_y_hex = fq_to_hex(&cy);
        let x_match = decompressed_x_hex == direct_x_hex;
        let y_match = decompressed_y_hex == direct_y_hex;
        eprintln!("  x MATCH: {}", x_match);
        eprintln!("  y MATCH: {}", y_match);

        claim_commitment_coords.push(json!({
            "x": direct_x_hex,
            "y": direct_y_hex,
            "compressed_hex": hex_str,
            "decompression_x_match": x_match,
            "decompression_y_match": y_match,
        }));
    }

    // === Dump PostSumcheckLayer structure for each intermediate layer ===
    // This gives us the product coefficients needed for oracle_eval computation
    let verifiable_ref = verifiable.get_gkr_circuit_description_ref();
    eprintln!("\n=== Circuit layer structure ===");
    eprintln!("  output_layers: {}", verifiable_ref.output_layers.len());
    eprintln!(
        "  intermediate_layers: {}",
        verifiable_ref.intermediate_layers.len()
    );
    eprintln!("  input_layers: {}", verifiable_ref.input_layers.len());

    // Serialize layer proofs JSON to inspect structure
    let proof_json = serde_json::to_value(&proof)?;
    let layer_proofs_json = proof_json["circuit_proof"]["layer_proofs"]
        .as_array()
        .expect("layer_proofs must be a JSON array");
    let mut layer_debug_info: Vec<serde_json::Value> = Vec::new();
    for (i, lp_entry) in layer_proofs_json.iter().enumerate() {
        let lp_arr = lp_entry
            .as_array()
            .expect("each layer proof entry must be a JSON array");
        let layer_id = &lp_arr[0];
        let lp = &lp_arr[1];
        let commitments = lp["commitments"]
            .as_array()
            .expect("commitments must be a JSON array");
        let pops = lp["proofs_of_product"]
            .as_array()
            .expect("proofs_of_product must be a JSON array");
        let agg = &lp["maybe_proof_of_claim_agg"];
        eprintln!("\n  Layer proof {} (id={}):", i, layer_id);
        eprintln!("    commitments: {}", commitments.len());
        eprintln!("    proofs_of_product: {}", pops.len());
        eprintln!("    has_claim_agg: {}", !agg.is_null());

        layer_debug_info.push(json!({
            "layer_id": layer_id.to_string(),
            "num_commitments": commitments.len(),
            "num_pops": pops.len(),
            "has_claim_agg": !agg.is_null(),
        }));
    }

    // === Custom verification pass to dump oracle_eval, dot_product, j_star ===
    // Replicate the verification logic to extract intermediate values
    use hyrax::gkr::layer::{evaluate_committed_psl, HyraxClaim};
    use hyrax::primitives::proof_of_sumcheck::ProofOfSumcheck;
    use remainder::layer::product::{new_with_values, PostSumcheckLayer};
    use shared_types::config::{
        global_config::global_claim_agg_strategy, ClaimAggregationStrategy,
    };
    use std::ops::Neg;

    let mut custom_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("gen-test-proof transcript");
    // Replay initial transcript setup (same as debug_transcript)
    {
        use remainder::prover::helpers::get_circuit_description_hash_as_field_elems;
        use shared_types::config::global_config::global_verifier_circuit_description_hash_type;
        let hash_elems = get_circuit_description_hash_as_field_elems(
            verifiable_ref,
            global_verifier_circuit_description_hash_type(),
        );
        custom_transcript.append_scalar_field_elems("Circuit description hash", &hash_elems);
        proof.public_inputs.iter().for_each(|(_, mle)| {
            custom_transcript.append_input_scalar_field_elems(
                "Public input layer values",
                &mle.as_ref()
                    .expect("public input MLE must be present in proof")
                    .f
                    .iter()
                    .collect::<Vec<_>>(),
            );
        });
        proof.hyrax_input_proofs.iter().for_each(|ip| {
            custom_transcript.append_input_ec_points(
                "Hyrax input layer commitment",
                ip.input_commitment.clone(),
            );
        });
        // FS challenges
        for fs_desc in &verifiable_ref.fiat_shamir_challenges {
            let num_evals = 1 << fs_desc.num_bits;
            custom_transcript.get_scalar_field_challenges("Verifier challenges", num_evals);
        }
    }

    // Output layer verification
    let mut claim_tracker: std::collections::HashMap<
        remainder::layer::LayerId,
        Vec<HyraxClaim<Fr, Bn256Point>>,
    > = std::collections::HashMap::new();
    for (_, _, olp) in &proof.circuit_proof.output_layer_proofs {
        let claim = hyrax::gkr::output_layer::HyraxOutputLayerProof::verify(
            olp,
            &verifiable_ref.output_layers[0],
            &mut custom_transcript,
        );
        claim_tracker.insert(claim.to_layer_id, vec![claim]);
    }

    // Intermediate layer verification with intermediate value dumping
    let mut layer_intermediates: Vec<serde_json::Value> = Vec::new();
    for (layer_id, layer_proof) in &proof.circuit_proof.layer_proofs {
        let layer_desc = verifiable_ref
            .intermediate_layers
            .iter()
            .find(|ld| ld.layer_id() == *layer_id)
            .unwrap_or_else(|| panic!("layer {:?} not found in circuit description", layer_id));

        let layer_claims_vec = claim_tracker
            .remove(&layer_desc.layer_id())
            .unwrap_or_else(|| {
                panic!(
                    "no claims found for layer {:?} in claim tracker",
                    layer_desc.layer_id()
                )
            });

        eprintln!("\n=== Layer {:?} verification intermediates ===", layer_id);
        eprintln!("  num_claims: {}", layer_claims_vec.len());
        eprintln!("  max_degree: {}", layer_desc.max_degree());

        // Get RLC random coefficients
        let random_coefficients = match global_claim_agg_strategy() {
            ClaimAggregationStrategy::RLC => custom_transcript
                .get_scalar_field_challenges("RLC Claim Agg Coefficients", layer_claims_vec.len()),
            _ => vec![Fr::one()],
        };

        // RLC eval (sum commitment)
        let rlc_eval = layer_claims_vec
            .iter()
            .zip(random_coefficients.iter())
            .fold(Bn256Point::zero(), |acc, (elem, rc)| {
                acc + elem.evaluation * *rc
            });

        // Verify sum matches
        assert_eq!(layer_proof.proof_of_sumcheck.sum, rlc_eval);

        // Absorb first message, collect bindings
        let num_rounds = layer_desc.sumcheck_round_indices().len();
        if num_rounds > 0 {
            custom_transcript.append_ec_point(
                "Commitment to sumcheck message",
                layer_proof.proof_of_sumcheck.messages[0],
            );
        }
        let mut bindings: Vec<Fr> = vec![];
        layer_proof
            .proof_of_sumcheck
            .messages
            .iter()
            .skip(1)
            .for_each(|msg| {
                let challenge =
                    custom_transcript.get_scalar_field_challenge("sumcheck round challenge");
                bindings.push(challenge);
                custom_transcript.append_ec_point("Commitment to sumcheck message", *msg);
            });
        if num_rounds > 0 {
            let final_chal =
                custom_transcript.get_scalar_field_challenge("sumcheck round challenge");
            bindings.push(final_chal);
        }

        eprintln!(
            "  bindings: {:?}",
            bindings
                .iter()
                .map(|b| format!("{:?}", b))
                .collect::<Vec<_>>()
        );

        // Absorb commitments
        custom_transcript.append_ec_points(
            "Commitments to all the layer's leaf values and intermediates",
            &layer_proof.commitments,
        );

        // Reconstruct PostSumcheckLayer
        let claim_points: Vec<Vec<Fr>> = layer_claims_vec.iter().map(|c| c.point.clone()).collect();
        let claim_points_refs: Vec<&[Fr]> = claim_points.iter().map(|v| v.as_slice()).collect();
        let psl_desc =
            layer_desc.get_post_sumcheck_layer(&bindings, &claim_points_refs, &random_coefficients);
        let psl: PostSumcheckLayer<Fr, Bn256Point> =
            new_with_values(&psl_desc, &layer_proof.commitments);

        // Dump product structure
        for (pi, product) in psl.0.iter().enumerate() {
            let coeff_repr = product.coefficient.to_repr();
            let mut coeff_be = coeff_repr.as_ref().to_vec();
            coeff_be.reverse();
            eprintln!(
                "  Product {}: coefficient=0x{}, num_intermediates={}",
                pi,
                hex::encode(&coeff_be),
                product.intermediates.len()
            );
            let result = product.get_result();
            let (rx, ry) = result
                .affine_coordinates()
                .unwrap_or((Fq::zero(), Fq::zero()));
            eprintln!("    result.x: {}", fq_to_hex(&rx));
            eprintln!("    result.y: {}", fq_to_hex(&ry));
        }

        // Compute oracle_eval
        let oracle_eval = evaluate_committed_psl(&psl);
        let (ox, oy) = oracle_eval
            .affine_coordinates()
            .unwrap_or((Fq::zero(), Fq::zero()));
        eprintln!("  oracle_eval.x: {}", fq_to_hex(&ox));
        eprintln!("  oracle_eval.y: {}", fq_to_hex(&oy));

        // Squeeze rhos and gammas (matching the proof_of_sumcheck verify)
        let n = layer_proof.proof_of_sumcheck.messages.len();
        let rhos = custom_transcript.get_scalar_field_challenges(
            "Proof of sumcheck RLC coefficients for batching rows",
            n + 1,
        );
        let gammas = custom_transcript.get_scalar_field_challenges(
            "Proof of sumcheck RLC coefficients for batching columns",
            n,
        );

        // Compute alpha
        let alpha = gammas
            .iter()
            .zip(layer_proof.proof_of_sumcheck.messages.iter())
            .fold(Bn256Point::zero(), |acc, (gamma, msg)| acc + *msg * *gamma);
        let (ax, ay) = alpha
            .affine_coordinates()
            .unwrap_or((Fq::zero(), Fq::zero()));
        eprintln!("  alpha.x: {}", fq_to_hex(&ax));
        eprintln!("  alpha.y: {}", fq_to_hex(&ay));

        // Compute j_star
        let j_star = ProofOfSumcheck::<Bn256Point>::calculate_j_star(
            &bindings,
            &rhos,
            &gammas,
            layer_desc.max_degree(),
        );
        eprintln!("  j_star ({} elements):", j_star.len());
        for (ji, jv) in j_star.iter().enumerate() {
            let jr = jv.to_repr();
            let mut jbe = jr.as_ref().to_vec();
            jbe.reverse();
            eprintln!("    j_star[{}]: 0x{}", ji, hex::encode(&jbe));
        }

        // Compute dot_product
        let dot_product =
            layer_proof.proof_of_sumcheck.sum * rhos[0] + oracle_eval * rhos[rhos.len() - 1].neg();
        let (dx, dy) = dot_product
            .affine_coordinates()
            .unwrap_or((Fq::zero(), Fq::zero()));
        eprintln!("  dot_product.x: {}", fq_to_hex(&dx));
        eprintln!("  dot_product.y: {}", fq_to_hex(&dy));

        // Dump rhos and gammas
        for (ri, rv) in rhos.iter().enumerate() {
            let rr = PrimeField::to_repr(rv);
            let mut rbe = rr.as_ref().to_vec();
            rbe.reverse();
            eprintln!("  rho[{}]: 0x{}", ri, hex::encode(&rbe));
        }
        for (gi, gv) in gammas.iter().enumerate() {
            let gr = PrimeField::to_repr(gv);
            let mut gbe = gr.as_ref().to_vec();
            gbe.reverse();
            eprintln!("  gamma[{}]: 0x{}", gi, hex::encode(&gbe));
        }

        layer_intermediates.push(json!({
            "layer_id": format!("{:?}", layer_id),
            "max_degree": layer_desc.max_degree(),
            "oracle_eval_x": fq_to_hex(&ox),
            "oracle_eval_y": fq_to_hex(&oy),
            "dot_product_x": fq_to_hex(&dx),
            "dot_product_y": fq_to_hex(&dy),
            "alpha_x": fq_to_hex(&ax),
            "alpha_y": fq_to_hex(&ay),
            "num_products": psl.0.len(),
        }));

        // Continue verification (verify PODP + PoP, extract claims)
        layer_proof.proof_of_sumcheck.verify(
            &rlc_eval,
            layer_desc.max_degree(),
            &psl,
            &bindings,
            &verifier_committer,
            &mut custom_transcript,
        );
        let product_triples: Vec<(Bn256Point, Bn256Point, Bn256Point)> = psl
            .0
            .iter()
            .filter_map(|p| p.get_product_triples())
            .flatten()
            .collect();
        product_triples
            .iter()
            .zip(layer_proof.proofs_of_product.iter())
            .for_each(|((x, y, z), pop)| {
                pop.verify(*x, *y, *z, &verifier_committer, &mut custom_transcript);
            });
        let new_claims: Vec<HyraxClaim<Fr, Bn256Point>> = psl
            .0
            .iter()
            .flat_map(hyrax::gkr::layer::get_claims_from_product)
            .collect();
        for claim in new_claims {
            claim_tracker
                .entry(claim.to_layer_id)
                .or_default()
                .push(claim);
        }
    }
    eprintln!("\n=== Custom verification pass complete ===");

    // Build the output JSON with all values
    let ec_points_hex: Vec<serde_json::Value> = input_commits
        .iter()
        .map(|(x, y)| json!({"x": fq_to_hex(x), "y": fq_to_hex(y)}))
        .collect();

    let output = json!({
        "proof_hex": format!("0x{}", hex::encode(&abi_bytes)),
        "proof_size_bytes": abi_bytes.len(),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "gens_size_bytes": gens_bytes.len(),
        "circuit_hash_raw": format!("0x{}", hex::encode(circuit_hash_raw)),
        "transcript_trace": {
            "circuit_hash_fq_1": fq_to_hex(&circuit_hash_1),
            "circuit_hash_fq_2": fq_to_hex(&circuit_hash_2),
            "public_input_fq_1": fq_to_hex(&pub_val_1),
            "public_input_fq_2": fq_to_hex(&pub_val_2),
            "public_input_hash_chain_1": fq_to_hex(&pub_hash_1),
            "public_input_hash_chain_2": fq_to_hex(&pub_hash_2),
            "input_commitment_points": ec_points_hex,
            "input_commitment_hash_chain_1": fq_to_hex(&ec_hash_1),
            "input_commitment_hash_chain_2": fq_to_hex(&ec_hash_2),
        },
        "output_claim_commitments": claim_commitment_coords,
        "challenges": {
            "after_circuit_hash_and_public_inputs": fq_to_hex(&challenge_after_6),
            "first_challenge_for_output_claim": fq_to_hex(&first_challenge),
            "first_challenge_matches_debug": matches,
        },
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    Ok(())
}
