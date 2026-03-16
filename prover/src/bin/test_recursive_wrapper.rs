//! Recursive Wrapper End-to-End Benchmark
//!
//! Proves the rule-engine guest with K sub-inputs, then wraps the receipts
//! using the recursive verification wrapper. Compares:
//!
//! 1. Monolithic: Single proof over the full input
//! 2. Decomposed: K sub-proofs (parallel), merge journals outside zkVM
//! 3. Decomposed + Wrapper: K sub-proofs + recursive wrapper proof
//!
//! Usage:
//!   cargo run --release --bin test_recursive_wrapper
//!   cargo run --release --bin test_recursive_wrapper -- --records 6 --chunks 2
//!   cargo run --release --bin test_recursive_wrapper -- --records 9 --chunks 3
//!   cargo run --release --bin test_recursive_wrapper -- --wrapper-only

use anyhow::{anyhow, Result};
use risc0_zkvm::{
    default_prover, get_prover_server, ExecutorEnv, InnerReceipt, ProverOpts, Receipt,
};
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════════
// Config
// ═══════════════════════════════════════════════════════════════════════════════

struct BenchConfig {
    /// Total number of records for the rule-engine
    records: usize,
    /// Number of chunks for decomposition
    chunks: usize,
    /// Whether to generate SNARK proofs
    use_snark: bool,
    /// Skip monolithic test (faster iteration)
    wrapper_only: bool,
    /// Only run decomposed + union (skip wrapper tests 3,4,5)
    union_only: bool,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            records: 6,
            chunks: 2,
            use_snark: false,
            wrapper_only: false,
            union_only: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input generation (risc0 serde format, mirrors prove_benchmark.rs)
// ═══════════════════════════════════════════════════════════════════════════════

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_i64(buf: &mut Vec<u8>, v: i64) {
    write_u32(buf, v as u32);
    write_u32(buf, (v as u64 >> 32) as u32);
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

/// Generate a rule-engine input with `n` records and fixed rules/aggregations.
fn generate_rule_engine_input(n: usize) -> Vec<u8> {
    let mut buf = Vec::new();

    // records: Vec<Record>
    write_u32(&mut buf, n as u32);
    for i in 0..n {
        let mut id = [0u8; 32];
        id[0] = (i & 0xFF) as u8;
        id[1] = ((i >> 8) & 0xFF) as u8;
        write_byte_array_32(&mut buf, &id);

        let int_fields: Vec<i64> = match i % 3 {
            0 => vec![50 + (i as i64), 42, 10],
            1 => vec![200 + (i as i64), 42, -5],
            _ => vec![101 + (i as i64 % 50), 99, 0],
        };
        write_vec_i64(&mut buf, &int_fields);

        let str_fields: Vec<Vec<u8>> = match i % 3 {
            0 => vec![b"normal".to_vec(), b"clean".to_vec()],
            1 => vec![b"attack".to_vec(), b"suspicious".to_vec()],
            _ => vec![b"attack".to_vec(), b"clean".to_vec()],
        };
        write_vec_vec_u8(&mut buf, &str_fields);
    }

    // rules: 3 fixed rules
    write_u32(&mut buf, 3);

    // Rule 0: AND(int_field[0] > 100, str_field[0] glob "attack*")
    write_u32(&mut buf, 2);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 2);
    write_i64(&mut buf, 100);
    write_vec_u8(&mut buf, b"");
    write_u32(&mut buf, 5);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);
    write_i64(&mut buf, 0);
    write_vec_u8(&mut buf, b"attack*");
    write_u32(&mut buf, 0);

    // Rule 1: OR(int_field[1] == 42, int_field[2] < 0)
    write_u32(&mut buf, 2);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 1);
    write_u32(&mut buf, 0);
    write_i64(&mut buf, 42);
    write_vec_u8(&mut buf, b"");
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 2);
    write_u32(&mut buf, 4);
    write_i64(&mut buf, 0);
    write_vec_u8(&mut buf, b"");
    write_u32(&mut buf, 1);

    // Rule 2: AND(str_field[1] contains "suspicious")
    write_u32(&mut buf, 1);
    write_u32(&mut buf, 2);
    write_u32(&mut buf, 1);
    write_u32(&mut buf, 0);
    write_i64(&mut buf, 0);
    write_vec_u8(&mut buf, b"suspicious");
    write_u32(&mut buf, 0);

    // aggregations: 2 fixed
    write_u32(&mut buf, 2);
    write_u32(&mut buf, 1);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 3);
    write_u32(&mut buf, 1);
    write_u32(&mut buf, 0xFFFFFFFF);

    buf
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shared types (mirror the wrapper guest)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct WrapperInput {
    inner_image_id: [u8; 32],
    sub_proof_count: u32,
    sub_journals: Vec<Vec<u8>>,
    merged_journal: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct WrapperOutput {
    inner_image_id: [u8; 32],
    sub_proof_count: u32,
    journals_hash: [u8; 32],
    merged_journal: Vec<u8>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn extract_seal(receipt: &Receipt) -> Result<Vec<u8>> {
    use risc0_zkvm::InnerReceipt;
    let seal = match &receipt.inner {
        InnerReceipt::Composite(r) => {
            bincode::serialize(&r.segments).map_err(|e| anyhow!("seal: {}", e))?
        }
        InnerReceipt::Succinct(r) => {
            bincode::serialize(&r.seal).map_err(|e| anyhow!("seal: {}", e))?
        }
        InnerReceipt::Groth16(r) => {
            let selector = &r.verifier_parameters.as_bytes()[..4];
            let mut encoded = Vec::with_capacity(4 + r.seal.len());
            encoded.extend_from_slice(selector);
            encoded.extend_from_slice(&r.seal);
            encoded
        }
        _ => return Err(anyhow!("Unknown receipt type")),
    };
    Ok(seal)
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{:.2}s", secs)
    } else {
        format!("{:.1}m", secs / 60.0)
    }
}

/// Compute risc0 image ID from ELF bytes → 32-byte digest
fn compute_image_id_bytes(elf: &[u8]) -> Result<[u8; 32]> {
    let digest =
        risc0_zkvm::compute_image_id(elf).map_err(|e| anyhow!("compute_image_id: {}", e))?;
    let mut id = [0u8; 32];
    id.copy_from_slice(digest.as_bytes());
    Ok(id)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Benchmark Stages
// ═══════════════════════════════════════════════════════════════════════════════

struct ProveTimings {
    label: String,
    total: Duration,
    sub_prove_times: Vec<Duration>,
    wrapper_time: Option<Duration>,
    seal_size: usize,
    journal_size: usize,
}

impl std::fmt::Display for ProveTimings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  {}", self.label)?;
        if self.sub_prove_times.len() == 1 {
            writeln!(
                f,
                "    Prove:       {}",
                format_duration(self.sub_prove_times[0])
            )?;
        } else {
            for (i, t) in self.sub_prove_times.iter().enumerate() {
                writeln!(f, "    Sub-proof {}: {}", i, format_duration(*t))?;
            }
            let sum: Duration = self.sub_prove_times.iter().sum();
            writeln!(f, "    Sum:         {}", format_duration(sum))?;
        }
        if let Some(wt) = self.wrapper_time {
            writeln!(f, "    Wrapper:     {}", format_duration(wt))?;
        }
        writeln!(f, "    Total:       {}", format_duration(self.total))?;
        writeln!(f, "    Seal:        {} bytes", self.seal_size)?;
        writeln!(f, "    Journal:     {} bytes", self.journal_size)?;
        Ok(())
    }
}

/// Monolithic: single proof over all records
fn run_monolithic(elf: &[u8], input: &[u8]) -> Result<ProveTimings> {
    let total_start = Instant::now();

    let prove_start = Instant::now();
    let env = ExecutorEnv::builder()
        .write_slice(input)
        .build()
        .map_err(|e| anyhow!("env: {}", e))?;
    let prover = default_prover();
    let prove_info = prover
        .prove(env, elf)
        .map_err(|e| anyhow!("prove: {}", e))?;
    let prove_time = prove_start.elapsed();

    let seal = extract_seal(&prove_info.receipt)?;
    let journal = prove_info.receipt.journal.bytes.clone();

    Ok(ProveTimings {
        label: "Monolithic (single proof)".to_string(),
        total: total_start.elapsed(),
        sub_prove_times: vec![prove_time],
        wrapper_time: None,
        seal_size: seal.len(),
        journal_size: journal.len(),
    })
}

/// Decomposed: K sub-proofs proved sequentially, journals merged outside zkVM
fn run_decomposed(
    elf: &[u8],
    sub_inputs: &[Vec<u8>],
) -> Result<(ProveTimings, Vec<Receipt>, Vec<Vec<u8>>)> {
    let total_start = Instant::now();
    let mut sub_prove_times = Vec::new();
    let mut receipts = Vec::new();
    let mut journals = Vec::new();

    for (i, sub_input) in sub_inputs.iter().enumerate() {
        println!("    Proving sub-input {}/{}...", i + 1, sub_inputs.len());
        let start = Instant::now();

        let env = ExecutorEnv::builder()
            .write_slice(sub_input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let prover = default_prover();
        let prove_info = prover
            .prove(env, elf)
            .map_err(|e| anyhow!("prove: {}", e))?;

        let elapsed = start.elapsed();
        println!("    Sub-proof {} done: {}", i, format_duration(elapsed));
        sub_prove_times.push(elapsed);
        journals.push(prove_info.receipt.journal.bytes.clone());
        receipts.push(prove_info.receipt);
    }

    let first_seal = extract_seal(&receipts[0])?;
    // Simple journal merge: length-prefixed concatenation
    let merged_journal = merge_journals(&journals);

    let timings = ProveTimings {
        label: format!("Decomposed ({} sub-proofs, no wrapper)", sub_inputs.len()),
        total: total_start.elapsed(),
        sub_prove_times,
        wrapper_time: None,
        seal_size: first_seal.len(),
        journal_size: merged_journal.len(),
    };

    Ok((timings, receipts, journals))
}

/// Decomposed with Succinct sub-proofs: K sub-proofs proved with ProverOpts::succinct()
///
/// Each sub-proof is compressed to a constant-size Succinct receipt during proving.
/// This makes the wrapper's resolve() much cheaper since it operates on Succinct
/// receipts instead of multi-segment Composite receipts.
fn run_decomposed_succinct(
    elf: &[u8],
    sub_inputs: &[Vec<u8>],
) -> Result<(ProveTimings, Vec<Receipt>, Vec<Vec<u8>>)> {
    let total_start = Instant::now();
    let mut sub_prove_times = Vec::new();
    let mut receipts = Vec::new();
    let mut journals = Vec::new();

    let opts = ProverOpts::succinct();

    for (i, sub_input) in sub_inputs.iter().enumerate() {
        println!(
            "    Proving sub-input {}/{} (succinct)...",
            i + 1,
            sub_inputs.len()
        );
        let start = Instant::now();

        let env = ExecutorEnv::builder()
            .write_slice(sub_input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let prover = default_prover();
        let prove_info = prover
            .prove_with_opts(env, elf, &opts)
            .map_err(|e| anyhow!("prove succinct: {}", e))?;

        let elapsed = start.elapsed();
        let receipt_kind = match &prove_info.receipt.inner {
            risc0_zkvm::InnerReceipt::Composite(_) => "Composite",
            risc0_zkvm::InnerReceipt::Succinct(_) => "Succinct",
            risc0_zkvm::InnerReceipt::Groth16(_) => "Groth16",
            _ => "Unknown",
        };
        println!(
            "    Sub-proof {} done ({}): {}",
            i,
            receipt_kind,
            format_duration(elapsed)
        );
        sub_prove_times.push(elapsed);
        journals.push(prove_info.receipt.journal.bytes.clone());
        receipts.push(prove_info.receipt);
    }

    let first_seal = extract_seal(&receipts[0])?;
    let merged_journal = merge_journals(&journals);

    let timings = ProveTimings {
        label: format!(
            "Decomposed Succinct ({} sub-proofs, pre-compressed)",
            sub_inputs.len()
        ),
        total: total_start.elapsed(),
        sub_prove_times,
        wrapper_time: None,
        seal_size: first_seal.len(),
        journal_size: merged_journal.len(),
    };

    Ok((timings, receipts, journals))
}

/// Decomposed + Wrapper: K sub-proofs + recursive wrapper proof
fn run_wrapped(
    wrapper_elf: &[u8],
    inner_image_id: [u8; 32],
    sub_receipts: Vec<Receipt>,
    sub_journals: &[Vec<u8>],
    sub_prove_times: &[Duration],
) -> Result<ProveTimings> {
    let total_start = Instant::now();

    // Merge journals
    let merged_journal = merge_journals(sub_journals);

    // Build wrapper input
    let wrapper_input = WrapperInput {
        inner_image_id,
        sub_proof_count: sub_receipts.len() as u32,
        sub_journals: sub_journals.to_vec(),
        merged_journal: merged_journal.clone(),
    };

    println!(
        "    Proving wrapper ({} assumptions)...",
        sub_receipts.len()
    );
    let wrapper_start = Instant::now();

    let mut env_builder = ExecutorEnv::builder();
    env_builder
        .write(&wrapper_input)
        .map_err(|e| anyhow!("write wrapper input: {}", e))?;

    for (i, receipt) in sub_receipts.into_iter().enumerate() {
        env_builder.add_assumption(receipt);
        println!("    Added assumption {}", i);
    }

    let env = env_builder.build().map_err(|e| anyhow!("env: {}", e))?;
    let prover = default_prover();
    let prove_info = prover
        .prove(env, wrapper_elf)
        .map_err(|e| anyhow!("wrapper prove: {}", e))?;
    let wrapper_time = wrapper_start.elapsed();
    println!("    Wrapper proof done: {}", format_duration(wrapper_time));

    // Verify the wrapper output (risc0 serde format, not bincode)
    let wrapper_journal = &prove_info.receipt.journal.bytes;
    let output: WrapperOutput = prove_info
        .receipt
        .journal
        .decode()
        .map_err(|e| anyhow!("decode wrapper output: {}", e))?;

    println!("    Wrapper output:");
    println!(
        "      inner_image_id: 0x{}...",
        hex::encode(&output.inner_image_id[..8])
    );
    println!("      sub_proof_count: {}", output.sub_proof_count);
    println!(
        "      journals_hash: 0x{}...",
        hex::encode(&output.journals_hash[..8])
    );
    println!(
        "      merged_journal: {} bytes",
        output.merged_journal.len()
    );

    let wrapper_seal = extract_seal(&prove_info.receipt)?;

    // Total = sum of sub-proof times + wrapper time (sequential here, parallel in prod)
    let sum_sub: Duration = sub_prove_times.iter().sum();

    Ok(ProveTimings {
        label: format!(
            "Decomposed + Wrapper ({} sub-proofs + recursive verify)",
            sub_prove_times.len()
        ),
        total: total_start.elapsed() + sum_sub, // include sub-proof times
        sub_prove_times: sub_prove_times.to_vec(),
        wrapper_time: Some(wrapper_time),
        seal_size: wrapper_seal.len(),
        journal_size: wrapper_journal.len(),
    })
}

/// Host-side Union: compress sub-receipts to Succinct, then pair-wise union()
///
/// No wrapper guest needed. Uses risc0's composition API directly:
///   composite_to_succinct() → into_unknown() → union() → single proof
///
/// Trust model: proves all K sub-computations happened, but does NOT
/// prove journal merge correctness (unlike the wrapper guest approach).
fn run_union(
    sub_receipts: &[Receipt],
    sub_journals: &[Vec<u8>],
    sub_prove_times: &[Duration],
) -> Result<ProveTimings> {
    let total_start = Instant::now();

    // Get the ProverServer for compression/union operations
    let prover_server = get_prover_server(&ProverOpts::default())
        .map_err(|e| anyhow!("get_prover_server: {}", e))?;

    // Step 1: Compress each Composite receipt to Succinct
    let compress_start = Instant::now();
    let mut succinct_unknown = Vec::new();

    for (i, receipt) in sub_receipts.iter().enumerate() {
        let composite = match &receipt.inner {
            InnerReceipt::Composite(c) => c,
            InnerReceipt::Succinct(_) => {
                return Err(anyhow!(
                    "Sub-proof {} is already Succinct, expected Composite",
                    i
                ));
            }
            _ => return Err(anyhow!("Unexpected receipt type for sub-proof {}", i)),
        };

        println!(
            "    Compressing sub-proof {}/{} to succinct ({} segments)...",
            i + 1,
            sub_receipts.len(),
            composite.segments.len()
        );
        let compress_one_start = Instant::now();
        let succinct = prover_server
            .composite_to_succinct(composite)
            .map_err(|e| anyhow!("composite_to_succinct {}: {}", i, e))?;
        println!(
            "    Sub-proof {} compressed: {}",
            i,
            format_duration(compress_one_start.elapsed())
        );

        // Convert to Unknown claim type so all receipts have the same type
        succinct_unknown.push(succinct.into_unknown());
    }
    let compress_time = compress_start.elapsed();
    println!(
        "    All {} sub-proofs compressed: {}",
        sub_receipts.len(),
        format_duration(compress_time)
    );

    // Step 2: Pair-wise union to combine all into one
    let union_start = Instant::now();
    let mut round = 0;
    while succinct_unknown.len() > 1 {
        round += 1;
        println!(
            "    Union round {} ({} receipts)...",
            round,
            succinct_unknown.len()
        );
        let mut next = Vec::new();
        let mut j = 0;
        while j + 1 < succinct_unknown.len() {
            let pair_start = Instant::now();
            let result = prover_server
                .union(&succinct_unknown[j], &succinct_unknown[j + 1])
                .map_err(|e| anyhow!("union({}, {}): {}", j, j + 1, e))?;
            println!(
                "      union({}, {}): {}",
                j,
                j + 1,
                format_duration(pair_start.elapsed())
            );
            // Convert UnionClaim → Unknown for next round
            next.push(result.into_unknown());
            j += 2;
        }
        // Odd one out passes through
        if j < succinct_unknown.len() {
            next.push(succinct_unknown[j].clone());
        }
        succinct_unknown = next;
    }
    let union_time = union_start.elapsed();
    println!("    Union done: {}", format_duration(union_time));

    let overhead = compress_time + union_time;
    println!(
        "    Total overhead: {} (compress {} + union {})",
        format_duration(overhead),
        format_duration(compress_time),
        format_duration(union_time)
    );

    let merged_journal = merge_journals(sub_journals);

    // The "seal" is the final succinct receipt's seal (Vec<u32>, 4 bytes each)
    let seal_size = succinct_unknown[0].seal.len() * 4;

    let sum_sub: Duration = sub_prove_times.iter().sum();

    Ok(ProveTimings {
        label: format!(
            "Host-side Union ({} sub-proofs, compress+union, no wrapper guest)",
            sub_prove_times.len()
        ),
        total: total_start.elapsed() + sum_sub,
        sub_prove_times: sub_prove_times.to_vec(),
        wrapper_time: Some(overhead),
        seal_size,
        journal_size: merged_journal.len(),
    })
}

fn merge_journals(journals: &[Vec<u8>]) -> Vec<u8> {
    let mut merged = Vec::new();
    merged.extend_from_slice(&(journals.len() as u32).to_le_bytes());
    for j in journals {
        merged.extend_from_slice(&(j.len() as u32).to_le_bytes());
    }
    for j in journals {
        merged.extend_from_slice(j);
    }
    merged
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main
// ═══════════════════════════════════════════════════════════════════════════════

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut config = BenchConfig::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--records" | "-n" => {
                config.records = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(6);
                i += 2;
            }
            "--chunks" | "-k" => {
                config.chunks = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(2);
                i += 2;
            }
            "--snark" => {
                config.use_snark = true;
                i += 1;
            }
            "--wrapper-only" => {
                config.wrapper_only = true;
                i += 1;
            }
            "--union-only" => {
                config.union_only = true;
                i += 1;
            }
            "--help" | "-h" => {
                println!("Usage: test_recursive_wrapper [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -n, --records <N>   Total records [default: 6]");
                println!("  -k, --chunks <K>    Number of chunks [default: 2]");
                println!("  --wrapper-only      Skip monolithic test");
                println!("  --union-only        Only run decomposed + union (skip wrapper tests)");
                println!("  --snark             Use Groth16 SNARK proofs");
                return Ok(());
            }
            _ => {
                i += 1;
            }
        }
    }

    // ── Load ELFs ────────────────────────────────────────────────────────
    let rule_engine_image_id = "f85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33";
    let rule_elf_path = format!("../programs/{}.elf", rule_engine_image_id);

    let rule_elf = std::fs::read(&rule_elf_path)
        .map_err(|e| anyhow!("Failed to read rule-engine ELF at {}: {}", rule_elf_path, e))?;

    let (wrapper_elf, inner_image_id) = if !config.union_only {
        let wrapper_elf_path = "../programs/recursive-wrapper.elf";
        let wrapper_elf = std::fs::read(wrapper_elf_path)
            .map_err(|e| anyhow!("Failed to read wrapper ELF at {}: {}", wrapper_elf_path, e))?;
        let inner_image_id = compute_image_id_bytes(&rule_elf)?;
        let wrapper_image_id = compute_image_id_bytes(&wrapper_elf)?;
        println!(
            "  Wrapper:        0x{}...",
            hex::encode(&wrapper_image_id[..8])
        );
        (Some(wrapper_elf), inner_image_id)
    } else {
        let inner_image_id = compute_image_id_bytes(&rule_elf)?;
        (None, inner_image_id)
    };

    println!("╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║             Recursive Wrapper End-to-End Benchmark                    ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Configuration:");
    println!("  Records:        {}", config.records);
    println!("  Chunks:         {}", config.chunks);
    println!("  SNARK:          {}", config.use_snark);
    println!("  Union-only:     {}", config.union_only);
    println!("  Rule-engine:    0x{}...", &rule_engine_image_id[..16]);
    println!(
        "  CPUs:           {}",
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(0)
    );
    println!();

    // ── Generate inputs ──────────────────────────────────────────────────
    let full_input = generate_rule_engine_input(config.records);
    println!(
        "Full input: {} bytes ({} records)",
        full_input.len(),
        config.records
    );

    // Split into K sub-inputs (each has config.records/chunks records + same rules/aggs)
    let records_per_chunk = config.records / config.chunks;
    let mut sub_inputs = Vec::new();
    for chunk_idx in 0..config.chunks {
        let start_record = chunk_idx * records_per_chunk;
        let end_record = if chunk_idx == config.chunks - 1 {
            config.records
        } else {
            start_record + records_per_chunk
        };
        let chunk_records = end_record - start_record;
        let sub_input = generate_rule_engine_input(chunk_records);
        println!(
            "  Chunk {}: {} records, {} bytes",
            chunk_idx,
            chunk_records,
            sub_input.len()
        );
        sub_inputs.push(sub_input);
    }
    println!();

    // ── Execute preflight ────────────────────────────────────────────────
    println!("── Preflight (execute only) ────────────────────────────────────────");
    {
        let start = Instant::now();
        let env = ExecutorEnv::builder()
            .write_slice(&full_input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let executor = risc0_zkvm::default_executor();
        let session = executor
            .execute(env, &rule_elf)
            .map_err(|e| anyhow!("exec: {}", e))?;
        println!(
            "  Full input: {} cycles, {} segments, {:?}",
            session.cycles(),
            session.segments.len(),
            start.elapsed()
        );
    }
    for (i, sub_input) in sub_inputs.iter().enumerate() {
        let start = Instant::now();
        let env = ExecutorEnv::builder()
            .write_slice(sub_input)
            .build()
            .map_err(|e| anyhow!("env: {}", e))?;
        let executor = risc0_zkvm::default_executor();
        let session = executor
            .execute(env, &rule_elf)
            .map_err(|e| anyhow!("exec: {}", e))?;
        println!(
            "  Chunk {}: {} cycles, {} segments, {:?}",
            i,
            session.cycles(),
            session.segments.len(),
            start.elapsed()
        );
    }
    println!();

    // ── Test 1: Monolithic ───────────────────────────────────────────────
    let monolithic_result = if !config.wrapper_only && !config.union_only {
        println!("── Test 1: Monolithic ─────────────────────────────────────────────");
        let result = run_monolithic(&rule_elf, &full_input)?;
        print!("{}", result);
        println!();
        Some(result)
    } else {
        None
    };

    // ── Test 2: Decomposed (Composite sub-proofs) ─────────────────────
    println!("── Test 2: Decomposed (Composite) ─────────────────────────────────");
    let (decomp_result, receipts, journals) = run_decomposed(&rule_elf, &sub_inputs)?;
    let composite_sub_times = decomp_result.sub_prove_times.clone();
    print!("{}", decomp_result);
    println!();

    // Wrapper tests (skip if --union-only)
    let mut composite_wrapped_time: Option<Duration> = None;
    let mut succinct_wrapped_time: Option<Duration> = None;
    let mut succinct_sub_times_opt: Option<Vec<Duration>> = None;

    if !config.union_only {
        let w_elf = wrapper_elf
            .as_ref()
            .expect("wrapper_elf is always Some when union_only is false");

        // Clone receipts before Test 3 consumes them (needed for Test 6: Union)
        let receipts_for_wrapper = receipts.clone();

        // ── Test 3: Decomposed + Wrapper (Composite assumptions) ─────────
        println!("── Test 3: Composite + Wrapper ──────────────────────────────────");
        let composite_wrapped = run_wrapped(
            w_elf,
            inner_image_id,
            receipts_for_wrapper,
            &journals,
            &composite_sub_times,
        )?;
        composite_wrapped_time = composite_wrapped.wrapper_time;
        print!("{}", composite_wrapped);
        println!();

        // ── Test 4: Decomposed (Succinct sub-proofs) ─────────────────────
        println!("── Test 4: Decomposed (Succinct) ──────────────────────────────────");
        let (succinct_result, succinct_receipts, succinct_journals) =
            run_decomposed_succinct(&rule_elf, &sub_inputs)?;
        succinct_sub_times_opt = Some(succinct_result.sub_prove_times.clone());
        print!("{}", succinct_result);
        println!();

        // ── Test 5: Decomposed + Wrapper (Succinct assumptions) ──────────
        println!("── Test 5: Succinct + Wrapper ───────────────────────────────────");
        let succinct_wrapped = run_wrapped(
            w_elf,
            inner_image_id,
            succinct_receipts,
            &succinct_journals,
            succinct_sub_times_opt
                .as_ref()
                .expect("succinct_sub_times_opt is always Some at this point"),
        )?;
        succinct_wrapped_time = succinct_wrapped.wrapper_time;
        print!("{}", succinct_wrapped);
        println!();
    }

    // ── Test 6: Host-side Union (Composite → Succinct → union) ─────────
    println!("── Test 6: Host-side Union ──────────────────────────────────────");
    let union_result = run_union(&receipts, &journals, &composite_sub_times)?;
    print!("{}", union_result);
    println!();

    // ── Summary ──────────────────────────────────────────────────────────
    println!("══════════════════════════════════════════════════════════════════════");
    println!(
        "  SUMMARY ({} records, {} chunks)",
        config.records, config.chunks
    );
    println!("══════════════════════════════════════════════════════════════════════");
    println!();

    let composite_sub_sum: Duration = composite_sub_times.iter().sum();

    if let Some(ref mono) = monolithic_result {
        println!(
            "  Monolithic:                  {}",
            format_duration(mono.total)
        );
    }

    println!(
        "  Composite sub-proofs (seq):  {}",
        format_duration(composite_sub_sum)
    );

    if let Some(cwt) = composite_wrapped_time {
        println!(
            "  Composite + Wrapper:         {} (subs) + {} (wrap) = {}",
            format_duration(composite_sub_sum),
            format_duration(cwt),
            format_duration(composite_sub_sum + cwt),
        );
    }

    if let (Some(ref sst), Some(swt)) = (&succinct_sub_times_opt, succinct_wrapped_time) {
        let succinct_sub_sum: Duration = sst.iter().sum();
        println!(
            "  Succinct sub-proofs (seq):   {}",
            format_duration(succinct_sub_sum)
        );
        println!(
            "  Succinct + Wrapper:          {} (subs) + {} (wrap) = {}",
            format_duration(succinct_sub_sum),
            format_duration(swt),
            format_duration(succinct_sub_sum + swt),
        );
    }

    // Union path
    let union_overhead = union_result.wrapper_time.unwrap_or(Duration::ZERO);
    println!(
        "  Composite + Union:           {} (subs) + {} (compress+union) = {}",
        format_duration(composite_sub_sum),
        format_duration(union_overhead),
        format_duration(composite_sub_sum + union_overhead),
    );

    // Overhead comparison
    println!();
    println!("  ── Aggregation overhead comparison ──");
    if let Some(cwt) = composite_wrapped_time {
        println!("    Wrapper (Composite):   {}", format_duration(cwt));
    }
    if let Some(swt) = succinct_wrapped_time {
        println!("    Wrapper (Succinct):    {}", format_duration(swt));
    }
    println!(
        "    Host-side Union:       {}",
        format_duration(union_overhead)
    );

    // Parallel estimates (production: sub-proofs run concurrently)
    if let Some(ref mono) = monolithic_result {
        println!();
        println!("  ── Parallel estimates (sub-proofs concurrent) ──");
        let max_composite_sub = composite_sub_times
            .iter()
            .max()
            .copied()
            .unwrap_or(Duration::ZERO);

        println!("    Monolithic:          {}", format_duration(mono.total));

        if let Some(cwt) = composite_wrapped_time {
            let parallel_composite = max_composite_sub + cwt;
            println!(
                "    Composite + Wrap:    {} (max sub {} + wrap {})",
                format_duration(parallel_composite),
                format_duration(max_composite_sub),
                format_duration(cwt)
            );
        }

        if let (Some(ref sst), Some(swt)) = (&succinct_sub_times_opt, succinct_wrapped_time) {
            let max_succinct_sub = sst.iter().max().copied().unwrap_or(Duration::ZERO);
            let parallel_succinct = max_succinct_sub + swt;
            println!(
                "    Succinct + Wrap:     {} (max sub {} + wrap {})",
                format_duration(parallel_succinct),
                format_duration(max_succinct_sub),
                format_duration(swt)
            );
        }

        let parallel_union = max_composite_sub + union_overhead;
        println!(
            "    Composite + Union:   {} (max sub {} + union {})",
            format_duration(parallel_union),
            format_duration(max_composite_sub),
            format_duration(union_overhead)
        );

        let speedup_union = mono.total.as_secs_f64() / parallel_union.as_secs_f64();
        println!(
            "    Union speedup:       {:.2}x vs monolithic",
            speedup_union
        );
    }

    println!();
    println!("  Trust model:");
    println!("    Monolithic:        Full trust (single proof covers everything)");
    println!("    Decomposed only:   TRUST GAP - journals merged outside zkVM");
    if composite_wrapped_time.is_some() {
        println!("    Decomposed+Wrap:   Full trust (wrapper verifies all sub-proofs inside zkVM)");
    }
    println!(
        "    Host-side Union:   Proves all sub-computations valid, but journal merge unproven"
    );

    Ok(())
}
