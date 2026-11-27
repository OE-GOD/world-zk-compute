# XGBoost Remainder (GKR + Hyrax) Example

Proves XGBoost decision tree inference using the Remainder proof system
(GKR protocol with Hyrax polynomial commitments). The prover demonstrates
that a given feature vector produces a specific classification without
revealing the input features.

## Architecture

The system encodes the full XGBoost inference pipeline as an 88-layer
DAG circuit:

### Phase 1a -- Leaf selection

Each tree's leaf values are arranged in a table indexed by path bits.
An MLE fold (using binary path bits as selectors) picks the correct
leaf for each tree, then a pairwise aggregation sums the selected
leaves across all trees.

### Phase 1b -- Comparison verification

For every internal node in every tree, bit decomposition proves that
each traversal decision matches the `feature[i] >= threshold` comparison.
The diff `feature - threshold` is decomposed into K=18 bits; the sign
bit must equal the corresponding path bit. Positions beyond the real
tree depth are masked by an `is_real` flag.

### DAG topology

The circuit has 88 compute layers organized as a DAG (not a simple
chain). Layers include fold operations, pairwise sums, comparison
checks, and virtual-tree parallel folds for threshold/feature/is_real
tables. The DAG topology is described by point templates, oracle
expressions, and atom-to-commitment mappings.

## Directory structure

```
xgboost-remainder/
  Cargo.toml
  src/
    lib.rs                re-exports
    circuit.rs            GKR circuit builder (Phase 1a + 1b)
    model.rs              XGBoost/LightGBM JSON model loading
    abi_encode.rs          ABI encoding for Solidity verifier calldata
    detect.rs             model format detection
    server.rs             warm prover HTTP server (axum)
    main.rs               CLI entry point
    lightgbm.rs           LightGBM model parser
    random_forest.rs, mlp.rs, logistic_regression.rs
                          other converters; not supported (see the status note
                          in the main README)
    bin/                  utility binaries (fixture generators, etc.)
  gnark-wrapper/          Go gnark Groth16 wrapper circuit
    circuit.go            simple GKR wrapper
    dag_circuit.go        DAG GKR wrapper (variable-width layers)
    dag_main.go           DAG prove/verify commands
    poseidon.go           gnark Poseidon circuit
    gnark-wrapper         binary, not checked in (build it: go build -o gnark-wrapper .)
  benches/
    prover_bench.rs       criterion benchmarks
  tests/
    server_integration.rs warm prover HTTP integration tests
  sample_model.json       sample XGBoost model for testing
  sample_input.json       sample input features
```

## Build

```bash
cd examples/xgboost-remainder
cargo build --release
```

Requires the `Remainder_CE` dependency (fetched from GitHub automatically
via the git dependency in Cargo.toml).

## Run

**One-shot mode** -- build circuit, prove, output ABI-encoded proof:

```bash
cargo run --release -- --model sample_model.json --input sample_input.json --output proof.json
```

**Execute-only mode** -- run inference without generating a proof:

```bash
cargo run --release -- --model sample_model.json --input sample_input.json --execute-only
```

**Warm server mode** -- precompile the circuit once, serve proofs via HTTP:

```bash
cargo run --release -- --model sample_model.json --serve --port 3000
```

Proofs that take longer than `--request-timeout` (default 120 s) get a 504.
A proof cannot be cancelled: it keeps running on the server after the 504
and its result is dropped. `openapi.yaml` describes `/health`, `/prove` and
`/prove/batch`; `/prove/bundle` is not in it yet. At most as many proofs run at
once as the server's async runtime has worker threads (one per CPU by default);
a request that waits for a free slot past its timeout gets the same 504. Under
sustained overload this can turn every answer into a 504: a request reaches a
free slot with little of its timeout left, starts its proof, times out, and
keeps the slot until the proof ends.

Supports XGBoost (`--model-format xgboost`) and LightGBM
(`--model-format lightgbm`) JSON model files.

## Testing

```bash
# Run all tests (34+ tests, ~95s in release mode)
cargo test --release
```

Tests include:
- Circuit construction for various tree depths
- Prove-and-verify round trips (5 tests)
- XGBoost JSON model parsing (4 tests)
- Fixture generation for on-chain contract tests
- LightGBM model parsing

## Generating fixtures

The test suite generates fixtures used by the Solidity verifier tests:

```bash
# Generate the phase1a DAG fixture for contract tests
cargo test --release gen_phase1a_fixture -- --nocapture --ignored
# Output: contracts/test/fixtures/phase1a_dag_fixture.json (~475KB)
```

## Generating Groth16 proofs

For the hybrid Groth16 verification path:

```bash
# 1. Generate the DAG Groth16 witness from a proof (JSON is written to stdout)
cargo run --release --bin gen_dag_groth16_witness > witness.json

# 2. Build the gnark wrapper and run it to produce the Groth16 proof
(cd gnark-wrapper && go build -o gnark-wrapper .)
cat witness.json | ./gnark-wrapper/gnark-wrapper prove-dag-json --config-json

# 3. The output includes the Groth16 proof and the Solidity verifier contract
```

## gnark-wrapper usage

The Go gnark wrapper compiles a Groth16 circuit that wraps the GKR
verification's Fr-arithmetic portion:

```bash
cd gnark-wrapper

# Simple GKR circuit (reads witness JSON from stdin)
go run . prove < witness.json

# DAG GKR circuit (reads witness JSON from stdin)
cat witness.json | go run . prove-dag-json --config-json

# Show circuit info
go run . dag-info --config-json < witness.json
```

The wrapper auto-exports a Solidity verifier contract
(`DAGRemainderGroth16Verifier.sol`) with embedded verification keys.

## On-chain verification

Proofs generated by this example are verified by Solidity contracts in
`contracts/src/remainder/`:

| Contract | Purpose |
|----------|---------|
| `RemainderVerifier.sol` | Main entry point (direct + Groth16 hybrid) |
| `GKRDAGVerifier.sol` | DAG compute layer verification library |
| `GKRDAGHybridVerifier.sol` | Hybrid transcript replay + EC checks |
| `DAGBatchVerifier.sol` | Multi-tx batch verification (15 txs) |
| `HyraxVerifier.sol` | Hyrax polynomial commitment verification |
| `SumcheckVerifier.sol` | Sumcheck protocol verification |
| `PoseidonSponge.sol` | Poseidon hash (Fiat-Shamir transcript) |

**Gas costs**:
- Direct DAG verification: ~254M gas (exceeds block limit, needs batching)
- Batch verification: 15 transactions plus an optional cleanup; measured as real transactions the largest step uses 31.7M gas
- Groth16 hybrid: ~251M gas (EC operations dominate)

## Related

- [Main project README](../../README.md)
- [Contracts](../../contracts/src/remainder/) -- Solidity verifier contracts
- [Contract tests](../../contracts/test/) -- Solidity tests using fixtures from this crate
