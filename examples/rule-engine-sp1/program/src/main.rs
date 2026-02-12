//! Rule Engine Guest Program (SP1 version)
//!
//! Same algorithm as the RISC Zero version — only the zkVM wrapper differs.
//!
//! A configurable rule evaluation engine that covers four core analysis capabilities:
//! 1. Pattern matching (glob/contains/prefix/suffix on byte strings)
//! 2. Aggregation (count, sum, min, max over integer fields)
//! 3. Comparison (gt, gte, lt, lte, eq, neq on integer fields)
//! 4. Rules (logical AND/OR expressions combining multiple conditions)

#![no_main]
sp1_zkvm::entrypoint!(main);

extern crate alloc;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════════
// Input types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Deserialize)]
struct RuleEngineInput {
    records: Vec<Record>,
    rules: Vec<Rule>,
    aggregations: Vec<AggDef>,
}

#[derive(serde::Deserialize)]
struct Record {
    id: [u8; 32],
    int_fields: Vec<i64>,
    str_fields: Vec<Vec<u8>>,
}

#[derive(serde::Deserialize)]
struct Rule {
    conditions: Vec<Condition>,
    /// 0 = AND, 1 = OR
    combine: u32,
}

#[derive(serde::Deserialize)]
struct Condition {
    /// 0=int_cmp, 1=str_eq, 2=str_contains, 3=str_prefix, 4=str_suffix, 5=str_glob
    cond_type: u32,
    field_idx: u32,
    /// For int_cmp: 0=eq, 1=neq, 2=gt, 3=gte, 4=lt, 5=lte
    compare_op: u32,
    int_value: i64,
    str_value: Vec<u8>,
}

#[derive(serde::Deserialize)]
struct AggDef {
    /// 0=count, 1=sum, 2=min, 3=max
    agg_type: u32,
    field_idx: u32,
    /// Index into rules[], or 0xFFFFFFFF for all records
    filter_rule: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Output types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Serialize)]
struct RuleEngineOutput {
    total_records: u32,
    rule_results: Vec<RuleResult>,
    agg_results: Vec<AggResult>,
    input_hash: [u8; 32],
}

#[derive(serde::Serialize)]
struct RuleResult {
    matching_count: u32,
    flagged_ids: Vec<[u8; 32]>,
}

#[derive(serde::Serialize)]
struct AggResult {
    value: i64,
    count: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════════

fn main() {
    let input = sp1_zkvm::io::read::<RuleEngineInput>();

    let input_hash = compute_input_hash(&input);

    // Evaluate each rule against all records
    let mut rule_results = Vec::with_capacity(input.rules.len());
    let mut rule_matches: Vec<Vec<bool>> = Vec::with_capacity(input.rules.len());

    for rule in &input.rules {
        let mut matching_count = 0u32;
        let mut flagged_ids = Vec::new();
        let mut matches = Vec::with_capacity(input.records.len());

        for record in &input.records {
            let matched = evaluate_rule(rule, record);
            matches.push(matched);
            if matched {
                matching_count += 1;
                flagged_ids.push(record.id);
            }
        }

        rule_results.push(RuleResult {
            matching_count,
            flagged_ids,
        });
        rule_matches.push(matches);
    }

    // Compute aggregations
    let mut agg_results = Vec::with_capacity(input.aggregations.len());

    for agg in &input.aggregations {
        let result = compute_aggregation(agg, &input.records, &rule_matches);
        agg_results.push(result);
    }

    let output = RuleEngineOutput {
        total_records: input.records.len() as u32,
        rule_results,
        agg_results,
        input_hash,
    };

    sp1_zkvm::io::commit(&output);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Condition evaluation
// ═══════════════════════════════════════════════════════════════════════════════

fn evaluate_condition(cond: &Condition, record: &Record) -> bool {
    match cond.cond_type {
        // int_cmp
        0 => {
            let idx = cond.field_idx as usize;
            if idx >= record.int_fields.len() {
                return false;
            }
            let val = record.int_fields[idx];
            match cond.compare_op {
                0 => val == cond.int_value,
                1 => val != cond.int_value,
                2 => val > cond.int_value,
                3 => val >= cond.int_value,
                4 => val < cond.int_value,
                5 => val <= cond.int_value,
                _ => false,
            }
        }
        // str_eq
        1 => {
            let idx = cond.field_idx as usize;
            if idx >= record.str_fields.len() {
                return false;
            }
            record.str_fields[idx] == cond.str_value
        }
        // str_contains
        2 => {
            let idx = cond.field_idx as usize;
            if idx >= record.str_fields.len() {
                return false;
            }
            bytes_contains(&record.str_fields[idx], &cond.str_value)
        }
        // str_prefix
        3 => {
            let idx = cond.field_idx as usize;
            if idx >= record.str_fields.len() {
                return false;
            }
            record.str_fields[idx].starts_with(&cond.str_value)
        }
        // str_suffix
        4 => {
            let idx = cond.field_idx as usize;
            if idx >= record.str_fields.len() {
                return false;
            }
            record.str_fields[idx].ends_with(&cond.str_value)
        }
        // str_glob
        5 => {
            let idx = cond.field_idx as usize;
            if idx >= record.str_fields.len() {
                return false;
            }
            glob_match(&cond.str_value, &record.str_fields[idx])
        }
        _ => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Rule evaluation
// ═══════════════════════════════════════════════════════════════════════════════

fn evaluate_rule(rule: &Rule, record: &Record) -> bool {
    if rule.conditions.is_empty() {
        return true;
    }

    if rule.combine == 0 {
        // AND: all conditions must match
        for cond in &rule.conditions {
            if !evaluate_condition(cond, record) {
                return false;
            }
        }
        true
    } else {
        // OR: any condition must match
        for cond in &rule.conditions {
            if evaluate_condition(cond, record) {
                return true;
            }
        }
        false
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Aggregation
// ═══════════════════════════════════════════════════════════════════════════════

fn compute_aggregation(
    agg: &AggDef,
    records: &[Record],
    rule_matches: &[Vec<bool>],
) -> AggResult {
    let filter_rule = agg.filter_rule;
    let field_idx = agg.field_idx as usize;

    let mut count = 0u32;
    let mut sum = 0i64;
    let mut min_val = i64::MAX;
    let mut max_val = i64::MIN;

    for (i, record) in records.iter().enumerate() {
        if filter_rule != 0xFFFFFFFF {
            let rule_idx = filter_rule as usize;
            if rule_idx >= rule_matches.len() || !rule_matches[rule_idx][i] {
                continue;
            }
        }

        count += 1;

        if agg.agg_type != 0 {
            if field_idx < record.int_fields.len() {
                let val = record.int_fields[field_idx];
                sum += val;
                if val < min_val {
                    min_val = val;
                }
                if val > max_val {
                    max_val = val;
                }
            }
        }
    }

    let value = match agg.agg_type {
        0 => count as i64,
        1 => sum,
        2 => {
            if count == 0 {
                0
            } else {
                min_val
            }
        }
        3 => {
            if count == 0 {
                0
            } else {
                max_val
            }
        }
        _ => 0,
    };

    AggResult { value, count }
}

// ═══════════════════════════════════════════════════════════════════════════════
// String matching utilities
// ═══════════════════════════════════════════════════════════════════════════════

fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let mut px = 0usize;
    let mut tx = 0usize;
    let mut star_px: Option<usize> = None;
    let mut star_tx = 0usize;

    while tx < text.len() {
        if px < pattern.len() && (pattern[px] == b'?' || pattern[px] == text[tx]) {
            px += 1;
            tx += 1;
        } else if px < pattern.len() && pattern[px] == b'*' {
            star_px = Some(px);
            star_tx = tx;
            px += 1;
        } else if let Some(sp) = star_px {
            px = sp + 1;
            star_tx += 1;
            tx = star_tx;
        } else {
            return false;
        }
    }

    while px < pattern.len() && pattern[px] == b'*' {
        px += 1;
    }

    px == pattern.len()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input hashing
// ═══════════════════════════════════════════════════════════════════════════════

fn compute_input_hash(input: &RuleEngineInput) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    for record in &input.records {
        hasher.update(&record.id);
        for &v in &record.int_fields {
            hasher.update(&v.to_le_bytes());
        }
        for field in &record.str_fields {
            hasher.update(&(field.len() as u32).to_le_bytes());
            hasher.update(field);
        }
    }

    for rule in &input.rules {
        hasher.update(&rule.combine.to_le_bytes());
        for cond in &rule.conditions {
            hasher.update(&cond.cond_type.to_le_bytes());
            hasher.update(&cond.field_idx.to_le_bytes());
            hasher.update(&cond.compare_op.to_le_bytes());
            hasher.update(&cond.int_value.to_le_bytes());
            hasher.update(&(cond.str_value.len() as u32).to_le_bytes());
            hasher.update(&cond.str_value);
        }
    }

    for agg in &input.aggregations {
        hasher.update(&agg.agg_type.to_le_bytes());
        hasher.update(&agg.field_idx.to_le_bytes());
        hasher.update(&agg.filter_rule.to_le_bytes());
    }

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
