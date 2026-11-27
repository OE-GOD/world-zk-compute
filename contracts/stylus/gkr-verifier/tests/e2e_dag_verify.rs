//! End-to-end integration test for the Stylus GKR DAG verifier.
//!
//! Loads the real 88-layer XGBoost proof fixture (the same one validated by
//! 17 passing Solidity DAGBatchVerifierTest tests) and runs the complete
//! verification pipeline in Rust: proof decoding, transcript setup, compute
//! layer verification, and input layer verification.

use gkr_verifier::test_utils::encode_circuit_desc_from_json;

/// Full E2E verification of the 88-layer XGBoost DAG proof.
///
/// This test exercises the complete Stylus GKR verifier pipeline using
/// the same fixture validated by 17 Solidity DAGBatchVerifierTest tests:
///   - 88 compute layers
///   - 2 input layers (1 committed, 1 public)
///   - 34 input groups with 34 eval proofs
///   - 7 public value claims
///
/// Run with: cargo test --release --no-default-features --target aarch64-apple-darwin -- e2e
#[test]
fn test_e2e_dag_verify_xgboost() {
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../contracts/test/fixtures/phase1a_dag_fixture.json"
    );
    let fixture_str =
        std::fs::read_to_string(fixture_path).expect("failed to read phase1a_dag_fixture.json");
    let fixture: serde_json::Value =
        serde_json::from_str(&fixture_str).expect("failed to parse fixture JSON");

    // 1. proof_data: hex decode (already has REM1 selector prefix)
    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let proof_data = hex::decode(&proof_hex[2..]).expect("invalid proof_hex");
    assert_eq!(
        &proof_data[0..4],
        b"REM1",
        "fixture must start with REM1 selector"
    );

    // 2. public_inputs: decode (unused by verifier but passed through)
    let pub_hex = fixture["public_inputs_hex"].as_str().unwrap();
    let public_inputs = hex::decode(&pub_hex[2..]).unwrap_or_default();

    // 3. gens_data: Pedersen generators
    let gens_hex = fixture["gens_hex"].as_str().unwrap();
    let gens_data = hex::decode(&gens_hex[2..]).expect("invalid gens_hex");

    // 4. circuit_desc_data: encode from JSON topology
    let circuit_desc_data = encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);

    // Run verification
    let result = gkr_verifier::verify_dag_proof_inner(
        &proof_data,
        &public_inputs,
        &gens_data,
        &circuit_desc_data,
    );
    assert!(result, "DAG proof verification must succeed");
}

/// Verify the fixture data is well-formed before running verification.
#[test]
fn test_e2e_fixture_sanity_checks() {
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../contracts/test/fixtures/phase1a_dag_fixture.json"
    );
    let fixture_str =
        std::fs::read_to_string(fixture_path).expect("failed to read phase1a_dag_fixture.json");
    let fixture: serde_json::Value =
        serde_json::from_str(&fixture_str).expect("failed to parse fixture JSON");

    // Check proof starts with REM1
    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let proof_data = hex::decode(&proof_hex[2..]).unwrap();
    assert_eq!(&proof_data[0..4], b"REM1");

    // Check circuit description
    let desc = &fixture["dag_circuit_description"];
    assert_eq!(desc["numComputeLayers"].as_u64().unwrap(), 88);
    assert_eq!(desc["numInputLayers"].as_u64().unwrap(), 2);

    // Check we have the expected number of layers
    let layer_types = desc["layerTypes"].as_array().unwrap();
    assert_eq!(layer_types.len(), 88);

    // Check inputIsCommitted: [true, false]
    let committed = desc["inputIsCommitted"].as_array().unwrap();
    assert_eq!(committed.len(), 2);
    assert!(committed[0].as_bool().unwrap());
    assert!(!committed[1].as_bool().unwrap());

    // Gens should decode properly
    let gens_hex = fixture["gens_hex"].as_str().unwrap();
    let gens_data = hex::decode(&gens_hex[2..]).unwrap();
    assert!(
        gens_data.len() > 64,
        "gens should contain at least one generator"
    );

    // Circuit desc should encode without errors
    let encoded = encode_circuit_desc_from_json(desc);
    assert!(!encoded.is_empty());
}

// ------------------------------------------------------------------
// Rejections of altered fixtures. Each one fails while decoding or before
// the first sumcheck, so none of them runs the EC work of a full verify.
// ------------------------------------------------------------------

struct Fixture {
    proof: Vec<u8>,
    public_inputs: Vec<u8>,
    gens: Vec<u8>,
    desc: serde_json::Value,
}

fn load_fixture() -> Fixture {
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../contracts/test/fixtures/phase1a_dag_fixture.json"
    );
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_path).expect("failed to read phase1a_dag_fixture.json"),
    )
    .expect("failed to parse fixture JSON");
    let hex_field = |key: &str| hex::decode(&fixture[key].as_str().unwrap()[2..]);
    Fixture {
        proof: hex_field("proof_hex").expect("invalid proof_hex"),
        public_inputs: hex_field("public_inputs_hex").unwrap_or_default(),
        gens: hex_field("gens_hex").expect("invalid gens_hex"),
        desc: fixture["dag_circuit_description"].clone(),
    }
}

fn verify(f: &Fixture, proof: &[u8], gens: &[u8], desc: &[u8]) -> bool {
    gkr_verifier::verify_dag_proof_inner(proof, &f.public_inputs, gens, desc)
}

/// Byte offset of the first entry of `key` in the encoded description.
fn desc_entry_offset(desc: &serde_json::Value, key: &str) -> usize {
    let order = [
        "layerTypes",
        "numSumcheckRounds",
        "atomOffsets",
        "atomTargetLayers",
        "atomCommitIdxs",
        "ptOffsets",
        "ptData",
        "inputIsCommitted",
    ];
    let mut off = 64; // numComputeLayers, numInputLayers
    for k in order {
        if k == key {
            return off + 32; // skip the length word
        }
        off += 32 * (1 + desc[k].as_array().unwrap().len());
    }
    panic!("unknown key {key}");
}

#[test]
#[should_panic(expected = "trailing bytes after encoded data")]
fn test_proof_with_trailing_bytes_is_rejected() {
    let f = load_fixture();
    let mut proof = f.proof.clone();
    proof.extend_from_slice(&[0xAB; 64]);
    let desc = encode_circuit_desc_from_json(&f.desc);
    verify(&f, &proof, &f.gens, &desc);
}

#[test]
#[should_panic(expected = "trailing bytes after encoded data")]
fn test_gens_with_trailing_bytes_are_rejected() {
    let f = load_fixture();
    let mut gens = f.gens.clone();
    gens.extend_from_slice(&[0xCD; 64]);
    let desc = encode_circuit_desc_from_json(&f.desc);
    verify(&f, &f.proof, &gens, &desc);
}

#[test]
#[should_panic(expected = "trailing bytes after encoded data")]
fn test_description_with_trailing_bytes_is_rejected() {
    let f = load_fixture();
    let mut desc = encode_circuit_desc_from_json(&f.desc);
    desc.extend_from_slice(&[0xEF; 64]);
    verify(&f, &f.proof, &f.gens, &desc);
}

#[test]
#[should_panic(expected = "value does not fit in 64 bits")]
fn test_pt_data_entry_above_64_bits_is_rejected() {
    // ptData[0] + 2^64 used to be read as ptData[0].
    let f = load_fixture();
    let mut desc = encode_circuit_desc_from_json(&f.desc);
    desc[desc_entry_offset(&f.desc, "ptData") + 23] |= 1;
    verify(&f, &f.proof, &f.gens, &desc);
}

#[test]
#[should_panic(expected = "value does not fit in 8 bits")]
fn test_layer_type_above_255_is_rejected() {
    // layerTypes[2] = 1 + 256 used to be read as 1.
    let f = load_fixture();
    let mut desc = encode_circuit_desc_from_json(&f.desc);
    desc[desc_entry_offset(&f.desc, "layerTypes") + 2 * 32 + 30] |= 1;
    verify(&f, &f.proof, &f.gens, &desc);
}

#[test]
#[should_panic(expected = "bool value must be 0 or 1")]
fn test_input_is_committed_other_than_zero_or_one_is_rejected() {
    let f = load_fixture();
    let mut desc = encode_circuit_desc_from_json(&f.desc);
    desc[desc_entry_offset(&f.desc, "inputIsCommitted") + 31] = 2;
    verify(&f, &f.proof, &f.gens, &desc);
}

fn desc_with_rounds_changed(f: &Fixture, layer: usize, delta: i64) -> Vec<u8> {
    let mut d = f.desc.clone();
    let rounds = d["numSumcheckRounds"].as_array_mut().unwrap();
    let v = rounds[layer].as_i64().unwrap() + delta;
    rounds[layer] = serde_json::json!(v);
    encode_circuit_desc_from_json(&d)
}

#[test]
#[should_panic(expected = "sumcheck message count differs from numSumcheckRounds")]
fn test_round_count_above_proof_is_rejected() {
    let f = load_fixture();
    verify(&f, &f.proof, &f.gens, &desc_with_rounds_changed(&f, 5, 1));
}

#[test]
#[should_panic(expected = "sumcheck message count differs from numSumcheckRounds")]
fn test_output_layer_round_count_below_proof_is_rejected() {
    // Layer 0's count also sets how many output challenges are squeezed.
    let f = load_fixture();
    verify(&f, &f.proof, &f.gens, &desc_with_rounds_changed(&f, 0, -1));
}

#[test]
#[should_panic(expected = "sumcheck message count differs from numSumcheckRounds")]
fn test_hybrid_round_count_mismatch_is_rejected() {
    let f = load_fixture();
    let desc = desc_with_rounds_changed(&f, 87, 1);
    let _ = gkr_verifier::verify_dag_proof_hybrid_inner(&f.proof, &f.public_inputs, &f.gens, &desc);
}

/// Offsets in a REM1 proof (selector included) of the layer-proof count word,
/// of the start of each layer proof, and of the end of the last one. Walks the same layout as
/// ProofDecoder::decode_proof_common: circuit hash, public input sections,
/// output claim commitments, then the layer proofs.
fn layer_section(proof: &[u8]) -> (usize, Vec<usize>, usize) {
    let word = |o: usize| -> usize {
        let w = &proof[o..o + 32];
        assert!(w[..24].iter().all(|&b| b == 0), "count above 64 bits");
        u64::from_be_bytes(w[24..].try_into().unwrap()) as usize
    };
    let mut o = 4 + 32; // selector, circuit hash
    let sections = word(o);
    o += 32;
    for _ in 0..sections {
        o += 32 + 32 * word(o);
    }
    o += 32 + 64 * word(o); // output claim commitments
    let count_offset = o;
    let layers = word(o);
    o += 32;
    let mut starts = Vec::with_capacity(layers);
    for _ in 0..layers {
        starts.push(o);
        o += 64; // sum
        o += 32 + 64 * word(o); // messages
        o += 128; // PODP commitD, commitDDotA
        o += 32 + 32 * word(o) + 64; // PODP z vector, zDelta, zBeta
        o += 32 + 64 * word(o); // commitments
        o += 32 + (192 + 160) * word(o); // proofs of product
        let has_agg = word(o);
        o += 32;
        if has_agg == 1 {
            for size in [64, 128, 96] {
                o += 32 + size * word(o);
            }
        }
    }
    (count_offset, starts, o)
}

/// The fixture proof with a copy of its last layer proof appended as an
/// extra, unused layer proof, and the layer-proof count raised to match.
fn proof_with_extra_layer_proof(f: &Fixture) -> Vec<u8> {
    let (count_offset, starts, end) = layer_section(&f.proof);
    let layers = starts.len() as u64;
    assert_eq!(layers, f.desc["numComputeLayers"].as_u64().unwrap());
    let mut proof = f.proof.clone();
    proof[count_offset + 24..count_offset + 32].copy_from_slice(&(layers + 1).to_be_bytes());
    let last = f.proof[*starts.last().unwrap()..end].to_vec();
    proof.splice(end..end, last);
    proof
}

#[test]
#[should_panic(expected = "layer proof count differs from numComputeLayers")]
fn test_proof_with_extra_layer_proof_is_rejected() {
    // Only the first numComputeLayers layer proofs are read, so an extra one
    // used to be accepted as a second encoding of the same proof.
    let f = load_fixture();
    let proof = proof_with_extra_layer_proof(&f);
    let desc = encode_circuit_desc_from_json(&f.desc);
    verify(&f, &proof, &f.gens, &desc);
}

#[test]
#[should_panic(expected = "layer proof count differs from numComputeLayers")]
fn test_hybrid_proof_with_extra_layer_proof_is_rejected() {
    let f = load_fixture();
    let proof = proof_with_extra_layer_proof(&f);
    let desc = encode_circuit_desc_from_json(&f.desc);
    let _ = gkr_verifier::verify_dag_proof_hybrid_inner(&proof, &f.public_inputs, &f.gens, &desc);
}

#[test]
#[should_panic(expected = "numSumcheckRounds length differs from numComputeLayers")]
fn test_extra_sumcheck_round_count_is_rejected() {
    // An unused round count after the last compute layer used to be ignored.
    let f = load_fixture();
    let mut d = f.desc.clone();
    let rounds = d["numSumcheckRounds"].as_array_mut().unwrap();
    let last = rounds.last().unwrap().clone();
    rounds.push(last);
    verify(&f, &f.proof, &f.gens, &encode_circuit_desc_from_json(&d));
}
