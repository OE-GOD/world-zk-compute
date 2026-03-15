# Cross-Chain Deployment Guide

This guide covers deploying World ZK Compute contracts across multiple
EVM-compatible chains. It explains which contracts are chain-agnostic,
how to manage deployment addresses, verifier router configuration, and
chain-specific constraints.

## Contract Classification

### Chain-Agnostic Contracts

These contracts deploy identically on any EVM chain (given sufficient
code size and gas limits):

| Contract | Purpose | Size Constraint |
|----------|---------|-----------------|
| `TEEMLVerifier` | TEE attestation + dispute resolution | Standard (<24KB) |
| `ExecutionEngine` | General-purpose verifiable compute | Standard (<24KB) |
| `ProgramRegistry` | Image ID registration | Standard (<24KB) |
| `ProverRegistry` | Prover reputation tracking | Standard (<24KB) |
| `ProverReputation` | Prover scoring | Standard (<24KB) |

These contracts use only standard EVM opcodes and have no chain-specific
logic. They can be deployed on Ethereum L1, Arbitrum, World Chain, or
any other EVM chain.

### Large Contracts (Require Relaxed Code Size Limits)

These contracts exceed the EIP-170 24KB contract code size limit and
can only be deployed on chains with relaxed limits (e.g., Arbitrum,
World Chain, or local Anvil with `--code-size-limit 200000`):

| Contract | Purpose | Approx Size |
|----------|---------|-------------|
| `RemainderVerifier` | GKR+Hyrax on-chain verifier | >24KB |
| `DAGBatchVerifier` | Multi-tx batched verification | >24KB (library) |
| `PoseidonSponge` | Poseidon hash (embedded constants) | >24KB |
| `GKRDAGVerifier` | DAG circuit verification (library) | >24KB |
| `HyraxVerifier` | Hyrax commitment verification | >24KB |
| `SumcheckVerifier` | Sumcheck protocol verification | Large |

These contracts are deployed as libraries or standalone verifiers.
The `RemainderVerifier` is the main entry point that links the others.

### Chain-Specific Contracts

| Contract | Chain | Purpose |
|----------|-------|---------|
| `RiscZeroVerifierAdapter` | Chains with risc0 router | Wraps risc0 verifier router into `IProofVerifier` |
| `RemainderVerifierAdapter` | Chains with RemainderVerifier | Wraps RemainderVerifier into `IProofVerifier` |
| `DAGRemainderGroth16Verifier` | Any (gnark-exported) | Groth16 verifier with embedded VK |
| Stylus GKR Verifier (WASM) | Arbitrum only | WASM port of GKR verifier using Stylus precompiles |

The Stylus verifier is Arbitrum-specific because it relies on Arbitrum
Stylus WASM execution and EVM precompile access via `RawCall`.

## Chain Configuration: `deployments/chains.json`

All supported chains are defined in `deployments/chains.json`. Each
entry contains the configuration needed for deployment and operation.

### Schema

```json
{
  "chains": [
    {
      "name": "chain-name",
      "chainId": 12345,
      "rpcUrl": "https://rpc.example.com",
      "explorerUrl": "https://explorer.example.com",
      "explorerApiKey": "ENV_VAR_NAME_FOR_API_KEY",
      "verifierRouter": "0x<address or empty>",
      "gasMultiplier": 1.0,
      "codeSizeLimit": 24576,
      "gasLimit": 30000000,
      "teeOnly": false,
      "notes": "Human-readable notes about this chain"
    }
  ]
}
```

### Field Reference

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Short identifier used in CLI flags and deployment filenames |
| `chainId` | `number` | EIP-155 chain ID |
| `rpcUrl` | `string` | Default JSON-RPC endpoint (public or env-var placeholder) |
| `explorerUrl` | `string` | Block explorer base URL (empty for localhost) |
| `explorerApiKey` | `string` | Environment variable name holding the explorer API key |
| `verifierRouter` | `string` | RISC Zero verifier router address (empty if not deployed) |
| `gasMultiplier` | `number` | Gas price multiplier for deployment (1.0 = no markup) |
| `codeSizeLimit` | `number` | Max contract bytecode size in bytes |
| `gasLimit` | `number` | Block gas limit (affects which contracts can be deployed) |
| `teeOnly` | `boolean` | If true, only TEE-path contracts can be deployed (no RemainderVerifier) |
| `notes` | `string` | Deployment notes and constraints |

### Current Chain Entries

| Chain | Chain ID | Code Limit | Gas Limit | Verifier Router | TEE-Only |
|-------|----------|------------|-----------|-----------------|----------|
| localhost | 31337 | 200KB | 500M | -- | No |
| sepolia | 11155111 | 24KB | 30M | `0x925d...187` | Yes |
| arbitrum-sepolia | 421614 | 200KB | 500M | -- | No |
| world-chain-sepolia | 4801 | 200KB | 500M | -- | No |

## Verifier Router Addresses

The RISC Zero verifier router is an already-deployed contract that
verifies risc0 zkVM proofs. Each chain may have its own router address.

### Known Router Addresses

| Chain | Address | Selector | Version |
|-------|---------|----------|---------|
| Sepolia | `0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187` | `73c457ba` | v3.0.x |
| Ethereum Mainnet | `0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319` | varies | varies |

If a chain does not have a verifier router, you must either:
1. Deploy a `MockRiscZeroVerifier` (for testing), or
2. Use a custom verifier (RemainderVerifier, eZKL) via `ProgramRegistry`.

The `DeployTestnet.s.sol` script accepts `VERIFIER_ADDRESS` as an
environment variable, allowing you to point to any verifier router.

## Deployment Records

Each deployment produces a JSON file at `deployments/<chain-name>.json`.

### Per-Chain Deployment File Format

```json
{
  "network": "arbitrum-sepolia",
  "chain_id": 421614,
  "deployed_at": "2025-01-15T12:00:00Z",
  "deployer": "0x<deployer address>",
  "block_number": 12345678,
  "contracts": {
    "RemainderVerifier": "0x<address>",
    "TEEMLVerifier": "0x<address>"
  },
  "dagCircuit": {
    "registered": true,
    "circuitHash": "0x<circuit hash>"
  },
  "verified": false
}
```

### Canonical Registry

The `deployments/registry.json` tracks all deployments in a single
file with richer metadata (version, tx hash, block number, cross-contract
wiring).

After each deployment, update the relevant file with the new addresses.
The SDK and integration tests read these files to resolve contract
addresses automatically.

## Address Registration Across Chains

When deploying to multiple chains, certain addresses must be
coordinated:

1. **Enclave keys**: The same TEE enclave key can be registered on
   multiple `TEEMLVerifier` instances across chains. Use the Admin CLI:
   ```bash
   admin-cli register-enclave \
     --rpc-url $RPC_URL \
     --contract $TEE_VERIFIER_ADDRESS \
     --private-key $OWNER_KEY \
     --enclave-key $ENCLAVE_ADDRESS \
     --image-hash $PCR0_HASH
   ```

2. **RemainderVerifier**: Must be wired to `TEEMLVerifier` via
   `setRemainderVerifier()`. This is chain-local -- each chain's
   `TEEMLVerifier` points to its own `RemainderVerifier`.

3. **DAG circuit registration**: Each `RemainderVerifier` needs its
   own `registerDAGCircuit()` call with the circuit description,
   circuit hash, and generators hash.

4. **RISC Zero programs**: Each chain's `ProgramRegistry` needs
   independent `registerProgram()` calls with the image ID.

5. **ExecutionEngine wiring**: Each chain's `ExecutionEngine` is
   constructed with references to its local `ProgramRegistry` and
   verifier address.

There is no cross-chain state sharing. Each chain maintains its own
independent set of registrations.

## How to Add a New Chain

### Checklist

1. **Add chain to `deployments/chains.json`**
   ```json
   {
     "name": "my-new-chain",
     "chainId": 99999,
     "rpcUrl": "https://rpc.mychain.com",
     "explorerUrl": "https://explorer.mychain.com",
     "explorerApiKey": "MYCHAIN_API_KEY",
     "verifierRouter": "",
     "gasMultiplier": 1.0,
     "codeSizeLimit": 24576,
     "gasLimit": 30000000,
     "notes": "Description of chain constraints"
   }
   ```

2. **Determine deployment scope**
   - If `codeSizeLimit < 50000`: Set `"teeOnly": true` and deploy
     only `TEEMLVerifier` + `ExecutionEngine` + `ProgramRegistry`.
   - If `codeSizeLimit >= 200000` and `gasLimit >= 500000000`: Full
     stack including `RemainderVerifier` and DAG circuit registration.
   - If `codeSizeLimit >= 200000` but `gasLimit < 500000000`: Deploy
     `RemainderVerifier` but use batch verification (15 txs) or
     Groth16 hybrid mode instead of single-tx verification.

3. **Check for a RISC Zero verifier router**
   - Look up the chain on the RISC Zero documentation or contract
     registry.
   - If available, add the address to `verifierRouter` in chains.json.
   - If not available, deploy `MockRiscZeroVerifier` (testing) or use
     a custom verifier adapter.

4. **Fund the deployer wallet**
   - Ensure the deployer has sufficient native tokens for deployment
     gas (~0.06 ETH equivalent for TEE-only, more for full stack).

5. **Deploy contracts**
   ```bash
   DEPLOYER_PRIVATE_KEY=0x... \
     bash scripts/deploy-multichain.sh --chain my-new-chain
   ```
   Or for dry-run first:
   ```bash
   DEPLOYER_PRIVATE_KEY=0x... \
     bash scripts/deploy-multichain.sh --chain my-new-chain --dry-run
   ```

6. **Verify contracts** (if explorer API key is available)
   ```bash
   DEPLOYER_PRIVATE_KEY=0x... MYCHAIN_API_KEY=xxx \
     bash scripts/deploy-multichain.sh --chain my-new-chain --verify
   ```

7. **Register enclave keys** (if using TEE path)
   ```bash
   admin-cli register-enclave \
     --rpc-url $RPC_URL \
     --contract $TEE_VERIFIER_ADDRESS \
     --private-key $OWNER_KEY \
     --enclave-key $ENCLAVE_ADDRESS \
     --image-hash $PCR0_HASH
   ```

8. **Update deployment records**
   - The deploy script auto-creates `deployments/my-new-chain.json`.
   - Optionally update `deployments/registry.json` with full metadata.

9. **Configure services**
   - Set `RPC_URL`, `CONTRACT_ADDRESS`, and `CHAIN_ID` in operator
     and indexer environment variables.
   - Update Docker Compose or Helm values for the new chain.

10. **Run E2E validation**
    ```bash
    RPC_URL=... CONTRACT_ADDRESS=... ./scripts/e2e-test.sh --network custom
    ```

## Chain-Specific Considerations

### Ethereum Mainnet / Sepolia

- **Code size limit**: 24KB (EIP-170). `RemainderVerifier` and its
  libraries exceed this limit. Only TEE-path contracts are deployable.
- **Gas limit**: 30M per block. Full GKR DAG verification (~254M gas)
  is impossible in a single transaction. Use batch verification
  (15 txs, 13-28M each) if ZK dispute resolution is needed on L1.
- **Verifier router**: Available at known addresses (see table above).
- **Block time**: ~12s. Set `POLL_INTERVAL_SECS=12` for the indexer.

### Arbitrum (Sepolia / One)

- **Code size limit**: 200KB+ (relaxed). All contracts deployable.
- **Gas limit**: 500M+ (Arbitrum uses ArbGas). Single-tx GKR
  verification is feasible but expensive (~254M gas).
- **Stylus support**: Arbitrum Stylus enables WASM execution.
  The Stylus GKR verifier (`contracts/stylus/gkr-verifier/`) can be
  deployed for cheaper verification via EVM precompiles. Set with
  `RemainderVerifier.setDAGStylusVerifier(circuitHash, stylusAddr)`.
- **Stylus deployment**: Build the WASM binary, deploy via
  `cargo stylus deploy`, then register in `RemainderVerifier`:
  ```bash
  cd contracts/stylus/gkr-verifier
  cargo stylus deploy --private-key $KEY --rpc-url $RPC
  ```
- **L1 data costs**: Calldata on Arbitrum is priced by L1 data
  posting costs. Large proofs (~400KB) cost significant L1 fees.
  Consider Groth16 hybrid mode for smaller proof sizes.

### World Chain (Sepolia / Mainnet)

- **Code size limit**: 200KB+ (OP Stack based). All contracts deployable.
- **Gas limit**: 500M+. Full ZK verification possible.
- **No Stylus**: World Chain is OP Stack based, not Orbit, so Stylus
  WASM is not available. Use the Solidity GKR verifier or Groth16 hybrid.
- **Gas pricing**: OP Stack uses L1 data costs similar to Arbitrum.
  Monitor L1 gas prices for proof submission costs.

### Local Development (Anvil)

- **Code size limit**: Configurable. Use `anvil --code-size-limit 200000`.
- **Gas limit**: Configurable. Use `anvil --gas-limit 500000000`.
- **No verifier router**: Deploy `MockRiscZeroVerifier` for risc0 testing.
- **Fast blocks**: Default 1s block time. Set `POLL_INTERVAL_SECS=1`.
- **Deployment**: Use `scripts/deploy-multichain.sh --chain localhost`
  or the Docker Compose stack.

## Deployment Scripts Reference

| Script | Purpose |
|--------|---------|
| `scripts/deploy-multichain.sh` | Deploy to one or all chains from chains.json |
| `scripts/deploy-sepolia.sh` | Deploy ExecutionEngine to Ethereum Sepolia |
| `scripts/deploy-sepolia-tee.sh` | Deploy TEEMLVerifier + register enclave on Sepolia |
| `scripts/deploy.sh` | General-purpose deployment helper |
| `contracts/script/DeployAll.s.sol` | Forge script: deploy full stack |
| `contracts/script/DeployTestnet.s.sol` | Forge script: deploy with real verifier router |
| `contracts/script/DeployLocal.s.sol` | Forge script: deploy with mock verifier |
| `contracts/script/DeployTEEMLVerifier.s.sol` | Forge script: deploy TEEMLVerifier only |
| `contracts/script/DeployRemainder.s.sol` | Forge script: deploy RemainderVerifier only |
| `contracts/script/DeployRemainderDAG.s.sol` | Forge script: deploy + register DAG circuit |
| `contracts/script/StylusSepoliaDeploy.s.sol` | Forge script: deploy + register Stylus verifier |

## Gas Cost Comparison Across Chains

| Operation | Ethereum L1 | Arbitrum | World Chain |
|-----------|-------------|----------|-------------|
| `submitResult()` | ~50K gas | ~50K gas | ~50K gas |
| `challenge()` | ~50K gas | ~50K gas | ~50K gas |
| `finalize()` | ~30K gas | ~30K gas | ~30K gas |
| Full GKR verify | N/A (>30M) | ~254M gas | ~254M gas |
| Batch verify (15 txs) | ~13-28M each | ~13-28M each | ~13-28M each |
| Groth16 hybrid | ~51M gas | ~51M gas | ~51M gas |

On L2 chains (Arbitrum, World Chain), the actual cost in ETH also
includes L1 data posting fees proportional to the calldata size.
Groth16 proofs are much smaller than full GKR proofs (~300 bytes vs
~400KB), so Groth16 hybrid mode is strongly preferred for production
L2 deployments.
