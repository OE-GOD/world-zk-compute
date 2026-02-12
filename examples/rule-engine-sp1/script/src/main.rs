//! Rule Engine Host Script (SP1 version)
//!
//! Benchmarks the rule engine on the SP1 zkVM.
//!
//! Usage:
//!   cargo run --release -- --execute     # Execute only (no proof, fast)
//!   cargo run --release -- --prove       # Generate and verify proof

use sp1_sdk::{include_elf, ProverClient, SP1Stdin};
use std::time::Instant;

const ELF: &[u8] = include_elf!("rule-engine-sp1-program");

// ═══════════════════════════════════════════════════════════════════════════════
// Types (must match guest program)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Serialize, serde::Deserialize)]
struct RuleEngineInput {
    records: Vec<Record>,
    rules: Vec<Rule>,
    aggregations: Vec<AggDef>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    id: [u8; 32],
    int_fields: Vec<i64>,
    str_fields: Vec<Vec<u8>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Rule {
    conditions: Vec<Condition>,
    combine: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Condition {
    cond_type: u32,
    field_idx: u32,
    compare_op: u32,
    int_value: i64,
    str_value: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AggDef {
    agg_type: u32,
    field_idx: u32,
    filter_rule: u32,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct RuleEngineOutput {
    total_records: u32,
    rule_results: Vec<RuleResult>,
    agg_results: Vec<AggResult>,
    input_hash: [u8; 32],
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct RuleResult {
    matching_count: u32,
    flagged_ids: Vec<[u8; 32]>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct AggResult {
    value: i64,
    count: u32,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let execute = args.iter().any(|a| a == "--execute");
    let prove = args.iter().any(|a| a == "--prove");

    if !execute && !prove {
        eprintln!("Usage: rule-engine-sp1-script [--execute] [--prove]");
        eprintln!("  --execute   Execute only (no proof, reports cycle count)");
        eprintln!("  --prove     Generate and verify a proof");
        std::process::exit(1);
    }

    println!("=== SP1 Rule Engine Benchmark ===\n");

    let input = create_sample_input();
    println!(
        "Input: {} records, {} rules, {} aggregations",
        input.records.len(),
        input.rules.len(),
        input.aggregations.len()
    );

    let client = ProverClient::from_env();
    let mut stdin = SP1Stdin::new();
    stdin.write(&input);

    if execute {
        println!("\n--- Execute Mode (no proof) ---");
        let start = Instant::now();
        let (mut output, report) = client.execute(ELF, &stdin).run()?;
        let exec_time = start.elapsed();

        let result = output.read::<RuleEngineOutput>();

        println!("SP1 Execution time: {:?}", exec_time);
        println!("SP1 Cycles: {}", report.total_instruction_count());
        println!("\nResults:");
        println!("  Total records: {}", result.total_records);
        println!("  Input hash: 0x{}", hex::encode(result.input_hash));
        for (i, rr) in result.rule_results.iter().enumerate() {
            println!(
                "  Rule {}: {} matches, flagged: [{}]",
                i,
                rr.matching_count,
                rr.flagged_ids
                    .iter()
                    .map(|id| format!("0x{}..{}", hex::encode(&id[..2]), hex::encode(&id[30..])))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for (i, ar) in result.agg_results.iter().enumerate() {
            println!("  Agg {}: value={}, count={}", i, ar.value, ar.count);
        }
    }

    if prove {
        println!("\n--- Prove Mode ---");
        let (pk, vk) = client.setup(ELF);

        let start = Instant::now();
        let proof = client.prove(&pk, &stdin).compressed().run()?;
        let prove_time = start.elapsed();

        let proof_size = bincode::serialize(&proof).map(|b| b.len()).unwrap_or(0);
        println!("SP1 Proof time: {:?}", prove_time);
        println!("SP1 Proof size: {} bytes", proof_size);

        let start = Instant::now();
        client.verify(&proof, &vk)?;
        let verify_time = start.elapsed();
        println!("SP1 Verify time: {:?}", verify_time);
        println!("SP1 Proof verified successfully!");
    }

    Ok(())
}

/// Create sample rule engine input that exercises all capabilities
fn create_sample_input() -> RuleEngineInput {
    let mut records = Vec::new();

    // 10 user registration records
    let regions = [
        b"us-east".as_slice(),
        b"us-west",
        b"eu-central",
        b"eu-west",
        b"ap-south",
        b"ap-east",
        b"us-east",
        b"eu-central",
        b"ap-south",
        b"us-east",
    ];
    let devices = [
        b"iphone-14".as_slice(),
        b"pixel-7",
        b"samsung-s23",
        b"iphone-15",
        b"pixel-8",
        b"samsung-s24",
        b"iphone-14",
        b"pixel-7",
        b"samsung-s23",
        b"iphone-15",
    ];

    for i in 0..10u8 {
        let mut id = [0u8; 32];
        id[0] = i;
        id[31] = i.wrapping_mul(17);

        records.push(Record {
            id,
            int_fields: vec![
                1700000000 + (i as i64 * 3600),       // timestamp
                (80 + (i as i64 * 3) % 25) as i64,    // quality_score (80-104)
                (i as i64 % 5) + 1,                    // session_count (1-5)
                if i >= 8 { 50 } else { 1 },           // registrations_per_hour (anomaly for last 2)
            ],
            str_fields: vec![
                regions[i as usize].to_vec(),
                devices[i as usize].to_vec(),
            ],
        });
    }

    let rules = vec![
        // Rule 0 (AND): high quality score AND from US region
        Rule {
            combine: 0, // AND
            conditions: vec![
                Condition {
                    cond_type: 0, // int_cmp
                    field_idx: 1, // quality_score
                    compare_op: 3, // gte
                    int_value: 90,
                    str_value: Vec::new(),
                },
                Condition {
                    cond_type: 3, // str_prefix
                    field_idx: 0, // region
                    compare_op: 0,
                    int_value: 0,
                    str_value: b"us".to_vec(),
                },
            ],
        },
        // Rule 1 (OR): suspicious — high registration rate OR very low quality
        Rule {
            combine: 1, // OR
            conditions: vec![
                Condition {
                    cond_type: 0, // int_cmp
                    field_idx: 3, // registrations_per_hour
                    compare_op: 2, // gt
                    int_value: 10,
                    str_value: Vec::new(),
                },
                Condition {
                    cond_type: 0, // int_cmp
                    field_idx: 1, // quality_score
                    compare_op: 4, // lt
                    int_value: 82,
                    str_value: Vec::new(),
                },
            ],
        },
        // Rule 2 (AND): glob match on device + contains on region
        Rule {
            combine: 0, // AND
            conditions: vec![
                Condition {
                    cond_type: 5, // str_glob
                    field_idx: 1, // device
                    compare_op: 0,
                    int_value: 0,
                    str_value: b"iphone-*".to_vec(),
                },
                Condition {
                    cond_type: 2, // str_contains
                    field_idx: 0, // region
                    compare_op: 0,
                    int_value: 0,
                    str_value: b"east".to_vec(),
                },
            ],
        },
    ];

    let aggregations = vec![
        // Agg 0: count all records
        AggDef {
            agg_type: 0,
            field_idx: 0,
            filter_rule: 0xFFFFFFFF,
        },
        // Agg 1: sum of quality_score for records matching rule 0
        AggDef {
            agg_type: 1, // sum
            field_idx: 1, // quality_score
            filter_rule: 0,
        },
        // Agg 2: max registrations_per_hour for suspicious records (rule 1)
        AggDef {
            agg_type: 3, // max
            field_idx: 3, // registrations_per_hour
            filter_rule: 1,
        },
        // Agg 3: min quality_score across all records
        AggDef {
            agg_type: 2, // min
            field_idx: 1, // quality_score
            filter_rule: 0xFFFFFFFF,
        },
    ];

    RuleEngineInput {
        records,
        rules,
        aggregations,
    }
}
