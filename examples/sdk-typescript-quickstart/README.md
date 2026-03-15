# TypeScript SDK Quickstart

## Overview

Demonstrates how to interact with the World ZK Compute TEEMLVerifier contract from
TypeScript using the viem library. Submits a TEE-attested ML inference result and
queries its on-chain validity.

## Prerequisites

- Node.js 18+
- A running Ethereum node (Anvil for local development)
- TEEMLVerifier contract deployed

Install dependencies:

```bash
npm install
```

## Run

Start a local Anvil node and run the example:

```bash
anvil --block-time 1 &
npx tsx index.ts
```

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0x5FbDB... \
PRIVATE_KEY=0xac09... \
npx tsx index.ts
```

## What It Does

1. **Connects** to the RPC endpoint using viem's `createPublicClient` and
   `createWalletClient` (configured for the Anvil chain)
2. **Reads** the required `proverStake()` from the contract
3. **Submits** a TEE inference result via `submitResult()` with the required stake
4. **Computes** the result ID as `keccak256(abi.encodePacked(modelHash, inputHash))`
5. **Queries** `isResultValid()` to check the result status

After submission, the result enters a challenge window. Call `finalizeResult(resultId)`
once the window expires to mark it as finalized.
