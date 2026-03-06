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
    assert!(encoded.len() > 0);
}
