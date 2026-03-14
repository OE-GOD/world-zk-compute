# Running Examples on Sepolia

This guide explains how to run the World ZK Compute example projects against
the Sepolia testnet. For the complete deployment and operations walkthrough, see
the [Sepolia Quickstart](sepolia-quickstart/README.md).

## Prerequisites

- Foundry (`forge`, `cast`) installed via `foundryup`
- An Alchemy (or other provider) Sepolia RPC URL
- Sepolia testnet ETH on the deployer wallet (at least 0.1 ETH)
- Contracts deployed to Sepolia (see the quickstart guide)

## Environment Variables

Set these before running any example:

```bash
# Required -- Sepolia JSON-RPC endpoint
export ALCHEMY_SEPOLIA_RPC_URL="https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY"

# Required -- deployer/admin wallet private key (must hold Sepolia ETH)
export DEPLOYER_PRIVATE_KEY="0x..."

# Set after deployment (or loaded from deployments/11155111.json)
export TEE_VERIFIER_ADDRESS="0x..."
export EXECUTION_ENGINE_ADDRESS="0x..."
```

Never commit private keys to version control. Use a local `.env` file or a
secrets manager.

## Example Projects

| Example | Sepolia Support | Notes |
|---------|:-:|-------|
| `rule-engine/` | Yes | Fully supported. The default E2E guest program. |
| `anomaly-detector/` | Yes | Proving works locally; submit proofs on-chain via the operator. |
| `signature-verified/` | Yes | Uses ECDSA precompile; works with local proving. |
| `sybil-detector/` | Yes | Uses SHA-256 precompile; works with local proving. |
| `xgboost-inference/` | Yes | XGBoost model inference inside the zkVM. |
| `xgboost-remainder/` | Partial | GKR/Hyrax direct verification exceeds Sepolia's 30M gas block limit. TEE path works. Groth16-wrapped proofs work within gas limits. |
| `xgboost-ezkl/` | No | Requires EZKL toolchain; not integrated with on-chain contracts. |
| `anomaly-detector-sp1/` | No | SP1-based; separate proving infrastructure. |
| `recursive-wrapper/` | No | Internal proving utility, not an end-user example. |
| `sdk-python-quickstart/` | Yes | Python SDK usage example. |
| `sdk-rust-quickstart/` | Yes | Rust SDK usage example. |
| `sdk-typescript-quickstart/` | Yes | TypeScript SDK usage example. |

## Running an Example on Sepolia

### 1. Deploy contracts (if not already deployed)

```bash
./scripts/deploy-sepolia-tee.sh
```

This writes deployment addresses to `deployments/11155111.json`.

### 2. Run the E2E test for a specific example

```bash
./scripts/e2e-test.sh --example rule-engine --network sepolia
```

### 3. Submit a request manually

```bash
./scripts/sepolia-submit-request.sh \
  --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
  --input-hash 0x3333333333333333333333333333333333333333333333333333333333333333 \
  --value 0.001ether
```

## Gas Costs

Typical gas costs on Sepolia (also representative of mainnet):

| Operation | Approximate Gas | Approximate Cost at 10 gwei |
|-----------|----------------:|----------------------------:|
| Deploy TEEMLVerifier | ~3,000,000 | 0.03 ETH |
| Deploy ExecutionEngine | ~2,500,000 | 0.025 ETH |
| Register enclave | ~60,000 | 0.0006 ETH |
| Submit result | ~150,000 | 0.0015 ETH |
| Challenge | ~80,000 | 0.0008 ETH |
| Finalize | ~50,000 | 0.0005 ETH |
| Groth16 verify | ~300,000 | 0.003 ETH |

Gas prices fluctuate. Check current prices:

```bash
cast gas-price --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

## Code Size Limitations

Sepolia enforces EIP-170, which limits deployed contract bytecode to 24,576
bytes. Some contracts in this project exceed that limit:

- **RemainderVerifier** (GKR + Hyrax on-chain verification) exceeds the limit.
  For full on-chain GKR verification, deploy to Arbitrum Sepolia instead, which
  allows up to 200 KB of contract bytecode.
- **DAGBatchVerifier** with all libraries linked also exceeds the limit on
  Sepolia.

The TEEMLVerifier and ExecutionEngine contracts are well within the limit and
deploy without issues on Sepolia.

For chains with larger code size limits:

```bash
./scripts/deploy-multichain.sh --chain arbitrum-sepolia
```

## Troubleshooting

Refer to the [Sepolia Quickstart troubleshooting section](sepolia-quickstart/README.md#troubleshooting)
for common issues including insufficient funds, nonce errors, RPC rate limiting,
and transaction debugging.
