# World ZK Compute

Cryptographic proof that AI models produce correct outputs. Verify ML inference without re-executing the model.

## What It Does

You have an ML model (XGBoost, LightGBM, Random Forest, etc.). World ZK Compute generates a cryptographic proof that the model ran correctly on given inputs and produced the declared output. Anyone can verify that proof -- locally, on-chain, or via API -- without access to the model weights or input data.

```
   ML Model + Input ──→ Prover (TEE or ZK) ──→ Proof Bundle (JSON)
                                                       │
                                        Verifier (API / SDK / on-chain)
                                                       │
                                                  Valid / Invalid
```

## Prerequisites

| Tool | Required for | Install |
|------|-------------|---------|
| **Rust** 1.82+ | Core build | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **Foundry** | Solidity contracts + demos | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| **Docker** | Full stack (optional) | [Docker Desktop](https://docs.docker.com/get-docker/) |

## Getting Started

### 1. Clone and build

```bash
git clone --recursive https://github.com/OE-GOD/world-zk-compute.git
cd world-zk-compute
make setup    # initializes submodules, builds contracts + Rust (~2-3 min first time)
```

> Already cloned without `--recursive`? Run `git submodule update --init --recursive` first.

### 2. Verify a proof

The repo ships with a real proof bundle. Verify it with the CLI:

```bash
cargo run -p zkml-verifier -- verify contracts/test/fixtures/phase1a_dag_fixture.json --json
```

Expected output:
```json
{"verified":true,"circuit_hash":"0x..."}
```

### 3. Run the on-chain demo

This walks through the full TEE lifecycle on a local Anvil chain -- deploy contracts, register a model, submit inference, wait the challenge window, and finalize. No Docker needed.

```bash
./scripts/tee-local-demo.sh --step
```

> Requires Foundry (anvil, forge, cast) + python3. Takes ~30-60 seconds.

### 4. Run tests

```bash
make test-contracts    # Solidity tests (~60s)
make test-sdk          # Rust SDK tests
make test              # Everything
```

### 5. Start the verifier API (Docker)

```bash
docker compose -f docker-compose.bank-demo.yml up -d    # first build takes ~5 min
curl http://localhost:3000/health                        # wait for {"status":"ok"}

# Verify a proof via REST
curl -s -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @contracts/test/fixtures/phase1a_dag_fixture.json
```

### 6. Run the full stack (advanced)

Starts Anvil + contract deployer + TEE enclave + ZK prover + operator service:

```bash
docker compose up -d --build    # first build takes ~10-15 min (4 Rust services)
docker compose logs -f          # watch all services
```

See [Development Setup](docs/DEVELOPMENT.md) for the full dev guide.

## Project Structure

```
contracts/              Solidity verifier contracts (Foundry)
  src/remainder/        GKR/Hyrax/Groth16 on-chain verifiers
  test/                 1,100+ Solidity tests
examples/               Prover implementations
  xgboost-remainder/    XGBoost inference circuit (GKR+Hyrax)
crates/                 Shared Rust libraries
  zkml-verifier/        Standalone proof verifier (Rust + WASM + Python FFI)
services/               Backend services (operator, indexer, verifier API)
tee/                    AWS Nitro enclave (TEE happy path)
prover/                 risc0-zkvm prover
scripts/                Build, test, and deployment scripts
docs/                   Full documentation
```

## How It Works

**TEE happy path:** The model runs inside an AWS Nitro enclave. The enclave signs the result. Cost: ~$0.0001 per inference.

**ZK dispute path:** If anyone challenges a result, a GKR+Hyrax zero-knowledge proof settles the dispute on-chain. The proof is purely mathematical -- no hardware trust required.

| Verification Path | Gas Cost | Transactions | Use Case |
|-------------------|----------|-------------|----------|
| REST API | Free | N/A | Off-chain audit, compliance |
| Stylus + Groth16 Hybrid | ~3-6M gas | 1 | On-chain dispute (Arbitrum) |
| Multi-tx Batch | <30M gas/tx | 15 | On-chain dispute (Ethereum L1) |
| Direct GKR+Hyrax | ~254M gas | 1 | High-gas-limit chains |

## Supported Models

| Model Type | Status |
|------------|--------|
| XGBoost | Supported |
| LightGBM | Supported |
| Random Forest | Supported |
| Logistic Regression | Supported |

## SDKs

| Language | Install | Docs |
|----------|---------|------|
| Rust | `cargo add zkml-verifier` | [Crate docs](crates/zkml-verifier/README.md) |
| Python | Build from source (see [Quick Start](docs/QUICKSTART.md#python)) | [Quick Start](docs/QUICKSTART.md#python) |
| JavaScript | WASM build (see [Quick Start](docs/QUICKSTART.md#javascript-browser--wasm)) | [Quick Start](docs/QUICKSTART.md#javascript-browser--wasm) |
| REST API | `docker compose -f docker-compose.verifier.yml up` | [API Reference](docs/API_REFERENCE.md) |

## Documentation

- [Quick Start](docs/QUICKSTART.md) -- Verify your first proof in 5 minutes
- [Development Setup](docs/DEVELOPMENT.md) -- Full dev environment guide
- [Contributing](CONTRIBUTING.md) -- How to contribute
- [API Reference](docs/API_REFERENCE.md) -- CLI, REST, Rust, Python, C FFI
- [Architecture](docs/ARCHITECTURE.md) -- System design and components
- [Security Model](docs/SECURITY_MODEL.md) -- Trust assumptions and threat model
- [Troubleshooting](docs/TROUBLESHOOTING.md) -- Common issues and fixes

## License

[Apache-2.0](LICENSE)
