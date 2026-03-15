# Sybil Detector

A RISC Zero guest program that analyzes World ID registration patterns to detect coordinated fake identity creation -- a core threat to the "one person = one World ID" guarantee.

## What It Does

Given a batch of registration records, the guest runs five detection heuristics and produces a provably correct risk assessment. The ZK proof guarantees that:

1. The detection algorithm was executed correctly on the input data.
2. Flagged registrations genuinely trigger detection heuristics.
3. Risk scores are mathematically correct and reproducible.

## Detection Heuristics

| Heuristic | Max Score | What It Detects |
|-----------|-----------|-----------------|
| **Temporal Clustering** | 200 | Burst of registrations at the same orb within a time window |
| **Geographic Impossibility** | 300 | Impossible travel speed between registrations (different locations, close timestamps) |
| **Session Anomaly** | 250 | Suspiciously fast iris scan sessions (below minimum expected duration) |
| **Orb Rate Abuse** | 200 | Orb exceeding maximum registrations per hour |
| **Quality Score Anomaly** | 250 | Statistical outliers in iris quality scores (low-quality bulk scans) |

Each registration accumulates a score from 0 to 1000. Registrations scoring 500 or above are flagged.

## Project Structure

```
sybil-detector/
  methods/
    guest/
      Cargo.toml       # Guest dependencies (risc0-zkvm, serde, sha2)
      src/main.rs       # Detection algorithm (runs inside RISC Zero zkVM)
```

## Prerequisites

- Rust toolchain (stable)
- RISC Zero toolchain:
  ```bash
  curl -L https://risczero.com/install | bash
  rzup install
  ```

## Build

```bash
cd examples/sybil-detector
cargo build --release
```

## Input Format

```rust
struct SybilDetectionInput {
    registrations: Vec<Registration>,  // Batch of registration records
    config: SybilConfig,               // Thresholds and parameters
}

struct Registration {
    id: [u8; 32],                  // Nullifier hash
    orb_id: [u8; 16],             // Orb identifier
    timestamp: u64,                // Unix timestamp
    geo_hash: u32,                 // Location grid cell
    session_duration_secs: u32,    // Iris scan session duration
    iris_score: u32,               // Iris quality score (0-1000)
    verification_latency_ms: u32,  // Time from scan to verification
}
```

## Output Format

```rust
struct SybilDetectionOutput {
    total_registrations: u32,
    flagged_count: u32,
    flagged_ids: Vec<[u8; 32]>,
    risk_scores: Vec<u32>,        // Per-flagged risk score (0-1000)
    cluster_count: u32,            // Number of suspicious clusters
    flags_breakdown: FlagsBreakdown,  // Count per heuristic
    input_hash: [u8; 32],
}
```

## Configuration

All detection thresholds are configurable via `SybilConfig`:

- `time_window_secs` -- window for temporal clustering
- `geo_proximity_threshold` -- geohash distance for "nearby" locations
- `min_cluster_size` -- minimum registrations to form a suspicious cluster
- `velocity_threshold_kmh` -- maximum travel speed before flagging
- `min_session_duration_secs` -- minimum expected iris scan duration
- `max_registrations_per_orb_per_hour` -- rate limit per orb

## See Also

- [World ID documentation](https://docs.worldcoin.org/)
- [World ZK Compute project root](../../)
