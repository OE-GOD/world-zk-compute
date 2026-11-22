//! Malformed and hostile inputs must come back as `Err`, never as a panic,
//! an abort or an unbounded allocation.
//!
//! These cases replay the logic of the cargo-fuzz targets in `fuzz/` as plain
//! tests, so they run in CI without cargo-fuzz. Every case fails before the
//! expensive EC checks, so the whole file runs in a few seconds.

use std::time::{Duration, Instant};

use zkml_verifier::decode::ProofDecoder;
use zkml_verifier::test_utils::encode_circuit_desc_from_json;
use zkml_verifier::{verify, verify_hybrid, verify_raw, ProofBundle, VerifyError};

fn fixture() -> serde_json::Value {
    let path = format!(
        "{}/../../contracts/test/fixtures/phase1a_dag_fixture.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read fixture at {}: {}", path, e));
    serde_json::from_str(&json).unwrap()
}

fn hex_field(fixture: &serde_json::Value, key: &str) -> Vec<u8> {
    hex::decode(&fixture[key].as_str().unwrap()[2..]).unwrap()
}

fn bundle(proof: &[u8], gens: &[u8], desc: serde_json::Value) -> ProofBundle {
    ProofBundle {
        proof_hex: format!("0x{}", hex::encode(proof)),
        public_inputs_hex: String::new(),
        gens_hex: format!("0x{}", hex::encode(gens)),
        dag_circuit_description: desc,
        model_hash: None,
        timestamp: None,
        prover_version: None,
        circuit_hash: None,
    }
}

/// A 32-byte big-endian word holding `v`.
fn word(v: u64) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[24..].copy_from_slice(&v.to_be_bytes());
    b
}

/// Deterministic xorshift generator (no extra dev-dependency needed).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Lengths that have crashed length-prefixed decoders: just over the element
/// cap, larger than any buffer, larger than the address space, and the
/// values where `len * size_of::<T>()` overflows.
const HOSTILE_LENGTHS: [u64; 7] = [
    100_001,
    1 << 32,
    1 << 40,
    1 << 47,
    1 << 62,
    (1 << 63) + 1,
    u64::MAX,
];

#[test]
fn malformed_circuit_description_json_is_an_error() {
    let f = fixture();
    let proof = hex_field(&f, "proof_hex");
    let gens = hex_field(&f, "gens_hex");
    let good = f["dag_circuit_description"].clone();

    let mut cases: Vec<(&str, serde_json::Value)> = vec![
        ("empty object", serde_json::json!({})),
        ("null", serde_json::Value::Null),
        ("array", serde_json::json!([])),
        ("string", serde_json::json!("0x00")),
    ];
    let mut d = good.clone();
    d["numComputeLayers"] = serde_json::json!("88");
    cases.push(("count as string", d));
    let mut d = good.clone();
    d["numInputLayers"] = serde_json::json!(-1);
    cases.push(("negative count", d));
    let mut d = good.clone();
    d["layerTypes"] = serde_json::json!({"0": 1});
    cases.push(("array as object", d));
    let mut d = good.clone();
    d["ptData"][0] = serde_json::json!(1.5);
    cases.push(("float element", d));
    let mut d = good.clone();
    d["inputIsCommitted"][0] = serde_json::json!(1);
    cases.push(("bool as number", d));
    let mut d = good.clone();
    d["oracleExprCoeffs"][0] = serde_json::json!("0x01");
    cases.push(("short coefficient", d));
    let mut d = good.clone();
    d["oracleExprCoeffs"][0] = serde_json::json!("0xzz");
    cases.push(("non-hex coefficient", d));
    let mut d = good.clone();
    d.as_object_mut().unwrap().remove("atomOffsets");
    cases.push(("missing key", d));

    for (name, desc) in cases {
        let b = bundle(&proof, &gens, desc);
        let err = verify(&b).expect_err(name);
        assert!(
            matches!(err, VerifyError::BundleParse(_)),
            "verify, {name}: {err}"
        );
        assert!(
            err.to_string().contains("dag_circuit_description"),
            "verify, {name}: {err}"
        );
        let err = verify_hybrid(&b).expect_err(name);
        assert!(
            matches!(err, VerifyError::BundleParse(_)),
            "verify_hybrid, {name}: {err}"
        );
    }
}

#[test]
fn hostile_circuit_description_lengths_are_an_error() {
    let f = fixture();
    let proof = hex_field(&f, "proof_hex");
    let gens = hex_field(&f, "gens_hex");
    let desc = encode_circuit_desc_from_json(&f["dag_circuit_description"]);
    let num_layers = f["dag_circuit_description"]["numComputeLayers"]
        .as_u64()
        .unwrap() as usize;

    // Byte offsets of the first two array-length words: layerTypes (u8
    // elements) and numSumcheckRounds (usize elements).
    let layer_types_len = 64;
    let sumcheck_rounds_len = 64 + 32 + 32 * num_layers;

    for len in HOSTILE_LENGTHS {
        for off in [layer_types_len, sumcheck_rounds_len] {
            let mut d = desc.clone();
            d[off..off + 32].copy_from_slice(&word(len));
            let err = verify_raw(&proof, &gens, &d).expect_err("hostile length");
            assert!(
                matches!(err, VerifyError::DecodeError(_)),
                "len {len:#x} at {off}: {err}"
            );
        }
    }
}

#[test]
fn hostile_layer_counts_are_an_error() {
    let f = fixture();
    let proof = hex_field(&f, "proof_hex");
    let gens = hex_field(&f, "gens_hex");

    for count in [89u64, 1 << 32, 1 << 47, 1 << 62, u64::MAX] {
        let mut desc = f["dag_circuit_description"].clone();
        desc["numComputeLayers"] = serde_json::json!(count);
        let b = bundle(&proof, &gens, desc);
        let err = verify(&b).expect_err("too many layers");
        assert!(
            matches!(err, VerifyError::VerificationFailed(_)),
            "verify, {count:#x}: {err}"
        );
        let err = verify_hybrid(&b).expect_err("too many layers");
        assert!(
            matches!(err, VerifyError::VerificationFailed(_)),
            "verify_hybrid, {count:#x}: {err}"
        );
    }
}

#[test]
fn hostile_proof_and_generator_lengths_are_an_error() {
    let f = fixture();
    let proof = hex_field(&f, "proof_hex");
    let gens = hex_field(&f, "gens_hex");
    let desc = f["dag_circuit_description"].clone();

    for len in HOSTILE_LENGTHS {
        // Number of public-input sections: first word after selector + hash.
        let mut p = proof.clone();
        p[36..68].copy_from_slice(&word(len));
        let b = bundle(&p, &gens, desc.clone());
        assert!(matches!(verify(&b), Err(VerifyError::DecodeError(_))));
        assert!(matches!(
            verify_hybrid(&b),
            Err(VerifyError::DecodeError(_))
        ));

        // Generator count: first word of the generator blob.
        let mut g = gens.clone();
        g[0..32].copy_from_slice(&word(len));
        let b = bundle(&proof, &g, desc.clone());
        assert!(matches!(verify(&b), Err(VerifyError::DecodeError(_))));
        assert!(matches!(
            verify_hybrid(&b),
            Err(VerifyError::DecodeError(_))
        ));
    }
}

#[test]
fn truncated_and_random_inputs_are_an_error() {
    let f = fixture();
    let proof = hex_field(&f, "proof_hex");
    let gens = hex_field(&f, "gens_hex");
    let desc_json = f["dag_circuit_description"].clone();
    let desc = encode_circuit_desc_from_json(&desc_json);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let start = Instant::now();

    for _ in 0..300 {
        // Truncated proof: always fails in the decoder.
        let cut = rng.below(proof.len());
        assert!(verify_raw(&proof[..cut], &gens, &desc).is_err());
        let b = bundle(&proof[..cut], &gens, desc_json.clone());
        assert!(verify(&b).is_err());
        assert!(verify_hybrid(&b).is_err());

        // Truncated generators and circuit description.
        let cut = rng.below(gens.len());
        assert!(verify_raw(&proof, &gens[..cut], &desc).is_err());
        let cut = rng.below(desc.len());
        assert!(verify_raw(&proof, &gens, &desc[..cut]).is_err());

        // Random buffers, with and without the selector.
        let (a, b, c) = (rng.below(600), rng.below(600), rng.below(600));
        let mut p = rng.bytes(a);
        if p.len() >= 4 && rng.below(2) == 0 {
            p[..4].copy_from_slice(b"REM1");
        }
        let (g, d) = (rng.bytes(b), rng.bytes(c));
        assert!(verify_raw(&p, &g, &d).is_err());

        // The decoders on their own (fuzz_proof_decode).
        let _ = ProofDecoder::try_decode_pedersen_gens(&p);
        let _ = ProofDecoder::new(&p).try_decode_proof_for_dag();

        // A hostile value in one of the two leading length words of the proof
        // body (section count, element count of the first section).
        let mut p = proof.clone();
        let off = 36 + 32 * rng.below(2);
        p[off..off + 32].copy_from_slice(&word(HOSTILE_LENGTHS[rng.below(7)]));
        assert!(verify_raw(&p, &gens, &desc).is_err());
    }

    // Random bytes offered as a JSON bundle (fuzz_bundle_json).
    for _ in 0..300 {
        let n = rng.below(256);
        let data = rng.bytes(n);
        assert!(ProofBundle::from_bytes(&data).is_err());
    }

    assert!(
        start.elapsed() < Duration::from_secs(60),
        "malformed inputs took {:?} to reject",
        start.elapsed()
    );
}
