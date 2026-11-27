# TypeScript SDK Quickstart

Demonstrates how to interact with the World ZK Compute TEEMLVerifier
contract from TypeScript using the SDK in [`sdk/typescript`](../../sdk/typescript/)
and [viem](https://viem.sh/).

## Prerequisites

- Node.js 18+
- Foundry (`anvil`, `forge`) for the local chain and the deployment
- TEEMLVerifier deployed behind its UUPS proxy (see Usage)

## Setup

The example uses the SDK from this repository (`file:../../sdk/typescript`),
so build it first:

```bash
cd sdk/typescript
npm ci
npm run build

cd ../../examples/sdk-typescript-quickstart
npm install
```

Dependencies: the SDK and `viem ^2.0.0` (runtime), `tsx ^4.0.0`,
`typescript ^5.0.0` and `@types/node` (dev).

## Usage

Start a local Anvil node, deploy the contract, then run the example:

```bash
# Terminal 1: start Anvil
anvil --block-time 1

# Terminal 2: deploy TEEMLVerifier (implementation + proxy).
# initialize() rejects address(0) for the RemainderVerifier; the quickstart never
# resolves a ZK dispute, so a non-zero placeholder is enough.
cd contracts
DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
REMAINDER_VERIFIER=0xDDdDddDdDdddDDddDDddDDDDdDdDDdDDdDDDDDDd \
forge script script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
  --rpc-url http://127.0.0.1:8545 --broadcast

# Terminal 2: run the quickstart
cd ../examples/sdk-typescript-quickstart
npx tsx index.ts
# or
npm start
```

The deploy script prints `TEEMLVerifier deployed at: <proxy address>`. On a fresh
Anvil this is the default `CONTRACT_ADDRESS`; the address of the implementation
contract (deployed one transaction earlier) must not be used.

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
npx tsx index.ts
```

## What it does

1. **Connects** to the RPC endpoint with the SDK's `TEEVerifier` and a viem
   `createPublicClient`.
2. **Hashes** the model bytes and the feature vector with `computeModelHash()`
   and `computeInputHash()`.
3. **Reads** the required `proverStake()` from the contract.
4. **Registers** the enclave signing key via `registerEnclave()` if it is not
   registered yet (admin only).
5. **Signs** the attestation as EIP-712 typed data
   `TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)`
   under the domain `TEEMLVerifier` / `1` / chain ID / contract address.
6. **Submits** the result via `submitResult()` with the required stake
   value attached.
7. **Computes** the result ID as
   `keccak256(encodePacked(submitter, modelHash, inputHash, blockNumber))`.
8. **Queries** `getResult()` and `isResultValid()` to check the on-chain
   result status.

After submission, the result enters a challenge window. Call
`tee.finalize(resultId)` once the window expires to mark it as
finalized.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `RPC_URL` | `http://127.0.0.1:8545` | Ethereum JSON-RPC endpoint |
| `CONTRACT_ADDRESS` | `0xe7f17...` | Deployed TEEMLVerifier proxy address |
| `PRIVATE_KEY` | Anvil account #0 | Contract admin and result submitter |
| `ENCLAVE_KEY` | Anvil account #1 | Key that signs the attestation in place of a TEE enclave |
| `INDEXER_URL` | `http://127.0.0.1:8081` | Indexer queried for `/health` (optional) |

## Related

- [TypeScript SDK](../../sdk/typescript/) -- full-featured TypeScript SDK
- [Main project](../../README.md)
