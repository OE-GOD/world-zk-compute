# Anomaly Detector -- risc0-zkvm Guest Program

A statistical anomaly detection algorithm that runs inside the RISC Zero
zkVM. It analyzes data points, flags outliers, and produces a
zero-knowledge proof -- the raw data stays private while the detection
results are publicly verifiable on-chain.

## What it does

The guest program accepts a set of data points (each with a feature
vector and timestamp), detection parameters, and a threshold, then:

1. Computes mean and standard deviation per feature across all points.
2. Scores each point using z-score normalization.
3. Flags points exceeding `threshold * 3.0` (3-sigma rule).
4. Computes an overall risk score.
5. Hashes all inputs with SHA-256 and commits the detection results
   (flagged IDs, anomaly count, risk score, input hash) to the journal.

Use cases: Sybil detection, presentation attack detection, operator
fraud detection, geographic anomaly detection.

## Directory structure

```
anomaly-detector/
  Cargo.toml              workspace root (members: host, methods/guest)
  host/
    src/main.rs            host program -- prepares input, demonstrates
                           the World ZK Compute submission flow
  methods/
    guest/src/main.rs      guest program -- anomaly detection inside zkVM
```

## How it works

**Guest** (inside the zkVM):
1. Reads detection input (data points, threshold, parameters).
2. Computes per-feature statistics (mean, stddev).
3. Scores each point via z-score normalization.
4. Flags outliers exceeding the threshold.
5. Commits detection output to the journal.

**Host** (untrusted):
1. Creates sample input with normal and anomalous data points.
2. Serializes input and computes its SHA-256 hash.
3. Demonstrates the submission flow: upload input, submit to chain,
   wait for proof, parse results from the journal.

The guest is designed as a template -- replace the z-score algorithm
with more sophisticated methods (Isolation Forest, DBSCAN, neural
networks) as needed.

## Prerequisites

- Rust 1.75+
- RISC Zero toolchain:
  ```bash
  cargo install cargo-binstall && cargo binstall cargo-risczero
  ```

## Build

```bash
cd examples/anomaly-detector
cargo build --release
```

## Run

```bash
cargo run --release -p anomaly-detector-host
```

The host program prepares 10 sample data points (8 normal + 2
anomalous) and walks through the World ZK Compute submission flow.

## Example output

```
=== World ZK Compute - Anomaly Detection Example ===

1. Preparing detection input...
   - 10 data points to analyze
   - Threshold: 0.8

2. Uploading input data...

3. Program image ID: 0x4242...

4. Submitting detection job to World ZK Compute...

5. Waiting for proof...

6. Detection results (from proof journal):
   - Total analyzed: 10
   - Anomalies found: 2
   - Risk score: 15.00%
   - Flagged IDs:
     - 0x0800...
     - 0x0900...

7. Proof verified on-chain!
```

## Detection parameters

| Parameter | Description |
|-----------|-------------|
| `threshold` | Base sensitivity (multiplied by 3 for z-score cutoff) |
| `window_size` | Sliding window size for temporal analysis |
| `min_cluster_size` | Minimum cluster size for density-based methods |
| `distance_threshold` | Distance cutoff for clustering |

## Related

- [Main project README](../../README.md)
- [Prover binary](../../prover/) -- generates on-chain verifiable proofs
- [Contracts](../../contracts/) -- Solidity on-chain verification
