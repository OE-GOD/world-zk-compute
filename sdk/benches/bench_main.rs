use criterion::{black_box, criterion_group, criterion_main, Criterion};

use alloy::primitives::U256;
use alloy::sol_types::SolType;
use world_zk_sdk::abi::RemainderVerifier::DAGCircuitDescription;
use world_zk_sdk::{retry_with_backoff, RetryPolicy};

use std::time::Duration;

// ---------------------------------------------------------------------------
// ABI encoding benchmarks
// ---------------------------------------------------------------------------

/// Build a sample DAGCircuitDescription similar to the one used in unit tests.
fn sample_dag_description() -> DAGCircuitDescription {
    DAGCircuitDescription {
        numComputeLayers: U256::from(2),
        numInputLayers: U256::from(1),
        layerTypes: vec![0, 1],
        numSumcheckRounds: vec![U256::from(3), U256::from(4)],
        atomOffsets: vec![U256::from(0), U256::from(1), U256::from(2)],
        atomTargetLayers: vec![U256::from(1), U256::from(2)],
        atomCommitIdxs: vec![U256::from(0), U256::from(0)],
        ptOffsets: vec![U256::from(0), U256::from(1), U256::from(2)],
        ptData: vec![U256::from(0), U256::from(1)],
        inputIsCommitted: vec![true],
        oracleProductOffsets: vec![U256::from(0), U256::from(1), U256::from(2)],
        oracleResultIdxs: vec![U256::from(0), U256::from(0)],
        oracleExprCoeffs: vec![U256::from(1), U256::from(1)],
    }
}

/// Build a larger DAGCircuitDescription to stress-test encoding performance.
fn large_dag_description() -> DAGCircuitDescription {
    let n = 100;
    DAGCircuitDescription {
        numComputeLayers: U256::from(n),
        numInputLayers: U256::from(10),
        layerTypes: (0..n).map(|i| (i % 3) as u8).collect(),
        numSumcheckRounds: (0..n).map(|i| U256::from(i + 1)).collect(),
        atomOffsets: (0..=n).map(|i| U256::from(i * 2)).collect(),
        atomTargetLayers: (0..n * 2).map(|i| U256::from(i % n)).collect(),
        atomCommitIdxs: (0..n * 2).map(|i| U256::from(i % 5)).collect(),
        ptOffsets: (0..=n).map(|i| U256::from(i * 3)).collect(),
        ptData: (0..n * 3).map(|i| U256::from(i)).collect(),
        inputIsCommitted: (0..10).map(|i| i % 2 == 0).collect(),
        oracleProductOffsets: (0..=n).map(|i| U256::from(i)).collect(),
        oracleResultIdxs: (0..n).map(|i| U256::from(i % 10)).collect(),
        oracleExprCoeffs: (0..n).map(|_| U256::from(1)).collect(),
    }
}

fn bench_abi_encode_small(c: &mut Criterion) {
    let desc = sample_dag_description();
    c.bench_function("abi_encode_small_dag_description", |b| {
        b.iter(|| {
            let encoded = DAGCircuitDescription::abi_encode(black_box(&desc));
            black_box(encoded);
        })
    });
}

fn bench_abi_decode_small(c: &mut Criterion) {
    let desc = sample_dag_description();
    let encoded = DAGCircuitDescription::abi_encode(&desc);
    c.bench_function("abi_decode_small_dag_description", |b| {
        b.iter(|| {
            let decoded = DAGCircuitDescription::abi_decode(black_box(&encoded)).unwrap();
            black_box(decoded);
        })
    });
}

fn bench_abi_encode_large(c: &mut Criterion) {
    let desc = large_dag_description();
    c.bench_function("abi_encode_large_dag_description", |b| {
        b.iter(|| {
            let encoded = DAGCircuitDescription::abi_encode(black_box(&desc));
            black_box(encoded);
        })
    });
}

fn bench_abi_roundtrip(c: &mut Criterion) {
    let desc = sample_dag_description();
    c.bench_function("abi_roundtrip_small_dag_description", |b| {
        b.iter(|| {
            let encoded = DAGCircuitDescription::abi_encode(black_box(&desc));
            let decoded = DAGCircuitDescription::abi_decode(&encoded).unwrap();
            black_box(decoded);
        })
    });
}

// ---------------------------------------------------------------------------
// Retry logic benchmarks
// ---------------------------------------------------------------------------

fn bench_retry_immediate_success(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    let policy = RetryPolicy {
        max_retries: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(10),
        jitter: 0.0,
        timeout: None,
        total_timeout: None,
    };

    c.bench_function("retry_with_backoff_immediate_success", |b| {
        b.iter(|| {
            rt.block_on(async {
                let result =
                    retry_with_backoff(black_box(&policy), || async { Ok::<_, anyhow::Error>(42) })
                        .await;
                black_box(result.unwrap());
            })
        })
    });
}

fn bench_retry_policy_delay_computation(c: &mut Criterion) {
    // Benchmark just the RetryPolicy construction and delay computation,
    // which is the pure-CPU part of the retry logic.
    c.bench_function("retry_policy_delay_computation", |b| {
        b.iter(|| {
            let policy = RetryPolicy {
                max_retries: 10,
                base_delay: Duration::from_millis(100),
                max_delay: Duration::from_secs(30),
                jitter: 0.2,
                timeout: None,
                total_timeout: None,
            };
            // The delay_for method is private, but we can test the policy
            // construction and default values as overhead measurement.
            black_box(&policy);
        })
    });
}

fn bench_is_retryable(c: &mut Criterion) {
    let retryable_err = anyhow::anyhow!("connection refused by remote host");
    let non_retryable_err = anyhow::anyhow!("invalid argument: bad address format");

    c.bench_function("is_retryable_positive", |b| {
        b.iter(|| {
            let result = world_zk_sdk::is_retryable(black_box(&retryable_err));
            black_box(result);
        })
    });

    c.bench_function("is_retryable_negative", |b| {
        b.iter(|| {
            let result = world_zk_sdk::is_retryable(black_box(&non_retryable_err));
            black_box(result);
        })
    });
}

// ---------------------------------------------------------------------------
// Hash benchmarks
// ---------------------------------------------------------------------------

fn bench_compute_input_hash(c: &mut Criterion) {
    let features: Vec<f64> = (0..50).map(|i| i as f64 * 0.1).collect();
    c.bench_function("compute_input_hash_50_features", |b| {
        b.iter(|| {
            let hash = world_zk_sdk::compute_input_hash(black_box(&features));
            black_box(hash);
        })
    });
}

fn bench_compute_model_hash(c: &mut Criterion) {
    // Simulate a small model file (4KB)
    let model_bytes: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    c.bench_function("compute_model_hash_4kb", |b| {
        b.iter(|| {
            let hash = world_zk_sdk::compute_model_hash(black_box(&model_bytes));
            black_box(hash);
        })
    });
}

fn bench_compute_result_hash(c: &mut Criterion) {
    let scores = vec![0.1, 0.8, 0.1];
    c.bench_function("compute_result_hash_3_scores", |b| {
        b.iter(|| {
            let hash = world_zk_sdk::compute_result_hash(black_box(&scores));
            black_box(hash);
        })
    });
}

// ---------------------------------------------------------------------------
// Criterion groups and main
// ---------------------------------------------------------------------------

criterion_group!(
    abi_benches,
    bench_abi_encode_small,
    bench_abi_decode_small,
    bench_abi_encode_large,
    bench_abi_roundtrip,
);

criterion_group!(
    retry_benches,
    bench_retry_immediate_success,
    bench_retry_policy_delay_computation,
    bench_is_retryable,
);

criterion_group!(
    hash_benches,
    bench_compute_input_hash,
    bench_compute_model_hash,
    bench_compute_result_hash,
);

criterion_main!(abi_benches, retry_benches, hash_benches);
