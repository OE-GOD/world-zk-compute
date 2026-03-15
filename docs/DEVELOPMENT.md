# Development Setup

Local development guide for World ZK Compute.

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust | 1.75+ | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Foundry | latest | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| Node.js | 18+ | `brew install node` or [nvm](https://github.com/nvm-sh/nvm) |
| Python | 3.9+ | `brew install python3` or system package manager |
| Docker | 24+ | [Docker Desktop](https://docs.docker.com/get-docker/) |
| jq | any | `brew install jq` or `apt install jq` |

## Quick Start

```bash
# Clone
git clone https://github.com/worldcoin/world-zk-compute.git
cd world-zk-compute

# Build contracts
cd contracts && forge install && forge build && cd ..

# Build Rust crates
cargo build --manifest-path sdk/Cargo.toml
cargo build --manifest-path services/operator/Cargo.toml
cargo build --manifest-path tee/enclave/Cargo.toml

# Install Python SDK
cd sdk/python && pip install -e ".[dev]" && cd ../..

# Install TypeScript SDK
cd sdk/typescript && npm install && cd ../..
```

## Running Locally

### Option A: Docker Compose (recommended)

```bash
docker compose up --build
```

This starts: Anvil (local chain), contract deployer, TEE enclave, warm prover, and operator.

### Option B: Manual

```bash
# Terminal 1: Start Anvil
anvil --gas-limit 500000000 --code-size-limit 200000

# Terminal 2: Deploy contracts
cd contracts && forge script script/DeployAll.s.sol:DeployAll \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast --code-size-limit 200000

# Terminal 3: Start enclave
cargo run --manifest-path tee/enclave/Cargo.toml

# Terminal 4: Start operator
OPERATOR_RPC_URL=http://127.0.0.1:8545 \
  cargo run --manifest-path services/operator/Cargo.toml -- watch
```

## Running Tests

```bash
make test          # All tests
make test-rust     # Rust only
make test-contracts # Solidity only
make test-python   # Python SDK only
make test-ts       # TypeScript SDK only
make preflight     # Pre-CI checks
```

## Code Quality

```bash
make fmt           # Format all code
make lint          # Lint all code (clippy + forge fmt --check)
make check         # Full validation (lint + scripts + compose)
```

## Project Layout

```
contracts/          Solidity contracts (Foundry)
sdk/                Rust SDK
sdk/python/         Python SDK
sdk/typescript/     TypeScript SDK
services/operator/  Operator service (event watcher + dispute handler)
services/admin-cli/ Admin CLI tool
services/indexer/   Event indexer service
tee/enclave/        TEE enclave (model inference)
crates/watcher/     Shared event watcher crate
crates/events/      Shared event types crate
examples/           Example applications
prover/             risc0-zkvm prover
programs/           Pre-compiled guest programs
scripts/            Utility scripts
deploy/             Helm charts and Kubernetes manifests
monitoring/         Prometheus + Grafana configs
docs/               Documentation
```

## Useful Make Targets

Run `make help` for the full list. Key targets:

| Target | Description |
|--------|-------------|
| `make test` | Run all tests |
| `make lint` | Run all linters |
| `make fmt` | Format all code |
| `make docker-up` | Start local Docker stack |
| `make deploy-local` | Deploy to local Anvil |
| `make preflight` | CI preflight checks |
| `make smoke-test` | E2E smoke test |
| `make snapshot` | Gas snapshot |

## Debugging

Set `RUST_LOG=debug` for verbose Rust logging:

```bash
RUST_LOG=debug cargo run --manifest-path services/operator/Cargo.toml
```

For JSON logging (useful with log aggregators):

```bash
RUST_LOG_FORMAT=json cargo run --manifest-path services/operator/Cargo.toml
```
