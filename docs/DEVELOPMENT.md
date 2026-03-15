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
| Go | 1.21+ | `brew install go` (only for gnark-wrapper development) |

Optional for ZK guest programs:

| Tool | Version | Install |
|------|---------|---------|
| risc0 toolchain | 3.0 | `cargo install cargo-binstall && cargo binstall cargo-risczero && rzup install r0vm 3.0.5` |

## Clone and setup

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

# Install Python SDK (editable mode with dev deps)
cd sdk/python && pip install -e ".[dev]" && cd ../..

# Install TypeScript SDK
cd sdk/typescript && npm install && cd ../..
```

## Running each component individually

### Anvil (local Ethereum node)

```bash
anvil --gas-limit 500000000 --code-size-limit 200000
```

Runs on `http://127.0.0.1:8545` with pre-funded accounts.

### Deploy contracts

```bash
cd contracts
forge script script/DeployAll.s.sol:DeployAll \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast --code-size-limit 200000
```

### TEE enclave

```bash
cargo run --manifest-path tee/enclave/Cargo.toml
# Default: http://127.0.0.1:8080
```

### Operator service

```bash
OPERATOR_RPC_URL=http://127.0.0.1:8545 \
OPERATOR_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  cargo run --manifest-path services/operator/Cargo.toml -- watch
```

### Warm prover (Remainder GKR)

```bash
cd examples/xgboost-remainder
cargo run --release -- --model sample_model.json --serve --port 3000
```

### Indexer service

```bash
cargo run --manifest-path services/indexer/Cargo.toml
```

## Docker Compose local stack

The easiest way to run everything together:

```bash
# Build and start all services
docker compose up --build

# Or start in detached mode
docker compose up -d --build

# View logs
docker compose logs -f

# Stop and clean up
docker compose down -v
```

Services started: Anvil, contract deployer (runs once), TEE enclave,
warm prover, private input server, operator.

For GPU-accelerated proving:

```bash
docker compose -f docker-compose.gpu.yml up --build
```

One-command demo script:

```bash
./scripts/tee-local-demo.sh        # starts, tests, tears down
./scripts/tee-local-demo.sh --keep # leaves the stack running
```

## Running tests

### All tests at once

```bash
make test          # Run everything
make preflight     # Pre-CI checks
```

### Per-component

```bash
# Solidity contracts
cd contracts && forge test -vv

# Rust SDK
cargo test --manifest-path sdk/Cargo.toml

# Operator service
cargo test --manifest-path services/operator/Cargo.toml

# TEE enclave
cargo test --manifest-path tee/enclave/Cargo.toml

# Admin CLI
cargo test --manifest-path services/admin-cli/Cargo.toml

# Indexer
cargo test --manifest-path services/indexer/Cargo.toml

# XGBoost Remainder (34+ tests, ~95s release)
cd examples/xgboost-remainder && cargo test --release

# Python SDK
cd sdk/python && pytest

# TypeScript SDK
cd sdk/typescript && npm test
```

### E2E tests

```bash
# Local E2E (requires Docker)
./scripts/e2e-test.sh --network local

# Sepolia E2E (requires funded wallet)
./scripts/e2e-test.sh --network sepolia

# TEE E2E
./scripts/tee-e2e.sh

# Docker-based smoke test
./scripts/docker-smoke-test.sh
```

## Common development tasks

### Adding a new guest program

1. Create a new directory under `examples/`:
   ```
   examples/my-program/
     Cargo.toml           (workspace with host + methods)
     host/
       Cargo.toml
       src/main.rs         (host: prepares input, runs zkVM)
     methods/
       Cargo.toml
       build.rs            (risc0 build script)
       guest/
         Cargo.toml
         src/main.rs       (guest: runs inside zkVM)
   ```
2. Add risc0-zkvm dependencies (match existing version: 3.0).
3. Build: `cargo build --release` (generates ELF/BIN in `methods/`).
4. The pre-compiled binary goes into `programs/` named by its image ID.

### Updating contracts

1. Edit Solidity files in `contracts/src/`.
2. Run tests: `cd contracts && forge test -vv`.
3. Check contract sizes: `./scripts/check-contract-sizes.sh`.
4. Update gas snapshots: `cd contracts && forge snapshot`.
5. If ABI changed, regenerate SDK bindings.

### SDK development

**Rust SDK** (`sdk/`):
```bash
cargo test --manifest-path sdk/Cargo.toml
```

**Python SDK** (`sdk/python/`):
```bash
cd sdk/python
pip install -e ".[dev]"
pytest
ruff check .
```

**TypeScript SDK** (`sdk/typescript/`):
```bash
cd sdk/typescript
npm install
npm test
npm run lint
```

### Generating test fixtures

Fixtures for Solidity tests are generated from Rust:

```bash
cd examples/xgboost-remainder

# Phase 1a DAG fixture (used by GKRDAGVerifierTest)
cargo test --release gen_phase1a_fixture -- --nocapture
# Output: contracts/test/fixtures/phase1a_dag_fixture.json

# Groth16 E2E fixture
cargo run --release --bin gen_dag_groth16_witness -- \
  --model sample_model.json --input sample_input.json
```

## Project layout

```
contracts/              Solidity contracts (Foundry)
  src/remainder/        GKR/Hyrax/Groth16 verifier contracts
  test/                 Forge tests + fixtures
  script/               Deployment scripts
sdk/                    Rust SDK
sdk/python/             Python SDK
sdk/typescript/         TypeScript SDK
services/operator/      Operator service (event watcher + dispute handler)
services/admin-cli/     Admin CLI tool
services/indexer/       Event indexer service
tee/enclave/            TEE enclave (model inference)
crates/watcher/         Shared event watcher crate
crates/events/          Shared event types crate
examples/               Example applications
  rule-engine/          Rule-based classification (risc0 guest)
  anomaly-detector/     Anomaly detection (risc0 guest)
  xgboost-remainder/    XGBoost inference (Remainder GKR+Hyrax)
  sdk-*-quickstart/     SDK quickstart examples
prover/                 risc0-zkvm prover binary
programs/               Pre-compiled guest program binaries
scripts/                Utility and E2E scripts
deploy/                 Helm charts and Kubernetes manifests
monitoring/             Prometheus + Grafana configs
docs/                   Documentation
```

## Useful Make targets

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

### Rust services

Set `RUST_LOG` for verbose logging:

```bash
RUST_LOG=debug cargo run --manifest-path services/operator/Cargo.toml
```

For JSON logging (useful with log aggregators):

```bash
RUST_LOG_FORMAT=json cargo run --manifest-path services/operator/Cargo.toml
```

### Contract debugging

Use Foundry's trace flags for detailed EVM traces:

```bash
forge test -vvvv --match-test testMyFunction
```

For gas profiling:

```bash
forge test --gas-report
```

### Docker services

```bash
# View logs for a specific service
docker compose logs -f operator

# Shell into a running container
docker compose exec enclave sh

# Check health endpoints
curl http://localhost:8080/health   # enclave
curl http://localhost:9090/health   # operator
curl http://localhost:3000/health   # warm prover
```

## Code style and linting

### Rust

- Use `rustfmt` (standard config).
- Run `cargo clippy` before committing.
- `cargo fmt --check` is enforced in CI.

### Solidity

- Use `forge fmt`.
- Run `forge fmt --check` in CI.
- Follow the naming conventions in existing contracts.

### Python

- Use `ruff` for linting and formatting.
- Type hints are encouraged.
- `ruff check` and `ruff format --check` run in CI.

### TypeScript

- Use `prettier` and `eslint`.
- `npm run lint` runs in CI.
