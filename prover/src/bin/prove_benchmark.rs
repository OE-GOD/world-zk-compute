//! Proving Benchmark — Multi-Program Scaling Analysis
//!
//! Compares baseline (default prover) vs optimized (SegmentProver with tuned
//! segment size and thread count) across all 5 guest programs.
//!
//! Usage:
//!   cargo run --release --bin prove_benchmark
//!   cargo run --release --bin prove_benchmark -- --scaling --all-programs
//!   cargo run --release --bin prove_benchmark -- -p sybil-detector --scaling
//!   cargo run --release --bin prove_benchmark -- --samples 2000 --optimized-only
//!   cargo run --release --bin prove_benchmark -- --progressive
//!
//! Output: Detailed timing breakdown for each approach.

use anyhow::{anyhow, Result};
use k256::ecdsa::{SigningKey, signature::Signer};
use risc0_zkvm::{default_executor, default_prover, ExecutorEnv};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════════
// Config
// ═══════════════════════════════════════════════════════════════════════════════

const ALL_PROGRAMS: &[&str] = &[
    "xgboost-inference",
    "anomaly-detector",
    "sybil-detector",
    "rule-engine",
    "signature-verified",
];

struct BenchConfig {
    program: String,
    samples: usize,
    runs: usize,
    optimized_only: bool,
    baseline_only: bool,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            program: "xgboost-inference".to_string(),
            samples: 5,
            runs: 2,
            optimized_only: false,
            baseline_only: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input Serialization (mirrors generate-test-input.py)
//
// risc0's serde format serializes everything as u32 little-endian words.
// Each primitive is zero-extended or split into u32 words.
// ═══════════════════════════════════════════════════════════════════════════════

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, v: u64) {
    write_u32(buf, v as u32);
    write_u32(buf, (v >> 32) as u32);
}

fn write_i64(buf: &mut Vec<u8>, v: i64) {
    write_u64(buf, v as u64);
}

fn write_f64(buf: &mut Vec<u8>, v: f64) {
    let bits = v.to_bits();
    buf.extend_from_slice(&(bits as u32).to_le_bytes());
    buf.extend_from_slice(&((bits >> 32) as u32).to_le_bytes());
}

fn write_byte_array_16(buf: &mut Vec<u8>, data: &[u8; 16]) {
    for &b in data.iter() {
        write_u32(buf, b as u32);
    }
}

fn write_byte_array_20(buf: &mut Vec<u8>, data: &[u8; 20]) {
    for &b in data.iter() {
        write_u32(buf, b as u32);
    }
}

fn write_byte_array_32(buf: &mut Vec<u8>, data: &[u8; 32]) {
    for &b in data.iter() {
        write_u32(buf, b as u32);
    }
}

fn write_vec_u8(buf: &mut Vec<u8>, data: &[u8]) {
    write_u32(buf, data.len() as u32);
    for &b in data {
        write_u32(buf, b as u32);
    }
}

fn write_vec_f64(buf: &mut Vec<u8>, v: &[f64]) {
    write_u32(buf, v.len() as u32);
    for &x in v {
        write_f64(buf, x);
    }
}

fn write_vec_i64(buf: &mut Vec<u8>, v: &[i64]) {
    write_u32(buf, v.len() as u32);
    for &x in v {
        write_i64(buf, x);
    }
}

fn write_vec_vec_u8(buf: &mut Vec<u8>, items: &[Vec<u8>]) {
    write_u32(buf, items.len() as u32);
    for item in items {
        write_vec_u8(buf, item);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input Generators
// ═══════════════════════════════════════════════════════════════════════════════

struct TreeNode {
    is_leaf: u32,
    feature_idx: u32,
    threshold: f64,
    left_child: u32,
    right_child: u32,
    value: f64,
}

fn generate_xgboost_input(num_samples: usize) -> Vec<u8> {
    let mut buf = Vec::new();

    // Model: 3 trees, 2 features, 2 classes, base_score=0.0
    write_u32(&mut buf, 2); // num_features
    write_u32(&mut buf, 2); // num_classes
    write_f64(&mut buf, 0.0); // base_score

    // Trees
    let trees: Vec<Vec<TreeNode>> = vec![
        vec![
            TreeNode { is_leaf: 0, feature_idx: 0, threshold: 5.0, left_child: 1, right_child: 2, value: 0.0 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: -0.3 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: 0.4 },
        ],
        vec![
            TreeNode { is_leaf: 0, feature_idx: 1, threshold: 3.0, left_child: 1, right_child: 2, value: 0.0 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: -0.2 },
            TreeNode { is_leaf: 0, feature_idx: 0, threshold: 7.0, left_child: 3, right_child: 4, value: 0.0 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: 0.1 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: 0.5 },
        ],
        vec![
            TreeNode { is_leaf: 0, feature_idx: 0, threshold: 6.0, left_child: 1, right_child: 2, value: 0.0 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: -0.1 },
            TreeNode { is_leaf: 1, feature_idx: 0, threshold: 0.0, left_child: 0, right_child: 0, value: 0.3 },
        ],
    ];

    write_u32(&mut buf, trees.len() as u32);
    for tree in &trees {
        write_u32(&mut buf, tree.len() as u32);
        for node in tree {
            write_u32(&mut buf, node.is_leaf);
            write_u32(&mut buf, node.feature_idx);
            write_f64(&mut buf, node.threshold);
            write_u32(&mut buf, node.left_child);
            write_u32(&mut buf, node.right_child);
            write_f64(&mut buf, node.value);
        }
    }

    // Samples
    write_u32(&mut buf, num_samples as u32);
    for i in 0..num_samples {
        let mut id = [0u8; 32];
        id[0] = (i + 1) as u8;
        write_byte_array_32(&mut buf, &id);

        // Alternate normal/anomalous features
        let features = if i % 2 == 0 {
            vec![2.0 + i as f64 * 0.1, 1.0 + i as f64 * 0.1]
        } else {
            vec![8.0 + i as f64 * 0.1, 5.0 + i as f64 * 0.1]
        };
        write_vec_f64(&mut buf, &features);
    }

    // Threshold
    write_f64(&mut buf, 0.5);

    buf
}

/// DetectionInput { data_points: Vec<DataPoint>, threshold: f64, params: DetectionParams }
/// DataPoint { id: [u8;32], features: Vec<f64>, timestamp: u64 }
/// DetectionParams { window_size: usize, min_cluster_size: usize, distance_threshold: f64 }
fn generate_anomaly_detector_input(n: usize) -> Vec<u8> {
    let mut buf = Vec::new();

    // data_points: Vec<DataPoint>
    write_u32(&mut buf, n as u32);
    for i in 0..n {
        let mut id = [0u8; 32];
        id[0] = ((i + 1) & 0xFF) as u8;
        id[1] = (((i + 1) >> 8) & 0xFF) as u8;
        write_byte_array_32(&mut buf, &id);

        // features: Vec<f64> (3 features, alternating normal/anomalous)
        let features: Vec<f64> = if i % 3 != 0 {
            vec![100.0 + (i as f64) * 0.5, 105.0 - (i as f64) * 0.3, 98.0 + (i as f64) * 0.2]
        } else {
            vec![300.0 + (i as f64), 50.0 - (i as f64) * 0.5, 400.0 + (i as f64) * 2.0]
        };
        write_vec_f64(&mut buf, &features);

        // timestamp: u64
        write_u64(&mut buf, 1700000001 + i as u64);
    }

    // threshold: f64
    write_f64(&mut buf, 0.5);

    // params: DetectionParams
    write_u64(&mut buf, 10); // window_size: usize (serialized as u64)
    write_u64(&mut buf, 3);  // min_cluster_size: usize
    write_f64(&mut buf, 2.0); // distance_threshold: f64

    buf
}

/// SybilDetectionInput { registrations: Vec<Registration>, config: SybilConfig }
/// Registration { id: [u8;32], orb_id: [u8;16], timestamp: u64, geo_hash: u32,
///                session_duration_secs: u32, iris_score: u32, verification_latency_ms: u32 }
/// SybilConfig { time_window_secs: u64, geo_proximity_threshold: u32, min_cluster_size: u32,
///               velocity_threshold_kmh: u32, min_session_duration_secs: u32,
///               max_registrations_per_orb_per_hour: u32 }
///
/// O(N^2) pairwise heuristics — hits Segmented boundary at ~60-100 registrations
fn generate_sybil_detector_input(n: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let base_time: u64 = 1700000000;

    let orb_a: [u8; 16] = { let mut o = [0u8; 16]; o[0] = 0xAA; o };
    let orb_b: [u8; 16] = { let mut o = [0u8; 16]; o[0] = 0xBB; o };
    let orb_c: [u8; 16] = { let mut o = [0u8; 16]; o[0] = 0xCC; o };

    let normal_count = (n * 7) / 10; // 70% normal, 30% suspicious

    // registrations: Vec<Registration>
    write_u32(&mut buf, n as u32);
    for i in 0..n {
        // id: [u8; 32]
        let mut id = [0u8; 32];
        id[0] = ((i + 1) & 0xFF) as u8;
        id[1] = (((i + 1) >> 8) & 0xFF) as u8;
        write_byte_array_32(&mut buf, &id);

        if i < normal_count {
            // Normal: spread timestamps, varied orbs, good scores
            let orb = match i % 3 {
                0 => &orb_a,
                1 => &orb_b,
                _ => &orb_c,
            };
            write_byte_array_16(&mut buf, orb);
            write_u64(&mut buf, base_time + (i as u64) * 7200); // 2 hours apart
            write_u32(&mut buf, (1000 + (i as u32) * 10) % 10000); // varied geo_hash
            write_u32(&mut buf, 40 + (i as u32 % 20)); // session: 40-60s
            write_u32(&mut buf, 800 + (i as u32 % 200)); // iris: 800-1000
            write_u32(&mut buf, 1500 + (i as u32 % 1000)); // latency: 1500-2500ms
        } else {
            // Suspicious: temporal cluster at same orb, fast sessions
            let cluster_idx = i - normal_count;
            write_byte_array_16(&mut buf, &orb_a); // all same orb
            write_u64(&mut buf, base_time + 36000 + (cluster_idx as u64) * 30); // 30s apart
            write_u32(&mut buf, 1000); // same location
            write_u32(&mut buf, 5 + (cluster_idx as u32 % 5)); // session: 5-10s
            write_u32(&mut buf, 500 + (cluster_idx as u32 * 20) % 300); // iris: 500-700
            write_u32(&mut buf, 300 + (cluster_idx as u32 * 50) % 400); // latency: 300-600ms
        }
    }

    // config: SybilConfig
    write_u64(&mut buf, 300);  // time_window_secs
    write_u32(&mut buf, 100);  // geo_proximity_threshold
    write_u32(&mut buf, 2);    // min_cluster_size
    write_u32(&mut buf, 1000); // velocity_threshold_kmh
    write_u32(&mut buf, 15);   // min_session_duration_secs
    write_u32(&mut buf, 5);    // max_registrations_per_orb_per_hour

    buf
}

/// RuleEngineInput { records: Vec<Record>, rules: Vec<Rule>, aggregations: Vec<AggDef> }
/// Record { id: [u8;32], int_fields: Vec<i64>, str_fields: Vec<Vec<u8>> }
/// Rule { conditions: Vec<Condition>, combine: u32 }
/// Condition { cond_type: u32, field_idx: u32, compare_op: u32, int_value: i64, str_value: Vec<u8> }
/// AggDef { agg_type: u32, field_idx: u32, filter_rule: u32 }
fn generate_rule_engine_input(n: usize) -> Vec<u8> {
    let mut buf = Vec::new();

    // records: Vec<Record>
    write_u32(&mut buf, n as u32);
    for i in 0..n {
        let mut id = [0u8; 32];
        id[0] = (i & 0xFF) as u8;
        id[1] = ((i >> 8) & 0xFF) as u8;
        write_byte_array_32(&mut buf, &id);

        // int_fields: Vec<i64> (3 fields per record)
        let int_fields: Vec<i64> = match i % 3 {
            0 => vec![50 + (i as i64), 42, 10],
            1 => vec![200 + (i as i64), 42, -5],
            _ => vec![101 + (i as i64 % 50), 99, 0],
        };
        write_vec_i64(&mut buf, &int_fields);

        // str_fields: Vec<Vec<u8>> (2 fields per record)
        let str_fields: Vec<Vec<u8>> = match i % 3 {
            0 => vec![b"normal".to_vec(), b"clean".to_vec()],
            1 => vec![b"attack".to_vec(), b"suspicious".to_vec()],
            _ => vec![b"attack".to_vec(), b"clean".to_vec()],
        };
        write_vec_vec_u8(&mut buf, &str_fields);
    }

    // rules: Vec<Rule> (3 fixed rules from Python reference)
    write_u32(&mut buf, 3);

    // Rule 0: AND(int_field[0] > 100, str_field[0] glob "attack*")
    write_u32(&mut buf, 2); // 2 conditions
    write_u32(&mut buf, 0); write_u32(&mut buf, 0); write_u32(&mut buf, 2); // int_cmp, field 0, gt
    write_i64(&mut buf, 100); write_vec_u8(&mut buf, b"");
    write_u32(&mut buf, 5); write_u32(&mut buf, 0); write_u32(&mut buf, 0); // str_glob, field 0, unused
    write_i64(&mut buf, 0); write_vec_u8(&mut buf, b"attack*");
    write_u32(&mut buf, 0); // combine: AND

    // Rule 1: OR(int_field[1] == 42, int_field[2] < 0)
    write_u32(&mut buf, 2); // 2 conditions
    write_u32(&mut buf, 0); write_u32(&mut buf, 1); write_u32(&mut buf, 0); // int_cmp, field 1, eq
    write_i64(&mut buf, 42); write_vec_u8(&mut buf, b"");
    write_u32(&mut buf, 0); write_u32(&mut buf, 2); write_u32(&mut buf, 4); // int_cmp, field 2, lt
    write_i64(&mut buf, 0); write_vec_u8(&mut buf, b"");
    write_u32(&mut buf, 1); // combine: OR

    // Rule 2: AND(str_field[1] contains "suspicious")
    write_u32(&mut buf, 1); // 1 condition
    write_u32(&mut buf, 2); write_u32(&mut buf, 1); write_u32(&mut buf, 0); // str_contains, field 1, unused
    write_i64(&mut buf, 0); write_vec_u8(&mut buf, b"suspicious");
    write_u32(&mut buf, 0); // combine: AND

    // aggregations: Vec<AggDef> (2 fixed)
    write_u32(&mut buf, 2);
    write_u32(&mut buf, 1); write_u32(&mut buf, 0); write_u32(&mut buf, 0); // sum, field 0, rule 0
    write_u32(&mut buf, 3); write_u32(&mut buf, 1); write_u32(&mut buf, 0xFFFFFFFF); // max, field 1, all

    buf
}

/// SignedDetectionInput { data_points: Vec<DataPoint>, threshold: f64,
///                        signature: Vec<u8>, signer_pubkey: Vec<u8>, expected_signer: [u8;20] }
/// DataPoint { id: [u8;32], features: Vec<i64>, timestamp: u64 }
///
/// Uses k256 to generate keypair, compute SHA-256 data hash matching guest's
/// hash_detection_data(), sign, and derive address via SHA256(uncompressed[1..])[12..32]
fn generate_signature_verified_input(n: usize) -> Vec<u8> {
    // Generate secp256k1 keypair
    let signing_key = SigningKey::random(&mut rand::rngs::OsRng);
    let verifying_key = signing_key.verifying_key();

    // Compressed public key (33 bytes)
    let compressed_pubkey = verifying_key.to_encoded_point(true);

    // Derive address: SHA-256(uncompressed_pubkey[1..])[12..32]
    let uncompressed = verifying_key.to_encoded_point(false);
    let uncompressed_bytes = uncompressed.as_bytes();
    let mut addr_hasher = Sha256::new();
    addr_hasher.update(&uncompressed_bytes[1..]); // skip 0x04 prefix
    let addr_hash = addr_hasher.finalize();
    let mut expected_signer = [0u8; 20];
    expected_signer.copy_from_slice(&addr_hash[12..32]);

    let threshold: f64 = 0.5;

    // Build data points
    struct PointData {
        id: [u8; 32],
        features: Vec<i64>,
        timestamp: u64,
    }

    let mut data_points = Vec::with_capacity(n);
    for i in 0..n {
        let mut id = [0u8; 32];
        id[0] = ((i + 1) & 0xFF) as u8;
        id[1] = (((i + 1) >> 8) & 0xFF) as u8;

        let features = if i % 3 != 0 {
            vec![100 + (i as i64), 105 - (i as i64 % 50), 98 + (i as i64 % 30)]
        } else {
            vec![300 + (i as i64), 50 - (i as i64 % 30), 400 + (i as i64)]
        };

        data_points.push(PointData {
            id,
            features,
            timestamp: 1700000001 + i as u64,
        });
    }

    // Compute data hash matching guest's hash_detection_data()
    let mut hasher = Sha256::new();
    for point in &data_points {
        hasher.update(&point.id);
        for &feature in &point.features {
            hasher.update(&feature.to_le_bytes());
        }
        hasher.update(&point.timestamp.to_le_bytes());
    }
    hasher.update(&threshold.to_le_bytes());
    let data_hash: [u8; 32] = hasher.finalize().into();

    // Sign: k256's sign() hashes with SHA-256 internally, matching guest's verify()
    let signature: k256::ecdsa::Signature = signing_key.sign(&data_hash);
    let sig_bytes = signature.to_bytes();

    // Serialize in risc0 serde format
    let mut buf = Vec::new();

    // data_points: Vec<DataPoint>
    write_u32(&mut buf, n as u32);
    for point in &data_points {
        write_byte_array_32(&mut buf, &point.id);
        write_vec_i64(&mut buf, &point.features);
        write_u64(&mut buf, point.timestamp);
    }

    // threshold: f64
    write_f64(&mut buf, threshold);

    // signature: Vec<u8> (64 bytes)
    write_vec_u8(&mut buf, sig_bytes.as_ref());

    // signer_pubkey: Vec<u8> (33 bytes compressed)
    write_vec_u8(&mut buf, compressed_pubkey.as_bytes());

    // expected_signer: [u8; 20]
    write_byte_array_20(&mut buf, &expected_signer);

    buf
}

// ═══════════════════════════════════════════════════════════════════════════════
// Program Dispatch
// ═══════════════════════════════════════════════════════════════════════════════

fn generate_input(program: &str, n: usize) -> Vec<u8> {
    match program {
        "xgboost-inference" => generate_xgboost_input(n),
        "anomaly-detector" => generate_anomaly_detector_input(n),
        "sybil-detector" => generate_sybil_detector_input(n),
        "rule-engine" => generate_rule_engine_input(n),
        "signature-verified" => generate_signature_verified_input(n),
        _ => panic!("Unknown program: {}", program),
    }
}

fn image_id_for_program(program: &str) -> Option<&'static str> {
    match program {
        "xgboost-inference" => Some("1b1e0e6ea0bbefbbb2ccfe269a687b2e46efaa243f36664776b49dc15716e2ac"),
        "anomaly-detector" => Some("24c3af8225689d633ce0b02a61cb6a58fe656db1f31185eedd69f656a982bc95"),
        "signature-verified" => Some("28d93899974adcfe07ccad0c251b65e4308f265b6e296b9b81f1267bbf3ddd34"),
        "rule-engine" => Some("aa98ec7c0ddfaa7a7861f96f7edb1fe8ba247e86f598674a36ccc1db0551c8ec"),
        "sybil-detector" => Some("ee666bd16e310391f57cc2c65f301b06fcc018573913edf699ac3ad65db146e4"),
        _ => None,
    }
}

fn default_scaling_sizes(program: &str) -> Vec<usize> {
    match program {
        // O(N^2) pairwise heuristics — smaller max
        "sybil-detector" => vec![5, 10, 20, 50, 100, 200, 500],
        _ => vec![5, 10, 50, 100, 200, 500, 1000, 2000, 5000],
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Benchmark Runners
// ═══════════════════════════════════════════════════════════════════════════════

struct TimingResult {
    label: String,
    execute_time: Duration,
    prove_time: Duration,
    total_time: Duration,
    cycles: u64,
    segments: usize,
    seal_size: usize,
    journal_size: usize,
}

impl std::fmt::Display for TimingResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  {}", self.label)?;
        writeln!(f, "    Execute:   {:>8.2?}", self.execute_time)?;
        writeln!(f, "    Prove:     {:>8.2?}", self.prove_time)?;
        writeln!(f, "    Total:     {:>8.2?}", self.total_time)?;
        writeln!(f, "    Cycles:    {:>10}", self.cycles)?;
        writeln!(f, "    Segments:  {:>10}", self.segments)?;
        writeln!(f, "    Seal:      {:>10} bytes", self.seal_size)?;
        writeln!(f, "    Journal:   {:>10} bytes", self.journal_size)?;
        let throughput = self.cycles as f64 / self.prove_time.as_secs_f64();
        writeln!(f, "    Throughput: {:>9.0} cycles/sec", throughput)?;
        if self.segments > 0 {
            let per_seg = self.prove_time.as_secs_f64() / self.segments as f64;
            writeln!(f, "    Per-seg:   {:>8.2}s (amortized)", per_seg)?;
        }
        Ok(())
    }
}

/// Baseline: default_prover().prove() with no tuning.
fn run_baseline(elf: &[u8], input: &[u8]) -> Result<TimingResult> {
    let total_start = Instant::now();

    // Execute first (preflight)
    let exec_start = Instant::now();
    let env = ExecutorEnv::builder()
        .write_slice(input)
        .build()
        .map_err(|e| anyhow!("env: {}", e))?;
    let executor = default_executor();
    let session = executor.execute(env, elf).map_err(|e| anyhow!("exec: {}", e))?;
    let execute_time = exec_start.elapsed();
    let cycles = session.cycles();
    let segments = session.segments.len();

    // Prove with defaults
    let prove_start = Instant::now();
    let env = ExecutorEnv::builder()
        .write_slice(input)
        .build()
        .map_err(|e| anyhow!("env: {}", e))?;
    let prover = default_prover();
    let prove_info = prover.prove(env, elf).map_err(|e| anyhow!("prove: {}", e))?;
    let prove_time = prove_start.elapsed();

    let seal = extract_seal(&prove_info.receipt)?;
    let journal = prove_info.receipt.journal.bytes.clone();

    Ok(TimingResult {
        label: "Baseline (default prover, no tuning)".to_string(),
        execute_time,
        prove_time,
        total_time: total_start.elapsed(),
        cycles,
        segments,
        seal_size: seal.len(),
        journal_size: journal.len(),
    })
}

/// Optimized: Preflight + strategy selection + tuned proving.
///
/// This mirrors what FastProver.prove_fast() does:
/// - Programs < 20M cycles → Direct (same as baseline, no overhead)
/// - Programs 20M-100M → Segmented (tuned po2 + threads)
/// - Programs 100M+ → Continuation (aggressive parallelism)
fn run_optimized(elf: &[u8], input: &[u8]) -> Result<TimingResult> {
    let total_start = Instant::now();

    // Step 1: Preflight (execute without proving)
    let exec_start = Instant::now();
    let env = ExecutorEnv::builder()
        .write_slice(input)
        .build()
        .map_err(|e| anyhow!("env: {}", e))?;
    let executor = default_executor();
    let session = executor.execute(env, elf).map_err(|e| anyhow!("exec: {}", e))?;
    let execute_time = exec_start.elapsed();
    let cycles = session.cycles();
    let default_segments = session.segments.len();

    // Step 2: Strategy selection
    let strategy = if cycles < 20_000_000 {
        "Direct"
    } else if cycles < 100_000_000 {
        "Segmented"
    } else if cycles < 500_000_000 {
        "Continuation"
    } else {
        "TooComplex"
    };

    // Step 3: Configure based on strategy
    let (po2_override, thread_override) = match strategy {
        "Segmented" => {
            let po2 = optimal_po2(cycles);
            let est = estimate_segments_for_po2(cycles, po2);
            let threads = recommended_thread_count(est);
            println!("    Strategy: Segmented, po2={}, est_segments={}, threads={}", po2, est, threads);
            (Some(po2), Some(threads))
        }
        "Continuation" => {
            let po2 = 18u32; // max parallelism
            let est = estimate_segments_for_po2(cycles, po2);
            let threads = recommended_thread_count(est);
            println!("    Strategy: Continuation, po2={}, est_segments={}, threads={}", po2, est, threads);
            (Some(po2), Some(threads))
        }
        _ => {
            println!("    Strategy: {} (no tuning, same as baseline)", strategy);
            (None, None)
        }
    };

    // Step 4: Prove with appropriate configuration
    if let Some(threads) = thread_override {
        std::env::set_var("RAYON_NUM_THREADS", threads.to_string());
    }

    let prove_start = Instant::now();
    let mut env_builder = ExecutorEnv::builder();
    env_builder.write_slice(input);
    if let Some(po2) = po2_override {
        env_builder.segment_limit_po2(po2);
    }
    let env = env_builder.build().map_err(|e| anyhow!("env: {}", e))?;

    let prover = default_prover();
    let prove_info = prover.prove(env, elf).map_err(|e| anyhow!("prove: {}", e))?;
    let prove_time = prove_start.elapsed();

    let seal = extract_seal(&prove_info.receipt)?;
    let journal = prove_info.receipt.journal.bytes.clone();

    // Count actual segments in the proof
    let actual_segments = count_segments(&prove_info.receipt);

    if thread_override.is_some() {
        std::env::remove_var("RAYON_NUM_THREADS");
    }

    Ok(TimingResult {
        label: format!(
            "Optimized ({}, preflight={:?})",
            strategy, execute_time
        ),
        execute_time,
        prove_time,
        total_time: total_start.elapsed(),
        cycles,
        segments: actual_segments.unwrap_or(default_segments),
        seal_size: seal.len(),
        journal_size: journal.len(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tuning heuristics (copied from segment_prover.rs to keep binary standalone)
// ═══════════════════════════════════════════════════════════════════════════════

fn optimal_po2(cycles: u64) -> u32 {
    match cycles {
        0..=10_000_000 => 21,
        10_000_001..=50_000_000 => 20,
        50_000_001..=200_000_000 => 19,
        _ => 18,
    }
}

fn estimate_segments_for_po2(cycles: u64, po2: u32) -> usize {
    let cycles_per_segment = 1u64 << po2;
    ((cycles + cycles_per_segment - 1) / cycles_per_segment) as usize
}

fn recommended_thread_count(segment_count: usize) -> usize {
    let cpu_count = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);
    let available = cpu_count.saturating_sub(1).max(1);
    segment_count.max(1).min(available).min(16)
}

fn extract_seal(receipt: &risc0_zkvm::Receipt) -> Result<Vec<u8>> {
    use risc0_zkvm::InnerReceipt;
    let seal = match &receipt.inner {
        InnerReceipt::Composite(r) => {
            bincode::serialize(&r.segments).map_err(|e| anyhow!("seal: {}", e))?
        }
        InnerReceipt::Succinct(r) => {
            bincode::serialize(&r.seal).map_err(|e| anyhow!("seal: {}", e))?
        }
        InnerReceipt::Groth16(r) => {
            bincode::serialize(&r.seal).map_err(|e| anyhow!("seal: {}", e))?
        }
        _ => return Err(anyhow!("Unknown receipt type")),
    };
    Ok(seal)
}

fn count_segments(receipt: &risc0_zkvm::Receipt) -> Option<usize> {
    use risc0_zkvm::InnerReceipt;
    match &receipt.inner {
        InnerReceipt::Composite(r) => Some(r.segments.len()),
        _ => None,
    }
}

fn format_duration_short(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else if secs < 3600.0 {
        format!("{:.1}m", secs / 60.0)
    } else {
        format!("{:.1}h", secs / 3600.0)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scaling Analysis — Execute-only, fast N → cycles mapping
// ═══════════════════════════════════════════════════════════════════════════════

fn run_scaling_analysis(program: &str, elf: &[u8], sizes: &[usize]) -> Result<()> {
    println!();
    println!("── {} ──", program);
    println!("{:>8} {:>12} {:>8} {:>10} {:>8} {:>8} {:>8}",
        "N", "Cycles", "Segments", "ExecTime", "Opt_po2", "EstSegs", "Strategy");
    println!("{}", "─".repeat(72));

    for &n in sizes {
        let input = generate_input(program, n);
        let start = Instant::now();
        let env = ExecutorEnv::builder()
            .write_slice(&input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let executor = default_executor();
        match executor.execute(env, elf) {
            Ok(session) => {
                let elapsed = start.elapsed();
                let cycles = session.cycles();
                let segs = session.segments.len();
                let po2 = optimal_po2(cycles);
                let est_segs = estimate_segments_for_po2(cycles, po2);
                let strategy = if cycles < 20_000_000 {
                    "Direct"
                } else if cycles < 100_000_000 {
                    "Segmented"
                } else if cycles < 500_000_000 {
                    "Continuation"
                } else {
                    "TooComplex"
                };
                println!("{:>8} {:>12} {:>8} {:>10.1?} {:>8} {:>8} {:>10}",
                    n, cycles, segs, elapsed, po2, est_segs, strategy);
            }
            Err(e) => {
                println!("{:>8}  ERROR: {}", n, e);
                break;
            }
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Progressive Mode — Actual proving at increasing sizes
// ═══════════════════════════════════════════════════════════════════════════════

fn run_progressive(program: &str, elf: &[u8], sizes: &[usize]) -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║              Progressive Proving Benchmark (optimized)                       ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Program: {}", program);
    println!("CPUs: {}", std::thread::available_parallelism().map(|p| p.get()).unwrap_or(0));
    println!();

    // Header
    println!("{:>7} {:>10} {:>5} {:>10} {:>8} {:>9} {:>10} {:>8} {:>8}",
        "N", "Cycles", "Segs", "Strategy", "po2", "Prove", "Total", "cy/sec", "seg/s");
    println!("{}", "─".repeat(88));

    let mut prev_prove_time: Option<Duration> = None;
    let mut prev_cycles: Option<u64> = None;

    for &n in sizes {
        let input = generate_input(program, n);

        // Preflight first
        let env = ExecutorEnv::builder()
            .write_slice(&input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let executor = default_executor();
        let session = executor.execute(env, elf).map_err(|e| anyhow!("exec: {}", e))?;
        let cycles = session.cycles();
        let default_segments = session.segments.len();

        let strategy = if cycles < 20_000_000 {
            "Direct"
        } else if cycles < 100_000_000 {
            "Segmented"
        } else if cycles < 500_000_000 {
            "Continuation"
        } else {
            // Skip too-complex programs
            println!("{:>7} {:>10} {:>5} {:>10}", n, cycles, default_segments, "SKIP");
            continue;
        };

        // Configure
        let (po2_override, thread_override) = match strategy {
            "Segmented" => {
                let po2 = optimal_po2(cycles);
                let threads = recommended_thread_count(estimate_segments_for_po2(cycles, po2));
                (Some(po2), Some(threads))
            }
            "Continuation" => {
                let po2 = 18u32;
                let threads = recommended_thread_count(estimate_segments_for_po2(cycles, po2));
                (Some(po2), Some(threads))
            }
            _ => (None, None),
        };

        if let Some(threads) = thread_override {
            std::env::set_var("RAYON_NUM_THREADS", threads.to_string());
        }

        // Prove
        let prove_start = Instant::now();
        let mut env_builder = ExecutorEnv::builder();
        env_builder.write_slice(&input);
        if let Some(po2) = po2_override {
            env_builder.segment_limit_po2(po2);
        }
        let env = env_builder.build().map_err(|e| anyhow!("env: {}", e))?;

        let prover = default_prover();
        let prove_info = prover.prove(env, elf).map_err(|e| anyhow!("prove: {}", e))?;
        let prove_time = prove_start.elapsed();
        let total_time = prove_start.elapsed();

        let actual_segments = count_segments(&prove_info.receipt).unwrap_or(default_segments);

        if thread_override.is_some() {
            std::env::remove_var("RAYON_NUM_THREADS");
        }

        // Compute metrics
        let throughput = cycles as f64 / prove_time.as_secs_f64();
        let seg_per_sec = actual_segments as f64 / prove_time.as_secs_f64();
        let po2_str = po2_override.map(|p| p.to_string()).unwrap_or_else(|| "def".to_string());

        println!("{:>7} {:>10} {:>5} {:>10} {:>8} {:>9} {:>10} {:>8.0} {:>8.2}",
            n, cycles, actual_segments, strategy, po2_str,
            format_duration_short(prove_time),
            format_duration_short(total_time),
            throughput, seg_per_sec);

        // Show scaling ratio vs previous
        if let (Some(prev_t), Some(prev_c)) = (prev_prove_time, prev_cycles) {
            let time_ratio = prove_time.as_secs_f64() / prev_t.as_secs_f64();
            let cycle_ratio = cycles as f64 / prev_c as f64;
            let efficiency = cycle_ratio / time_ratio; // >1 means sublinear scaling (good)
            if efficiency > 1.05 {
                println!("         └── {:.1}x more cycles but only {:.1}x slower (scaling efficiency: {:.2}x)",
                    cycle_ratio, time_ratio, efficiency);
            } else if efficiency < 0.95 {
                println!("         └── {:.1}x more cycles, {:.1}x slower (overhead at this scale)",
                    cycle_ratio, time_ratio);
            }
        }

        prev_prove_time = Some(prove_time);
        prev_cycles = Some(cycles);
    }

    println!();
    println!("Strategy thresholds:");
    println!("  Direct:       < 20M cycles (no segment tuning)");
    println!("  Segmented:    20M - 100M cycles (tuned po2, parallel segments)");
    println!("  Continuation: 100M - 500M cycles (po2=18, max parallelism)");

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main
// ═══════════════════════════════════════════════════════════════════════════════

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let mut config = BenchConfig::default();
    let mut scaling_mode = false;
    let mut progressive_mode = false;
    let mut all_programs = false;
    let mut progressive_sizes: Vec<usize> = vec![];

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--program" | "-p" => {
                config.program = args.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "--samples" | "-s" => {
                config.samples = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(5);
                i += 2;
            }
            "--runs" | "-r" => {
                config.runs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(2);
                i += 2;
            }
            "--scaling" => {
                scaling_mode = true;
                i += 1;
            }
            "--progressive" => {
                progressive_mode = true;
                i += 1;
            }
            "--all-programs" => {
                all_programs = true;
                i += 1;
            }
            "--sizes" => {
                // Custom sizes for progressive mode: --sizes 100,500,1000,2000
                if let Some(s) = args.get(i + 1) {
                    progressive_sizes = s.split(',')
                        .filter_map(|x| x.trim().parse().ok())
                        .collect();
                }
                i += 2;
            }
            "--optimized-only" => {
                config.optimized_only = true;
                i += 1;
            }
            "--baseline-only" => {
                config.baseline_only = true;
                i += 1;
            }
            "--help" | "-h" => {
                println!("Usage: prove_benchmark [OPTIONS]");
                println!();
                println!("Modes:");
                println!("  (default)        Head-to-head: baseline vs optimized");
                println!("  --scaling        Execute-only analysis at many sample counts");
                println!("  --progressive    Actual proving at increasing sizes (optimized)");
                println!();
                println!("Options:");
                println!("  -p, --program <name>    Program to bench [default: xgboost-inference]");
                println!("                          (xgboost-inference, anomaly-detector, sybil-detector,");
                println!("                           rule-engine, signature-verified)");
                println!("  -s, --samples <N>       Number of samples [default: 5]");
                println!("  -r, --runs <N>          Runs per method [default: 2]");
                println!("  --all-programs           Run scaling analysis for all 5 programs");
                println!("  --optimized-only        Skip baseline (faster for large programs)");
                println!("  --baseline-only         Skip optimized");
                println!("  --sizes 100,500,1000    Custom sizes for progressive mode");
                return Ok(());
            }
            _ => {
                i += 1;
            }
        }
    }

    // ── Scaling mode: execute-only analysis ──────────────────────────
    if scaling_mode {
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║              Scaling Analysis (execute-only)                 ║");
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
        println!("CPUs: {}", std::thread::available_parallelism().map(|p| p.get()).unwrap_or(0));

        let programs: Vec<&str> = if all_programs {
            ALL_PROGRAMS.to_vec()
        } else {
            vec![config.program.as_str()]
        };

        for &program in &programs {
            let image_id = match image_id_for_program(program) {
                Some(id) => id,
                None => {
                    eprintln!("Unknown program: {}", program);
                    continue;
                }
            };

            let elf_path = format!("../programs/{}.elf", image_id);
            let elf = match std::fs::read(&elf_path) {
                Ok(data) => data,
                Err(e) => {
                    println!("\n── {} ──", program);
                    println!("  SKIP: {} ({})", elf_path, e);
                    continue;
                }
            };

            let sizes = if progressive_sizes.is_empty() {
                default_scaling_sizes(program)
            } else {
                progressive_sizes.clone()
            };

            run_scaling_analysis(program, &elf, &sizes)?;
        }

        println!();
        println!("Strategy thresholds:");
        println!("  Direct:       < 20M cycles (no segment tuning needed)");
        println!("  Segmented:    20M - 100M cycles (tune po2 for parallelism)");
        println!("  Continuation: 100M - 500M cycles (aggressive parallelism)");
        println!("  Too Complex:  > 500M cycles (reject or use Bonsai)");
        return Ok(());
    }

    // Resolve ELF path
    let image_id = match image_id_for_program(&config.program) {
        Some(id) => id,
        None => {
            eprintln!("Unknown program: {}. Supported: {:?}", config.program, ALL_PROGRAMS);
            std::process::exit(1);
        }
    };

    let elf_path = format!("../programs/{}.elf", image_id);
    let elf = std::fs::read(&elf_path)
        .map_err(|e| anyhow!("Failed to read ELF at {}: {}", elf_path, e))?;

    // ── Progressive mode: actual proving at increasing sizes ─────────
    if progressive_mode {
        let sizes = if progressive_sizes.is_empty() {
            vec![200, 500, 1000, 2000]
        } else {
            progressive_sizes
        };
        return run_progressive(&config.program, &elf, &sizes);
    }

    // ── Head-to-head mode ────────────────────────────────────────────
    let input = generate_input(&config.program, config.samples);

    let input_hash = {
        let mut h = Sha256::new();
        h.update(&input);
        hex::encode(h.finalize())
    };

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          Proving Benchmark — Baseline vs Optimized          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Program:     {}", config.program);
    println!("ELF:         {} ({} bytes)", elf_path, elf.len());
    println!("Samples:     {}", config.samples);
    println!("Input size:  {} bytes", input.len());
    println!("Input hash:  0x{}...", &input_hash[..16]);
    println!("Runs:        {} per method", config.runs);
    println!("CPUs:        {}", std::thread::available_parallelism().map(|p| p.get()).unwrap_or(0));
    if config.optimized_only {
        println!("Mode:        optimized-only");
    } else if config.baseline_only {
        println!("Mode:        baseline-only");
    }
    println!();

    // ── Warmup execution ──────────────────────────────────────────────
    println!("── Warmup (execute only) ───────────────────────────────────");
    {
        let start = Instant::now();
        let env = ExecutorEnv::builder()
            .write_slice(&input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let executor = default_executor();
        let session = executor.execute(env, &elf).map_err(|e| anyhow!("exec: {}", e))?;
        let elapsed = start.elapsed();
        let cycles = session.cycles();
        let strategy = if cycles < 20_000_000 {
            "Direct"
        } else if cycles < 100_000_000 {
            "Segmented"
        } else {
            "Continuation"
        };
        println!(
            "  Cycles: {}, Segments: {}, Strategy: {}, Time: {:?}",
            cycles,
            session.segments.len(),
            strategy,
            elapsed
        );
    }
    println!();

    // ── Run benchmarks ────────────────────────────────────────────────
    let mut baseline_results = Vec::new();
    let mut optimized_results = Vec::new();

    for run in 0..config.runs {
        println!("── Run {}/{} ─────────────────────────────────────────────", run + 1, config.runs);

        if !config.optimized_only {
            println!("  [Baseline]");
            match run_baseline(&elf, &input) {
                Ok(result) => {
                    print!("{}", result);
                    baseline_results.push(result);
                }
                Err(e) => {
                    println!("    FAILED: {}", e);
                }
            }
        }

        if !config.baseline_only {
            println!("  [Optimized]");
            match run_optimized(&elf, &input) {
                Ok(result) => {
                    print!("{}", result);
                    optimized_results.push(result);
                }
                Err(e) => {
                    println!("    FAILED: {}", e);
                }
            }
        }

        println!();
    }

    // ── Summary ───────────────────────────────────────────────────────
    let have_both = !baseline_results.is_empty() && !optimized_results.is_empty();
    let have_any = !baseline_results.is_empty() || !optimized_results.is_empty();

    if have_any {
        println!("══════════════════════════════════════════════════════════════");
        println!("  SUMMARY");
        println!("══════════════════════════════════════════════════════════════");
        println!();

        if !baseline_results.is_empty() {
            let avg_prove: Duration = baseline_results.iter().map(|r| r.prove_time).sum::<Duration>()
                / baseline_results.len() as u32;
            let avg_total: Duration = baseline_results.iter().map(|r| r.total_time).sum::<Duration>()
                / baseline_results.len() as u32;
            let throughput = baseline_results[0].cycles as f64 / avg_prove.as_secs_f64();
            println!("  Baseline avg prove:    {:>8.2?}  ({:.0} cycles/sec)", avg_prove, throughput);
            println!("  Baseline avg total:    {:>8.2?}", avg_total);
        }

        if !optimized_results.is_empty() {
            let avg_prove: Duration = optimized_results.iter().map(|r| r.prove_time).sum::<Duration>()
                / optimized_results.len() as u32;
            let avg_total: Duration = optimized_results.iter().map(|r| r.total_time).sum::<Duration>()
                / optimized_results.len() as u32;
            let throughput = optimized_results[0].cycles as f64 / avg_prove.as_secs_f64();
            println!("  Optimized avg prove:   {:>8.2?}  ({:.0} cycles/sec)", avg_prove, throughput);
            println!("  Optimized avg total:   {:>8.2?}", avg_total);
        }

        if have_both {
            let avg_baseline_prove: Duration = baseline_results.iter().map(|r| r.prove_time).sum::<Duration>()
                / baseline_results.len() as u32;
            let avg_optimized_prove: Duration = optimized_results.iter().map(|r| r.prove_time).sum::<Duration>()
                / optimized_results.len() as u32;

            let avg_baseline_total: Duration = baseline_results.iter().map(|r| r.total_time).sum::<Duration>()
                / baseline_results.len() as u32;
            let avg_optimized_total: Duration = optimized_results.iter().map(|r| r.total_time).sum::<Duration>()
                / optimized_results.len() as u32;

            let prove_speedup = avg_baseline_prove.as_secs_f64() / avg_optimized_prove.as_secs_f64();
            let total_speedup = avg_baseline_total.as_secs_f64() / avg_optimized_total.as_secs_f64();

            let prove_delta = if avg_optimized_prove < avg_baseline_prove {
                avg_baseline_prove - avg_optimized_prove
            } else {
                Duration::ZERO
            };

            println!();
            println!("  Prove speedup:         {:>8.2}x", prove_speedup);
            println!("  Time saved (prove):    {:>8.2?}", prove_delta);
            println!("  Total speedup:         {:>8.2}x", total_speedup);
            println!();

            if prove_speedup >= 1.0 {
                println!("  Result: Optimized is {:.1}% faster at proving", (prove_speedup - 1.0) * 100.0);
            } else {
                println!("  Result: Baseline is {:.1}% faster (optimization overhead dominates for small programs)",
                    (1.0 / prove_speedup - 1.0) * 100.0);
            }
        }
    }

    Ok(())
}
