# TypeScript SDK Quickstart

Demonstrates how to interact with the World ZK Compute TEEMLVerifier
contract from TypeScript using [viem](https://viem.sh/).

## Prerequisites

- Node.js 18+
- A running Ethereum node (Anvil for local development)
- TEEMLVerifier contract deployed (see [deploy script](../../scripts/deploy.sh))

## Setup

```bash
cd examples/sdk-typescript-quickstart
npm install
```

Dependencies: `viem ^2.0.0` (runtime), `tsx ^4.0.0` and
`typescript ^5.0.0` (dev).

## Usage

Start a local Anvil node, deploy contracts, then run the example:

```bash
# Terminal 1: start Anvil
anvil --block-time 1

# Terminal 2: run the quickstart
cd examples/sdk-typescript-quickstart
npx tsx index.ts
# or
npm start
```

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0x5FbDB2315678afecb367f032d93F642f64180aa3 \
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
npx tsx index.ts
```

## What it does

1. **Connects** to the RPC endpoint using viem's `createPublicClient` and
   `createWalletClient` (configured for the Anvil chain).
2. **Reads** the required `proverStake()` from the contract.
3. **Submits** a TEE inference result via `submitResult()` with the
   required stake value attached.
4. **Computes** the result ID as
   `keccak256(encodePacked(modelHash, inputHash))`.
5. **Queries** `isResultValid()` to check the on-chain result status.

After submission, the result enters a challenge window. Call
`finalizeResult(resultId)` once the window expires to mark it as
finalized.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `RPC_URL` | `http://127.0.0.1:8545` | Ethereum JSON-RPC endpoint |
| `CONTRACT_ADDRESS` | `0x5FbDB...` | Deployed TEEMLVerifier address |
| `PRIVATE_KEY` | Anvil account #0 | Sender private key |

## Related

- [TypeScript SDK](../../sdk/typescript/) -- full-featured TypeScript SDK
- [Main project](../../README.md)
