# Guest Programs

This document covers how guest programs are built, named, and managed in World ZK Compute.

## Overview

Guest programs are RISC-V binaries that run inside the risc0-zkvm to produce zero-knowledge proofs of computation. Each program takes structured input, performs computation, and produces a journal (output) that can be verified on-chain.

## Programs Directory

Pre-compiled guest program binaries live in `programs/`, named by their **image ID** (hex):

| File | Program | risc0 Version |
|------|---------|---------------|
| `f85c194e...b56e33.elf` | Rule Engine | v3.0 |
| `24c3af82...82bc95.elf` | Anomaly Detector | v1.2 |
| `28d93899...3ddd34.elf` | Sybil Detector | v1.2 |
| `bed77c59...22f73.elf` | Signature-Verified | v1.2 |
| `d6b60ae7...8aefe4.elf` | XGBoost Inference | v1.2 |
| `ee666bd1...46e4.elf` | (Legacy) | v1.2 |
| `recursive-wrapper.elf` | Recursive Wrapper | v3.0 |

> **Note:** Despite the `.elf` extension, risc0 v3.0 programs are in `.bin` format. The extension is kept for historical compatibility.

## Guest Program Inventory

| Program | Location | Purpose |
|---------|----------|---------|
| **Rule Engine** | `examples/rule-engine/` | Pattern matching, aggregation, comparison rules (AND/OR) |
| **Anomaly Detector** | `examples/anomaly-detector/` | Temporal + clustering analysis for anomaly detection |
| **Sybil Detector** | `examples/sybil-detector/` | 5 heuristic checks: temporal clustering, geo-impossibility, session anomaly, orb rate abuse, quality score |
| **Signature-Verified** | `examples/signature-verified/` | ECDSA verification + detection using accelerated precompiles |
| **XGBoost Inference** | `examples/xgboost-inference/` | Tree traversal, leaf selection, aggregation, threshold flagging |
| **XGBoost Remainder** | `examples/xgboost-remainder/` | GKR+Hyrax circuit-based XGBoost inference with Groth16 |
| **Recursive Wrapper** | `examples/recursive-wrapper/` | Verifies K sub-proofs inside zkVM, merges journals |

### SP1 Variants (Experimental)

- `examples/rule-engine-sp1/` — SP1 version of rule engine
- `examples/anomaly-detector-sp1/` — SP1 version of anomaly detector

## How Image IDs Are Computed

Image IDs are deterministic 32-byte hashes of the compiled guest ELF binary, computed by risc0:

```rust
let image_id = risc0_zkvm::compute_image_id(elf_bytes)?;
```

The image ID uniquely identifies a guest program. Any change to the guest code, dependencies, or risc0 version will produce a **different image ID**.

The prover computes image IDs at runtime (`prover/src/monitor.rs`):
```rust
fn compute_image_id(elf: &[u8]) -> anyhow::Result<B256> {
    let digest = risc0_zkvm::compute_image_id(elf)?;
    Ok(B256::from_slice(digest.as_bytes()))
}
```

## Building Guest Programs

### Directory Structure

Each guest program follows this layout:

```
examples/{program-name}/
├── Cargo.toml              # Workspace root
├── methods/
│   ├── Cargo.toml          # Methods crate (risc0-build)
│   ├── build.rs            # Calls risc0_build::embed_methods()
│   └── guest/
│       ├── Cargo.toml      # Guest dependencies (risc0-zkvm)
│       └── src/main.rs     # Guest program code
└── host/
    ├── Cargo.toml          # Host program
    └── src/main.rs         # Prover/executor code
```

### Build Steps

1. **Install r0vm** (must match risc0-zkvm crate version):
   ```bash
   rzup install r0vm 3.0.5   # For risc0-zkvm 3.0
   ```

2. **Build the guest program:**
   ```bash
   cd examples/rule-engine
   cargo build --release
   ```
   This triggers `methods/build.rs` which calls `risc0_build::embed_methods()` to compile the guest to RISC-V.

3. **The host binary** can then access the compiled guest:
   ```rust
   use rule_engine_methods::{RULE_ENGINE_GUEST_ELF, RULE_ENGINE_IMAGE_ID};
   ```

4. **Copy to programs directory** (named by image ID):
   ```bash
   # The image ID is printed during build or can be computed
   cp target/release/build/.../rule_engine_guest.bin \
      programs/<IMAGE_ID_HEX>.elf
   ```

### Rebuilding After Code Changes

When you modify guest code:

1. Rebuild the guest: `cargo build --release` in the example directory
2. Compute the new image ID (it will change if any code changed)
3. Replace the old binary in `programs/` with the new one, using the new image ID as filename
4. Update any references to the old image ID (e.g., in tests, configs, CLAUDE.md)

## Version Compatibility

### Current State

- **risc0 v3.0**: Only the rule-engine guest has been upgraded
- **risc0 v1.2**: The other 4 guests (anomaly-detector, sybil-detector, signature-verified, xgboost-inference) still use v1.2

### Important Rules

- **r0vm version must match** the `risc0-zkvm` crate version (e.g., r0vm 3.0.5 for risc0-zkvm 3.0)
- **Image IDs change** when rebuilding with a different risc0 version
- **v3.0 uses `.bin` format** for `prove()`, not raw ELF
- The Sepolia verifier router (`0x925d8331...`) requires the risc0 v3.0.x selector (`73c457ba`). The v1.2.x selector is tombstoned.

### Upgrading a Guest to v3.0

1. Update `methods/Cargo.toml`: `risc0-build = "3.0"`
2. Update `methods/guest/Cargo.toml`: `risc0-zkvm = "3.0"`
3. Update `host/Cargo.toml`: `risc0-zkvm = { version = "3.0", features = ["prove"] }`
4. Remove serde from guest if using manual wire format (v3.0 recommends avoiding serde in guests)
5. Rebuild and update the image ID everywhere

## Prover Configuration

The prover can restrict which guest programs it accepts:

```rust
// prover/src/config.rs
pub struct ProverConfig {
    pub allowed_image_ids: Vec<B256>,  // Empty = accept all
    // ...
}
```

## GPU Acceleration

Guest program proving can be accelerated with GPU:

```bash
cargo build --release --features cuda    # NVIDIA GPU
cargo build --release --features metal   # Apple GPU (macOS)
```

## Testing

Run guest program tests via their host executors:

```bash
cd examples/rule-engine && cargo test
cd examples/xgboost-remainder && cargo test  # Slow (~95s)
```

Or via Makefile targets:
```bash
make test-xgboost    # XGBoost remainder tests
```
