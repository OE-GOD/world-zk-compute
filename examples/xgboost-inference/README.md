# XGBoost Inference (RISC Zero)

A RISC Zero guest program that performs XGBoost gradient-boosted tree inference inside the zkVM. The proof guarantees that the model was applied correctly to every sample, tree traversal was deterministic, and flagging decisions match the threshold exactly.

## What It Does

1. Reads an XGBoost model (trees, thresholds, base score) and a batch of samples.
2. For each sample, traverses every tree to compute leaf values.
3. Aggregates tree outputs: `score = base_score + sum(tree_leaf_values)`.
4. Flags samples where `score >= threshold`.
5. Commits predictions, flagged IDs, and an input hash as public output.

## Comparison with xgboost-remainder

This example and `xgboost-remainder` both perform XGBoost inference, but they use different proof systems:

| Aspect | This (xgboost-inference) | xgboost-remainder |
|--------|--------------------------|-------------------|
| Proof system | RISC Zero zkVM (STARK) | Remainder (GKR + Hyrax) |
| Prover | General-purpose zkVM | Custom arithmetic circuit |
| On-chain verification | risc0 verifier router | GKR DAG verifier (Solidity) |
| Proving time | Minutes (general zkVM) | Seconds (specialized circuit) |
| Model format | Serialized structs | XGBoost JSON import |
| Circuit complexity | N/A (interpreted execution) | 88-layer DAG circuit |

**When to use which:**
- Use **xgboost-inference** for simplicity and general-purpose zkVM compatibility.
- Use **xgboost-remainder** for faster proving, smaller proofs, and lower on-chain verification cost (at the cost of a more complex setup).

## Project Structure

```
xgboost-inference/
  methods/
    Cargo.toml         # Methods crate
    build.rs           # Guest compilation via risc0-build
    src/lib.rs         # Exports compiled guest image ID
    guest/
      Cargo.toml       # Guest dependencies
      src/main.rs      # XGBoost inference logic (manual wire format)
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
cd examples/xgboost-inference
cargo build --release
```

## Guest Optimizations

The guest uses manual wire format parsing instead of serde to reduce cycle count:

- Reads u32 LE words directly via `env::read::<u32>()`.
- Reconstructs f64 from two u32 words.
- Writes output as a flat `Vec<u32>` via `env::commit_slice`.

This avoids the overhead of serde's visitor pattern, which is significant inside a zkVM where every instruction is proven.

## Input Format (Wire)

The guest reads from the host in this order:

1. **Model**: `num_features` (u32), `num_classes` (u32), `base_score` (f64), trees (each: `num_nodes` + node array)
2. **Samples**: `num_samples` (u32), then per sample: `id` ([u8; 32] as 32 u32 words) + `features` (Vec of f64)
3. **Threshold**: f64

## Output Format (Wire)

Written as u32 words: `total_samples`, predictions array (id + score + flagged), `flagged_count`, `flagged_ids`, `input_hash`.

## See Also

- [xgboost-remainder](../xgboost-remainder/) -- GKR + Hyrax version with custom arithmetic circuits
- [RISC Zero documentation](https://dev.risczero.com/)
- [World ZK Compute project root](../../)
