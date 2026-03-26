# World ZK Compute

Cryptographic proof that AI models produce correct outputs. Verify ML inference without re-executing the model.

## What It Does

A bank runs an XGBoost credit scoring model. World ZK Compute generates a cryptographic proof that the model ran correctly on the given inputs and produced the declared output. Anyone can verify that proof -- locally, on-chain, or via API -- without access to the model weights or input data.

## Quick Start

### Verify a proof (Python)

```bash
pip install zkml-verifier
```

```python
from zkml_verifier import verify_file

result = verify_file("proof_bundle.json")
print(f"Verified: {result['verified']}")
```

### Verify a proof (Rust)

```bash
cargo install zkml-verifier
zkml-verifier verify proof_bundle.json
```

### Verify a proof (REST API)

```bash
curl -s -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @proof_bundle.json
```

```json
{ "verified": true, "circuit_hash": "0xabcdef..." }
```

### Verify a proof (Browser)

A live WASM demo is available at [`web/index.html`](web/index.html). The browser verifier runs the same GKR+Hyrax cryptographic checks as the on-chain Solidity contracts. No backend required.

### Run the full stack with Docker

```bash
docker compose -f docker-compose.bank-demo.yml up -d
```

See the [Quick Start tutorial](docs/QUICKSTART.md) for detailed setup instructions.

## Supported Models

| Model Type | Status |
|------------|--------|
| XGBoost | Supported |
| LightGBM | Supported |
| Random Forest | Supported |
| Logistic Regression | Supported |

## How It Works

```
                    +-----------+
   ML Model -----→ |  Prover   | -----→ Proof (JSON bundle)
   (XGBoost,       |  (TEE or  |             |
    LightGBM,      |   ZK)     |             |
    etc.)          +-----------+             |
                                             ▼
                                      +-----------+
                                      | Verifier  | -----→ Valid / Invalid
                                      | (API, SDK,|
                                      |  on-chain)|
                                      +-----------+
```

**TEE happy path:** The model runs inside an AWS Nitro enclave. The enclave signs the result. Cost: ~$0.0001 per inference.

**ZK dispute path:** If anyone challenges a result, a GKR+Hyrax zero-knowledge proof settles the dispute on-chain. The proof is purely mathematical -- no hardware trust required.

## Verification Paths

| Path | Gas Cost | Transactions | Use Case |
|------|----------|-------------|----------|
| REST API | Free | N/A | Off-chain audit, compliance |
| Stylus + Groth16 Hybrid | ~3-6M gas | 1 | On-chain dispute (Arbitrum) |
| Multi-tx Batch | <30M gas/tx | 15 | On-chain dispute (Ethereum L1) |
| Direct GKR+Hyrax | ~254M gas | 1 | High-gas-limit chains |

## SDKs

| Language | Install | Docs |
|----------|---------|------|
| Python | `pip install zkml-verifier` | [Quick Start](docs/QUICKSTART.md#python) |
| Rust | `cargo add zkml-verifier` | [Crate README](crates/zkml-verifier/README.md) |
| JavaScript | `npm install @worldzk/verifier` | [Quick Start](docs/QUICKSTART.md#javascript-browser--wasm) |
| REST API | `docker compose -f docker-compose.verifier.yml up` | [API Reference](docs/API_REFERENCE.md) |

## Test Coverage

1,570+ tests across all components. Every push runs the full suite in CI.

```
Solidity:     789 tests    (verifier, batch, Groth16 hybrid, TEE, fuzz)
Python SDK:   449 tests    (client, verifier, XGBoost, LightGBM, batch)
TypeScript:   199 tests    (TEE verifier, event watcher, Anvil integration)
Stylus WASM:   85 tests    (BN254 field/EC, Poseidon, GKR, Hyrax, sumcheck)
Remainder:     34 tests    (XGBoost circuit, prove-and-verify, ABI encoding)
gnark Go:      14 tests    (Groth16 circuit, proving, chunked EC)
```

## Documentation

- [Quick Start](docs/QUICKSTART.md) -- Verify your first proof in 5 minutes
- [API Reference](docs/API_REFERENCE.md) -- CLI, REST, Rust, Python, C FFI
- [Security Model](docs/SECURITY_MODEL.md) -- Trust assumptions and threat model
- [Architecture](docs/ARCHITECTURE.md) -- System design and components
- [TEE Deployment](docs/TEE_DEPLOYMENT.md) -- AWS Nitro enclave setup
- [Contract Deployment](docs/CONTRACT_DEPLOYMENT.md) -- On-chain deployment
- [Troubleshooting](docs/TROUBLESHOOTING.md) -- Common issues and fixes

## License

[Apache-2.0](LICENSE)
