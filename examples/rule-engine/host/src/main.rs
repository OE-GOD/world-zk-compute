//! Rule Engine Host Program (RISC Zero)
//!
//! Benchmarks the rule engine on the RISC Zero zkVM.
//!
//! Usage:
//!   cargo run --release --bin rule-engine-host

use risc0_zkvm::{default_executor, ExecutorEnv};
use rule_engine_methods::RULE_ENGINE_GUEST_ELF;
use std::time::Instant;

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
    println!("=== RISC Zero Rule Engine Benchmark ===\n");

    let input = create_sample_input();
    println!(
        "Input: {} records, {} rules, {} aggregations",
        input.records.len(),
        input.rules.len(),
        input.aggregations.len()
    );

    // Build executor environment with input
    let env = ExecutorEnv::builder()
        .write(&input)?
        .build()?;

    // Execute (no proof — just get cycle count)
    println!("\n--- Execute Mode (no proof) ---");
    let start = Instant::now();
    let executor = default_executor();
    let session = executor.execute(env, RULE_ENGINE_GUEST_ELF)?;
    let exec_time = start.elapsed();

    let cycles = session.cycles();
    println!("RISC0 Execution time: {:?}", exec_time);
    println!("RISC0 Cycles: {}", cycles);

    // Decode output from journal
    let result: RuleEngineOutput = session.journal.decode()?;

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

    Ok(())
}

/// Create sample rule engine input (identical to SP1 version)
fn create_sample_input() -> RuleEngineInput {
    let mut records = Vec::new();

    let regions: Vec<&[u8]> = vec![
        b"us-east", b"us-west", b"eu-central", b"eu-west", b"ap-south",
        b"ap-east", b"us-east", b"eu-central", b"ap-south", b"us-east",
    ];
    let devices: Vec<&[u8]> = vec![
        b"iphone-14", b"pixel-7", b"samsung-s23", b"iphone-15", b"pixel-8",
        b"samsung-s24", b"iphone-14", b"pixel-7", b"samsung-s23", b"iphone-15",
    ];

    for i in 0..10u8 {
        let mut id = [0u8; 32];
        id[0] = i;
        id[31] = i.wrapping_mul(17);

        records.push(Record {
            id,
            int_fields: vec![
                1700000000 + (i as i64 * 3600),
                (80 + (i as i64 * 3) % 25) as i64,
                (i as i64 % 5) + 1,
                if i >= 8 { 50 } else { 1 },
            ],
            str_fields: vec![
                regions[i as usize].to_vec(),
                devices[i as usize].to_vec(),
            ],
        });
    }

    let rules = vec![
        Rule {
            combine: 0,
            conditions: vec![
                Condition {
                    cond_type: 0,
                    field_idx: 1,
                    compare_op: 3,
                    int_value: 90,
                    str_value: Vec::new(),
                },
                Condition {
                    cond_type: 3,
                    field_idx: 0,
                    compare_op: 0,
                    int_value: 0,
                    str_value: b"us".to_vec(),
                },
            ],
        },
        Rule {
            combine: 1,
            conditions: vec![
                Condition {
                    cond_type: 0,
                    field_idx: 3,
                    compare_op: 2,
                    int_value: 10,
                    str_value: Vec::new(),
                },
                Condition {
                    cond_type: 0,
                    field_idx: 1,
                    compare_op: 4,
                    int_value: 82,
                    str_value: Vec::new(),
                },
            ],
        },
        Rule {
            combine: 0,
            conditions: vec![
                Condition {
                    cond_type: 5,
                    field_idx: 1,
                    compare_op: 0,
                    int_value: 0,
                    str_value: b"iphone-*".to_vec(),
                },
                Condition {
                    cond_type: 2,
                    field_idx: 0,
                    compare_op: 0,
                    int_value: 0,
                    str_value: b"east".to_vec(),
                },
            ],
        },
    ];

    let aggregations = vec![
        AggDef { agg_type: 0, field_idx: 0, filter_rule: 0xFFFFFFFF },
        AggDef { agg_type: 1, field_idx: 1, filter_rule: 0 },
        AggDef { agg_type: 3, field_idx: 3, filter_rule: 1 },
        AggDef { agg_type: 2, field_idx: 1, filter_rule: 0xFFFFFFFF },
    ];

    RuleEngineInput { records, rules, aggregations }
}
