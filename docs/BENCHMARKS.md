# ZKML Verifier Benchmarks

Performance measurements for the zkml-verifier crate on the 88-layer XGBoost credit scoring circuit.

## Running Benchmarks

```bash
# Full benchmark suite
cargo bench -p zkml-verifier

# Specific benchmark
cargo bench -p zkml-verifier -- verify_dag

# HTML report (opens in browser)
cargo bench -p zkml-verifier -- --output-format html
open target/criterion/report/index.html
```

## Results

All measurements on Apple M-series (ARM64), single-threaded, release build.

### Native (aarch64-apple-darwin)

| Benchmark | Time | Description |
|---|---|---|
| `verify_dag_88layer` | ~4-5s | Full 88-layer XGBoost GKR DAG verification |
| `decode_proof_blob` | ~1ms | Parse binary proof blob (133KB) |
| `parse_bundle_json` | ~5ms | Parse JSON fixture (475KB) |
| `hex_decode_proof` | ~0.5ms | Decode hex proof string to bytes |

### Breakdown (88-layer XGBoost)

| Phase | Approx. % | Description |
|---|---|---|
| Transcript setup | ~5% | Poseidon sponge initialization + public input absorption |
| Compute layers | ~60% | 88 GKR sumcheck verifications + oracle evaluations |
| Input layers | ~30% | Hyrax commitment verification (EC operations) |
| Proof decoding | ~5% | Binary proof parsing + circuit description decoding |

### On-Chain Gas (for reference)

| Verification Path | Gas | Target Chain |
|---|---|---|
| Direct GKR (Solidity) | ~254M | L2 (exceeds L1 limit) |
| Multi-tx batch | ~17-28M/tx | L1 (15 txs) |
| Stylus WASM | ~5-25M | Arbitrum |
| Stylus + Groth16 | ~3-6M | Arbitrum |

## Notes

- The E2E benchmark (`verify_dag_88layer`) includes proof decoding, transcript setup, compute layer verification, and input layer verification.
- EC operations (multi-scalar multiplication, point addition) dominate the compute layer phase.
- The native verifier uses pure Rust BN254 arithmetic. On Arbitrum Stylus, EC operations use EVM precompiles which are significantly faster.
- Sample size is set to 10 iterations due to the ~4s per-verification cost.
