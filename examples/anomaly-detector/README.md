# Anomaly Detector Example

## Overview

A statistical anomaly detection algorithm running inside the RISC Zero zkVM. It analyzes
a set of data points, computes z-scores per feature, and flags outliers -- all proven in
zero knowledge. The raw data remains private; only the detection results (flagged IDs and
risk score) are publicly committed.

Use cases: Sybil detection, presentation attack detection, operator fraud detection,
geographic anomaly detection.

## Prerequisites

- Rust 1.75+
- RISC Zero toolchain: `cargo install cargo-binstall && cargo binstall cargo-risczero`

## Build

```bash
cd examples/anomaly-detector
cargo build --release
```

## Run

The host program prepares sample data (8 normal + 2 anomalous points) and walks through
the World ZK Compute submission flow:

```bash
cargo run --release -p anomaly-detector-host
```

Output includes: total analyzed, anomalies found, risk score, and flagged IDs.

## How It Works

**Guest** (`methods/guest/src/main.rs`):
1. Reads detection input (data points with feature vectors, threshold, parameters)
2. Computes mean and standard deviation per feature across all data points
3. Scores each point using z-score normalization
4. Flags points exceeding `threshold * 3.0` (3-sigma rule)
5. Hashes all inputs with SHA-256 for integrity verification
6. Commits detection output (flagged IDs, risk score, input hash) to the journal

**Host** (`host/src/main.rs`):
1. Creates sample input with normal and anomalous data points
2. Demonstrates the submission flow: hash input, upload, submit to chain, wait for proof
3. Prints detection results

The guest is designed as a template -- replace the z-score algorithm with more
sophisticated methods (Isolation Forest, DBSCAN, neural networks) as needed.
