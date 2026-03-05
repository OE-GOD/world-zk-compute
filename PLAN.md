# Plan: Compile & Deploy Stylus GKR Verifier to Local Devnode

## Goal
Get the existing Stylus GKR DAG verifier compiling (native + WASM), deploy to a local Arbitrum Stylus devnode, and run a real XGBoost proof through it.

## Current State
- **4,056 lines of Rust** across 11 source files — complete port of Solidity DAG verifier
- **Native build (`--no-default-features`)**: compiles, but tests fail (19 `vec!` macro errors in test module)
- **WASM build**: fails — `stylus-sdk 0.9.2` pulls `ruint 1.17.2` which is incompatible with Rust 1.92
- **cargo-stylus**: installed (upgrading to 0.10.0)
- **Docker**: installed but daemon not running
- **nitro-devnode**: not cloned yet

## Steps

### Step 1: Fix compilation errors
1. Add `use alloc::vec;` to `#[cfg(test)] mod tests` in `lib.rs`
2. Upgrade `Cargo.toml`: `stylus-sdk` 0.9→0.10, `alloy-primitives` 0.8→1.0, `alloy-sol-types` 0.8→1.0
3. Fix any API breakage from alloy 0.8→1.0 (check `Address::from_word`, `B256`)
4. Verify: `cargo test --no-default-features` passes
5. Verify: `cargo build --target wasm32-unknown-unknown --release` succeeds

### Step 2: Add E2E native test with real XGBoost proof
1. Create a test that loads `contracts/test/fixtures/phase1a_dag_fixture.json`
2. Encode proof/gens/circuit_desc as the Stylus verifier expects
3. Run `verify_dag_proof_inner()` natively to validate correctness
4. This proves the Rust port works before deploying

### Step 3: Set up local devnode & deploy
1. User starts Docker daemon
2. Clone `nitro-devnode`, run `./run-dev-node.sh`
3. `cargo stylus check --endpoint='http://localhost:8547'`
4. `cargo stylus deploy --endpoint='http://localhost:8547' --private-key=<devnode_key> --no-verify`

### Step 4: E2E test on devnode
1. Send XGBoost proof via `cast call` to deployed contract
2. Verify it returns `true`
3. Measure gas and compare to EVM baseline (252M)

## Key Risks
- **alloy 1.0 API**: `Address::from_word`, `B256` may have minor signature changes
- **WASM code size**: 4K lines + Poseidon constants may approach 128KB compressed limit
- **Steps 3-4 require Docker**: Will complete Steps 1-2 first (no Docker needed)
