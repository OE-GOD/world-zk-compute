//! Helpers for encoding circuit descriptions from JSON (used by `verify`, tests and benches).
use crate::error::{Result, VerifyError};
use crate::field::U256;

pub fn push_u256(buf: &mut Vec<u8>, val: U256) {
    buf.extend_from_slice(&val.to_be_bytes());
}

pub fn push_usize(buf: &mut Vec<u8>, val: usize) {
    push_u256(buf, U256::from_u64(val as u64));
}

/// Encode a JSON circuit description into the flat 32-byte-word layout.
///
/// Panics on a malformed description. Use [`try_encode_circuit_desc_from_json`]
/// for input that is not trusted.
pub fn encode_circuit_desc_from_json(desc: &serde_json::Value) -> Vec<u8> {
    try_encode_circuit_desc_from_json(desc).unwrap_or_else(|e| panic!("{e}"))
}

/// Encode a JSON circuit description into the flat 32-byte-word layout,
/// returning an error instead of panicking when a field is missing or has the
/// wrong type.
pub fn try_encode_circuit_desc_from_json(desc: &serde_json::Value) -> Result<Vec<u8>> {
    fn bad(key: &str, what: &str) -> VerifyError {
        VerifyError::BundleParse(format!("dag_circuit_description.{key}: {what}"))
    }
    fn uint(desc: &serde_json::Value, key: &str) -> Result<u64> {
        desc[key]
            .as_u64()
            .ok_or_else(|| bad(key, "expected an unsigned integer"))
    }
    fn array<'a>(desc: &'a serde_json::Value, key: &str) -> Result<&'a Vec<serde_json::Value>> {
        desc[key]
            .as_array()
            .ok_or_else(|| bad(key, "expected an array"))
    }
    fn push_uint_array(buf: &mut Vec<u8>, desc: &serde_json::Value, key: &str) -> Result<()> {
        let arr = array(desc, key)?;
        push_usize(buf, arr.len());
        for v in arr {
            let n = v
                .as_u64()
                .ok_or_else(|| bad(key, "expected unsigned integers"))?;
            push_u256(buf, U256::from_u64(n));
        }
        Ok(())
    }

    let mut buf = Vec::new();
    push_u256(&mut buf, U256::from_u64(uint(desc, "numComputeLayers")?));
    push_u256(&mut buf, U256::from_u64(uint(desc, "numInputLayers")?));
    for key in [
        "layerTypes",
        "numSumcheckRounds",
        "atomOffsets",
        "atomTargetLayers",
        "atomCommitIdxs",
        "ptOffsets",
        "ptData",
    ] {
        push_uint_array(&mut buf, desc, key)?;
    }
    let bools = array(desc, "inputIsCommitted")?;
    push_usize(&mut buf, bools.len());
    for v in bools {
        let b = v
            .as_bool()
            .ok_or_else(|| bad("inputIsCommitted", "expected booleans"))?;
        push_usize(&mut buf, usize::from(b));
    }
    for key in ["oracleProductOffsets", "oracleResultIdxs"] {
        push_uint_array(&mut buf, desc, key)?;
    }
    let coeffs = array(desc, "oracleExprCoeffs")?;
    push_usize(&mut buf, coeffs.len());
    for v in coeffs {
        let hex_str = v
            .as_str()
            .ok_or_else(|| bad("oracleExprCoeffs", "expected hex strings"))?;
        let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        let decoded = hex::decode(stripped).map_err(|e| bad("oracleExprCoeffs", &e.to_string()))?;
        if decoded.len() != 32 {
            return Err(bad("oracleExprCoeffs", "each coefficient must be 32 bytes"));
        }
        buf.extend_from_slice(&decoded);
    }
    Ok(buf)
}
