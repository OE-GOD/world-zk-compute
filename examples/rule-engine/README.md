# Rule Engine -- risc0-zkvm Guest Program

A rule-based classification engine that runs inside the RISC Zero zkVM,
producing a zero-knowledge proof that rules were evaluated correctly
without revealing the underlying data.

## What it does

The guest program accepts a set of **records**, **rules**, and
**aggregation definitions**, then:

1. Evaluates every rule against every record. Supported condition types
   include integer comparisons (gt, gte, lt, lte, eq, neq), string
   prefix, suffix, contains, and glob matching.
2. Rules combine conditions with AND or OR logic.
3. Produces per-rule match counts and lists of flagged record IDs.
4. Computes aggregations (count, sum, min, max) over matching subsets.
5. Hashes all inputs with SHA-256 (risc0 accelerated precompile) and
   commits the results plus the input hash to the zkVM journal.

## Directory structure

```
rule-engine/
  Cargo.toml              workspace root (members: host, methods)
  host/
    src/main.rs            host program -- prepares input, runs the zkVM
    Cargo.toml
  methods/
    guest/src/main.rs      guest program -- runs inside the zkVM (#![no_std])
    build.rs               risc0 build script (compiles guest ELF)
    src/lib.rs             re-exports image ID and ELF bytes
```

## How host and guest interact

```
 Host (untrusted)                       Guest (inside zkVM)
 ─────────────────                      ──────────────────
 1. Serialize RuleEngineInput    ─────>  env::read()
 2. Call executor.execute()              parse input (manual wire format)
                                         evaluate rules against records
                                         compute aggregations
                                  <────  env::commit(RuleEngineOutput)
 3. Read journal
 4. Optionally generate STARK/Groth16 proof
```

The host serializes `RuleEngineInput` (records, rules, aggregations) into
the executor environment. The guest deserializes it using a manual wire-format
reader (no serde dependency -- lower cycle count), evaluates the rules,
and commits `RuleEngineOutput` to the journal. The host then reads the
journal and can optionally generate a proof.

## Prerequisites

- Rust 1.75+
- RISC Zero toolchain:
  ```bash
  cargo install cargo-binstall && cargo binstall cargo-risczero
  rzup install r0vm 3.0.5
  ```

## Build

```bash
cd examples/rule-engine
cargo build --release
```

## Run (execute-only, no proof)

```bash
cargo run --release --bin rule-engine-host
```

This runs the guest in execute-only mode and prints the cycle count plus
evaluation results.

## Example input/output

**Input** (created programmatically in the host):
- 10 records, each with 4 integer fields (timestamp, score, count, flag)
  and 2 string fields (region, device)
- 3 rules:
  - Rule 0 (AND): score > 90 AND region starts with "us"
  - Rule 1 (OR): flag >= 10 OR score <= 82
  - Rule 2 (AND): device matches "iphone-*" AND region contains "east"
- 4 aggregations: count all, sum scores for rule-0 matches, max flag for
  rule-1 matches, min score overall

**Output**:
```
=== RISC Zero Rule Engine Benchmark ===

Input: 10 records, 3 rules, 4 aggregations

--- Execute Mode (no proof) ---
RISC0 Execution time: 1.23s
RISC0 Cycles: 524288

Results:
  Total records: 10
  Input hash: 0xabcdef...
  Rule 0: 3 matches, flagged: [0x00..11, 0x06..62, 0x09..99]
  Rule 1: 2 matches, flagged: [0x08..88, 0x09..99]
  Agg 0: value=10, count=10
  Agg 1: value=267, count=3
```

## Related

- [Main project README](../../README.md)
- [Prover binary](../../prover/) -- generates on-chain verifiable proofs
- [Contracts](../../contracts/) -- Solidity on-chain verification
- [E2E test script](../../scripts/e2e-test.sh) -- end-to-end test with proof generation
