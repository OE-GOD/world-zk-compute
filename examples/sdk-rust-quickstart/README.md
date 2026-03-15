# Rust SDK Quickstart

## Overview

Demonstrates how to interact with the World ZK Compute TEEMLVerifier contract from Rust
using the alloy library. Submits a TEE-attested ML inference result and queries its
on-chain validity.

## Prerequisites

- Rust 1.75+
- A running Ethereum node (Anvil for local development)
- TEEMLVerifier contract deployed

## Run

Start a local Anvil node and run the example:

```bash
anvil --block-time 1 &
cargo run
```

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0x5FbDB... \
PRIVATE_KEY=0xac09... \
cargo run
```

## What It Does

1. **Connects** to the RPC endpoint using alloy's `ProviderBuilder` with a wallet signer
2. **Reads** the required `proverStake()` from the contract
3. **Submits** a TEE inference result via `submitResult()` with the required stake value
4. **Computes** the result ID as `keccak256(modelHash ++ inputHash)`
5. **Queries** `isResultValid()` to check the result status

The contract ABI is defined inline using alloy's `sol!` macro, which generates
type-safe bindings at compile time. After submission, the result enters a challenge
window. Call `finalizeResult(resultId)` once the window expires.
