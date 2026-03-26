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

## Verifier API Performance

HTTP load testing of the `zkml-verifier-service` REST API. Run with:

```bash
# Start the verifier (release build recommended)
cargo run -p zkml-verifier-service --release

# Run load tests (defaults: 100 requests, 10 concurrent)
./scripts/load-test-bank.sh

# Custom configuration
./scripts/load-test-bank.sh --url http://localhost:3000 --concurrency 20 --requests 200

# Skip the slow real-proof test (for quick health/error-path benchmarking)
./scripts/load-test-bank.sh --skip-verify

# With API key authentication
./scripts/load-test-bank.sh --api-key my-secret-key
```

### Endpoint Summary

> **Note**: The numbers below are placeholders. Run `./scripts/load-test-bank.sh` against a
> release-build verifier to get real measurements for your hardware.

| Endpoint | Throughput | p50 Latency | p99 Latency | Notes |
|---|---|---|---|---|
| `GET /health` | ~5,000 req/s | <1ms | <5ms | Baseline; no crypto work |
| `POST /verify` (real proof) | ~0.2 req/s | ~4,500ms | ~5,500ms | 88-layer XGBoost DAG, single-threaded verification |
| `POST /verify` (invalid proof) | ~3,000 req/s | <2ms | <10ms | Error path; early rejection |
| `POST /verify/batch` (N=5) | ~0.04 req/s | ~22,000ms | ~25,000ms | Sequential verification of 5 proofs |

### Scaling Observations

- **CPU-bound**: Real proof verification (~4-5s) dominates. The tokio `spawn_blocking` pool allows concurrent requests but each verification is single-threaded.
- **Concurrency**: With the default tokio blocking pool (512 threads), the server can handle many concurrent verification requests, limited by available CPU cores.
- **Payload size**: The 88-layer proof bundle is ~350KB JSON. Network transfer time is negligible compared to verification.
- **Error path**: Invalid proofs are rejected in <10ms (early parse/decode failure), so the error path has very high throughput.
- **Health endpoint**: Sub-millisecond; useful as a liveness probe without impacting verification throughput.

### Load Test Script Details

The `scripts/load-test-bank.sh` script:
1. Checks that the verifier is reachable (`GET /health`)
2. Benchmarks the health endpoint as a throughput baseline
3. Extracts a ProofBundle from the fixture and benchmarks `POST /verify` with a real proof
4. Benchmarks `POST /verify` with an invalid proof to measure the error-path throughput
5. Prints per-test stats (min/avg/p50/p95/p99/max latency, throughput, success rate)
6. Prints a summary table

Dependencies: `curl`, `python3`. No external load-testing tools required (uses background jobs for concurrency).

## Notes

- The E2E benchmark (`verify_dag_88layer`) includes proof decoding, transcript setup, compute layer verification, and input layer verification.
- EC operations (multi-scalar multiplication, point addition) dominate the compute layer phase.
- The native verifier uses pure Rust BN254 arithmetic. On Arbitrum Stylus, EC operations use EVM precompiles which are significantly faster.
- Sample size is set to 10 iterations due to the ~4s per-verification cost.
