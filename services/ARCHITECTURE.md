# Services Architecture

## Overview

World ZK Compute runs four services plus a standalone prover node.
Together they handle ML inference inside a TEE, on-chain result submission,
dispute resolution via ZK proofs, event indexing, and contract administration.

```
                          +-----------+
                          | Ethereum  |
                          |  (RPC)    |
                          +-----+-----+
                   RPC/WS |     | RPC
          +---------------+     +---------------+
          |                                     |
  +-------v--------+                   +--------v-------+
  |   Operator      |  HTTP /infer     |   Prover       |
  |  (tee-operator) +------+--------->+| (world-zk-     |
  |  :9090 metrics  |      |          || prover) :8081  |
  +-------+---------+      |          |+----------------+
          |                 |
          | HTTP            |
          v                 |
  +-------+---------+      |
  |   TEE Enclave   |      |
  |  :8080          +------+
  |  /infer,/health |
  |  /attestation   |
  +-----------------+
                            +------------------+
  +------------------+      |   Indexer        |
  |   Admin CLI      |      |  (tee-indexer)   |
  |  (no server)     |      |  :8081 REST + WS |
  +------------------+      +------------------+
          |                          |
          | RPC               polls chain
          v                          v
       Ethereum               SQLite/Postgres
```

## Services

### 1. TEE Enclave (`tee/enclave`) -- port 8080

Runs inside AWS Nitro / Intel TDX. Loads an XGBoost/LightGBM model,
serves inference via HTTP, returns ECDSA-signed attestations.

**Endpoints:** `GET /health`, `GET /health/detailed`, `GET /info`,
`GET /attestation`, `POST /infer`, `GET /metrics`,
`POST /admin/reload` (Bearer), `POST|DELETE|GET /admin/models` (Bearer)

**Key env vars:** `MODEL_PATH` (/app/model/model.json), `PORT` (8080),
`ENCLAVE_PRIVATE_KEY`, `NITRO_ENABLED` (false), `CHAIN_ID` (1),
`ADMIN_API_KEY`, `MAX_REQUESTS_PER_MINUTE` (120),
`WATCHDOG_ENABLED` (true), `EXPECTED_MODEL_HASH`

### 2. Operator (`services/operator`) -- metrics port 9090

Orchestrates inference-to-proof pipeline. Submits results on-chain,
watches for disputes, resolves them with ZK proofs. Supports webhooks
and Prometheus metrics.

**Subcommands:** `submit`, `watch`, `run` (submit+watch), `register`, `models`

**Metrics endpoints (port 9090):** `GET /health`, `GET /metrics` (Prometheus),
`GET /metrics/json`

**Key env vars:** `RPC_URL`, `PRIVATE_KEY` (required),
`TEE_VERIFIER_ADDRESS` (required), `ENCLAVE_URL` (http://localhost:8080),
`MODEL_PATH`, `PROOFS_DIR` (./proofs), `PROVER_STAKE`,
`PROVER_URL` (optional warm prover), `WEBHOOK_URL` (Slack-compatible),
`NITRO_VERIFICATION` (false), `ALERT_CONFIG_JSON`

Also supports TOML config via `--config <path>` with `[[models]]` multi-model registry.

### 3. Indexer (`services/indexer`) -- port 8081

Polls TEEMLVerifier contract events, stores in SQLite/Postgres,
serves REST API and WebSocket stream.

**REST endpoints:** `GET /health`, `GET /api/v1/results` (query: status,
submitter, model_hash, limit), `GET /api/v1/results/:id`,
`GET /api/v1/stats`. Backward-compat aliases at `/results`, `/stats`.

**WebSocket:** `GET /ws/events` -- subscribe with
`{"subscribe": ["ResultSubmitted", "ResultChallenged"]}` or receive all.
Ping/pong heartbeat every 30s.

**Key env vars:** `RPC_URL`, `CONTRACT_ADDRESS`, `DB_TYPE` (sqlite),
`DB_PATH` (./indexer.db), `DATABASE_URL` (Postgres), `PORT` (8081),
`POLL_INTERVAL_SECS` (12)

### 4. Admin CLI (`services/admin-cli`) -- no server

CLI for TEEMLVerifier contract admin. Runs one command and exits.

**Commands:** `status`, `pause`, `unpause`, `register-enclave`,
`revoke-enclave`, `set-stake`, `set-bond`, `set-verifier`,
`transfer-ownership`, `accept-ownership`

**Flags/env:** `--rpc-url` / `RPC_URL`, `--contract` / `CONTRACT_ADDRESS`,
`--private-key` / `PRIVATE_KEY` (write ops), `--dry-run`

### 5. Prover (`prover/`) -- health port 8081

Standalone prover node. Monitors ExecutionEngine for requests, claims
jobs, generates ZK proofs (risc0 zkVM), submits for rewards.

**Health endpoints:** `GET /health`, `GET /metrics` (Prometheus),
`GET /status` (detailed JSON)

**Key env vars:** `RPC_URL`, `WS_URL` (optional), `PRIVATE_KEY`,
`ENGINE_ADDRESS`, `PROVING_MODE` (gpu-fallback), `HEALTH_PORT` (8081),
`MAX_CONCURRENT` (4), `USE_SNARK` (false)

## Communication Patterns

| From | To | Protocol | Purpose |
|------|----|----------|---------|
| Operator | Enclave | HTTP REST | `/infer`, `/health`, `/attestation`, `/info` |
| Operator | Ethereum | JSON-RPC | Submit results, resolve disputes, finalize |
| Operator | Prover | HTTP POST | `POST /prove` (optional, via `PROVER_URL`) |
| Indexer | Ethereum | JSON-RPC | Poll `getLogs` for contract events |
| Admin CLI | Ethereum | JSON-RPC | Read/write contract state |
| Prover | Ethereum | JSON-RPC + WS | Poll/subscribe events, submit proofs |
| Clients | Indexer | WebSocket | Real-time event stream |

## Port Assignments

| Port | Service | Purpose |
|------|---------|---------|
| 8080 | TEE Enclave | Inference API |
| 8081 | Indexer | REST API + WebSocket |
| 8081 | Prover | Health/metrics (if indexer not co-located) |
| 9090 | Operator | Prometheus metrics + health |

## Dependency Graph

```
Operator ──> TEE Enclave     (HTTP: inference + attestation)
Operator ──> Ethereum RPC    (submit, watch, finalize, register)
Operator ──> Prover [opt]    (HTTP: proof generation via PROVER_URL)
Indexer  ──> Ethereum RPC    (poll events)
Indexer  ──> SQLite/Postgres (persist indexed events)
Admin CLI ──> Ethereum RPC   (read/write contract state)
Prover   ──> Ethereum RPC    (claim jobs, submit proofs)
Prover   ──> Ethereum WS     (event subscription, optional)
```

All services are independently deployable. The only hard runtime
dependency is Operator -> Enclave. Everything else communicates
through the chain or is optional.

## Health Checks

Every long-running service exposes `/health` returning JSON with
a `"status": "ok"` (or `"healthy"`) field:

- **Enclave** `/health` -- includes `model_loaded`, `model_hash`
- **Operator** `:9090/health` -- includes `uptime_secs`, `last_block_polled`
- **Indexer** `/health` -- includes `last_indexed_block`, `total_results`
- **Prover** `/health` -- returns 200 when running, 503 otherwise
