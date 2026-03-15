# Python SDK Quickstart

## Overview

Demonstrates how to interact with the World ZK Compute TEEMLVerifier contract from Python.
Submits a TEE-attested ML inference result, queries its validity, and shows the
optimistic finalization flow.

## Prerequisites

- Python 3.9+
- A running Ethereum node (Anvil for local development)
- TEEMLVerifier contract deployed

Install dependencies:

```bash
pip install -r requirements.txt
```

## Run

Start a local Anvil node and run the example:

```bash
anvil --block-time 1 &
python main.py
```

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0x5FbDB... \
PRIVATE_KEY=0xac09... \
python main.py
```

## What It Does

1. **Connects** to the RPC endpoint and reads the required prover stake
2. **Submits** a TEE inference result (`submitResult`) with model hash, input hash,
   result, enclave image hash, and attestation bytes
3. **Computes** the result ID as `keccak256(modelHash, inputHash)`
4. **Queries** `isResultValid()` to check the result status

After submission, the result enters a challenge window. Call `finalizeResult(resultId)`
once the window expires to mark it as finalized.
