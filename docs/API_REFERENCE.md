# ZKML Verifier API Reference

## CLI (`zkml-verifier`)

### Verify a proof bundle

```sh
zkml-verifier verify <proof_bundle.json> [--hybrid] [--json]
```

| Flag | Description |
|------|-------------|
| `--hybrid` | Transcript replay only (no EC ops, ~5x faster) |
| `--json` | Output result as JSON |

### Assemble a proof bundle

```sh
zkml-verifier bundle --proof <file> --gens <file> --desc <file> [-o output.json]
```

| Flag | Description |
|------|-------------|
| `--proof`, `-p` | Proof data file (binary or hex text) |
| `--gens`, `-g` | Pedersen generators file (binary or hex text) |
| `--desc`, `-d` | DAG circuit description JSON file |
| `--pub-inputs` | Public inputs file (optional) |
| `-o`, `--output` | Output path (default: stdout) |

Files can be raw binary (auto hex-encoded) or `0x`-prefixed hex text.

---

## REST API (`zkml-verifier-service`)

Default port: `3000` (configurable via `PORT` env var).

### Health

```
GET /health
```

Response:
```json
{ "status": "ok", "circuits_loaded": 0 }
```

### Verify (full)

```
POST /verify
Content-Type: application/json
```

Request body: `ProofBundle` JSON (see below).

Response:
```json
{ "verified": true, "circuit_hash": "0x8e7c..." }
```

### Verify (hybrid)

```
POST /verify/hybrid
Content-Type: application/json
```

Response:
```json
{
  "circuit_hash": "0x8e7c...",
  "transcript_digest": "0x1639...",
  "num_compute_layers": 88
}
```

### Batch verify

```
POST /verify/batch
Content-Type: application/json
```

Request:
```json
{ "bundles": [ <ProofBundle>, <ProofBundle>, ... ] }
```

Max 100 bundles per request.

Response:
```json
{
  "results": [
    { "index": 0, "verified": true, "circuit_hash": "0x...", "error": null },
    { "index": 1, "verified": false, "circuit_hash": "", "error": "..." }
  ],
  "total": 2,
  "valid": 1
}
```

### Register circuit (warm endpoint)

```
POST /circuits
Content-Type: application/json
```

Request:
```json
{
  "circuit_id": "xgboost-sample",
  "gens_hex": "0x...",
  "dag_circuit_description": { ... }
}
```

Response:
```json
{ "circuit_id": "xgboost-sample", "status": "registered" }
```

### List registered circuits

```
GET /circuits
```

Response:
```json
{ "circuits": ["xgboost-sample"] }
```

### Warm verify

```
POST /circuits/:circuit_id/verify
Content-Type: application/json
```

Request (proof only — gens and desc come from registration):
```json
{ "proof_hex": "0x...", "public_inputs_hex": "" }
```

### Warm hybrid verify

```
POST /circuits/:circuit_id/verify/hybrid
Content-Type: application/json
```

Same request format as warm verify.

---

## ProofBundle JSON format

```json
{
  "proof_hex": "0x52454d31...",
  "public_inputs_hex": "0x...",
  "gens_hex": "0x...",
  "dag_circuit_description": {
    "numComputeLayers": 88,
    "numInputLayers": 8,
    "layerTypes": [1, 0, ...],
    "numSumcheckRounds": [5, 3, ...],
    "atomOffsets": [0, 2, ...],
    "atomTargetLayers": [1, 2, ...],
    "atomCommitIdxs": [0, 0, ...],
    "ptOffsets": [0, 2, ...],
    "ptData": [0, 1, ...],
    "inputIsCommitted": [true, true, ...],
    "oracleProductOffsets": [0, 1, ...],
    "oracleResultIdxs": [0, 0, ...],
    "oracleExprCoeffs": ["0x01", "0x01", ...]
  },
  "model_hash": "0x...",
  "timestamp": 1711468800,
  "prover_version": "0.1.0",
  "circuit_hash": "0x..."
}
```

Required fields: `proof_hex`, `gens_hex`, `dag_circuit_description`.
Optional fields: `public_inputs_hex`, `model_hash`, `timestamp`, `prover_version`, `circuit_hash`.

---

## Rust library API

```rust
use zkml_verifier::{ProofBundle, verify, verify_hybrid, verify_raw};

// From JSON file
let bundle = ProofBundle::from_file("proof.json")?;
let result = verify(&bundle)?;
assert!(result.verified);

// Hybrid (no EC ops)
let hybrid = verify_hybrid(&bundle)?;
println!("{}", hex::encode(hybrid.transcript_digest));

// Raw bytes
let result = verify_raw(proof_bytes, gens_bytes, circuit_desc_bytes)?;
```

## C FFI

```c
// Returns 1 (verified), 0 (not verified), -1 (error)
int zkml_verify_json(const char* json, char** error_out);
int zkml_verify_file(const char* path, char** error_out);
int zkml_verify_raw(const uint8_t* proof, size_t proof_len,
                    const uint8_t* pub_inputs, size_t pub_inputs_len,
                    const uint8_t* gens, size_t gens_len,
                    const uint8_t* circuit_desc, size_t circuit_desc_len,
                    char** error_out);
void zkml_free_string(char* ptr);
const char* zkml_version();
```

## Python

```python
from zkml_verifier import verify_json, verify_bundle

result = verify_json('{"proof_hex": "0x...", ...}')
print(result["verified"])

result = verify_bundle({"proof_hex": "0x...", "gens_hex": "0x...", ...})
```

Requires: `cargo build --release -p zkml-verifier` (builds `libzkml_verifier.dylib`/`.so`).
