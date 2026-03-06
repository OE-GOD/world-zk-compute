//! Shared test helpers for encoding circuit descriptions from JSON fixtures.
//!
//! Used by unit tests (lib.rs), integration tests (e2e_dag_verify.rs), and
//! the gen_calldata binary.  Gated behind `#[cfg(not(target_arch = "wasm32"))]`
//! so nothing here ends up in the WASM binary.

use crate::field::U256;

/// Encode a U256 as 32 big-endian bytes and append to buffer.
pub fn push_u256(buf: &mut Vec<u8>, val: U256) {
    buf.extend_from_slice(&val.to_be_bytes());
}

/// Encode a usize as a U256 and append.
pub fn push_usize(buf: &mut Vec<u8>, val: usize) {
    push_u256(buf, U256::from_u64(val as u64));
}

/// Encode the `dag_circuit_description` JSON object into the flat binary format
/// expected by `decode_circuit_description`.
pub fn encode_circuit_desc_from_json(desc: &serde_json::Value) -> Vec<u8> {
    let mut buf = Vec::new();

    fn push_hex(buf: &mut Vec<u8>, hex_str: &str) {
        let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let decoded = hex::decode(stripped).expect("invalid hex in oracleExprCoeffs");
        assert_eq!(decoded.len(), 32, "oracleExprCoeff must be 32 bytes");
        buf.extend_from_slice(&decoded);
    }

    // num_compute_layers, num_input_layers
    push_usize(
        &mut buf,
        desc["numComputeLayers"].as_u64().unwrap() as usize,
    );
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
