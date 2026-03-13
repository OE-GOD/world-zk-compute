# Contributing to World ZK Compute

## Development Setup

```bash
# Clone the repo
git clone https://github.com/worldcoin/world-zk-compute.git
cd world-zk-compute

# Install dependencies
make setup

# Run all tests
make test
```

### Prerequisites

- Rust 1.82+ (`rustup update stable`)
- Node.js 18+ and npm (for TypeScript SDK)
- Python 3.10+ (for Python SDK)
- Foundry (`curl -L https://foundry.paradigm.xyz | bash && foundryup`)

## Running Tests

```bash
make test              # All tests
make test-contracts    # Solidity tests (forge test)
make test-operator     # Operator service tests
make test-sdk          # Rust SDK tests
make test-python       # Python SDK tests
make test-typescript   # TypeScript SDK tests
```

## Code Style

- **Rust**: `cargo fmt` (rustfmt) and `cargo clippy`
- **Solidity**: `forge fmt`
- **TypeScript**: Prettier
- **Python**: Black + Ruff

Run `make fmt` to format all code and `make lint` to check.

## Pull Request Guidelines

1. Create a feature branch from `main`
2. Keep PRs focused on a single change
3. Include tests for new functionality
4. Ensure `make test` passes before submitting
5. Write clear commit messages describing _why_, not just _what_

## Commit Messages

Use imperative mood: "Add feature" not "Added feature" or "Adds feature".

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
