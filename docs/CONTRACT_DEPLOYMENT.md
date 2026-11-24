# Contract Deployment Strategy: EIP-170 Considerations

## EIP-170 Bytecode Size Limit

EIP-170 limits deployed contract bytecode to **24,576 bytes (24 KB)**. Several contracts in this project exceed this limit.

## Contracts Exceeding EIP-170

| Contract | Source Size | Status |
|----------|-----------|--------|
| `RemainderVerifier.sol` | ~2,784 lines (~125 KB source) | Exceeds limit (135,533 bytes runtime) |
| `PoseidonSponge.sol` | ~2,569 lines (~119 KB source) | Internal library, compiled into `RemainderVerifier` |
| `DAGRemainderGroth16Verifier.sol` | ~31,261 lines (~2 MB source) | Exceeds limit (461,497 bytes runtime) |
| `GKRDAGVerifier.sol` | ~905 lines (~39 KB source) | Internal library, compiled into `RemainderVerifier` |
| `GKRDAGHybridVerifier.sol` | ~1,181 lines (~49 KB source) | Internal library, compiled into `RemainderVerifier` |
| `RemainderGroth16Verifier.sol` | ~1,219 lines (~68 KB source) | Under limit (14,714 bytes runtime) |
| `GKRHybridVerifier.sol` | ~647 lines (~29 KB source) | Internal library, compiled into `RemainderVerifier` |
| `GKRVerifier.sol` | ~584 lines (~26 KB source) | Internal library, compiled into `RemainderVerifier` |

Contracts under the limit:
- `HyraxVerifier.sol` (~15 KB source)
- `HyraxProofDecoder.sol` (~17 KB source)
- `TEEMLVerifier.sol`
- `ExecutionEngine.sol`

## Current Mitigation Strategies

### 1. Library Architecture

Core verification logic is implemented as Solidity libraries (`library` keyword). None of their functions is `public` or `external`, so the compiler copies them into the bytecode of the contract that uses them; they are not linked at deployment time:

- `GKRVerifier` — GKR protocol verification
- `GKRDAGVerifier` — DAG circuit verification
- `GKRHybridVerifier` — Hybrid Fr verification
- `GKRDAGHybridVerifier` — DAG hybrid verification
- `HyraxVerifier` — Hyrax polynomial commitment
- `HyraxProofDecoder` — Proof binary decoder
- `PoseidonSponge` — Poseidon hash (Fiat-Shamir)
- `SumcheckVerifier` — Sumcheck protocol
- `DAGBatchVerifier` — Multi-tx batch state management

The libraries are not deployed separately: the `RemainderVerifier` artifact has no link references, and no deploy script passes `--libraries`. They are part of its 135,533 bytes of runtime bytecode and do not reduce its deployed size.

### 2. Multi-Transaction Batch Verification (DAGBatchVerifier)

For the 88-layer XGBoost DAG circuit (about 198M gas in one transaction for the phase1a fixture, `eth_estimateGas` on Anvil), verification is split across multiple transactions:

- **`LAYERS_PER_BATCH = 8`**: 88 compute layers / 8 = 11 compute batches
- **`GROUPS_PER_FINALIZE_BATCH = 16`**: 34 eval groups / 16 = 3 finalize txs
- **Total: 1 setup + 11 continue + 3 finalize = 15 transactions**

Gas per transaction, measured for the phase1a fixture as real transactions on Anvil: start 8.6M, continue 18.6–31.7M, finalize 14.7–27.7M. Two of the eleven continue transactions (31.4M and 31.7M) are over a 30M block gas limit. An optional cleanup transaction uses 1.8M.

### 3. Stylus WASM Verifier (Arbitrum)

A complete GKR DAG verifier is implemented in Rust/WASM for Arbitrum Stylus:

- **Location**: `contracts/stylus/gkr-verifier/`
- **Size**: ~60 KB raw WASM → ~22.8 KB Brotli after `wasm-opt` (under 24 KB Stylus limit)
- **Integration**: `RemainderVerifier.verifyDAGProofStylus()` delegates to the WASM contract via `staticcall`

This avoids EIP-170 for the WASM contract, by running verification in WASM rather than EVM bytecode. The Solidity entry point is still `RemainderVerifier` (135,533 bytes), which Arbitrum One and Arbitrum Sepolia reject: both enforce the 24,576-byte limit.

### 4. Function Splitting (Stack-Depth Management)

`DAGBatchVerifier.sol` and `RemainderVerifier.sol` use aggressive function extraction to avoid stack-too-deep errors:

- `_decodeBatchProof()`, `_execBatchVerify()`, `_callBatchVerify()`
- `_runBatchVerifyForSession()` / `_runBatchVerifyInner()` chains
- `BatchVerifyCtx` struct to pass state between split functions

### 5. Optimizer Settings

In `contracts/foundry.toml`:
- **Optimizer**: Enabled with 200 runs
- **`via_ir`**: Commented out — Yul optimizer hits stack layout limits with DAGBatchVerifier
- Compiler: Solidity 0.8.20

## Deployment Modes

### TEE-Only Mode (Sepolia)

On Sepolia testnet, the system runs in **TEE-only mode** (`deployments/chains.json` marks Sepolia, Arbitrum Sepolia and World Chain Sepolia as `teeOnly`):
- `TEEMLVerifier` and `ExecutionEngine` can be deployed (both under EIP-170: 14,714 and 11,230 bytes runtime). `deployments/` records no `TEEMLVerifier` address on any chain; `docs/CONTRACT_ADDRESSES.md` lists an `ExecutionEngine` on Sepolia
- `RemainderVerifier` (with its libraries compiled in) is NOT deployed
- TEE attestation provides trust, ZK verification is not needed on-chain
- This sidesteps the EIP-170 issue entirely for testnet

### Full ZK Mode (Future Mainnet)

For on-chain ZK verification, options include:

1. **Library deployment**: Deploy each library separately, link to RemainderVerifier
2. **UUPS Proxy (implemented)**: RemainderVerifier is deployed behind a `UUPSProxy` with `initialize()`. Deploy scripts use: `new RemainderVerifier()` → `new UUPSProxy(impl, abi.encodeCall(RemainderVerifier.initialize, (admin)))`. Critical admin ops are guarded by `onlyTimelocked`.
3. **Diamond pattern (EIP-2535)**: Split into facets for each verification mode
4. **Stylus delegation**: Use WASM verifier on Arbitrum (implemented and tested)

### Recommended Approach

For **Arbitrum/World Chain** (Stylus-capable):
- Deploy `RemainderVerifier` as a thin registry/dispatcher (under EIP-170; the current contract is 135,533 bytes and would have to be split first)
- Use Stylus WASM verifier for actual GKR verification
- Use `DAGBatchVerifier` for multi-tx flow

For **other EVM chains**:
- Deploy libraries separately
- Use proxy pattern to keep RemainderVerifier under limit
- Or use the hybrid Groth16 path (smaller on-chain footprint)

## Size Check Script

`scripts/check-contract-sizes.sh` runs `forge build --sizes` and checks eight production contracts against the 24 KB limit. It's integrated into `.github/workflows/contracts-ci.yml`.

```bash
cd contracts && bash ../scripts/check-contract-sizes.sh
```

The script reports `RemainderVerifier` (135,533 bytes, 551% of the limit) as `FAIL`, warns that a contract over the limit cannot be deployed on a chain that enforces it, and exits with status 1.

## Contract Upgrade Strategy

RemainderVerifier is a **UUPS implementation** deployed behind `UUPSProxy` (`contracts/src/Upgradeable.sol`). Changes require:
1. Deploy the new implementation
2. Call `upgradeTo()` / `upgradeToAndCall()` on the proxy as the admin (or through the timelock once `setTimelock()` has been called)
