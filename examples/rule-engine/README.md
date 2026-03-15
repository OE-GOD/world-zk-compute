# Rule Engine Example

## Overview

A general-purpose rule engine that runs inside the RISC Zero zkVM (v3.0). It evaluates
configurable rules and aggregations against a set of records, producing a verifiable proof
that the computation was performed correctly.

Capabilities:
- **Pattern matching** -- glob, contains, prefix, suffix on string fields
- **Comparison** -- gt, gte, lt, lte, eq, neq on integer fields
- **Aggregation** -- count, sum, min, max with optional rule-based filtering
- **Rules** -- logical AND/OR expressions combining multiple conditions

## Prerequisites

- Rust 1.75+
- RISC Zero toolchain: `cargo install cargo-binstall && cargo binstall cargo-risczero`
- `rzup install r0vm 3.0.5` (must match risc0-zkvm 3.0 crate version)

## Build

```bash
cd examples/rule-engine
cargo build --release
```

## Run

Execute the host program, which feeds sample data to the guest and runs it in the zkVM:

```bash
cargo run --release --bin rule-engine-host
```

The host creates 10 sample records with integer and string fields, defines 3 rules
(AND/OR combinations), and 4 aggregation queries. It prints cycle count, execution time,
and the decoded results.

## How It Works

1. **Host** (`host/src/main.rs`) serializes records, rules, and aggregation definitions
   into the zkVM environment using risc0 serde format.
2. **Guest** (`methods/guest/src/main.rs`) runs inside the zkVM:
   - Parses input using a manual wire-format reader (no serde dependency for lower cycle count)
   - Evaluates each rule against every record (AND/OR logic)
   - Computes aggregations (count/sum/min/max) with optional rule filters
   - Hashes all inputs with SHA-256 (using risc0's accelerated precompile)
   - Commits results to the journal
3. **Output** includes matching counts, flagged record IDs, aggregation values,
   and an input hash for integrity verification.

The guest uses `#![no_std]` with manual serialization to minimize cycle overhead.
SHA-256 uses the risc0 precompile for ~100x faster hashing inside the VM.
