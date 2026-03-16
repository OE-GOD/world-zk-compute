//! Criterion benchmarks for the XGBoost-Remainder prover.
//!
//! Measures performance of:
//! - Model loading/parsing
//! - Witness generation (prepare_circuit_inputs)
//! - Circuit building (build_full_inference_circuit)
//! - Full prove-and-verify cycle (build_and_prove)
//!
//! Run with:
//!   cargo bench --manifest-path examples/xgboost-remainder/Cargo.toml --bench prover_bench
//!
//! Quick compilation check (no timing, runs each benchmark once):
//!   cargo bench --manifest-path examples/xgboost-remainder/Cargo.toml --bench prover_bench -- --test

use criterion::{criterion_group, criterion_main, Criterion};
use xgboost_remainder::circuit::{build_full_inference_circuit, prepare_circuit_inputs};
use xgboost_remainder::model;

/// Return a deterministic set of features for the sample model (5 features).
fn sample_features() -> Vec<f64> {
    vec![0.6, 0.2, 0.8, 0.5, 0.3]
}

/// JSON representation of the sample model, matching sample_model.json.
fn sample_model_json() -> &'static str {
    // Inline a minimal XgboostModel JSON for parse benchmarks.
    // This matches the structure returned by model::sample_model().
    r#"{
      "num_features": 5,
      "num_classes": 2,
      "max_depth": 3,
      "base_score": 0.0,
      "trees": [
        {
          "nodes": [
            {"feature_index":0,"threshold":0.5,"left_child":1,"right_child":2,"leaf_value":0.0,"is_leaf":false},
            {"feature_index":1,"threshold":0.3,"left_child":3,"right_child":4,"leaf_value":0.0,"is_leaf":false},
            {"feature_index":2,"threshold":0.7,"left_child":5,"right_child":6,"leaf_value":0.0,"is_leaf":false},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":-0.8,"is_leaf":true},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":0.2,"is_leaf":true},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":0.3,"is_leaf":true},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":0.9,"is_leaf":true}
          ]
        },
        {
          "nodes": [
            {"feature_index":3,"threshold":0.4,"left_child":1,"right_child":2,"leaf_value":0.0,"is_leaf":false},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":-0.5,"is_leaf":true},
            {"feature_index":4,"threshold":0.6,"left_child":3,"right_child":4,"leaf_value":0.0,"is_leaf":false},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":0.4,"is_leaf":true},
            {"feature_index":-1,"threshold":0.0,"left_child":0,"right_child":0,"leaf_value":0.7,"is_leaf":true}
          ]
        }
      ]
    }"#
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_model_loading(c: &mut Criterion) {
    let json = sample_model_json();

    c.bench_function("model_parse_json", |b| {
        b.iter(|| {
            let model: model::XgboostModel =
                serde_json::from_str(json).expect("failed to parse sample model JSON");
            criterion::black_box(model);
        });
    });
}

fn bench_witness_generation(c: &mut Criterion) {
    let mdl = model::sample_model();
    let features = sample_features();

    c.bench_function("witness_prepare_circuit_inputs", |b| {
        b.iter(|| {
            let inputs = prepare_circuit_inputs(&mdl, &features);
            criterion::black_box(inputs);
        });
    });

    // Sub-benchmarks for individual witness components
    c.bench_function("witness_compute_path_bits", |b| {
        b.iter(|| {
            let result = model::compute_path_bits(&mdl, &features);
            criterion::black_box(result);
        });
    });

    c.bench_function("witness_compute_leaf_sum", |b| {
        b.iter(|| {
            let result = model::compute_leaf_sum(&mdl, &features);
            criterion::black_box(result);
        });
    });

    c.bench_function("witness_collect_all_leaf_values", |b| {
        b.iter(|| {
            let result = model::collect_all_leaf_values(&mdl);
            criterion::black_box(result);
        });
    });

    let max_depth = mdl.trees.iter().map(model::tree_depth).max().unwrap_or(0);
    c.bench_function("witness_compute_comparison_tables", |b| {
        b.iter(|| {
            let result = model::compute_comparison_tables(&mdl, max_depth);
            criterion::black_box(result);
        });
    });

    c.bench_function("witness_compute_comparison_witness", |b| {
        b.iter(|| {
            let result =
                model::compute_comparison_witness(&mdl, &features, max_depth, model::DEFAULT_DECOMP_K);
            criterion::black_box(result);
        });
    });

    c.bench_function("witness_predict", |b| {
        b.iter(|| {
            let result = model::predict(&mdl, &features);
            criterion::black_box(result);
        });
    });
}

fn bench_circuit_build(c: &mut Criterion) {
    let mdl = model::sample_model();
    let features = sample_features();
    let inputs = prepare_circuit_inputs(&mdl, &features);

    c.bench_function("circuit_build_full_inference", |b| {
        b.iter(|| {
            let circuit = build_full_inference_circuit(
                inputs.num_trees_padded,
                inputs.max_depth,
                inputs.num_features_padded,
                &inputs.fi_padded,
                inputs.decomp_k,
            );
            criterion::black_box(circuit);
        });
    });
}

fn bench_prove_and_verify(c: &mut Criterion) {
    let mdl = model::sample_model();
    let features = sample_features();
    let predicted = model::predict(&mdl, &features);

    // build_and_prove includes: prepare_circuit_inputs + circuit build + prove + verify + ABI encode.
    // This is the end-to-end latency that matters most.
    //
    // NOTE: This benchmark is slow (seconds per iteration). Criterion will adapt
    // its iteration count accordingly. Use `--sample-size 10` to speed up CI.
    let mut group = c.benchmark_group("prove_and_verify");
    group.sample_size(10);
    group.bench_function("build_and_prove_sample_model", |b| {
        b.iter(|| {
            let result =
                xgboost_remainder::circuit::build_and_prove(&mdl, &features, predicted);
            assert!(result.is_ok(), "build_and_prove failed: {:?}", result.err());
            criterion::black_box(result);
        });
    });
    group.finish();

    // Warm-prover benchmark: CachedProver reuses circuit + Pedersen generators.
    // Measures per-request latency after one-time setup.
    let mut group = c.benchmark_group("warm_prover");
    group.sample_size(10);

    let cached_prover =
        xgboost_remainder::circuit::CachedProver::new(mdl.clone());

    group.bench_function("cached_prover_prove", |b| {
        b.iter(|| {
            let result = cached_prover.prove(&features, predicted);
            assert!(result.is_ok(), "cached prove failed: {:?}", result.err());
            criterion::black_box(result);
        });
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_model_loading,
    bench_witness_generation,
    bench_circuit_build,
    bench_prove_and_verify,
);

criterion_main!(benches);
