# Performance SLOs (Service Level Objectives)

This document defines the performance targets for World ZK Compute. Each SLO
specifies a metric, its target, how it is measured, and which component owns it.

---

## 1. TEE Inference Latency

The TEE path is the happy-path for ML inference. It must be fast enough that
end users perceive near-instant results.

| Percentile | Target    | Measurement Point                       |
|------------|-----------|------------------------------------------|
| P50        | < 100 ms  | Enclave `POST /inference` response time  |
| P99        | < 500 ms  | Enclave `POST /inference` response time  |
| P99.9      | < 2 s     | Enclave `POST /inference` response time  |

**Scope:** Time from the enclave receiving the HTTP request to returning the
signed attestation. Does **not** include network round-trip or on-chain
submission.

**How to measure:**
- Enclave exposes Prometheus histogram `inference_duration_seconds`.
- Load test with `scripts/load-test-prover.sh` against a running enclave.

**Owner:** `tee/enclave/`

---

## 2. ZK Proof Generation

ZK proofs are generated only during disputes (TEE challenge) or for
Remainder/GKR direct verification. The SLO ensures dispute resolution stays
within a practical window.

| Proof System         | Target       | Notes                                          |
|----------------------|--------------|-------------------------------------------------|
| Remainder GKR+Hyrax  | < 10 min     | XGBoost sample model (88 compute layers)        |
| Remainder + Groth16  | < 15 min     | GKR prove + gnark Groth16 wrapping              |
| risc0 Groth16        | < 10 min     | Rule-engine guest program (x86_64 with Docker)  |
| Warm prover (cached) | < 5 min      | CachedProver reuses circuit + Pedersen gens     |

**Scope:** Wall-clock time from `build_and_prove()` call to verified proof
output. Includes witness generation, circuit construction, and proving.

**How to measure:**
```bash
# XGBoost Remainder benchmarks
cargo bench --manifest-path examples/xgboost-remainder/Cargo.toml \
  --bench prover_bench -- --sample-size 10

# Prover micro-benchmarks
cargo bench --manifest-path prover/Cargo.toml --bench prover_benchmarks
```

**Owner:** `examples/xgboost-remainder/`, `prover/`

---

## 3. On-chain Verification Gas

All on-chain operations must fit within the Ethereum block gas limit (30M gas).
DAG batch verification splits the 88-layer XGBoost circuit across multiple
transactions.

| Operation                          | Target     | Measured            |
|------------------------------------|------------|---------------------|
| DAG batch start                    | < 30M gas  | ~17.5M gas          |
| DAG batch continue (per step)      | < 30M gas  | 13-28M gas          |
| DAG batch finalize (per step)      | < 30M gas  | 9-22M gas           |
| TEE submit + finalize (happy path) | < 500K gas | ~310K gas           |
| Single-tx GKR direct verify        | N/A        | ~213M gas (batched) |
| Groth16 (gnark) verification       | < 500K gas | ~250K gas           |

**Scope:** Gas consumed by a single transaction call. Each DAG batch step must
independently stay under 30M.

**How to measure:**
```bash
cd contracts
forge test --gas-report --match-contract DAGBatchVerifier
forge test --gas-report --match-contract GasProfile
forge snapshot
```

**Reference:** See `docs/GAS_OPTIMIZATION.md` for detailed per-operation gas
breakdowns and optimization opportunities.

**Owner:** `contracts/`

---

## 4. Event Polling Latency

Operator and indexer services must detect new on-chain events quickly to ensure
timely dispute responses and UI updates.

| Service      | Target         | Default Poll Interval | Config Variable          |
|--------------|----------------|-----------------------|--------------------------|
| Operator     | < 5 s          | 5 s                   | Hardcoded in `main.rs`   |
| Indexer      | < 15 s         | 12 s                  | `POLL_INTERVAL_SECS`     |
| Rust SDK     | Caller-defined | N/A (caller passes)   | `poll_interval` parameter|
| Python SDK   | < 5 s          | 2 s                   | `poll_interval` parameter|
| TypeScript SDK| < 5 s         | 2 s                   | `pollInterval` option    |

**Scope:** Time between a block being mined and the service detecting the event.
Equals `poll_interval + RPC latency`.

**How to measure:**
- Operator: `operator_last_block_polled` Prometheus metric vs chain head.
- Indexer: Compare `GET /status` block number against chain head.
- SDK: Instrument `poll_events()` / `watch()` call latency.

**Owner:** `services/operator/`, `services/indexer/`, `sdk/`

---

## 5. SDK Client Timeouts

Default timeout values for each SDK client. These apply to HTTP requests against
the prover API or RPC endpoint.

| SDK           | HTTP Timeout | Proof Wait Timeout | Poll Interval | Retry Config              |
|---------------|-------------|--------------------|--------------|-----------------------------|
| TypeScript    | 30 s        | 300 s (5 min)      | 2 s          | 3 retries, 1s base delay    |
| Python (sync) | 30 s        | 300 s (5 min)      | 2 s          | 3 retries, 1s base delay    |
| Python (async)| 30 s        | 300 s (5 min)      | 2 s          | 3 retries, 1s base delay    |
| Rust          | N/A (tokio) | N/A (caller)       | Caller       | 3 retries, 1s-30s exp backoff|

**Source files:**
- TypeScript: `sdk/typescript/src/client.ts` (`DEFAULT_TIMEOUT = 30000`)
- Python: `sdk/python/worldzk/client.py` (`DEFAULT_TIMEOUT = 30.0`)
- Rust: `sdk/src/retry.rs` (`base_delay: 1s, max_delay: 30s`)

**Owner:** `sdk/`

---

## 6. Benchmark Results

The table below holds measured values from CI benchmark runs. Rows marked with
`--` are placeholders to be filled by the benchmarks CI workflow.

### 6.1 XGBoost Remainder Prover (`examples/xgboost-remainder`)

| Benchmark                              | Measured   | CI Run   |
|----------------------------------------|------------|----------|
| `model_parse_json`                     | --         | --       |
| `witness_prepare_circuit_inputs`       | --         | --       |
| `witness_compute_path_bits`            | --         | --       |
| `witness_compute_leaf_sum`             | --         | --       |
| `witness_collect_all_leaf_values`      | --         | --       |
| `witness_compute_comparison_tables`    | --         | --       |
| `witness_compute_comparison_witness`   | --         | --       |
| `witness_predict`                      | --         | --       |
| `circuit_build_full_inference`         | --         | --       |
| `build_and_prove_sample_model`         | --         | --       |
| `cached_prover_prove`                  | --         | --       |

### 6.2 Prover Micro-benchmarks (`prover/`)

| Benchmark                              | Measured   | CI Run   |
|----------------------------------------|------------|----------|
| `input_hashing/1024`                   | --         | --       |
| `input_hashing/10240`                  | --         | --       |
| `input_hashing/102400`                 | --         | --       |
| `input_hashing/1024000`               | --         | --       |
| `hex_operations/encode_1000_bytes`     | --         | --       |
| `hex_operations/decode_1000_bytes`     | --         | --       |
| `json/serialize`                       | --         | --       |
| `json/deserialize`                     | --         | --       |
| `cache_key_generation`                 | --         | --       |
| `base64/encode_10kb`                   | --         | --       |
| `base64/decode_10kb`                   | --         | --       |

### 6.3 On-chain Gas (from `forge snapshot`)

| Test                                         | Gas         |
|----------------------------------------------|-------------|
| `DAGBatchVerifierTest:test_dag_batch_e2e`    | 281,553,679 |
| `DAGBatchVerifierTest:test_dag_batch_gas`    | 281,567,979 |
| `GasProfileTest:test_gas_requestExecution`   | 188,510     |
| `GasProfileTest:test_gas_claimExecution`     | 237,537     |
| `GasProfileTest:test_gas_submitProof`        | 361,409     |
| `GasProfileTest:test_gas_submitResult`       | 257,457     |
| `GasProfileTest:test_gas_finalize`           | 290,678     |

> Gas values sourced from `contracts/.gas-snapshot`. Updated by running
> `forge snapshot` in `contracts/`.

---

## Alerting Thresholds

When deploying to production, configure alerts at **2x the SLO target** to
catch regressions before they become user-facing.

| Metric                    | Warning (2x)  | Critical (5x) |
|---------------------------|---------------|----------------|
| TEE inference P99         | > 1 s         | > 2.5 s        |
| ZK proof generation       | > 20 min      | > 50 min       |
| Event polling lag (blocks) | > 10 blocks   | > 50 blocks    |
| SDK HTTP timeout rate      | > 1%          | > 5%           |

---

## References

- `docs/GAS_OPTIMIZATION.md` -- detailed gas cost analysis
- `contracts/.gas-snapshot` -- Foundry gas snapshot
- `examples/xgboost-remainder/benches/prover_bench.rs` -- XGBoost benchmarks
- `prover/benches/prover_benchmarks.rs` -- prover micro-benchmarks
- `services/operator/src/main.rs` -- operator poll loop (5s interval)
- `services/indexer/src/main.rs` -- indexer poll loop (`POLL_INTERVAL_SECS`)
- `sdk/typescript/src/client.ts` -- TypeScript SDK timeouts
- `sdk/python/worldzk/client.py` -- Python SDK timeouts
- `sdk/src/retry.rs` -- Rust SDK retry configuration
