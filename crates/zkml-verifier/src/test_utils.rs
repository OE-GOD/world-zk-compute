//! Shared test helpers for encoding circuit descriptions from JSON fixtures.
use crate::field::U256;

pub fn push_u256(buf: &mut Vec<u8>, val: U256) {
    buf.extend_from_slice(&val.to_be_bytes());
}

pub fn push_usize(buf: &mut Vec<u8>, val: usize) {
    push_u256(buf, U256::from_u64(val as u64));
}

pub fn encode_circuit_desc_from_json(desc: &serde_json::Value) -> Vec<u8> {
    let mut buf = Vec::new();
    fn push_hex(buf: &mut Vec<u8>, hex_str: &str) {
        let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let decoded = hex::decode(stripped).expect("invalid hex in oracleExprCoeffs");
        assert_eq!(decoded.len(), 32, "oracleExprCoeff must be 32 bytes");
        buf.extend_from_slice(&decoded);
    }
    push_usize(
        &mut buf,
        desc["numComputeLayers"].as_u64().unwrap() as usize,
    );
    push_usize(&mut buf, desc["numInputLayers"].as_u64().unwrap() as usize);
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
    let bools = desc["inputIsCommitted"].as_array().unwrap();
    push_usize(&mut buf, bools.len());
    for v in bools {
        push_usize(&mut buf, if v.as_bool().unwrap() { 1 } else { 0 });
    }
    for key in &["oracleProductOffsets", "oracleResultIdxs"] {
        let arr = desc[key].as_array().unwrap();
        push_usize(&mut buf, arr.len());
        for v in arr {
            push_usize(&mut buf, v.as_u64().unwrap() as usize);
        }
    }
    let coeffs = desc["oracleExprCoeffs"].as_array().unwrap();
    push_usize(&mut buf, coeffs.len());
    for v in coeffs {
        push_hex(&mut buf, v.as_str().unwrap());
    }
    buf
}
