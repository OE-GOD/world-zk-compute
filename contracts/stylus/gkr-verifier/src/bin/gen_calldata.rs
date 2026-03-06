//! Generates ABI-encoded calldata for the Stylus GKR verifier.
//!
//! Loads the phase1a_dag_fixture.json, encodes the circuit description into
//! the flat binary format, and outputs ABI-encoded calldata for
//! `verifyDagProof(bytes,bytes,bytes,bytes)`.
//!
//! Run with:
//!   cargo run --no-default-features --target aarch64-apple-darwin --bin gen_calldata

use std::env;
use std::fs;

use gkr_verifier::field::U256;

/// Encode a U256 as 32 big-endian bytes and append to buffer.
fn push_u256(buf: &mut Vec<u8>, val: U256) {
    buf.extend_from_slice(&val.to_be_bytes());
}

/// Encode a usize as a U256 and append.
fn push_usize(buf: &mut Vec<u8>, val: usize) {
    push_u256(buf, U256::from_u64(val as u64));
}

/// Encode the `dag_circuit_description` JSON object into the flat binary format
/// expected by the verifier's `decode_circuit_description`.
fn encode_circuit_desc_from_json(desc: &serde_json::Value) -> Vec<u8> {
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

/// ABI-encode `bytes` value: length (32 bytes) + padded data.
fn abi_encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // Length as uint256
    let mut len_bytes = [0u8; 32];
    let len = data.len() as u64;
    len_bytes[24..32].copy_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&len_bytes);
    // Data padded to 32-byte boundary
    out.extend_from_slice(data);
    let pad = (32 - (data.len() % 32)) % 32;
    out.extend(std::iter::repeat(0u8).take(pad));
    out
}

fn main() {
    // Find fixture path relative to cargo manifest dir or via env var
    let fixture_path = env::var("FIXTURE_PATH").unwrap_or_else(|_| {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        format!(
            "{}/../.././../contracts/test/fixtures/phase1a_dag_fixture.json",
            manifest_dir
        )
    });

    // Canonicalize to handle the relative path properly
    let fixture_path = if fixture_path.contains("..") {
        // Build the correct path from the manifest dir
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        format!(
            "{}/../../../contracts/test/fixtures/phase1a_dag_fixture.json",
            manifest_dir
        )
    } else {
        fixture_path
    };

    eprintln!("Loading fixture from: {}", fixture_path);
    let fixture_str = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture: {}: {}", fixture_path, e));
    let fixture: serde_json::Value =
        serde_json::from_str(&fixture_str).expect("failed to parse fixture JSON");

    // 1. proof_data: hex decode (already has REM1 selector prefix)
    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let proof_data = hex::decode(&proof_hex[2..]).expect("invalid proof_hex");
    assert_eq!(&proof_data[0..4], b"REM1", "fixture must start with REM1");

    // 2. public_inputs: decode
    let pub_hex = fixture["public_inputs_hex"].as_str().unwrap();
    let public_inputs = hex::decode(&pub_hex[2..]).unwrap_or_default();

    // 3. gens_data: Pedersen generators
    let gens_hex = fixture["gens_hex"].as_str().unwrap();
    let gens_data = hex::decode(&gens_hex[2..]).expect("invalid gens_hex");

    // 4. circuit_desc_data: encode from JSON topology
    let circuit_desc_data = encode_circuit_desc_from_json(&fixture["dag_circuit_description"]);

    eprintln!("proof_data:        {} bytes", proof_data.len());
    eprintln!("public_inputs:     {} bytes", public_inputs.len());
    eprintln!("gens_data:         {} bytes", gens_data.len());
    eprintln!("circuit_desc_data: {} bytes", circuit_desc_data.len());

    // Compute function selector: keccak256("verifyDagProof(bytes,bytes,bytes,bytes)")[:4]
    // We compute it manually to avoid pulling in keccak crate
    // Pre-computed: 0x75927775
    // Let's verify by using a simple approach - the selector from export-abi
    let selector: [u8; 4] = {
        use std::process::Command;
        // Try to use cast to compute selector, fall back to hardcoded
        let output = Command::new("cast")
            .args(["sig", "verifyDagProof(bytes,bytes,bytes,bytes)"])
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let sig_hex = String::from_utf8(o.stdout).unwrap();
                let sig_hex = sig_hex.trim().strip_prefix("0x").unwrap_or(sig_hex.trim());
                let bytes = hex::decode(sig_hex).unwrap();
                [bytes[0], bytes[1], bytes[2], bytes[3]]
            }
            _ => {
                eprintln!("Warning: could not run `cast sig`, computing selector manually");
                // Use tiny_keccak if available, otherwise panic
                panic!("Please install foundry (cast) to compute the selector, or set SELECTOR env var");
            }
        }
    };

    eprintln!("selector: 0x{}", hex::encode(selector));

    // ABI encode: selector + 4 offsets + 4 (length-prefixed + padded) byte arrays
    let mut calldata = Vec::new();

    // 4-byte selector
    calldata.extend_from_slice(&selector);

    // The 4 `bytes` args use dynamic encoding:
    // offset_0, offset_1, offset_2, offset_3 (each uint256)
    // then data_0, data_1, data_2, data_3

    let encoded_proof = abi_encode_bytes(&proof_data);
    let encoded_pub = abi_encode_bytes(&public_inputs);
    let encoded_gens = abi_encode_bytes(&gens_data);
    let encoded_desc = abi_encode_bytes(&circuit_desc_data);

    // Offsets are relative to the start of the arguments (after selector)
    // First 4 slots are the offset pointers (4 * 32 = 128 bytes)
    let offset0: u64 = 4 * 32; // 128
    let offset1: u64 = offset0 + encoded_proof.len() as u64;
    let offset2: u64 = offset1 + encoded_pub.len() as u64;
    let offset3: u64 = offset2 + encoded_gens.len() as u64;

    // Write offsets as uint256
    for offset in &[offset0, offset1, offset2, offset3] {
        let mut buf = [0u8; 32];
        buf[24..32].copy_from_slice(&offset.to_be_bytes());
        calldata.extend_from_slice(&buf);
    }

    // Write encoded byte arrays
    calldata.extend_from_slice(&encoded_proof);
    calldata.extend_from_slice(&encoded_pub);
    calldata.extend_from_slice(&encoded_gens);
    calldata.extend_from_slice(&encoded_desc);

    let total_size = calldata.len();
    eprintln!(
        "Total calldata:    {} bytes ({:.1} KB)",
        total_size,
        total_size as f64 / 1024.0
    );
    eprintln!(
        "Calldata gas (16/byte): ~{:.1}M",
        (total_size as f64 * 16.0) / 1_000_000.0
    );

    // Output the full calldata as hex to stdout
    println!("0x{}", hex::encode(&calldata));

    // Also write to a file for the benchmark script
    let output_path = env::var("CALLDATA_OUTPUT").unwrap_or_else(|_| {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        format!("{}/calldata.hex", manifest_dir)
    });
    fs::write(&output_path, format!("0x{}", hex::encode(&calldata)))
        .unwrap_or_else(|e| eprintln!("Warning: could not write calldata file: {}", e));
    eprintln!("Calldata written to: {}", output_path);
}
