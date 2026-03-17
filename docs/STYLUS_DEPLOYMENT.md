# Stylus GKR DAG Verifier — Deployment Guide

Deploy the GKR DAG verifier as an Arbitrum Stylus (WASM) contract on Arbitrum Sepolia testnet.

## Prerequisites

| Tool | Install |
|------|---------|
| Rust + `wasm32-unknown-unknown` target | `rustup target add wasm32-unknown-unknown` |
| cargo-stylus | `cargo install cargo-stylus` |
| Foundry (forge, cast) | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| wasm-opt (binaryen) | `brew install binaryen` / `apt install binaryen` |
| brotli | `brew install brotli` / `apt install brotli` |

**Wallet**: You need a private key with ~0.05 ETH on Arbitrum Sepolia.

Faucets:
- https://faucets.chain.link/arbitrum-sepolia
- https://www.alchemy.com/faucets/arbitrum-sepolia
- https://faucet.quicknode.com/arbitrum/sepolia

## WASM Size Budget

Arbitrum Stylus has a **24KB Brotli-compressed size limit** for deployed WASM.

Check your current sizes:

```bash
cd contracts/stylus/gkr-verifier
make size
```

Expected output (as of initial build):

| Metric | Size |
|--------|------|
| Raw WASM | ~60 KB |
| Optimized (wasm-opt) | ~49 KB |
| Raw Brotli | ~24.1 KB |
| **Optimized Brotli** | **~22.8 KB** |

The optimized Brotli size must stay under 24,576 bytes. The `wasm-opt` step is required — raw Brotli may exceed the limit.

## Deploy via Script (Recommended)

The deployment script handles all 6 phases automatically:

```bash
# Full deployment (Stylus + Solidity adapter + verification)
PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh

# Stylus contract only
PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --stylus-only

# Use existing Stylus contract, deploy adapter only
PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --stylus-address 0x...

# Skip verification simulation
PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --skip-verify

# Custom RPC
PRIVATE_KEY=0x... ./scripts/stylus-sepolia-deploy.sh --rpc https://your-rpc.com
```

### What the script does

1. **Phase 0** — Check prerequisites (cargo-stylus, forge, cast, wasm32 target)
2. **Phase 1** — Validate network connection (chain ID 421614)
3. **Phase 2** — Build WASM binary
4. **Phase 3** — Deploy Stylus contract via `cargo stylus deploy`
5. **Phase 4** — Deploy Solidity RemainderVerifier adapter via forge script
6. **Phase 5** — Run `cast call` verification simulation (off-chain)
7. **Phase 6** — Print deployment summary with addresses and Arbiscan links

## Deploy via CI (GitHub Actions)

Trigger the **Stylus Deploy** workflow from the Actions tab.

### Inputs

| Input | Default | Description |
|-------|---------|-------------|
| `dry_run` | `true` | Build + validate only, don't deploy |
| `stylus_only` | `false` | Skip Solidity adapter deployment |
| `skip_verify` | `false` | Skip cast call verification |

### Required Secrets

| Secret | Description |
|--------|-------------|
| `ARB_SEPOLIA_PRIVATE_KEY` | Deployer private key (hex, 0x-prefixed) |

### Workflow

1. **Build job** — Build WASM, optimize, check Brotli size, run `cargo stylus check`
2. **Deploy job** (only if `dry_run=false`) — Deploy Stylus + adapter, run verification, save addresses as artifact

## Manual Deployment

### Step 1: Build and optimize WASM

```bash
cd contracts/stylus/gkr-verifier
make opt
```

### Step 2: Validate deployability

```bash
cargo stylus check --endpoint https://sepolia-rollup.arbitrum.io/rpc
```

### Step 3: Deploy Stylus contract

```bash
cargo stylus deploy \
  --endpoint https://sepolia-rollup.arbitrum.io/rpc \
  --private-key 0x...
```

Save the deployed contract address from the output.

### Step 4: Deploy Solidity adapter

```bash
cd contracts
DEPLOYER_KEY=0x... STYLUS_VERIFIER=0x<stylus-address> \
  forge script script/StylusSepoliaDeploy.s.sol:StylusSepoliaDeploy \
  --rpc-url https://sepolia-rollup.arbitrum.io/rpc \
  --broadcast -vvv
```

### Step 5: Verify deployment

```bash
# Check the contract exists
cast code --rpc-url https://sepolia-rollup.arbitrum.io/rpc 0x<stylus-address>

# View on Arbiscan
open https://sepolia.arbiscan.io/address/0x<stylus-address>
```

## Deployed Addresses

| Network | Component | Address |
|---------|-----------|---------|
| Arbitrum Sepolia | Stylus Verifier | *TBD — deploy first* |
| Arbitrum Sepolia | RemainderVerifier | *TBD — deploy first* |

Update this table after each deployment.

## Reactivation

Stylus contracts require **yearly reactivation**. The contract will become inactive after ~1 year and must be reactivated by calling `ArbWasm.activateProgram()` or redeploying.

Set a calendar reminder for 11 months after deployment.

## Troubleshooting

### "program too large" during `cargo stylus check`

The Brotli-compressed WASM exceeds 24KB. Run `make size` to verify. Ensure `wasm-opt` is being used — the raw binary without optimization will be too large.

### "insufficient funds" during deployment

Fund your wallet with Arbitrum Sepolia ETH. You need ~0.05 ETH for the full deployment (Stylus + Solidity adapter).

### `cargo stylus check` fails with connection error

Check your RPC endpoint. The default `https://sepolia-rollup.arbitrum.io/rpc` is rate-limited. Consider using an Alchemy or Infura endpoint.

### Verification simulation returns false or reverts

The `cast call` simulation requires the full fixture data. Ensure `contracts/test/fixtures/phase1a_dag_fixture.json` exists and matches the registered circuit.

### "contract not activated" after deployment

New Stylus contracts should be auto-activated on deployment. If not, call `ArbWasm.activateProgram(address)` with the contract address on Arbitrum Sepolia.

### WASM size growing over limit

If code changes push the Brotli size past 24KB:
1. Check for new dependencies — each dep adds code
2. Ensure `opt-level = "z"`, `lto = true`, `strip = true`, `panic = "abort"` in `Cargo.toml`
3. Run `wasm-opt -O3` — this typically saves 10-20%
4. Use the `verify!` macro (strips panic strings on WASM) instead of `assert!`
5. Avoid `Debug`/`Display` impls behind `#[cfg(not(target_arch = "wasm32"))]`
