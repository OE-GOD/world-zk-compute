# Running Examples on Sepolia

This guide explains how to run the World ZK Compute example projects against
the Sepolia testnet. For the complete deployment and operations walkthrough, see
the [Sepolia Quickstart](sepolia-quickstart/README.md).

## Prerequisites

- Foundry (`forge`, `cast`) installed via `foundryup`
- An Alchemy (or other provider) Sepolia RPC URL
- Sepolia testnet ETH on the deployer wallet (more than 0.1 ETH: `sepolia-e2e.sh` sends the
  0.1 ETH prover stake from it, on top of gas)
- Contracts deployed to Sepolia (see the quickstart guide)

## Environment Variables

Set these before running any example:

```bash
# Required -- Sepolia JSON-RPC endpoint
export ALCHEMY_SEPOLIA_RPC_URL="https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY"

# Required -- deployer/admin wallet private key (must hold Sepolia ETH)
export DEPLOYER_PRIVATE_KEY="0x..."

# Required by deploy-sepolia-tee.sh -- private key of the TEE enclave signer it registers
export ENCLAVE_PRIVATE_KEY="0x..."

# Required by deploy-sepolia-tee.sh -- address TEEMLVerifier calls to resolve
# disputes (TEEMLVerifier.initialize rejects address(0))
export REMAINDER_VERIFIER="0x..."

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
| `xgboost-remainder/` | Partial | The on-chain verifier (RemainderVerifier, 135,888 bytes) and the Groth16 wrapper verifier (DAGGroth16Verifier, 461,497 bytes) exceed Sepolia's 24,576-byte contract size limit and cannot be deployed there. Only the TEE path (TEEMLVerifier) can be deployed. |
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

### 2. Run the E2E test

The per-example test, `./scripts/e2e-test.sh --example rule-engine --network sepolia`, does not
run on Sepolia: it reads `SEPOLIA_RPC_URL`, `PRIVATE_KEY`, `PROVER_PRIVATE_KEY` and
`REQUESTER_PRIVATE_KEY` instead of the variables above, and then calls
`forge script script/DeployTestnet.s.sol:DeployTestnetScript`, a contract name that does not
exist (`Error: Could not find target contract`).

The TEE lifecycle against the contracts from step 1 is exercised by:

```bash
./scripts/sepolia-e2e.sh
```

### 3. Submit a request manually

```bash
./scripts/sepolia-submit-request.sh \
  --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
  --input-hash 0x3333333333333333333333333333333333333333333333333333333333333333 \
  --value 0.001ether
```

## Gas Costs

Gas used by each operation, measured on a local Anvil chain under the Osaka gas rules
(`anvil --hardfork osaka`), not on Sepolia itself. The Groth16 row was not re-measured:

| Operation | Approximate Gas | Approximate Cost at 10 gwei |
|-----------|----------------:|----------------------------:|
| Deploy TEEMLVerifier (implementation 3,439,885 + proxy 319,604) | ~3,760,000 | 0.038 ETH |
| Deploy ExecutionEngine | ~2,610,000 | 0.026 ETH |
| Register enclave | ~75,000 | 0.00075 ETH |
| Submit result | ~250,000 | 0.0025 ETH |
| Challenge | ~92,000 | 0.00092 ETH |
| Finalize | ~54,000 | 0.00054 ETH |
| Groth16 verify | ~300,000 | 0.003 ETH |

Gas prices fluctuate. Check current prices:

```bash
cast gas-price --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

## Code Size Limitations

Sepolia enforces EIP-170, which limits deployed contract bytecode to 24,576
bytes. Some contracts in this project exceed that limit:

- **RemainderVerifier** (GKR + Hyrax on-chain verification) is 135,888 bytes and
  cannot be deployed. Arbitrum Sepolia is not an alternative: it enforces the same
  24,576-byte limit, as do Arbitrum One, World Chain, World Chain Sepolia and
  Ethereum mainnet.
- **DAGBatchVerifier** is an internal library compiled into RemainderVerifier, so
  it is part of those 135,888 bytes, not a separate deployment.

The TEEMLVerifier and ExecutionEngine contracts are well within the limit and
deploy without issues on Sepolia.

`deployments/chains.json` marks `sepolia`, `arbitrum-sepolia` and `world-chain-sepolia` as
`teeOnly`. To deploy the TEE path to Arbitrum Sepolia, use `deploy.sh`
(`deploy-multichain.sh` runs `DeployAll.s.sol`, which contains RemainderVerifier, and fails
with `create initcode size limit`). `deploy.sh` also deploys ProgramRegistry and ExecutionEngine
there, with a `MockRiscZeroVerifier` behind the ExecutionEngine, because
`deployments/chains.json` has no `verifierRouter` for this chain:

```bash
REMAINDER_VERIFIER=0x... DEPLOYER_PRIVATE_KEY=0x... \
  ./scripts/deploy.sh --chain arbitrum-sepolia
```

## Troubleshooting

Refer to the [Sepolia Quickstart troubleshooting section](sepolia-quickstart/README.md#troubleshooting)
for common issues including insufficient funds, nonce errors, RPC rate limiting,
and transaction debugging.
