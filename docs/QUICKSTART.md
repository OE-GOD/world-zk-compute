# Verify Your First Proof in 5 Minutes

This tutorial walks you through verifying a ZKML inference proof using the
`zkml-verifier` library. Pick your language -- Python, Rust, JavaScript, or
plain cURL -- and follow along.

## What You Need

1. A **proof bundle** JSON file. The repository ships a test fixture at
   `contracts/test/fixtures/phase1a_dag_fixture.json`, or you can use the
   minimal sample at `examples/quickstart/sample_proof.json`.
2. One of: Python 3.8+, Rust 1.75+, Node.js 20+, or cURL.

A proof bundle is a self-contained JSON object with these keys:

| Key | Description |
|-----|-------------|
| `proof_hex` | Hex-encoded proof data (starts with the `REM1` selector) |
| `gens_hex` | Hex-encoded Pedersen generators |
| `dag_circuit_description` | Circuit topology as a JSON object |
| `public_inputs_hex` | (optional) Hex-encoded public inputs |
| `model_hash` | (optional) Keccak-256 hash of the ML model |
| `timestamp` | (optional) Unix timestamp of proof generation |
| `prover_version` | (optional) Prover version string |

---

## Python

### Install

```bash
pip install zkml-verifier
```

> **Note:** The Python package wraps a native Rust shared library via ctypes.
> If you're building from source instead of pip, first run
> `cargo build --release -p zkml-verifier` and set
> `ZKML_LIB_DIR=target/release` so the library can be found.

### Verify from a file

```python
from zkml_verifier import verify_file

result = verify_file("proof_bundle.json")
print(f"Verified: {result['verified']}")
```

### Verify from a JSON string

```python
import json
from zkml_verifier import verify_json

with open("proof_bundle.json") as f:
    json_str = f.read()

result = verify_json(json_str)
print(f"Verified: {result['verified']}")
if result.get("error"):
    print(f"Error: {result['error']}")
```

### Verify from a Python dict

```python
from zkml_verifier import verify_bundle

bundle = {
    "proof_hex": "0x52454d31...",
    "gens_hex": "0x...",
    "dag_circuit_description": { ... }
}

result = verify_bundle(bundle)
print(f"Verified: {result['verified']}")
```

### Error handling

```python
from zkml_verifier import verify_file, VerifyError

try:
    result = verify_file("proof_bundle.json")
    print(f"Verified: {result['verified']}")
except FileNotFoundError:
    print("File not found")
except VerifyError as e:
    print(f"Verification error: {e}")
```

### Check library version

```python
from zkml_verifier import version
print(version())  # e.g. "0.1.0"
```

See [`examples/quickstart/verify.py`](../examples/quickstart/verify.py) for a
complete runnable script.

---

## Rust

### Add to Cargo.toml

```toml
[dependencies]
zkml-verifier = "0.1"
```

Or, if using a local checkout:

```toml
[dependencies]
zkml-verifier = { path = "crates/zkml-verifier" }
```

### Verify from a file

```rust
use zkml_verifier::{verify, ProofBundle};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bundle = ProofBundle::from_file("proof_bundle.json")?;
    let result = verify(&bundle)?;

    println!("Verified: {}", result.verified);
    println!("Circuit hash: 0x{}", hex::encode(result.circuit_hash));
    Ok(())
}
```

### Verify from a JSON string

```rust
use zkml_verifier::{verify, ProofBundle};

let json = std::fs::read_to_string("proof_bundle.json")?;
let bundle = ProofBundle::from_json(&json)?;
let result = verify(&bundle)?;
assert!(result.verified);
```

### Low-level API (raw byte slices)

If you already have raw byte slices for the proof, generators, and circuit
description, skip the JSON layer:

```rust
use zkml_verifier::verify_raw;

let result = verify_raw(&proof_bytes, &gens_bytes, &circuit_desc_bytes)?;
println!("Verified: {}", result.verified);
```

### CLI

The crate includes a binary that can verify proof bundles from the command line:

```bash
# Install
cargo install zkml-verifier

# Verify (human-readable output)
zkml-verifier verify proof_bundle.json

# Verify (JSON output)
zkml-verifier verify proof_bundle.json --json

# Hybrid mode (transcript replay without EC checks)
zkml-verifier verify proof_bundle.json --hybrid --json
```

See [`examples/quickstart/verify.rs`](../examples/quickstart/verify.rs) for
example code.

---

## JavaScript (Browser / WASM)

The JavaScript package uses WebAssembly compiled from the same Rust verifier.

### Install

```bash
npm install zkml-verifier
```

### Verify in the browser

```javascript
import init, { verify_proof_json, version } from 'zkml-verifier';

// Initialize WASM (required once before any verification)
await init();

console.log('Library version:', version());

// Load your proof bundle as a JSON string
const bundleJson = JSON.stringify({
  proof_hex: '0x52454d31...',
  gens_hex: '0x...',
  dag_circuit_description: { /* ... */ }
});

const result = verify_proof_json(bundleJson);
console.log('Verified:', result.verified);
console.log('Circuit hash:', result.circuit_hash);

if (result.error) {
  console.error('Error:', result.error);
}

// Free WASM memory when done
result.free();
```

### Verify in Node.js

```javascript
const fs = require('fs');

async function main() {
  const { default: init, verify_proof_json } = await import('zkml-verifier');
  await init();

  const bundleJson = fs.readFileSync('proof_bundle.json', 'utf8');
  const result = verify_proof_json(bundleJson);

  console.log('Verified:', result.verified);
  if (result.circuit_hash) {
    console.log('Circuit hash:', result.circuit_hash);
  }

  result.free();
}

main();
```

### Build WASM from source

```bash
# Install wasm-pack
cargo install wasm-pack

# Build for browser
wasm-pack build --target web crates/zkml-verifier -- --features wasm

# The output is in crates/zkml-verifier/pkg/
```

See [`examples/quickstart/verify.js`](../examples/quickstart/verify.js) for a
complete Node.js example using the REST API fallback.

---

## REST API

The verifier ships as a Docker service. No SDK installation required -- just
send HTTP requests.

### Start the service

```bash
# Option 1: Standalone verifier
docker compose -f docker-compose.verifier.yml up -d

# Option 2: Full bank demo stack (verifier + optional Anvil)
docker compose -f docker-compose.bank-demo.yml up -d
```

The service listens on port 3000 by default.

### Health check

```bash
curl http://localhost:3000/health
```

```json
{ "status": "ok", "circuits_loaded": 0 }
```

### Verify a proof

```bash
curl -s -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @proof_bundle.json
```

Response:

```json
{
  "verified": true,
  "circuit_hash": "0xabcdef1234..."
}
```

### Hybrid verification (transcript replay only)

```bash
curl -s -X POST http://localhost:3000/verify/hybrid \
  -H "Content-Type: application/json" \
  -d @proof_bundle.json
```

Response:

```json
{
  "circuit_hash": "0xabcdef1234...",
  "transcript_digest": "0x5678...",
  "num_compute_layers": 88
}
```

### Warm circuit verification

For repeated verifications of the same circuit, pre-register the circuit
configuration to avoid re-parsing generators and topology each time:

```bash
# 1. Register
curl -s -X POST http://localhost:3000/circuits \
  -H "Content-Type: application/json" \
  -d '{
    "circuit_id": "xgboost-v1",
    "gens_hex": "0x...",
    "dag_circuit_description": { ... }
  }'

# 2. Verify (only needs proof_hex)
curl -s -X POST http://localhost:3000/circuits/xgboost-v1/verify \
  -H "Content-Type: application/json" \
  -d '{ "proof_hex": "0x52454d31..." }'
```

### API endpoints summary

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check + circuit count |
| `POST` | `/verify` | Full verification (send complete ProofBundle) |
| `POST` | `/verify/hybrid` | Hybrid verification (transcript only) |
| `POST` | `/verify/batch` | Batch verification (array of bundles) |
| `POST` | `/circuits` | Register a circuit config |
| `GET` | `/circuits` | List registered circuits |
| `POST` | `/circuits/:id/verify` | Verify against pre-loaded circuit |
| `POST` | `/circuits/:id/verify/hybrid` | Hybrid verify against pre-loaded circuit |

---

## Using the Test Fixture

The repository includes a test fixture you can use to verify your setup works:

```bash
# Path to the test fixture
FIXTURE=contracts/test/fixtures/phase1a_dag_fixture.json

# Rust CLI
cargo run -p zkml-verifier -- verify $FIXTURE --json

# Python (with native library)
python examples/quickstart/verify.py $FIXTURE

# Node.js (with REST API)
node examples/quickstart/verify.js $FIXTURE

# cURL (with Docker service running)
curl -s -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @$FIXTURE | python3 -m json.tool
```

---

## Troubleshooting

### Python: "Could not find libzkml_verifier.dylib"

The native library was not found. Either:
- Install via pip: `pip install zkml-verifier`
- Or build from source: `cargo build --release -p zkml-verifier`
  and set `ZKML_LIB_DIR` to the directory containing the `.so`/`.dylib`

### JavaScript: "WASM module not initialized"

Call `await init()` before any verification function. This loads and
compiles the WebAssembly module.

### REST API: "Connection refused"

The verifier service is not running. Start it with:
```bash
docker compose -f docker-compose.verifier.yml up -d
```

### Verification returns an error

Common causes:
- **"invalid selector (expected REM1)"** -- The `proof_hex` does not start
  with the `REM1` magic bytes (`0x52454d31`). Ensure you are using a
  Remainder GKR proof, not a risc0 or Groth16 proof.
- **"proof data too short"** -- The proof blob is truncated or empty.
- **"bundle parse error"** -- The JSON is malformed or missing required
  fields (`proof_hex`, `gens_hex`, `dag_circuit_description`).

---

## Next Steps

- [API Reference](API_REFERENCE.md) -- Full contract and service API documentation
- [Proof Format](PROOF_FORMAT.md) -- Wire format for all proof types
- [Integration Guide](INTEGRATION_GUIDE.md) -- End-to-end integration patterns
- [Publishing Guide](PUBLISHING.md) -- How to build and publish packages
- [Security Model](SECURITY_MODEL.md) -- Trust assumptions and threat model
