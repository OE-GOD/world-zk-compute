//! Fuzz target for verify() with structured ProofBundle inputs.
//! Uses Arbitrary to generate structurally valid bundles (valid JSON shape,
//! valid hex strings) but with random content that exercises the full
//! verification pipeline. The verify() function must always return Ok or Err.
#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zkml_verifier::{verify, ProofBundle};

/// A fuzzable input that can be converted to a ProofBundle.
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// Raw bytes for the proof (will be hex-encoded with REM1 prefix)
    proof_body: Vec<u8>,
    /// Raw bytes for generators (will be hex-encoded)
    gens_body: Vec<u8>,
    /// Whether to include a valid-looking REM1 prefix
    use_valid_prefix: bool,
    /// Extra JSON fields
    model_hash: Option<String>,
}

fuzz_target!(|input: FuzzInput| {
    // Build proof_hex: optionally prepend "REM1" selector bytes
    let mut proof_bytes = Vec::new();
    if input.use_valid_prefix {
        proof_bytes.extend_from_slice(b"REM1");
    }
    proof_bytes.extend_from_slice(&input.proof_body);
    let proof_hex = format!("0x{}", hex::encode(&proof_bytes));

    let gens_hex = format!("0x{}", hex::encode(&input.gens_body));

    // Build a minimal (but structurally valid) DAG circuit description JSON
    let dag_desc = serde_json::json!({
        "numComputeLayers": 0,
        "numInputLayers": 0,
        "layerTypes": [],
        "numSumcheckRounds": [],
        "atomOffsets": [0],
        "atomTargetLayers": [],
        "atomCommitIdxs": [],
        "ptOffsets": [0],
        "ptData": [],
        "inputIsCommitted": [],
        "oracleProductOffsets": [0],
        "oracleResultIdxs": [],
        "oracleExprCoeffs": []
    });

    let bundle = ProofBundle {
        proof_hex,
        public_inputs_hex: String::new(),
        gens_hex,
        dag_circuit_description: dag_desc,
        model_hash: input.model_hash,
        timestamp: None,
        prover_version: None,
        circuit_hash: None,
    };

    // verify() must never panic — it uses catch_unwind internally
    let _ = verify(&bundle);
});
