# Python SDK Quickstart

Demonstrates how to interact with the World ZK Compute TEEMLVerifier
contract from Python using `web3.py` and `eth-account`.

## Prerequisites

- Python 3.9+
- A running Ethereum node (Anvil for local development)
- TEEMLVerifier contract deployed (see [deploy script](../../scripts/deploy.sh))

## Setup

```bash
cd examples/sdk-python-quickstart
pip install -r requirements.txt
```

Dependencies: `web3>=6.0.0`, `eth-account>=0.9.0`

## Usage

Start a local Anvil node, deploy contracts, then run the example:

```bash
# Terminal 1: start Anvil
anvil --block-time 1

# Terminal 2: deploy contracts and run the quickstart
cd examples/sdk-python-quickstart
python main.py
```

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0x5FbDB2315678afecb367f032d93F642f64180aa3 \
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
python main.py
```

## What it does

1. **Connects** to the RPC endpoint and reads the required prover stake.
2. **Submits** a TEE inference result via `submitResult()` with model hash,
   input hash, result, enclave image hash, and attestation bytes.
3. **Computes** the result ID as `keccak256(modelHash, inputHash)`.
4. **Queries** `isResultValid()` to check the on-chain result status.

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

- [Python SDK](../../sdk/python/) -- full-featured Python SDK
- [Main project](../../README.md)
