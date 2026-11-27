# World ZK Compute

## Project Structure

- `prover/` — Rust risc0-zkvm v3.0 prover
- `contracts/` — Foundry Solidity contracts (verifiers, tests); `contracts/stylus/` holds the Stylus GKR verifier
- `crates/` — shared Rust crates (`zkml-verifier`, `events`, `watcher`)
- `services/` — off-chain services (verifier API, gateway, operator, indexer, ...)
- `tee/` — TEE enclave
- `examples/xgboost-remainder/` — XGBoost tree inference circuit + GKR/Hyrax/Groth16
- `programs/` — Pre-compiled guest program binaries
- `scripts/` — E2E, demo, deploy, and load-test scripts
- `sdk/` — Client SDKs
- `web/` — in-browser WASM verifier demo and landing page; `web/portfolio/` only holds redirects to the github.io archive

## Working Rules

- Read the status note at the top of `README.md` first. Don't add or restore claims that weights stay private, that verification is sound or zero-knowledge, or that random forest, neural-network or logistic-regression models are supported, until the issues listed there are fixed.
- After editing Rust, run `cargo check --workspace` (or `-p <crate>`); after editing Solidity, run `cd contracts && forge build`. Fix compile errors before moving on.
- Formatters (`forge fmt`, `cargo fmt`) rewrite files — re-read a file before editing it again after formatting.
