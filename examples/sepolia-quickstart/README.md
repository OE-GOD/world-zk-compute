# Sepolia Quickstart

End-to-end guide for deploying and running World ZK Compute on the Sepolia testnet.

## Prerequisites

| Tool | Install |
|------|---------|
| Foundry (forge, cast) | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| jq | `brew install jq` or `apt-get install jq` |
| Alchemy (or other) Sepolia RPC key | https://www.alchemy.com/ |
| Sepolia testnet ETH | https://sepoliafaucet.com/ or https://www.alchemy.com/faucets/ethereum-sepolia |

You need at least **0.1 ETH** on the deployer wallet (deployment + gas for registration and execution requests).

## 1. Environment Setup

Copy the example environment file and fill in your values:

```bash
cd world-zk-compute

# Create a local .env file (not committed to git)
cat > .env.sepolia <<'EOF'
# Required: Alchemy (or other provider) Sepolia RPC URL
ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/YOUR_API_KEY

# Required: Deployer wallet private key (must have Sepolia ETH)
DEPLOYER_PRIVATE_KEY=0x...

# Required: TEE enclave signer private key
ENCLAVE_PRIVATE_KEY=0x...

# Optional: separate requester key (defaults to deployer)
# REQUESTER_PRIVATE_KEY=0x...
EOF

# Load environment
source .env.sepolia
```

> **Security Note**: Never commit private keys to version control. Use a `.env` file
> or a secrets manager. The deployer key becomes the contract admin.

## 2. Deploy Contracts

Deploy the TEEMLVerifier and ExecutionEngine contracts to Sepolia:

```bash
./scripts/deploy-sepolia-tee.sh
```

This script will:
- Deploy TEEMLVerifier and ExecutionEngine contracts
- Register the enclave signer address
- Fund requester/prover wallets if needed
- Write deployment addresses to `deployments/11155111.json`

To also verify contracts on Etherscan:

```bash
ETHERSCAN_API_KEY="<your-etherscan-api-key>" \
  ./scripts/deploy.sh --chain sepolia --verify
```

### Dry Run

To simulate deployment without broadcasting transactions:

```bash
./scripts/deploy.sh --chain sepolia --dry-run
```

If you already have deployed contracts, skip this step and set the addresses manually:

```bash
export TEE_VERIFIER_ADDRESS=0x...
export EXECUTION_ENGINE_ADDRESS=0x...
```

## 3. Check Deployment Status

Verify that the contracts are deployed and the enclave signer is registered:

```bash
./scripts/sepolia-status.sh
```

Or check balances:

```bash
./scripts/check-sepolia-balances.sh
```

## 4. Register a Program

Register the program image ID in the on-chain ProgramRegistry:

```bash
./scripts/sepolia-register-program.sh \
  --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
  --name "xgboost-rule-engine"
```

Or register manually with `cast`:

```bash
# Compute the program hash (keccak256 of the model JSON)
PROGRAM_HASH=$(cast keccak "$(cat path/to/model.json)")

cast send $EXECUTION_ENGINE_ADDRESS \
  "registerProgram(bytes32)" \
  "$PROGRAM_HASH" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --private-key "$DEPLOYER_PRIVATE_KEY"
```

The script checks if the program is already registered and verifies after registration.

### Upload Private Inputs (Optional)

If your computation requires private inputs, upload them to the input server first:

```bash
./scripts/upload-private-input.sh \
  --request-id 0x<request-id> \
  ./my-input.json
```

## 5. Submit an Execution Request

Submit a TEE inference request on-chain:

```bash
./scripts/sepolia-submit-request.sh \
  --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
  --input-hash 0x3333333333333333333333333333333333333333333333333333333333333333 \
  --value 0.001ether
```

Or manually:

```bash
cast send $EXECUTION_ENGINE_ADDRESS \
  "submitRequest(bytes32,bytes)" \
  "$PROGRAM_HASH" \
  "$(cast abi-encode 'f(int256[])' '[1000000,2000000,3000000]')" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --private-key "$REQUESTER_PRIVATE_KEY" \
  --value 0.001ether
```

The script prints the request ID after submission, which you use to query results.

## 6. Query the Result

Once the operator picks up the request and submits a TEE result:

```bash
./scripts/sepolia-query-result.sh \
  --result-id 0x<result-id-from-step-5>
```

Or query directly:

```bash
# Get the latest request ID from events
REQUEST_ID=$(cast logs \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --address "$EXECUTION_ENGINE_ADDRESS" \
  "RequestSubmitted(uint256,address,bytes32)" \
  --from-block latest | jq -r '.[0].topics[1]')

# Query the result
cast call $EXECUTION_ENGINE_ADDRESS \
  "getResult(uint256)(uint8,bytes)" \
  "$REQUEST_ID" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

## 7. Run the Full E2E Test

The E2E script exercises the complete lifecycle (deploy, register, submit, attest, query):

```bash
./scripts/sepolia-e2e.sh
```

Use `--dry-run` to print commands without executing:

```bash
./scripts/sepolia-e2e.sh --dry-run
```

## Running the Operator Service

To run the operator as a long-running service watching for on-chain events:

```bash
# Using a config file (see services/operator/config.sepolia.toml)
cargo run -p tee-operator -- --config services/operator/config.sepolia.toml watch

# Or using environment variables
OPERATOR_RPC_URL="$ALCHEMY_SEPOLIA_RPC_URL" \
TEE_VERIFIER_ADDRESS="$TEE_VERIFIER_ADDRESS" \
OPERATOR_PRIVATE_KEY="$DEPLOYER_PRIVATE_KEY" \
ENCLAVE_URL="http://localhost:8080" \
  cargo run -p tee-operator -- watch
```

## Monitoring

### Watch Live Events

Monitor on-chain events (submissions, challenges, finalizations) in real time:

```bash
./scripts/watch-sepolia-events.sh --poll 12
```

### Check Operator Health

If you are running an operator service:

```bash
./scripts/operator-health.sh --url http://localhost:9090
```

### Dispute a Result

To challenge a suspicious result:

```bash
CHALLENGER_PRIVATE_KEY=0x... \
  ./scripts/sepolia-dispute.sh 0x<result-id>
```

Use `--dry-run` to simulate without sending a transaction:

```bash
./scripts/sepolia-dispute.sh --dry-run 0x<result-id>
```

## Relevant Scripts

| Script | Description |
|--------|-------------|
| `scripts/deploy-sepolia-tee.sh` | Deploy contracts + register enclave on Sepolia |
| `scripts/deploy.sh` | Unified deployment script (supports `--chain sepolia`) |
| `scripts/sepolia-e2e.sh` | Full end-to-end lifecycle test |
| `scripts/sepolia-status.sh` | Check contract deployment status |
| `scripts/sepolia-submit-request.sh` | Submit an execution request |
| `scripts/sepolia-query-result.sh` | Query execution result |
| `scripts/sepolia-dispute.sh` | Initiate or resolve a dispute (ZK fallback) |
| `scripts/check-sepolia-balances.sh` | Check wallet balances |
| `scripts/watch-sepolia-events.sh` | Watch for on-chain events in real-time |
| `scripts/upload-private-input.sh` | Upload private inputs for a program |
| `scripts/operator-health.sh` | Check operator service health |

## File Layout

```
deployments/
  11155111.json          # Sepolia deployment addresses (auto-generated)
  chains.json            # Chain configuration (chain IDs, RPC URLs, gas limits)
services/operator/
  config.sepolia.toml    # Operator config example for Sepolia
  config.example.toml    # Generic operator config reference
scripts/
  deploy-sepolia-tee.sh  # TEE-specific Sepolia deployment
  deploy.sh              # Unified deployment script
  sepolia-*.sh           # Sepolia-specific operation scripts
```

## Troubleshooting

### "insufficient funds for gas"

Your deployer wallet does not have enough Sepolia ETH. Top up from a faucet.
Deployment costs approximately 0.05 ETH; each transaction costs approximately 0.001 ETH.

```bash
cast balance $(cast wallet address "$DEPLOYER_PRIVATE_KEY") \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

### "nonce too low"

A previous transaction is still pending or your nonce is out of sync. Wait for
pending transactions to confirm, or manually set `--nonce` in cast commands:

```bash
cast nonce $(cast wallet address "$DEPLOYER_PRIVATE_KEY") \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

### "execution reverted"

Common causes:
- **Program not registered**: Run `sepolia-register-program.sh` before submitting requests.
- **Contract paused**: The admin may have paused the contract. Check with
  `cast call <addr> "paused()(bool)" --rpc-url $ALCHEMY_SEPOLIA_RPC_URL`.
- **Insufficient bond**: For challenges, ensure you send at least `challengeBondAmount` ETH.

### "Enclave signer not registered"

The enclave public key must be registered in the TEEMLVerifier contract:

```bash
ENCLAVE_ADDRESS=$(cast wallet address "$ENCLAVE_PRIVATE_KEY")
cast call $TEE_VERIFIER_ADDRESS \
  "isRegisteredEnclave(address)(bool)" \
  "$ENCLAVE_ADDRESS" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

If it returns `false`, register it:

```bash
cast send $TEE_VERIFIER_ADDRESS \
  "registerEnclave(address)" \
  "$ENCLAVE_ADDRESS" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --private-key "$DEPLOYER_PRIVATE_KEY"
```

### "deployment file not found"

Scripts look for `deployments/11155111.json`. Run `deploy.sh --chain sepolia` first,
or pass contract addresses manually via flags (e.g., `--engine-address 0x...`).

### Sepolia contract size limits

Sepolia enforces EIP-170 (24,576 byte code size limit). The full RemainderVerifier
contract exceeds this limit. Only TEEMLVerifier and ExecutionEngine are deployable on
Sepolia. For full GKR/Groth16 verification, deploy to Arbitrum Sepolia instead:

```bash
# Arbitrum Sepolia has a 200KB code size limit
./scripts/deploy-multichain.sh --chain arbitrum-sepolia
```

### RPC rate limiting

If you see HTTP 429 errors, your RPC provider is rate-limiting requests. Solutions:
- Use a paid Alchemy/Infura plan.
- Add `--rpc-url` to point to a different provider.
- Increase poll intervals for event watching (`--interval 30`).

### Cast version mismatch

Ensure you have a recent version of Foundry (`foundryup` to update). Some cast features
require Foundry v0.2.0 or later.

### Transaction stuck / not mining

Sepolia block times are approximately 12 seconds. If a transaction is stuck:

```bash
# Check transaction status
cast tx $TX_HASH --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"

# Speed up with higher gas price
cast send ... --gas-price $(cast gas-price --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" | awk '{print $1 * 2}')
```
