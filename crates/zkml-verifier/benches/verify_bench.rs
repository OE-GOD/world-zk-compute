//! Criterion benchmarks for zkml-verifier.
//!
//! Run with: cargo bench -p zkml-verifier
//!
//! Requires `contracts/test/fixtures/phase1a_dag_fixture.json` for E2E benchmarks.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

fn load_fixture() -> Option<serde_json::Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/test/fixtures/phase1a_dag_fixture.json"
    );
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn make_bundle(fixture: &serde_json::Value) -> zkml_verifier::ProofBundle {
    zkml_verifier::ProofBundle {
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
    }
}

/// Full end-to-end verification via the high-level `verify()` API.
fn bench_verify_full(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_verify_full: fixture not found");
            return;
        }
    };

    let bundle = make_bundle(&fixture);

    c.bench_function("verify_dag_88layer", |b| {
        b.iter(|| {
            let result = zkml_verifier::verify(&bundle);
            assert!(result.is_ok());
            assert!(result.unwrap().verified);
        })
    });
}

/// Hybrid verification (transcript replay only, no EC checks).
fn bench_verify_hybrid(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_verify_hybrid: fixture not found");
            return;
        }
    };

    let bundle = make_bundle(&fixture);

    c.bench_function("verify_dag_88layer_hybrid", |b| {
        b.iter(|| {
            let result = zkml_verifier::verify_hybrid(&bundle);
            assert!(result.is_ok());
        })
    });
}

/// Low-level `verify_raw()` from pre-decoded byte slices (no JSON parsing overhead).
fn bench_verify_raw(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_verify_raw: fixture not found");
            return;
        }
    };

    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let proof_data = hex::decode(proof_hex.strip_prefix("0x").unwrap_or(proof_hex)).unwrap();

    let gens_hex = fixture["gens_hex"].as_str().unwrap();
    let gens_data = hex::decode(gens_hex.strip_prefix("0x").unwrap_or(gens_hex)).unwrap();

    let circuit_desc_data = zkml_verifier::test_utils::encode_circuit_desc_from_json(
        &fixture["dag_circuit_description"],
    );

    c.bench_function("verify_raw_dag_88layer", |b| {
        b.iter(|| {
            let result = zkml_verifier::verify_raw(&proof_data, &gens_data, &circuit_desc_data);
            assert!(result.is_ok());
            assert!(result.unwrap().verified);
        })
    });
}

/// Binary proof blob decoding (no verification).
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

/// JSON bundle parsing (serde deserialization).
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

/// Hex string decoding.
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

/// Circuit description encoding from JSON to binary.
fn bench_circuit_desc_encode(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_circuit_desc_encode: fixture not found");
            return;
        }
    };

    let desc_json = fixture["dag_circuit_description"].clone();

    c.bench_function("encode_circuit_desc", |b| {
        b.iter(|| {
            let _ = zkml_verifier::test_utils::encode_circuit_desc_from_json(&desc_json);
        })
    });
}

/// Measure proof and bundle sizes (runs once, prints to stderr).
fn bench_proof_sizes(c: &mut Criterion) {
    let fixture = match load_fixture() {
        Some(f) => f,
        None => {
            eprintln!("Skipping bench_proof_sizes: fixture not found");
            return;
        }
    };

    let proof_hex = fixture["proof_hex"].as_str().unwrap();
    let proof_bytes = hex::decode(proof_hex.strip_prefix("0x").unwrap_or(proof_hex)).unwrap();
    let gens_hex = fixture["gens_hex"].as_str().unwrap();
    let gens_bytes = hex::decode(gens_hex.strip_prefix("0x").unwrap_or(gens_hex)).unwrap();
    let json_str = serde_json::to_string(&fixture).unwrap();
    let circuit_desc_data = zkml_verifier::test_utils::encode_circuit_desc_from_json(
        &fixture["dag_circuit_description"],
    );

    eprintln!("=== Proof / Bundle Sizes (88-layer XGBoost) ===");
    eprintln!(
        "  Proof blob (binary):        {} bytes ({:.1} KB)",
        proof_bytes.len(),
        proof_bytes.len() as f64 / 1024.0
    );
    eprintln!(
        "  Generators (binary):        {} bytes ({:.1} KB)",
        gens_bytes.len(),
        gens_bytes.len() as f64 / 1024.0
    );
    eprintln!(
        "  Circuit desc (binary):      {} bytes ({:.1} KB)",
        circuit_desc_data.len(),
        circuit_desc_data.len() as f64 / 1024.0
    );
    eprintln!(
        "  JSON bundle (full):         {} bytes ({:.1} KB)",
        json_str.len(),
        json_str.len() as f64 / 1024.0
    );
    eprintln!(
        "  Total binary payload:       {} bytes ({:.1} KB)",
        proof_bytes.len() + gens_bytes.len() + circuit_desc_data.len(),
        (proof_bytes.len() + gens_bytes.len() + circuit_desc_data.len()) as f64 / 1024.0
    );
    eprintln!("================================================");

    // Use BenchmarkId so the size info appears in criterion output
    let mut group = c.benchmark_group("proof_sizes");
    group.bench_with_input(
        BenchmarkId::new("proof_blob_bytes", proof_bytes.len()),
        &proof_bytes,
        |b, data| {
            b.iter(|| data.len());
        },
    );
    group.bench_with_input(
        BenchmarkId::new("json_bundle_bytes", json_str.len()),
        &json_str,
        |b, data| {
            b.iter(|| data.len());
        },
    );
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets =
        bench_verify_full,
        bench_verify_hybrid,
        bench_verify_raw,
        bench_proof_decode,
        bench_bundle_parse,
        bench_hex_decode,
        bench_circuit_desc_encode,
        bench_proof_sizes
}
criterion_main!(benches);
