//! Proving Benchmark — Head-to-Head Comparison
//!
//! Compares baseline (default prover) vs optimized (SegmentProver with tuned
//! segment size and thread count) on a real guest program.
//!
//! Usage:
//!   cargo run --release --bin prove_benchmark
//!   cargo run --release --bin prove_benchmark -- --samples 10
//!   cargo run --release --bin prove_benchmark -- --progressive
//!   cargo run --release --bin prove_benchmark -- --samples 2000 --optimized-only
//!
//! Output: Detailed timing breakdown for each approach.

use anyhow::{anyhow, Result};
use risc0_zkvm::{default_executor, default_prover, ExecutorEnv};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════════
// Config
// ═══════════════════════════════════════════════════════════════════════════════

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
// Input Generation (mirrors generate-test-input.py)
// ═══════════════════════════════════════════════════════════════════════════════

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_f64(buf: &mut Vec<u8>, v: f64) {
    let bits = v.to_bits();
    buf.extend_from_slice(&(bits as u32).to_le_bytes());
    buf.extend_from_slice(&((bits >> 32) as u32).to_le_bytes());
}

fn write_byte_array_32(buf: &mut Vec<u8>, data: &[u8; 32]) {
    for &b in data.iter() {
        write_u32(buf, b as u32);
    }
}

fn write_vec_f64(buf: &mut Vec<u8>, v: &[f64]) {
    write_u32(buf, v.len() as u32);
    for &x in v {
        write_f64(buf, x);
    }
}

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
// Progressive Mode — Scaling analysis with actual proving
// ═══════════════════════════════════════════════════════════════════════════════

fn run_progressive(elf: &[u8], sizes: &[usize]) -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║              Progressive Proving Benchmark (optimized)                       ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("CPUs: {}", std::thread::available_parallelism().map(|p| p.get()).unwrap_or(0));
    println!();

    // Header
    println!("{:>7} {:>10} {:>5} {:>10} {:>8} {:>9} {:>10} {:>8} {:>8}",
        "Samples", "Cycles", "Segs", "Strategy", "po2", "Prove", "Total", "cy/sec", "seg/s");
    println!("{}", "─".repeat(88));

    let mut prev_prove_time: Option<Duration> = None;
    let mut prev_cycles: Option<u64> = None;

    for &n in sizes {
        let input = generate_xgboost_input(n);

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
                println!("  -s, --samples <N>       Number of samples [default: 5]");
                println!("  -r, --runs <N>          Runs per method [default: 2]");
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

    // Resolve ELF path
    let image_id = match config.program.as_str() {
        "xgboost-inference" => "1b1e0e6ea0bbefbbb2ccfe269a687b2e46efaa243f36664776b49dc15716e2ac",
        other => {
            eprintln!("Unknown program: {}. Currently only xgboost-inference is supported.", other);
            std::process::exit(1);
        }
    };

    let elf_path = format!("../programs/{}.elf", image_id);
    let elf = std::fs::read(&elf_path)
        .map_err(|e| anyhow!("Failed to read ELF at {}: {}", elf_path, e))?;

    // ── Scaling mode: execute-only analysis ──────────────────────────
    if scaling_mode {
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║              Scaling Analysis (execute-only)                 ║");
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
        println!("{:>8} {:>12} {:>8} {:>10} {:>8} {:>8} {:>8}",
            "Samples", "Cycles", "Segments", "ExecTime", "Opt_po2", "EstSegs", "Threads");
        println!("{}", "─".repeat(72));

        for n in [5, 10, 50, 100, 200, 500, 1000, 2000, 5000] {
            let input = generate_xgboost_input(n);
            let start = Instant::now();
            let env = ExecutorEnv::builder()
                .write_slice(&input)
                .build()
                .map_err(|e| anyhow!("env: {}", e))?;
            let executor = default_executor();
            match executor.execute(env, &elf) {
                Ok(session) => {
                    let elapsed = start.elapsed();
                    let cycles = session.cycles();
                    let segs = session.segments.len();
                    let po2 = optimal_po2(cycles);
                    let est_segs = estimate_segments_for_po2(cycles, po2);
                    let threads = recommended_thread_count(est_segs);
                    println!("{:>8} {:>12} {:>8} {:>10.1?} {:>8} {:>8} {:>8}",
                        n, cycles, segs, elapsed, po2, est_segs, threads);
                }
                Err(e) => {
                    println!("{:>8}  ERROR: {}", n, e);
                    break;
                }
            }
        }
        println!();
        println!("Strategy thresholds:");
        println!("  Direct:       < 20M cycles (no segment tuning needed)");
        println!("  Segmented:    20M - 100M cycles (tune po2 for parallelism)");
        println!("  Continuation: 100M - 500M cycles (aggressive parallelism)");
        println!("  Too Complex:  > 500M cycles (reject or use Bonsai)");
        return Ok(());
    }

    // ── Progressive mode: actual proving at increasing sizes ─────────
    if progressive_mode {
        let sizes = if progressive_sizes.is_empty() {
            vec![200, 500, 1000, 2000]
        } else {
            progressive_sizes
        };
        return run_progressive(&elf, &sizes);
    }

    // ── Head-to-head mode ────────────────────────────────────────────
    let input = match config.program.as_str() {
        "xgboost-inference" => generate_xgboost_input(config.samples),
        _ => unreachable!(),
    };

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
