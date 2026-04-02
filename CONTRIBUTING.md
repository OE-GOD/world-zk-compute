# Contributing to World ZK Compute

## Prerequisites

- **Rust** 1.82+ (`rustup update stable`)
- **Foundry** (`curl -L https://foundry.paradigm.xyz | bash && foundryup`)
- **Docker** (for Groth16 proving and containerized services)
- **r0vm 3.0.5** (`rzup install r0vm 3.0.5`) — must match risc0-zkvm crate version
- **Node.js** 18+ and npm (for TypeScript SDK)
- **Python** 3.10+ (for Python SDK)

## Development Setup

```bash
# Clone the repo (--recursive pulls Foundry submodules)
git clone --recursive https://github.com/OE-GOD/world-zk-compute.git
cd world-zk-compute

# Build all components (contracts + Rust workspace)
make setup

# Run all tests
make test
```

> **Already cloned without `--recursive`?** Run `git submodule update --init --recursive` to fetch the Foundry dependencies.

## Project Structure

| Directory | Purpose |
|-----------|---------|
| `prover/` | Rust risc0-zkvm v3.0 prover |
| `contracts/` | Foundry Solidity contracts (verifiers, TEE, execution engine) |
| `examples/` | Guest programs (rule-engine, anomaly-detector, xgboost, etc.) |
| `programs/` | Pre-compiled guest program binaries (named by image ID) |
| `sdk/` | Client SDKs (Rust, TypeScript, Python) |
| `services/` | Backend services (operator, indexer, admin-cli) |
| `tee/` | TEE enclave implementation |
| `crates/` | Shared library crates (events, watcher) |
| `scripts/` | Build, test, and deployment scripts |
| `monitoring/` | Prometheus, Grafana, alerting configs |

## Running Tests

```bash
make test              # All tests
make test-contracts    # Solidity tests (forge test)
make test-operator     # Operator service tests
make test-sdk          # Rust SDK tests
make test-python       # Python SDK tests
make test-typescript   # TypeScript SDK tests
make test-xgboost      # XGBoost remainder tests (~95s)
make test-stylus       # Stylus GKR verifier tests
```

### E2E Tests

```bash
./scripts/e2e-test.sh --network local    # Local (requires Anvil)
./scripts/e2e-test.sh --network sepolia  # Sepolia (requires funded wallet)
```

## Code Style

- **Rust**: `cargo fmt` (rustfmt) and `cargo clippy`
- **Solidity**: `forge fmt`
- **TypeScript**: Prettier
- **Python**: Black + Ruff

Run `make fmt` to format all code and `make lint` to check.

## Adding a New Guest Program

See [GUEST_PROGRAMS.md](GUEST_PROGRAMS.md) for the full workflow. In brief:

1. Create `examples/{your-program}/` with `methods/guest/` and `host/` subdirectories
2. Add `risc0-build = "3.0"` to `methods/Cargo.toml` with a `build.rs` calling `embed_methods()`
3. Implement guest logic in `methods/guest/src/main.rs`
4. Build with `cargo build --release`
5. Copy compiled binary to `programs/` named by its image ID (hex)
6. Add tests in the host directory

## Adding a New Contract Test

1. Create `contracts/test/MyContract.t.sol`
2. Import `forge-std/Test.sol` and your contract
3. Use `setUp()` for deployment and initialization
4. Name test functions with `test_` prefix (e.g., `test_myFeature_works`)
5. Use `vm.expectRevert()` for negative tests, `vm.prank()` for access control tests
6. Run: `forge test --match-contract MyContractTest -vvv`

## Pull Request Guidelines

1. Create a feature branch from `main`
2. Keep PRs focused on a single change
3. Include tests for new functionality
4. Ensure `make test` passes before submitting
5. Write clear commit messages describing _why_, not just _what_

## CI Expectations

PRs must pass:
- Rust compilation and tests
- Solidity compilation and tests
- Contract size check (EIP-170 limit)
- Code formatting checks

## Commit Messages

Use imperative mood: "Add feature" not "Added feature" or "Adds feature".

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
