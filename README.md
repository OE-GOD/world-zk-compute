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
| **Foundry** | Solidity contracts | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| **Docker** | Full stack / demos | [Docker Desktop](https://docs.docker.com/get-docker/) |
| Node.js 18+ | TypeScript SDK (optional) | `brew install node` or [nvm](https://github.com/nvm-sh/nvm) |
| Python 3.10+ | Python SDK (optional) | `brew install python3` |

## Getting Started

### 1. Clone and build

```bash
git clone --recursive https://github.com/OE-GOD/world-zk-compute.git
cd world-zk-compute
make setup
```

> Already cloned without `--recursive`? Run `git submodule update --init --recursive` first.

### 2. Run tests

```bash
make test-contracts    # Solidity tests (~60s)
make test-sdk          # Rust SDK tests
make test              # Everything (takes a few minutes)
```

### 3. Try it out

**Option A: Verify a proof with the Rust CLI**

```bash
cargo run -p zkml-verifier -- verify contracts/test/fixtures/phase1a_dag_fixture.json --json
```

**Option B: Run the full stack with Docker**

```bash
docker compose -f docker-compose.bank-demo.yml up -d
curl http://localhost:3000/health
```

**Option C: Install the Python SDK**

```bash
pip install zkml-verifier
```

```python
from zkml_verifier import verify_file
result = verify_file("proof_bundle.json")
print(f"Verified: {result['verified']}")
```

## Project Structure

```
contracts/              Solidity verifier contracts (Foundry)
  src/remainder/        GKR/Hyrax/Groth16 on-chain verifiers
  test/                 1,100+ Solidity tests
examples/               Prover implementations
  xgboost-remainder/    XGBoost inference circuit (GKR+Hyrax)
crates/                 Shared Rust libraries
  zkml-verifier/        Standalone proof verifier (Rust + Python FFI + WASM)
sdk/                    Client SDKs (Rust, Python, TypeScript)
services/               Backend services (operator, indexer, admin-cli)
tee/                    AWS Nitro enclave (TEE happy path)
prover/                 risc0-zkvm prover
programs/               Pre-compiled guest program binaries
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
| Python | `pip install zkml-verifier` | [Quick Start](docs/QUICKSTART.md#python) |
| Rust | `cargo add zkml-verifier` | [Crate README](crates/zkml-verifier/README.md) |
| JavaScript | `npm install @worldzk/verifier` | [Quick Start](docs/QUICKSTART.md#javascript-browser--wasm) |
| REST API | `docker compose -f docker-compose.verifier.yml up` | [API Reference](docs/API_REFERENCE.md) |

## Documentation

- [Quick Start](docs/QUICKSTART.md) -- Verify your first proof in 5 minutes
- [Development Setup](docs/DEVELOPMENT.md) -- Full dev environment guide
- [Contributing](CONTRIBUTING.md) -- How to contribute
- [API Reference](docs/API_REFERENCE.md) -- CLI, REST, Rust, Python, C FFI
- [Architecture](docs/ARCHITECTURE.md) -- System design and components
- [Security Model](docs/SECURITY_MODEL.md) -- Trust assumptions and threat model
- [TEE Deployment](docs/TEE_DEPLOYMENT.md) -- AWS Nitro enclave setup
- [Contract Deployment](docs/CONTRACT_DEPLOYMENT.md) -- On-chain deployment
- [Troubleshooting](docs/TROUBLESHOOTING.md) -- Common issues and fixes

## License

[Apache-2.0](LICENSE)
