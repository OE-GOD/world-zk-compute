# Python SDK Quickstart

Submits one TEE-attested result to a `TEEMLVerifier` contract on a local
Anvil chain with the Python SDK (`worldzk`), then reads it back.

Read the status note at the top of the [repository README](../../README.md)
before relying on what the contract reports as "valid".

## Prerequisites

- Python 3.9+
- [Foundry](https://book.getfoundry.sh/) (`anvil`, `forge`)

## Setup

The `worldzk` package is not published on PyPI; `requirements.txt` installs
it from this checkout together with `web3` and `eth-account`.

```bash
cd examples/sdk-python-quickstart
pip install -r requirements.txt
```

## Usage

```bash
# Terminal 1: start Anvil
anvil

# Terminal 2: deploy TEEMLVerifier behind its UUPS proxy
cd contracts
DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
REMAINDER_VERIFIER=0xDDdDddDdDdddDDddDDddDDDDdDdDDdDDdDDDDDDd \
forge script script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
    --rpc-url http://127.0.0.1:8545 --broadcast

# Then run the quickstart
cd ../examples/sdk-python-quickstart
python main.py
```

The deploy script prints `TEEMLVerifier deployed at: <address>`. On a fresh
Anvil chain that is `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512`, the default
`CONTRACT_ADDRESS`; pass the printed address otherwise. `REMAINDER_VERIFIER`
is a placeholder: the contract requires a non-zero address, and this example
never resolves a dispute with a proof.

Override defaults via environment variables:

```bash
RPC_URL=http://localhost:8545 \
CONTRACT_ADDRESS=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
python main.py
```

## What it does

1. **Connects** to the RPC endpoint as the contract admin (the deployer).
2. **Registers** an enclave signing key with `registerEnclave()`. A freshly
   generated local key stands in for a real enclave.
3. **Signs** an EIP-712 attestation over the model hash, input hash and
   result hash with that key. `submitResult()` rejects anything that is not
   signed by a registered enclave key.
4. **Submits** the result with the required prover stake and prints the
   result ID from the `ResultSubmitted` event.
5. **Queries** `getResult()` and `isResultValid()` (false until finalized).
6. **Optionally** reads health, results and stats from the TEE indexer when
   one is running at `INDEXER_URL`; otherwise this step is skipped.

After submission, the result enters a one-hour challenge window. Call
`tee.finalize(result_id)` once the window has passed to mark it as finalized.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `RPC_URL` | `http://127.0.0.1:8545` | Ethereum JSON-RPC endpoint |
| `CONTRACT_ADDRESS` | `0xe7f17...0512` | Deployed TEEMLVerifier proxy address |
| `PRIVATE_KEY` | Anvil account #0 | Sender private key (must be the contract admin) |
| `CHAIN_ID` | `31337` | Chain ID used in the EIP-712 domain |
| `INDEXER_URL` | `http://127.0.0.1:8081` | TEE indexer (optional) |

## Related

- [Python SDK](../../sdk/python/) -- full-featured Python SDK
- [Main project](../../README.md)
