# World ZK Compute -- REST API Reference

This document covers the HTTP APIs exposed by each service in the World ZK Compute stack.

---

## Operator Service (default port 9090)

The operator service runs as `tee-operator watch --metrics-port 9090` and exposes
health/metrics endpoints. All routes are also available under `/api/v1/` with an
`X-API-Version: v1` response header.

### GET /health

Basic liveness check.

**Response** `200 OK`

```json
{
  "status": "ok",
  "uptime_secs": 3600,
  "last_block_polled": 12345678
}
```

**Example**

```bash
curl http://localhost:9090/health
```

---

### GET /metrics

Prometheus text exposition format. Suitable for scraping by Prometheus.

**Response** `200 OK` (`Content-Type: text/plain; version=0.0.4; charset=utf-8`)

```
# HELP operator_challenges_detected Total challenges detected
# TYPE operator_challenges_detected counter
operator_challenges_detected 5
# HELP operator_proofs_submitted Total proofs submitted to resolve disputes
# TYPE operator_proofs_submitted counter
operator_proofs_submitted 10
# HELP operator_disputes_resolved Total disputes resolved successfully
# TYPE operator_disputes_resolved counter
operator_disputes_resolved 3
# HELP operator_disputes_failed Total disputes that failed to resolve
# TYPE operator_disputes_failed counter
operator_disputes_failed 1
# HELP operator_errors_total Total errors encountered
# TYPE operator_errors_total counter
operator_errors_total 2
# HELP operator_finalizations_total Total result finalizations
# TYPE operator_finalizations_total counter
operator_finalizations_total 8
# HELP operator_active_disputes Current number of active disputes
# TYPE operator_active_disputes gauge
operator_active_disputes 0
# HELP operator_uptime_seconds Operator uptime in seconds
# TYPE operator_uptime_seconds gauge
operator_uptime_seconds 3600
# HELP operator_last_block_polled Last block number polled
# TYPE operator_last_block_polled gauge
operator_last_block_polled 12345678
```

**Example**

```bash
curl http://localhost:9090/metrics
```

---

### GET /metrics/json

JSON format of the same metrics (backward compatibility).

**Response** `200 OK`

```json
{
  "uptime_secs": 3600,
  "last_block_polled": 12345678,
  "total_submissions": 10,
  "total_challenges": 5,
  "total_disputes_resolved": 3,
  "total_disputes_failed": 1,
  "total_finalizations": 8,
  "total_errors": 2,
  "active_disputes": 0
}
```

**Example**

```bash
curl http://localhost:9090/metrics/json | jq .
```

---

## TEE Enclave Service (default port 8080)

The enclave runs ML inference inside a Trusted Execution Environment (AWS Nitro
or Intel TDX). Configured via environment variables; see `tee/enclave/src/config.rs`.

### GET /health

Basic health check. Returns model status and SHA-256 hash of loaded model bytes.

**Response** `200 OK`

```json
{
  "status": "ok",
  "model_loaded": true,
  "model_hash": "a1b2c3d4e5f6..."
}
```

**Example**

```bash
curl http://localhost:8080/health
```

---

### GET /health/detailed

Detailed health with model status, watchdog state, and degradation flags. If the
watchdog is disabled, computes a one-shot health evaluation.

**Response** `200 OK`

```json
{
  "status": "healthy",
  "uptime_secs": 7200,
  "total_inferences": 150,
  "total_errors": 2,
  "error_rate_pct": 1.3,
  "avg_latency_ms": 12.5,
  "degraded": false,
  "degraded_reason": null
}
```

**Example**

```bash
curl http://localhost:8080/health/detailed | jq .
```

---

### GET /info

Returns the enclave's Ethereum signer address and the keccak256 hash of the
loaded model.

**Response** `200 OK`

```json
{
  "enclave_address": "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65",
  "model_hash": "0xaabbccddee..."
}
```

**Example**

```bash
curl http://localhost:8080/info
```

---

### GET /attestation

Retrieve the TEE attestation document. Optionally pass a hex-encoded nonce for
freshness binding. When running on AWS Nitro, this returns a real CBOR-encoded
attestation document. In dev mode, returns a mock attestation.

**Query parameters**

| Name  | Type   | Required | Description                                |
|-------|--------|----------|--------------------------------------------|
| nonce | string | No       | Hex-encoded nonce (with or without `0x`)   |

**Response** `200 OK`

```json
{
  "document": "...",
  "pcrs": { ... },
  "timestamp": 1710000000
}
```

**Example**

```bash
curl "http://localhost:8080/attestation?nonce=deadbeef"
```

---

### POST /infer

Run ML inference on the provided feature vector. Requires an `Authorization`
header when `API_KEY` is configured on the enclave. The enclave signs the result
with its ECDSA key, producing an attestation compatible with `TEEMLVerifier.sol`.

**Headers**

| Name          | Value            | Required                   |
|---------------|------------------|----------------------------|
| Authorization | Bearer {api_key} | Yes (when API_KEY is set)  |
| Content-Type  | application/json | Yes                        |

**Request body**

```json
{
  "features": [5.1, 3.5, 1.4, 0.2],
  "chain_id": 11155111,
  "nonce": "unique-nonce-string",
  "timestamp": 1710000000
}
```

| Field     | Type     | Required | Description                                      |
|-----------|----------|----------|--------------------------------------------------|
| features  | float[]  | Yes      | Input feature vector                             |
| chain_id  | integer  | No       | Override enclave-wide CHAIN_ID (default: 1)      |
| nonce     | string   | No       | Client nonce for replay protection               |
| timestamp | integer  | No       | Unix timestamp (seconds) for replay protection   |

**Response** `200 OK`

```json
{
  "result": "0x...",
  "model_hash": "0x...",
  "input_hash": "0x...",
  "result_hash": "0x...",
  "attestation": "0x...",
  "enclave_address": "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65",
  "chain_id": 11155111,
  "nonce": 42,
  "timestamp": 1710000000
}
```

**Error responses**

| Status | Condition                    |
|--------|------------------------------|
| 401    | Missing or invalid API key   |
| 429    | Rate limited (see `Retry-After` header) |
| 500    | Model inference error        |

**Example**

```bash
curl -X POST http://localhost:8080/infer \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer my-api-key" \
  -d '{"features": [5.1, 3.5, 1.4, 0.2]}'
```

---

### GET /metrics

Returns enclave metrics as a JSON snapshot including inference counts, latency,
model info, and rate limiting stats.

**Response** `200 OK`

```json
{
  "total_inferences": 150,
  "total_errors": 2,
  "avg_inference_ms": 12.5,
  "uptime_secs": 7200,
  "model_name": "iris-xgboost",
  "model_hash": "0xabc123...",
  "attestation_refreshes": 10,
  "total_rate_limited": 3,
  "num_trees": 100,
  "num_features": 4,
  "num_classes": 3
}
```

**Example**

```bash
curl http://localhost:8080/metrics | jq .
```

---

### POST /admin/reload-model

Hot-reload the primary model from a new file path. Requires the admin API key
(`ADMIN_API_KEY` env var). The old model continues serving until the new one is
fully loaded and validated.

**Request body**

```json
{
  "model_path": "/models/xgboost-v2.json",
  "model_format": "auto"
}
```

| Field        | Type   | Required | Description                                |
|--------------|--------|----------|--------------------------------------------|
| model_path   | string | Yes      | Filesystem path to the new model file      |
| model_format | string | No       | Format hint: `"auto"`, `"xgboost"`, `"lightgbm"` (default: `"auto"`) |

**Response** `200 OK`

```json
{
  "success": true,
  "num_trees": 120,
  "num_features": 4,
  "model_hash": "0x...",
  "model_sha256": "abc123..."
}
```

**Example**

```bash
curl -X POST http://localhost:8080/admin/reload-model \
  -H "Content-Type: application/json" \
  -d '{"model_path": "/models/new-model.json"}'
```

---

### GET /models

List all models loaded in the multi-model registry.

**Response** `200 OK`

```json
{
  "models": [
    {
      "model_id": "iris-v1",
      "model_hash": "0x...",
      "size_bytes": 24576
    }
  ],
  "count": 1,
  "max_models": 10
}
```

**Example**

```bash
curl http://localhost:8080/models | jq .
```

---

### POST /models/load

Load a new model into the multi-model registry.

**Request body**

```json
{
  "model_id": "fraud-detector-v2",
  "model_path": "/models/fraud-v2.json",
  "expected_hash": "abc123def456..."
}
```

| Field         | Type   | Required | Description                               |
|---------------|--------|----------|-------------------------------------------|
| model_id      | string | Yes      | Unique identifier for the model           |
| model_path    | string | Yes      | Filesystem path to the model file         |
| expected_hash | string | No       | Expected SHA-256 hash for integrity check |

**Response** `200 OK`

```json
{
  "success": true,
  "model_id": "fraud-detector-v2",
  "model_hash": "0x...",
  "size_bytes": 51200
}
```

**Example**

```bash
curl -X POST http://localhost:8080/models/load \
  -H "Content-Type: application/json" \
  -d '{"model_id": "my-model", "model_path": "/models/model.json"}'
```

---

### DELETE /models/{model_id}

Unload a model from the multi-model registry.

**Response** `200 OK`

```json
{
  "success": true,
  "model_id": "fraud-detector-v2"
}
```

**Example**

```bash
curl -X DELETE http://localhost:8080/models/fraud-detector-v2
```

---

## Private Input Server (default port 3001)

A minimal Python HTTP server for storing and retrieving private inputs consumed
by the TEE enclave. Uses only the Python standard library.

### GET /health

Health check. Returns the number of inputs currently stored.

**Response** `200 OK`

```json
{
  "status": "healthy",
  "inputs_stored": 5
}
```

**Example**

```bash
curl http://localhost:3001/health
```

---

### POST /inputs

Store a JSON input. The server computes the SHA-256 hash and returns it as the
retrieval key.

**Request body** -- arbitrary JSON object.

```json
{
  "features": [1.0, 2.0, 3.0],
  "label": "test"
}
```

**Response** `200 OK`

```json
{
  "hash": "e3b0c44298fc1c149afbf4c8996fb924..."
}
```

**Example**

```bash
curl -X POST http://localhost:3001/inputs \
  -H "Content-Type: application/json" \
  -d '{"features": [1.0, 2.0, 3.0]}'
```

---

### GET /inputs/{hash}

Retrieve a previously stored input by its SHA-256 hash.

**Response** `200 OK` -- the original JSON body.

**Error** `404 Not Found` if the hash is unknown.

**Example**

```bash
curl http://localhost:3001/inputs/e3b0c44298fc1c149afbf4c8996fb924...
```

---

## Warm Prover (default port 3000)

The warm prover keeps a pre-warmed risc0 prover instance for faster proving.
It exposes a health/metrics/status HTTP interface on a configurable port
(default 3000).

### GET /health

Basic liveness check for the prover.

**Response** `200 OK`

```json
{
  "status": "ok",
  "name": "World ZK Prover"
}
```

**Example**

```bash
curl http://localhost:3000/health
```

---

### GET /metrics

Prometheus-style metrics for the prover (proofs completed, latency, etc.).

**Response** `200 OK` (`Content-Type: text/plain`)

**Example**

```bash
curl http://localhost:3000/metrics
```

---

### GET /status

Detailed prover status including active proofs, concurrency, and GPU state.

**Response** `200 OK`

```json
{
  "status": "idle",
  "active_proofs": 0,
  "max_concurrency": 4,
  "gpu_available": false
}
```

**Example**

```bash
curl http://localhost:3000/status | jq .
```

---

## Common Patterns

### Authentication

The TEE enclave supports API key authentication via the `Authorization: Bearer <key>`
header. Set the `API_KEY` environment variable on the enclave to enable it.

Admin endpoints (e.g., `/admin/reload-model`) require the `ADMIN_API_KEY` env var.

### Rate Limiting

The enclave applies a sliding-window rate limiter to `POST /infer`. When rate
limited, the server returns `429 Too Many Requests` with a `Retry-After` header
indicating seconds to wait.

### Versioned API

The operator service supports versioned routes under `/api/v1/`. All versioned
responses include an `X-API-Version: v1` header. Unversioned routes remain
available for backward compatibility.

### Error Format

Most error responses use a simple JSON format:

```json
{
  "error": "description of what went wrong"
}
```

HTTP status codes follow standard conventions: 400 for bad requests, 401 for
unauthorized, 404 for not found, 429 for rate limited, 500 for internal errors.
