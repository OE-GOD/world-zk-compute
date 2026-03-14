# World ZK Compute -- Deployed Contract Addresses

This document lists known contract deployments across supported networks.
Canonical deployment data is maintained in `deployments/registry.json` and
`deployments/chains.json`.

---

## Sepolia (chainId: 11155111)

Ethereum Sepolia testnet. Subject to the EIP-170 code size limit (24,576 bytes),
which prevents deploying `RemainderVerifier` and related large contracts. Only
TEE-path contracts are deployable here.

| Contract              | Address                                      |
|-----------------------|----------------------------------------------|
| MockRiscZeroVerifier  | `0x0D194f172a3a50e0E293d0d8f21774b1a222362E` |
| ProgramRegistry       | `0x7F9EFc73E50a4f6ec6Ab7B464f6556a89fDeD3ac` |
| ExecutionEngine       | `0x9CFd1CF0e263420e010013373Ec4008d341a483e` |
| RISC Zero Verifier Router | `0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187` |

**Block explorer:** <https://sepolia.etherscan.io>

**Notes:**
- The RISC Zero Verifier Router is a shared contract deployed by the risc0 team.
  It uses selector `73c457ba` (risc0 v3.0.x). The v1.2.x selector `c101b42b` is
  tombstoned.
- EIP-170 prevents deploying `RemainderVerifier` (exceeds 24KB). Use Arbitrum
  Sepolia for GKR/remainder verification.

---

## Localhost / Anvil (chainId: 31337)

Local development network. Contracts are deployed fresh on each `anvil` start
using `DeployAll.s.sol` or `DeployFullStack.s.sol`.

```bash
anvil --gas-limit 500000000 --code-size-limit 200000
```

Deterministic addresses from the default Anvil mnemonic (deployer nonce 0, 1, 2):

| Contract              | Address                                      |
|-----------------------|----------------------------------------------|
| MockRiscZeroVerifier  | `0x5FbDB2315678afecb367f032d93F642f64180aa3` |
| ProgramRegistry       | `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512` |
| ExecutionEngine       | `0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0` |

These addresses are only valid when deploying from the first Anvil account
(`0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`) with a fresh nonce.

**Notes:**
- Supports `--code-size-limit 200000` for deploying `RemainderVerifier` and
  other large contracts.
- Gas limit raised to 500M for GKR DAG verification tests.

---

## Arbitrum Sepolia (chainId: 421614)

Primary deployment target for the full system including `RemainderVerifier`.
Arbitrum Sepolia supports large contracts (200KB+ code size) and high gas limits.

| Contract              | Address                                      |
|-----------------------|----------------------------------------------|
| (no deployments yet)  |                                              |

**Block explorer:** <https://sepolia.arbiscan.io>

**RPC:** `https://sepolia-rollup.arbitrum.io/rpc`

**Notes:**
- Supports code size up to 200,000 bytes (no EIP-170 limit).
- Gas limit up to 500M, sufficient for GKR DAG batch verification.
- This is the recommended network for testing the full GKR + Groth16 hybrid
  verification pipeline.

---

## World Chain Sepolia (chainId: 4801)

World Chain Sepolia testnet. Target for production deployment.

| Contract              | Address                                      |
|-----------------------|----------------------------------------------|
| (no deployments yet)  |                                              |

**Block explorer:** <https://sepolia.worldscan.org>

**RPC:** `https://worldchain-sepolia.g.alchemy.com/public`

---

## Deployment Workflow

### Deploy to a new chain

```bash
# 1. Add chain config to deployments/chains.json
# 2. Set environment variables
export RPC_URL="https://..."
export PRIVATE_KEY="0x..."

# 3. Deploy
cd contracts
forge script script/DeployFullStack.s.sol \
  --rpc-url "$RPC_URL" \
  --private-key "$PRIVATE_KEY" \
  --broadcast

# 4. Verify deployment
./scripts/verify-deployment.sh --chain-id <CHAIN_ID> --rpc-url "$RPC_URL"
```

### Verify an existing deployment

```bash
./scripts/verify-deployment.sh --chain-id 11155111
```

This checks bytecode presence, ownership, pause state, and contract wiring.

---

## Files

| File                          | Description                          |
|-------------------------------|--------------------------------------|
| `deployments/chains.json`    | Chain configs (RPC, gas, limits)     |
| `deployments/registry.json`  | Canonical contract address registry  |
| `contracts/script/DeployAll.s.sol`       | Basic deployment script   |
| `contracts/script/DeployFullStack.s.sol` | Full stack deployment     |
| `scripts/verify-deployment.sh`           | Post-deploy health check  |
