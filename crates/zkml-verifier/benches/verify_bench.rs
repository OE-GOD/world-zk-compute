//! Criterion benchmarks for zkml-verifier.
//!
//! Run with: cargo bench -p zkml-verifier
//!
//! Requires `contracts/test/fixtures/phase1a_dag_fixture.json` for E2E benchmarks.

use criterion::{criterion_group, criterion_main, Criterion};

fn load_fixture() -> Option<serde_json::Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/test/fixtures/phase1a_dag_fixture.json"
    );
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn bench_verify_full(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_verify_full: fixture not found");
            return;
        }
    };

    let bundle = zkml_verifier::ProofBundle {
        proof_hex: fixture["proof_hex"].as_str().unwrap().to_string(),
        gens_hex: fixture["gens_hex"].as_str().unwrap().to_string(),
        public_inputs_hex: fixture
            .get("public_inputs_hex")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        dag_circuit_description: fixture["dag_circuit_description"].clone(),
        model_hash: None,
        timestamp: None,
        prover_version: None,
        circuit_hash: None,
    };

    c.bench_function("verify_dag_88layer", |b| {
        b.iter(|| {
            let result = zkml_verifier::verify(&bundle);
            assert!(result.is_ok());
        })
    });
}

fn bench_proof_decode(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_proof_decode: fixture not found");
            return;
        }
    };

    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let stripped = proof_hex.strip_prefix("0x").unwrap_or(proof_hex);
    let proof_bytes = hex::decode(stripped).unwrap();

    c.bench_function("decode_proof_blob", |b| {
        b.iter(|| {
            let mut dec = zkml_verifier::decode::ProofDecoder::new(&proof_bytes[4..]);
            let _ = dec.decode_proof_for_dag();
        })
    });
}

fn bench_bundle_parse(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_bundle_parse: fixture not found");
            return;
        }
    };

    let json_str = serde_json::to_string(&fixture).unwrap();

    c.bench_function("parse_bundle_json", |b| {
        b.iter(|| {
            let _bundle: zkml_verifier::ProofBundle = serde_json::from_str(&json_str).unwrap();
        })
    });
}

fn bench_hex_decode(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_hex_decode: fixture not found");
            return;
        }
    };

    let proof_hex = fixture["proof_hex"].as_str().unwrap().to_string();

    c.bench_function("hex_decode_proof", |b| {
        b.iter(|| {
            let stripped = proof_hex.strip_prefix("0x").unwrap_or(&proof_hex);
            let _ = hex::decode(stripped).unwrap();
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_verify_full, bench_proof_decode, bench_bundle_parse, bench_hex_decode
}
criterion_main!(benches);
