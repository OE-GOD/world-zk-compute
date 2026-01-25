//! Prover Performance Benchmarks
//!
//! Run with: cargo bench
//!
//! Measures:
//! - Proof generation time
//! - Input hashing performance
//! - Cache lookup performance
//! - Validation overhead

#![allow(unused)]

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use std::time::Duration;

/// Benchmark input hashing (SHA-256)
fn bench_input_hashing(c: &mut Criterion) {
    use sha2::{Sha256, Digest};

    let mut group = c.benchmark_group("input_hashing");

    for size in [1024, 10240, 102400, 1024000].iter() {
        let input = vec![0u8; *size];

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| {
                let mut hasher = Sha256::new();
                hasher.update(black_box(&input));
                hasher.finalize()
            });
        });
    }

    group.finish();
}

/// Benchmark hex encoding/decoding
fn bench_hex_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("hex_operations");

    let data = vec![0xABu8; 1000];
    let hex_string = hex::encode(&data);

    group.bench_function("encode_1000_bytes", |b| {
        b.iter(|| hex::encode(black_box(&data)));
    });

    group.bench_function("decode_1000_bytes", |b| {
        b.iter(|| hex::decode(black_box(&hex_string)));
    });

    group.finish();
}

/// Benchmark validation functions
fn bench_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("validation");

    let valid_address = "0x1234567890abcdef1234567890abcdef12345678";
    let valid_hash = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

    group.bench_function("validate_address", |b| {
        b.iter(|| {
            let addr = black_box(valid_address);
            addr.len() == 42
                && addr.starts_with("0x")
                && addr[2..].chars().all(|c| c.is_ascii_hexdigit())
        });
    });

    group.bench_function("validate_bytes32", |b| {
        b.iter(|| {
            let hash = black_box(valid_hash);
            hash.len() == 66
                && hash.starts_with("0x")
                && hash[2..].chars().all(|c| c.is_ascii_hexdigit())
        });
    });

    group.finish();
}

/// Benchmark JSON serialization
fn bench_json_serialization(c: &mut Criterion) {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct JobResult {
        request_id: u64,
        image_id: String,
        input_hash: String,
        output_hash: String,
        cycles: u64,
        proof_size: usize,
    }

    let result = JobResult {
        request_id: 12345,
        image_id: "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_string(),
        input_hash: "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string(),
        output_hash: "0x9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba".to_string(),
        cycles: 1_000_000,
        proof_size: 256 * 1024,
    };

    let mut group = c.benchmark_group("json");

    group.bench_function("serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&result)));
    });

    let json_str = serde_json::to_string(&result).unwrap();
    group.bench_function("deserialize", |b| {
        b.iter(|| serde_json::from_str::<JobResult>(black_box(&json_str)));
    });

    group.finish();
}

/// Benchmark cache key generation
fn bench_cache_key(c: &mut Criterion) {
    use sha2::{Sha256, Digest};

    let image_id = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let input = vec![0u8; 10240];

    c.bench_function("cache_key_generation", |b| {
        b.iter(|| {
            let mut hasher = Sha256::new();
            hasher.update(black_box(image_id.as_bytes()));
            hasher.update(black_box(&input));
            hex::encode(hasher.finalize())
        });
    });
}

/// Benchmark concurrent operations simulation
fn bench_concurrent_simulation(c: &mut Criterion) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let counter = Arc::new(AtomicU64::new(0));

    c.bench_function("atomic_increment", |b| {
        b.iter(|| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
    });
}

/// Benchmark memory allocation patterns
fn bench_memory_allocation(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");

    group.bench_function("alloc_1kb", |b| {
        b.iter(|| {
            let v: Vec<u8> = vec![0; 1024];
            black_box(v)
        });
    });

    group.bench_function("alloc_1mb", |b| {
        b.iter(|| {
            let v: Vec<u8> = vec![0; 1024 * 1024];
            black_box(v)
        });
    });

    group.bench_function("alloc_10mb", |b| {
        b.iter(|| {
            let v: Vec<u8> = vec![0; 10 * 1024 * 1024];
            black_box(v)
        });
    });

    group.finish();
}

/// Benchmark base64 encoding/decoding (for data URLs)
fn bench_base64(c: &mut Criterion) {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let mut group = c.benchmark_group("base64");

    let data = vec![0xABu8; 10240];
    let encoded = STANDARD.encode(&data);

    group.throughput(Throughput::Bytes(10240));

    group.bench_function("encode_10kb", |b| {
        b.iter(|| STANDARD.encode(black_box(&data)));
    });

    group.bench_function("decode_10kb", |b| {
        b.iter(|| STANDARD.decode(black_box(&encoded)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_input_hashing,
    bench_hex_operations,
    bench_validation,
    bench_json_serialization,
    bench_cache_key,
    bench_concurrent_simulation,
    bench_memory_allocation,
    bench_base64,
);

criterion_main!(benches);
