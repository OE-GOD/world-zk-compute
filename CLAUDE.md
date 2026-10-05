# World ZK Compute

## Project Structure

- `prover/` — Rust risc0-zkvm v3.0 prover
- `contracts/` — Foundry Solidity contracts (verifiers, tests); `contracts/stylus/` holds the Stylus GKR verifier
- `crates/` — shared Rust crates (`zkml-verifier`, `events`, `watcher`)
- `services/` — off-chain services (verifier API, gateway, operator, indexer, ...)
- `tee/` — TEE enclave
- `examples/xgboost-remainder/` — XGBoost tree inference circuit + GKR/Hyrax/Groth16
- `programs/` — Pre-compiled guest program binaries
- `scripts/` — E2E test scripts
- `sdk/` — Client SDKs
- `web/` — in-browser WASM verifier demo and landing page

## Working Rules

- After editing Rust, run `cargo check --workspace` (or `-p <crate>`); after editing Solidity, run `cd contracts && forge build`. Fix compile errors before moving on.
- Formatters (`forge fmt`, `cargo fmt`) rewrite files — re-read a file before editing it again after formatting.
