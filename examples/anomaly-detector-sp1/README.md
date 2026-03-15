# Anomaly Detector (SP1)

SP1 port of the anomaly detection guest program. This implements the same z-score-based statistical anomaly detection algorithm as the RISC Zero version, but compiled for the [SP1 zkVM](https://github.com/succinctlabs/sp1) instead.

## What It Does

Given a set of data points with feature vectors, the guest program:

1. Computes per-feature mean and standard deviation.
2. Scores each data point using z-scores (distance from mean in units of std dev).
3. Flags data points exceeding a configurable threshold (3-sigma rule).
4. Commits the results (flagged IDs, risk score, input hash) as public output.

The SP1 proof guarantees that the detection algorithm ran correctly and the output matches the input data exactly.

## Project Structure

```
anomaly-detector-sp1/
  Cargo.toml          # Workspace: members = ["script"], exclude = ["program"]
  program/
    Cargo.toml        # SP1 guest program crate
    src/main.rs       # Detection algorithm (runs inside SP1 zkVM)
  script/
    Cargo.toml        # Host-side script (builds guest, generates proof)
    build.rs          # Builds the SP1 guest ELF
    src/main.rs       # Host: loads input, runs prover, verifies proof
```

## Prerequisites

- Rust toolchain (stable)
- SP1 toolchain installed via [sp1up](https://succinctlabs.github.io/sp1/getting-started/install.html):
  ```bash
  curl -L https://sp1up.succinct.xyz | bash
  sp1up
  ```

## Build

Build the guest program and host script:

```bash
cd examples/anomaly-detector-sp1
cargo build --release
```

The guest ELF is compiled automatically by `script/build.rs` during the build.

## Run

Generate a proof and verify it:

```bash
cd examples/anomaly-detector-sp1
cargo run --release --bin script
```

This will:
1. Compile the guest program to a RISC-V ELF.
2. Execute the guest with sample input data.
3. Generate an SP1 proof.
4. Verify the proof locally.

## Input Format

The guest expects a `DetectionInput` struct (serialized via serde):

```rust
struct DetectionInput {
    data_points: Vec<DataPoint>,  // Items to analyze
    threshold: f64,               // Detection sensitivity (0.0-1.0)
    params: DetectionParams,      // Window size, cluster size, distance threshold
}
```

## Output Format

The guest commits a `DetectionOutput`:

```rust
struct DetectionOutput {
    total_analyzed: usize,
    anomalies_found: usize,
    flagged_ids: Vec<[u8; 32]>,
    risk_score: f64,
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
| Proof system | STARK + Groth16 wrap | STARK + Groth16 wrap |

The detection logic is the same. Only the zkVM interface layer differs.

## See Also

- [RISC Zero anomaly detector](../anomaly-detector/) -- original RISC Zero version
- [SP1 documentation](https://succinctlabs.github.io/sp1/)
