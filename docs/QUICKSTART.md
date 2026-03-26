# Quickstart: Verify Your First Proof in 5 Minutes

Verify a ZKML inference proof using the zkml-verifier in Python, JavaScript, or Rust.

## Prerequisites

- A proof bundle JSON file (see [PROOF_FORMAT.md](PROOF_FORMAT.md))
- Or use the included test fixture

## Option A: Python

```bash
pip install zkml-verifier
```

```python
from zkml_verifier import verify_bundle

bundle = {
    "proof_hex": "0x52454d31...",  # your proof data
    "gens_hex": "0x...",
    "dag_circuit_description": { ... }
}

result = verify_bundle(bundle)
print(f"Verified: {result['verified']}")
```

Or from a file:

```python
import json
from zkml_verifier import verify_json

with open("proof_bundle.json") as f:
    result = verify_json(f.read())

print(f"Verified: {result['verified']}")
```

See `examples/quickstart/verify.py` for a complete example.

## Option B: JavaScript (Node.js)

```bash
npm install @worldzk/verifier
```

```javascript
import { verify } from '@worldzk/verifier';

const bundle = JSON.parse(fs.readFileSync('proof_bundle.json', 'utf8'));
const result = await verify(bundle);
console.log(`Verified: ${result.verified}`);
```

See `examples/quickstart/verify.js` for a complete example.

## Option C: Rust

```toml
[dependencies]
zkml-verifier = { path = "crates/zkml-verifier" }
```

```rust
use zkml_verifier::{verify, ProofBundle};

let bundle = ProofBundle::from_file("proof_bundle.json").unwrap();
let result = verify(&bundle).unwrap();
println!("Verified: {}", result.verified);
```

## Option D: REST API

Start the verifier service:

```bash
docker compose -f docker-compose.bank-demo.yml up -d
```

Then POST a proof bundle:

```bash
curl -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @proof_bundle.json
```

Response:
```json
{
  "verified": true,
  "circuit_hash": "0xabcd..."
}
```

## Using the Test Fixture

The repository includes a test fixture you can use to verify everything works:

```bash
# Rust CLI
cargo run -p zkml-verifier -- verify contracts/test/fixtures/phase1a_dag_fixture.json

# Python
python examples/quickstart/verify.py contracts/test/fixtures/phase1a_dag_fixture.json

# REST API
curl -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @contracts/test/fixtures/phase1a_dag_fixture.json
```

## Next Steps

- [API Reference](API_REFERENCE.md) — Full contract and service API documentation
- [Proof Format](PROOF_FORMAT.md) — Wire format for all proof types
- [Integration Guide](INTEGRATION_GUIDE.md) — End-to-end integration patterns
- [Benchmarks](BENCHMARKS.md) — Performance measurements
