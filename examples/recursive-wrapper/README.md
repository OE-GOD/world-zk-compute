# Recursive Proof Wrapper

A RISC Zero guest program that verifies multiple sub-proof receipts inside the zkVM, producing a single aggregated proof that covers all sub-computations.

## What It Does

When a large computation is split into K independent pieces (e.g., processing K batches of data), each piece produces its own RISC Zero receipt. Without recursive wrapping, a verifier must check all K receipts independently.

This wrapper guest:

1. Receives K sub-proof journals from the host.
2. Calls `env::verify(image_id, journal)` for each sub-proof inside the zkVM.
3. Computes a SHA-256 hash over all sub-journals (integrity commitment).
4. Commits a single `WrapperOutput` that covers all K sub-computations.

The result is one receipt that proves all K inner computations were valid.

## Why This Matters

Without recursive verification, journal merging happens outside the zkVM where a malicious prover could fabricate journals. By verifying sub-proofs inside a proven execution, the trust gap is closed -- the wrapper receipt guarantees every inner receipt was valid.

## How It Works

```
  Inner Proof 1 ─┐
  Inner Proof 2 ─┤
  Inner Proof 3 ─┼──> Recursive Wrapper Guest ──> Single Aggregated Receipt
  ...            │    (verifies each inside zkVM)
  Inner Proof K ─┘
```

The host provides inner receipts via `add_assumption()`. The guest calls `env::verify()` which checks each `(image_id, journal)` pair against the assumptions.

## Project Structure

```
recursive-wrapper/
  Cargo.toml           # Workspace: members = ["methods"]
  methods/
    Cargo.toml         # Methods crate (builds guest, exports image ID)
    build.rs           # Guest compilation via risc0-build
    guest/
      Cargo.toml       # Guest program dependencies
      src/main.rs      # Recursive verification logic
    src/lib.rs         # Exports compiled guest image ID
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
cd examples/recursive-wrapper
cargo build --release
```

## Input / Output

**Input** (`WrapperInput`):

```rust
struct WrapperInput {
    inner_image_id: [u8; 32],    // Image ID of the inner guest
    sub_proof_count: u32,         // Number of sub-proofs to verify
    sub_journals: Vec<Vec<u8>>,   // Journals from each sub-proof
    merged_journal: Vec<u8>,      // Pre-merged journal from host
}
```

**Output** (`WrapperOutput`):

```rust
struct WrapperOutput {
    inner_image_id: [u8; 32],    // Which inner guest was verified
    sub_proof_count: u32,         // How many sub-proofs were verified
    journals_hash: [u8; 32],      // SHA-256 commitment to all sub-journals
    merged_journal: Vec<u8>,      // Merged result covering all sub-computations
}
```

## Usage Pattern

```rust
// Host side (pseudocode)
let inner_receipts: Vec<Receipt> = run_inner_proofs(batches);

let wrapper_input = WrapperInput {
    inner_image_id: INNER_GUEST_ID,
    sub_proof_count: inner_receipts.len() as u32,
    sub_journals: inner_receipts.iter().map(|r| r.journal.bytes.clone()).collect(),
    merged_journal: merge_journals(&inner_receipts),
};

let env = ExecutorEnv::builder()
    .write(&wrapper_input)?;
for receipt in &inner_receipts {
    env.add_assumption(receipt.clone());
}

let wrapper_receipt = prover.prove(env.build()?, WRAPPER_ELF)?;
// wrapper_receipt is a single proof covering all inner computations
```

## See Also

- [RISC Zero composition docs](https://dev.risczero.com/api/zkvm/composition)
- [World ZK Compute project root](../../)
