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

Measured on Apple M-series (ARM64), single-threaded, `--release` profile.
Criterion sample size: 10 iterations.

### Native Verification (aarch64-apple-darwin)

| Benchmark | Mean | Range | Description |
|---|---|---|---|
| `verify_dag_88layer` | 6.29s | 4.75s - 8.65s | Full GKR DAG verification via `verify()` (JSON bundle input) |
| `verify_raw_dag_88layer` | 2.16s | 2.05s - 2.29s | Full GKR DAG verification via `verify_raw()` (pre-decoded binary input) |
| `verify_dag_88layer_hybrid` | 629ms | 621ms - 640ms | Hybrid verification (transcript replay + Fr arithmetic only, no EC checks) |
| `decode_proof_blob` | ~1ms | - | Parse binary proof blob (131 KB) |
| `parse_bundle_json` | ~5ms | - | Parse JSON fixture (433 KB) |
| `hex_decode_proof` | ~0.5ms | - | Decode hex proof string to bytes |
| `encode_circuit_desc` | ~3-4s | - | Convert circuit description from JSON to binary encoding |

The large gap between `verify()` (~6.3s) and `verify_raw()` (~2.2s) is dominated by
`encode_circuit_desc_from_json()`, which iterates over large JSON arrays (atom offsets,
point templates, oracle coefficients) element by element to produce the binary encoding.
For production use, pre-encode the circuit description once and use `verify_raw()`.

### Proof and Bundle Sizes (88-layer XGBoost)

| Component | Size | Notes |
|---|---|---|
| Proof blob (binary) | 130.7 KB | Includes REM1 selector + circuit hash + GKR proof + input proofs |
| Generators (binary) | 32.2 KB | 512 Pedersen generator points (EC coordinates) |
| Circuit description (JSON) | 15.9 KB | Topology, atom routing, oracle expressions |
| Total binary payload | 162.9 KB | Proof + generators (excludes circuit desc) |
| Full JSON bundle | 432.9 KB | All fields hex-encoded with circuit description |

### Verification Phase Breakdown (88-layer XGBoost)

Based on the hybrid vs. full verification times:

| Phase | Time | % of Full | Description |
|---|---|---|---|
| Transcript setup | ~30ms | ~1.4% | Poseidon sponge init + public input absorption |
| GKR compute layers (Fr only) | ~550ms | ~25.5% | 88 sumcheck verifications + oracle evals (field arithmetic) |
| Hyrax input layers (Fr only) | ~50ms | ~2.3% | Input layer claim verification (field arithmetic) |
| EC operations | ~1.5s | ~69.4% | Multi-scalar multiplication, Hyrax commitments, point additions |
| Proof decoding | ~1ms | ~0.1% | Binary proof blob parsing |
| **Total (verify_raw)** | **~2.16s** | **100%** | |

EC operations (BN254 scalar multiplication and point addition in pure Rust) dominate
verification time at ~70% of total. The hybrid path skips EC checks entirely, reducing
verification to ~630ms.

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

Expected latencies based on native verification times (~2.2s for `verify_raw`, ~6.3s for
`verify` with JSON bundle). The REST API uses `verify()` internally so includes JSON
circuit description encoding overhead.

| Endpoint | Throughput | p50 Latency | p99 Latency | Notes |
|---|---|---|---|---|
| `GET /health` | ~5,000 req/s | <1ms | <5ms | Baseline; no crypto work |
| `POST /verify` (real proof) | ~0.15 req/s | ~6,300ms | ~8,600ms | 88-layer XGBoost DAG, single-threaded verification |
| `POST /verify` (invalid proof) | ~3,000 req/s | <2ms | <10ms | Error path; early rejection |
| `POST /verify/hybrid` | ~1.5 req/s | ~630ms | ~700ms | Transcript replay only, no EC |

### Scaling Observations

- **CPU-bound**: Real proof verification (~2-6s depending on API) dominates. The tokio `spawn_blocking` pool allows concurrent requests but each verification is single-threaded.
- **Concurrency**: With the default tokio blocking pool (512 threads), the server can handle many concurrent verification requests, limited by available CPU cores.
- **Payload size**: The 88-layer proof bundle is ~433KB JSON. Network transfer time is negligible compared to verification.
- **Error path**: Invalid proofs are rejected in <10ms (early parse/decode failure), so the error path has very high throughput.
- **Health endpoint**: Sub-millisecond; useful as a liveness probe without impacting verification throughput.
- **Optimization**: Use `verify_raw()` with pre-encoded binary circuit descriptions to cut API latency from ~6.3s to ~2.2s.

### Load Test Script Details

The `scripts/load-test-bank.sh` script:
1. Checks that the verifier is reachable (`GET /health`)
2. Benchmarks the health endpoint as a throughput baseline
3. Extracts a ProofBundle from the fixture and benchmarks `POST /verify` with a real proof
4. Benchmarks `POST /verify` with an invalid proof to measure the error-path throughput
5. Prints per-test stats (min/avg/p50/p95/p99/max latency, throughput, success rate)
6. Prints a summary table

Dependencies: `curl`, `python3`. No external load-testing tools required (uses background jobs for concurrency).

## Warm Prover Cache (xgboost-remainder)

The warm prover HTTP server (`--serve` mode) caches all expensive one-time artifacts at startup
via `CachedProver`, so subsequent proof requests only perform witness-dependent work.

### What is cached at startup

| Artifact | Cost to build | Reused per request? |
|---|---|---|
| GKR circuit structure (`base_circuit`) | 100-500ms | Yes (cloned per request) |
| Pedersen generators (512 EC points) | 50-200ms | Yes (shared reference) |
| Circuit description (ABI-encoded topology) | ~10ms | Yes (returned in bundles) |
| Circuit hash (SHA-256 of description) | <1ms | Yes |
| Prover/verifier configs | <1ms | Yes |
| Feature index table (`fi_padded`) | ~10ms | Yes |
| Hex-encoded generators (`gens_hex`) | ~5ms | Yes (pre-encoded once in server) |

### Optimization: Reuse Pedersen generators for verification

Prior to the fix, `CachedProver::prove()` created a **new** `PedersenCommitter` (512 EC points)
for the verification step on every request, despite having identical generators already cached.
Since `PedersenCommitter::new(512, label, None)` is deterministic (generators derived from label),
the cached instance is safe to reuse for both proving and verification.

**Fix**: Replace `PedersenCommitter::new(...)` in the verify path with `&self.pedersen_committer`.

**Expected improvement**: Eliminates ~50-200ms of redundant EC point derivation per proof request,
depending on hardware. For an XGBoost model with a ~2-4s total prove time, this represents a
2-10% speedup on warm (second+) requests.

### How to measure

```bash
# Start warm prover
cargo run --release -p xgboost-remainder -- \
  --model models/sample_model.json --serve --port 8080

# Cold request (first proof -- includes any lazy init)
time curl -s -X POST localhost:8080/prove/bundle \
  -H 'Content-Type: application/json' \
  -d '{"features": [1.0, 2.0, 3.0, 4.0]}' | python3 -c \
  "import sys,json; d=json.load(sys.stdin); print(f'prove_time_ms={d[\"prove_time_ms\"]}')"

# Warm request (second proof -- reuses all cached state)
time curl -s -X POST localhost:8080/prove/bundle \
  -H 'Content-Type: application/json' \
  -d '{"features": [5.0, 6.0, 7.0, 8.0]}' | python3 -c \
  "import sys,json; d=json.load(sys.stdin); print(f'prove_time_ms={d[\"prove_time_ms\"]}')"
```

The `prove_time_ms` field in the response measures only the proving step (excluding HTTP overhead).
Compare warm vs cold to see the cache benefit.

## Notes

- The E2E benchmark (`verify_dag_88layer`) includes proof decoding, transcript setup, compute layer verification, and input layer verification.
- EC operations (multi-scalar multiplication, point addition) dominate native verification at ~70% of total time.
- The native verifier uses pure Rust BN254 arithmetic. On Arbitrum Stylus, EC operations use EVM precompiles which are significantly faster.
- Sample size is set to 10 iterations due to the ~2s per-verification cost.
- The `verify()` API is ~3x slower than `verify_raw()` due to JSON circuit description encoding. Pre-encode for production use.
- Hybrid verification (~630ms) is suitable for dispute protocols where EC checks are delegated to a Groth16 proof.
