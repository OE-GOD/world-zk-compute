//! Rule Engine Guest Program — Optimized
//!
//! Manual wire format parsing replaces serde deserialization for reduced cycle count.
//! The wire format (risc0 serde u32 LE words) is unchanged — no host/script changes needed.
//!
//! Covers four core analysis capabilities:
//! 1. Pattern matching (glob/contains/prefix/suffix on byte strings)
//! 2. Aggregation (count, sum, min, max over integer fields)
//! 3. Comparison (gt, gte, lt, lte, eq, neq on integer fields)
//! 4. Rules (logical AND/OR expressions combining multiple conditions)

#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec::Vec;
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

// ═══════════════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════════════

struct Record {
    id: [u8; 32],
    int_fields: Vec<i64>,
    str_fields: Vec<Vec<u8>>,
}

struct Rule {
    conditions: Vec<Condition>,
    /// 0 = AND, 1 = OR
    combine: u32,
}

struct Condition {
    /// 0=int_cmp, 1=str_eq, 2=str_contains, 3=str_prefix, 4=str_suffix, 5=str_glob
    cond_type: u32,
    field_idx: u32,
    /// For int_cmp: 0=eq, 1=neq, 2=gt, 3=gte, 4=lt, 5=lte
    compare_op: u32,
    int_value: i64,
    str_value: Vec<u8>,
}

struct AggDef {
    /// 0=count, 1=sum, 2=min, 3=max
    agg_type: u32,
    field_idx: u32,
    /// Index into rules[], or 0xFFFFFFFF for all records
    filter_rule: u32,
}

// Output types — no serde, manually serialized via commit_output()
struct RuleResult {
    matching_count: u32,
    flagged_ids: Vec<[u8; 32]>,
}

struct AggResult {
    value: i64,
    count: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Manual wire format reader — bypasses serde visitor pattern
//
// Wire format (risc0 serde u32 LE words):
//   u32       → 1 word
//   i64       → 2 words (low, high)
//   u8        → 1 word (zero-extended)
//   [u8; 32]  → 32 words (each byte as u32)
//   Vec<T>    → 1 word (length) + length × T
// ═══════════════════════════════════════════════════════════════════════════════

#[inline(always)]
fn read_word() -> u32 {
    env::read::<u32>()
}

#[inline(always)]
fn read_i64_val() -> i64 {
    let low = read_word() as u64;
    let high = read_word() as u64;
    ((high << 32) | low) as i64
}

fn read_byte_array_32() -> [u8; 32] {
    let mut arr = [0u8; 32];
    for b in arr.iter_mut() {
        *b = read_word() as u8;
    }
    arr
}

fn read_vec_i64() -> Vec<i64> {
    let len = read_word() as usize;
    let mut v = Vec::with_capacity(len);
    for _ in 0..len {
        v.push(read_i64_val());
    }
    v
}

fn read_vec_u8() -> Vec<u8> {
    let len = read_word() as usize;
    let mut v = Vec::with_capacity(len);
    for _ in 0..len {
        v.push(read_word() as u8);
    }
    v
}

fn read_input() -> (Vec<Record>, Vec<Rule>, Vec<AggDef>) {
    // Records
    let num_records = read_word() as usize;
    let mut records = Vec::with_capacity(num_records);
    for _ in 0..num_records {
        let id = read_byte_array_32();
        let int_fields = read_vec_i64();
        let num_str_fields = read_word() as usize;
        let mut str_fields = Vec::with_capacity(num_str_fields);
        for _ in 0..num_str_fields {
            str_fields.push(read_vec_u8());
        }
        records.push(Record { id, int_fields, str_fields });
    }

    // Rules
    let num_rules = read_word() as usize;
    let mut rules = Vec::with_capacity(num_rules);
    for _ in 0..num_rules {
        let num_conditions = read_word() as usize;
        let mut conditions = Vec::with_capacity(num_conditions);
        for _ in 0..num_conditions {
            conditions.push(Condition {
                cond_type: read_word(),
                field_idx: read_word(),
                compare_op: read_word(),
                int_value: read_i64_val(),
                str_value: read_vec_u8(),
            });
        }
        let combine = read_word();
        rules.push(Rule { conditions, combine });
    }

    // Aggregations
    let num_aggs = read_word() as usize;
    let mut aggregations = Vec::with_capacity(num_aggs);
    for _ in 0..num_aggs {
        aggregations.push(AggDef {
            agg_type: read_word(),
            field_idx: read_word(),
            filter_rule: read_word(),
        });
    }

    (records, rules, aggregations)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════════

fn main() {
    let (records, rules, aggregations) = read_input();

    let input_hash = compute_input_hash(&records, &rules, &aggregations);

    // Evaluate each rule against all records
    let mut rule_results = Vec::with_capacity(rules.len());
    let mut rule_matches: Vec<Vec<bool>> = Vec::with_capacity(rules.len());

    for rule in &rules {
        let mut matching_count = 0u32;
        let mut flagged_ids = Vec::new();
        let mut matches = Vec::with_capacity(records.len());

        for record in &records {
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
    let mut agg_results = Vec::with_capacity(aggregations.len());

    for agg in &aggregations {
        let result = compute_aggregation(agg, &records, &rule_matches);
        agg_results.push(result);
    }

    commit_output(records.len() as u32, &rule_results, &agg_results, &input_hash);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Manual output serialization — writes u32 words matching risc0 serde format
// ═══════════════════════════════════════════════════════════════════════════════

fn commit_output(
    total_records: u32,
    rule_results: &[RuleResult],
    agg_results: &[AggResult],
    input_hash: &[u8; 32],
) {
    // Pre-calculate total words needed to avoid reallocations
    let mut word_count = 1 + 1 + 1 + 32; // total_records + rule_results len + agg_results len + hash
    for r in rule_results {
        word_count += 1 + 1 + r.flagged_ids.len() * 32; // matching_count + vec len + ids
    }
    word_count += agg_results.len() * 3; // each: value(2 words) + count(1 word)

    let mut words = Vec::with_capacity(word_count);

    // total_records: u32
    words.push(total_records);

    // rule_results: Vec<RuleResult>
    words.push(rule_results.len() as u32);
    for result in rule_results {
        words.push(result.matching_count);
        words.push(result.flagged_ids.len() as u32);
        for id in &result.flagged_ids {
            for &byte in id {
                words.push(byte as u32);
            }
        }
    }

    // agg_results: Vec<AggResult>
    words.push(agg_results.len() as u32);
    for result in agg_results {
        let val = result.value as u64;
        words.push(val as u32);        // low
        words.push((val >> 32) as u32); // high
        words.push(result.count);
    }

    // input_hash: [u8; 32]
    for &byte in input_hash {
        words.push(byte as u32);
    }

    env::commit_slice(&words);
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
        return false;
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
        // Apply filter if specified
        if filter_rule != 0xFFFFFFFF {
            let rule_idx = filter_rule as usize;
            if rule_idx >= rule_matches.len() || !rule_matches[rule_idx][i] {
                continue;
            }
        }

        count += 1;

        if agg.agg_type != 0 {
            // Not a plain count — need the field value
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

/// O(n*m) substring search
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

/// Iterative glob matching supporting `*` (any sequence) and `?` (any single byte)
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

    // Consume trailing stars
    while px < pattern.len() && pattern[px] == b'*' {
        px += 1;
    }

    px == pattern.len()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input hashing
// ═══════════════════════════════════════════════════════════════════════════════

fn compute_input_hash(records: &[Record], rules: &[Rule], aggregations: &[AggDef]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    for record in records {
        hasher.update(&record.id);
        for &v in &record.int_fields {
            hasher.update(&v.to_le_bytes());
        }
        for field in &record.str_fields {
            hasher.update(&(field.len() as u32).to_le_bytes());
            hasher.update(field);
        }
    }

    for rule in rules {
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

    for agg in aggregations {
        hasher.update(&agg.agg_type.to_le_bytes());
        hasher.update(&agg.field_idx.to_le_bytes());
        hasher.update(&agg.filter_rule.to_le_bytes());
    }

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
