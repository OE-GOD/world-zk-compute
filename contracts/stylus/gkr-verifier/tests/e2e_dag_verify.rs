//! End-to-end integration test for the Stylus GKR DAG verifier.
//!
//! Loads the real 88-layer XGBoost proof fixture (the same one validated by
//! 17 passing Solidity DAGBatchVerifierTest tests) and runs the complete
//! verification pipeline in Rust: proof decoding, transcript setup, compute
//! layer verification, and input layer verification.

use gkr_verifier::field::U256;

/// Helper: encode a U256 as 32 big-endian bytes and append to buffer.
fn push_u256(buf: &mut Vec<u8>, val: U256) {
    buf.extend_from_slice(&val.to_be_bytes());
}

/// Helper: encode a usize as a U256 and append.
fn push_usize(buf: &mut Vec<u8>, val: usize) {
    push_u256(buf, U256::from_u64(val as u64));
}

/// Encode the `dag_circuit_description` JSON object into the flat binary format
/// expected by the verifier's `decode_circuit_description`.
fn encode_circuit_desc_from_json(desc: &serde_json::Value) -> Vec<u8> {
    let mut buf = Vec::new();

    // Helper: push hex string as raw 32-byte value
    fn push_hex(buf: &mut Vec<u8>, hex_str: &str) {
        let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let decoded = hex::decode(stripped).expect("invalid hex in oracleExprCoeffs");
        assert_eq!(decoded.len(), 32, "oracleExprCoeff must be 32 bytes");
        buf.extend_from_slice(&decoded);
    }

    // num_compute_layers, num_input_layers
    push_usize(&mut buf, desc["numComputeLayers"].as_u64().unwrap() as usize);
    push_usize(&mut buf, desc["numInputLayers"].as_u64().unwrap() as usize);

    // Length-prefixed integer arrays
    for key in &[
        "layerTypes",
        "numSumcheckRounds",
        "atomOffsets",
        "atomTargetLayers",
        "atomCommitIdxs",
        "ptOffsets",
        "ptData",
    ] {
        let arr = desc[key].as_array().unwrap();
        push_usize(&mut buf, arr.len());
        for v in arr {
            push_usize(&mut buf, v.as_u64().unwrap() as usize);
        }
    }

    // inputIsCommitted (booleans -> 0 or 1)
    let bools = desc["inputIsCommitted"].as_array().unwrap();
    push_usize(&mut buf, bools.len());
    for v in bools {
        push_usize(&mut buf, if v.as_bool().unwrap() { 1 } else { 0 });
    }

    // More integer arrays
    for key in &["oracleProductOffsets", "oracleResultIdxs"] {
        let arr = desc[key].as_array().unwrap();
        push_usize(&mut buf, arr.len());
        for v in arr {
            push_usize(&mut buf, v.as_u64().unwrap() as usize);
        }
    }

    // oracleExprCoeffs (hex strings -> raw U256 bytes)
    let coeffs = desc["oracleExprCoeffs"].as_array().unwrap();
    push_usize(&mut buf, coeffs.len());
    for v in coeffs {
        push_hex(&mut buf, v.as_str().unwrap());
    }

    buf
}

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
    let fixture_str = std::fs::read_to_string(fixture_path)
        .expect("failed to read phase1a_dag_fixture.json");
    let fixture: serde_json::Value =
        serde_json::from_str(&fixture_str).expect("failed to parse fixture JSON");

    // 1. proof_data: hex decode (already has REM1 selector prefix)
    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let proof_data = hex::decode(&proof_hex[2..]).expect("invalid proof_hex");
    assert_eq!(&proof_data[0..4], b"REM1", "fixture must start with REM1 selector");

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
    let fixture_str = std::fs::read_to_string(fixture_path)
        .expect("failed to read phase1a_dag_fixture.json");
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
    assert!(gens_data.len() > 64, "gens should contain at least one generator");

    // Circuit desc should encode without errors
    let encoded = encode_circuit_desc_from_json(desc);
    assert!(encoded.len() > 0);
}
