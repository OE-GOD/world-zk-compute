# XGBoost Remainder (GKR + Hyrax) Example

## Overview

Proves XGBoost decision tree inference using the Remainder proof system (GKR protocol
with Hyrax polynomial commitments). The prover demonstrates that a given feature vector
produces a specific classification without revealing the input features.

## Architecture

The system builds an 88-layer DAG circuit encoding the full XGBoost inference pipeline:

1. **Leaf selection** -- MLE fold selects the correct leaf per tree using binary path bits
2. **Comparison verification** -- bit decomposition proves each tree traversal decision
   matches `feature[i] >= threshold` comparisons
3. **Aggregation** -- pairwise sum of selected leaves across all trees
4. **Output** -- proves the aggregate equals the claimed prediction class

Proof verification can happen on-chain via Solidity contracts in `contracts/src/remainder/`,
either directly (~254M gas for the full DAG) or via multi-transaction batch verification
(15 transactions, each under 30M gas).

## Build

```bash
cd examples/xgboost-remainder
cargo build --release
```

Requires the Remainder_CE dependency (fetched from GitHub automatically).

## Run

**One-shot mode** -- build circuit, prove, and output ABI-encoded proof:

```bash
cargo run --release -- --model model.json --input input.json --output proof.json
```

**Execute-only mode** -- run inference without generating a proof:

```bash
cargo run --release -- --model model.json --input input.json --execute-only
```

**Warm server mode** -- precompile the circuit once, serve proofs via HTTP:

```bash
cargo run --release -- --model model.json --serve --port 3000
```

Supports XGBoost (`--model-format xgboost`) and LightGBM (`--model-format lightgbm`)
JSON model files.

## Testing

```bash
cargo test --release
```

Runs 34+ tests including circuit construction, prove-and-verify round trips, JSON model
parsing, and fixture generation for on-chain verification tests.

## Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point (one-shot and server modes) |
| `src/circuit.rs` | GKR circuit builder for XGBoost inference |
| `src/model.rs` | XGBoost/LightGBM model loading and utilities |
| `src/abi_encode.rs` | ABI encoding for Solidity verifier calldata |
| `src/server.rs` | Warm prover HTTP server (axum) |
