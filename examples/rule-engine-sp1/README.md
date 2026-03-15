# Rule Engine (SP1)

SP1 port of the rule-based classification engine. This implements the same configurable rule evaluation as the RISC Zero version, but compiled for the [SP1 zkVM](https://github.com/succinctlabs/sp1).

## What It Does

A general-purpose rule engine that evaluates records against configurable rules inside a zkVM. The proof guarantees correct execution of:

1. **Pattern matching** -- glob, contains, prefix, suffix on byte string fields.
2. **Comparisons** -- gt, gte, lt, lte, eq, neq on integer fields.
3. **Logical rules** -- AND/OR combinations of multiple conditions.
4. **Aggregations** -- count, sum, min, max over matching records.

## Project Structure

```
rule-engine-sp1/
  Cargo.toml          # Workspace: members = ["script"], exclude = ["program"]
  program/
    Cargo.toml        # SP1 guest program crate
    src/main.rs       # Rule evaluation logic (runs inside SP1 zkVM)
  script/
    Cargo.toml        # Host-side script
    build.rs          # Builds the SP1 guest ELF
    src/main.rs       # Host: loads rules + data, runs prover
```

## Prerequisites

- Rust toolchain (stable)
- SP1 toolchain:
  ```bash
  curl -L https://sp1up.succinct.xyz | bash
  sp1up
  ```

## Build

```bash
cd examples/rule-engine-sp1
cargo build --release
```

## Run

```bash
cd examples/rule-engine-sp1
cargo run --release --bin script
```

## Input Format

```rust
struct RuleEngineInput {
    records: Vec<Record>,       // Data records to evaluate
    rules: Vec<Rule>,           // Rules to apply (AND/OR conditions)
    aggregations: Vec<AggDef>,  // Aggregation queries
}
```

Each `Record` has `id: [u8; 32]`, `int_fields: Vec<i64>`, and `str_fields: Vec<Vec<u8>>`.

## Output Format

```rust
struct RuleEngineOutput {
    total_records: u32,
    rule_results: Vec<RuleResult>,  // Per-rule: matching count + flagged IDs
    agg_results: Vec<AggResult>,    // Per-aggregation: value + count
    input_hash: [u8; 32],
}
```

## Comparison with RISC Zero Version

| Aspect | This (SP1) | RISC Zero |
|--------|-----------|-----------|
| zkVM | SP1 | RISC Zero v3.0 |
| Entry point | `sp1_zkvm::entrypoint!` | `risc0_zkvm::guest::entry!` |
| I/O | `sp1_zkvm::io::read/commit` | `risc0_zkvm::guest::env::read/commit` |
| Algorithm | Identical | Identical |

## See Also

- [RISC Zero rule engine](../rule-engine/) -- original RISC Zero version
- [SP1 documentation](https://succinctlabs.github.io/sp1/)
