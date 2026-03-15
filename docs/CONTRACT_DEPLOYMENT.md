# Contract Deployment Strategy: EIP-170 Considerations

## EIP-170 Bytecode Size Limit

EIP-170 limits deployed contract bytecode to **24,576 bytes (24 KB)**. Several contracts in this project exceed this limit.

## Contracts Exceeding EIP-170

| Contract | Source Size | Status |
|----------|-----------|--------|
| `RemainderVerifier.sol` | ~2,285 lines (~99 KB source) | Exceeds limit |
| `PoseidonSponge.sol` | ~2,569 lines (~119 KB source) | Deployed as library |
| `DAGRemainderGroth16Verifier.sol` | ~31,261 lines (~2 MB source) | Exceeds limit |
| `GKRDAGVerifier.sol` | ~895 lines (~38 KB source) | Deployed as library |
| `GKRDAGHybridVerifier.sol` | ~1,167 lines (~49 KB source) | Deployed as library |
| `RemainderGroth16Verifier.sol` | ~1,219 lines (~68 KB source) | Exceeds limit |
| `GKRHybridVerifier.sol` | ~632 lines (~29 KB source) | Deployed as library |
| `GKRVerifier.sol` | ~564 lines (~25 KB source) | Deployed as library |

Contracts under the limit:
- `HyraxVerifier.sol` (~15 KB source)
- `HyraxProofDecoder.sol` (~17 KB source)
- `TEEMLVerifier.sol`
- `ExecutionEngine.sol`

## Current Mitigation Strategies

### 1. Library Architecture

Core verification logic is implemented as Solidity libraries (`library` keyword), which are linked at deployment time rather than being part of the deploying contract's bytecode:

- `GKRVerifier` — GKR protocol verification
- `GKRDAGVerifier` — DAG circuit verification
- `GKRHybridVerifier` — Hybrid Fr verification
- `GKRDAGHybridVerifier` — DAG hybrid verification
- `HyraxVerifier` — Hyrax polynomial commitment
- `HyraxProofDecoder` — Proof binary decoder
- `PoseidonSponge` — Poseidon hash (Fiat-Shamir)
- `SumcheckVerifier` — Sumcheck protocol
- `DAGBatchVerifier` — Multi-tx batch state management

Libraries are deployed separately and linked to `RemainderVerifier` via `forge create --libraries`.

### 2. Multi-Transaction Batch Verification (DAGBatchVerifier)

For the 88-layer XGBoost DAG circuit (~254M gas), verification is split across multiple transactions:

- **`LAYERS_PER_BATCH = 8`**: 88 compute layers / 8 = 11 compute batches
- **`GROUPS_PER_FINALIZE_BATCH = 16`**: 34 eval groups / 16 = 3 finalize txs
- **Total: 1 setup + 11 continue + 3 finalize = 15 transactions**

Gas per transaction: 9–28M (all under 30M block gas limit).

### 3. Stylus WASM Verifier (Arbitrum)

A complete GKR DAG verifier is implemented in Rust/WASM for Arbitrum Stylus:

- **Location**: `contracts/stylus/gkr-verifier/`
- **Size**: 52 KB raw WASM → 24.5 KB Brotli (under 24 KB Stylus limit)
- **Integration**: `RemainderVerifier.verifyDAGProofStylus()` delegates to the WASM contract via `staticcall`

This avoids EIP-170 entirely by running verification in WASM rather than EVM bytecode.

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

On Sepolia testnet, the system runs in **TEE-only mode**:
- `TEEMLVerifier` and `ExecutionEngine` are deployed (both under EIP-170)
- `RemainderVerifier` and its libraries are NOT deployed
- TEE attestation provides trust, ZK verification is not needed on-chain
- This sidesteps the EIP-170 issue entirely for testnet

### Full ZK Mode (Future Mainnet)

For on-chain ZK verification, options include:

1. **Library deployment**: Deploy each library separately, link to RemainderVerifier
2. **Proxy pattern (EIP-1967)**: Split RemainderVerifier behind a UUPS proxy (infrastructure exists in `contracts/src/Upgradeable.sol`)
3. **Diamond pattern (EIP-2535)**: Split into facets for each verification mode
4. **Stylus delegation**: Use WASM verifier on Arbitrum (implemented and tested)

### Recommended Approach

For **Arbitrum/World Chain** (Stylus-capable):
- Deploy `RemainderVerifier` as a thin registry/dispatcher (under EIP-170)
- Use Stylus WASM verifier for actual GKR verification
- Use `DAGBatchVerifier` for multi-tx flow

For **other EVM chains**:
- Deploy libraries separately
- Use proxy pattern to keep RemainderVerifier under limit
- Or use the hybrid Groth16 path (smaller on-chain footprint)

## Size Check Script

`scripts/check-contract-sizes.sh` runs `forge build --sizes` and checks all contracts against the 24 KB limit. It's integrated into `.github/workflows/contracts-ci.yml`.

```bash
cd contracts && bash ../scripts/check-contract-sizes.sh
```

The script notes that `RemainderVerifier` is expected to exceed the limit when deployed via Arbitrum Stylus.

## Contract Upgrade Strategy

RemainderVerifier is **non-upgradeable** (`Ownable2Step`). Changes require:
1. Deploy new contract
2. Re-register all circuits
3. Update references in ExecutionEngine/TEEMLVerifier

See `docs/RUNBOOK.md` Section 6 for the upgrade procedure.
