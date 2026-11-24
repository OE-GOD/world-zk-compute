# Sepolia Quickstart

End-to-end guide for deploying and running World ZK Compute on the Sepolia testnet.

## Prerequisites

| Tool | Install |
|------|---------|
| Foundry (forge, cast) | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| jq | `brew install jq` or `apt-get install jq` |
| Alchemy (or other) Sepolia RPC key | https://www.alchemy.com/ |
| Sepolia testnet ETH | https://sepoliafaucet.com/ or https://www.alchemy.com/faucets/ethereum-sepolia |

You need more than **0.1 ETH** on the deployer wallet: besides deployment and gas for registration and execution requests, `sepolia-e2e.sh` (step 7) sends the 0.1 ETH prover stake from it, which `finalize` returns.

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

# Required by deploy-sepolia-tee.sh and deploy.sh: address TEEMLVerifier calls to
# resolve disputes (TEEMLVerifier.initialize rejects address(0)). No RemainderVerifier
# can be deployed on Sepolia (see "Sepolia contract size limits"), so there is no
# working address to put here
REMAINDER_VERIFIER=0x...

# Optional: separate requester key (defaults to deployer)
# REQUESTER_PRIVATE_KEY=0x...
EOF

# Load environment (set -a exports the variables; a plain `source` leaves them
# invisible to the scripts)
set -a; source .env.sepolia; set +a
```

> **Security Note**: Never commit private keys to version control. Use a `.env` file
> or a secrets manager. The deployer key becomes the contract admin.

## 2. Deploy Contracts

Deploy the TEEMLVerifier and ExecutionEngine contracts to Sepolia:

```bash
./scripts/deploy-sepolia-tee.sh
```

This script will:
- Deploy ProgramRegistry, TEEMLVerifier (implementation + proxy) and ExecutionEngine contracts
- Register the program image ID and the enclave signer address
- Fund the operator/requester wallets with 0.01 ETH each if `OPERATOR_PRIVATE_KEY` / `REQUESTER_PRIVATE_KEY` are set
- Write deployment addresses to `deployments/11155111.json`

To also verify contracts on Etherscan, run the deployment with `ETHERSCAN_API_KEY` set instead
(the script then passes `--verify` to `deploy.sh`):

```bash
ETHERSCAN_API_KEY="<your-etherscan-api-key>" \
  ./scripts/deploy-sepolia-tee.sh
```

After deploying, export the contract addresses. `sepolia-status.sh` reads them only from
these variables, and `sepolia-dispute.sh` and `watch-sepolia-events.sh` need them too:

```bash
export TEE_VERIFIER_ADDRESS=$(jq -r .contracts.TEEMLVerifier.address deployments/11155111.json)
export EXECUTION_ENGINE_ADDRESS=$(jq -r .contracts.ExecutionEngine.address deployments/11155111.json)
export PROGRAM_REGISTRY_ADDRESS=$(jq -r .contracts.ProgramRegistry.address deployments/11155111.json)
```

If you already have deployed contracts, skip this step and set the addresses manually:

```bash
export TEE_VERIFIER_ADDRESS=0x...
export EXECUTION_ENGINE_ADDRESS=0x...
```

### Dry Run

To simulate deployment without broadcasting transactions (do this before deploying: it
overwrites `deployments/11155111.json` with placeholder addresses):

```bash
./scripts/deploy.sh --chain sepolia --dry-run
```

## 3. Check Deployment Status

Check RPC connectivity and read TEEMLVerifier's admin, pause state, prover stake and challenge bond and ExecutionEngine's next request ID (the script does not check enclave registration):

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
# Programs are registered in ProgramRegistry by image ID
# (address: .contracts.ProgramRegistry.address in deployments/11155111.json)
IMAGE_ID=0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33

cast send $PROGRAM_REGISTRY_ADDRESS \
  "registerProgram(bytes32,string,string,bytes32)" \
  "$IMAGE_ID" "xgboost-rule-engine" "" \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
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

Submit an execution request to the ExecutionEngine. Requests are picked up by a prover node (`world-zk-prover run`, see `docs/DEPLOYMENT.md`); the TEE operator does not act on them:

```bash
./scripts/sepolia-submit-request.sh \
  --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
  --input-hash 0x3333333333333333333333333333333333333333333333333333333333333333 \
  --value 0.001ether
```

Or manually:

```bash
# requestExecution(imageId, inputDigest, inputUrl, callbackContract, expirationSeconds)
cast send $EXECUTION_ENGINE_ADDRESS \
  "requestExecution(bytes32,bytes32,string,address,uint256)" \
  "$IMAGE_ID" \
  0x3333333333333333333333333333333333333333333333333333333333333333 \
  "" \
  0x0000000000000000000000000000000000000000 \
  3600 \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --private-key "${REQUESTER_PRIVATE_KEY:-$DEPLOYER_PRIVATE_KEY}" \
  --value 0.001ether
```

The script prints the request ID after submission, which you use to query the request.

## 6. Query the Result

TEE results are stored in TEEMLVerifier under their own result ID, which is not the request ID from step 5. Whatever submits the result prints it: `tee-operator submit` logs `result_id=0x...`, and `sepolia-e2e.sh` (step 7) prints `Result ID:`. Query a result with:

```bash
./scripts/sepolia-query-result.sh \
  --result-id 0x<result-id>
```

With Foundry 1.10.0 this script prints `No result found` even for a result that exists (it cannot parse numbers that `cast` prints as `1795502832 [1.795e9]`). The validity flag can be read directly:

```bash
cast call $TEE_VERIFIER_ADDRESS \
  "isResultValid(bytes32)(bool)" \
  0x<result-id> \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

To query the execution request from step 5 directly:

```bash
# Get the ID of the most recent request since FROM_BLOCK (the block your request
# was mined in, or an earlier one)
REQUEST_ID=$(cast logs \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --address "$EXECUTION_ENGINE_ADDRESS" \
  "ExecutionRequested(uint256,address,bytes32,bytes32,string,uint8,uint256,uint256)" \
  --from-block "$FROM_BLOCK" --json | jq -r '.[-1].topics[1]')

# Query the request (status, prover, deadlines)
cast call $EXECUTION_ENGINE_ADDRESS \
  "getRequest(uint256)" \
  "$REQUEST_ID" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

## 7. Run the Full E2E Test

The E2E script runs against the contracts deployed in step 2 (it does not deploy). It checks connectivity and the three contracts, submits an execution request, signs a test result with `ENCLAVE_PRIVATE_KEY` and submits it to TEEMLVerifier with the 0.1 ETH prover stake, queries it, and prints the `finalize` command to run once the 1-hour challenge window has passed:

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
| `scripts/sepolia-query-result.sh` | Query a TEE result in TEEMLVerifier |
| `scripts/sepolia-dispute.sh` | Challenge a TEE result (opens a dispute) |
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

### "UnregisteredEnclave" (enclave signer not registered)

The enclave public key must be registered in the TEEMLVerifier contract:

```bash
ENCLAVE_ADDRESS=$(cast wallet address "$ENCLAVE_PRIVATE_KEY")
cast call $TEE_VERIFIER_ADDRESS \
  "enclaves(address)((bool,bool,bytes32,uint256))" \
  "$ENCLAVE_ADDRESS" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL"
```

If the first two fields (`registered`, `active`) are not `true`, register it
(the second argument is the enclave image hash, e.g. the first 32 bytes of PCR0):

```bash
cast send $TEE_VERIFIER_ADDRESS \
  "registerEnclave(address,bytes32)" \
  "$ENCLAVE_ADDRESS" \
  "$ENCLAVE_IMAGE_HASH" \
  --rpc-url "$ALCHEMY_SEPOLIA_RPC_URL" \
  --private-key "$DEPLOYER_PRIVATE_KEY"
```

### "deployment file not found"

Scripts look for `deployments/11155111.json`. Run `deploy.sh --chain sepolia` first,
or pass contract addresses manually via flags (e.g., `--engine-address 0x...`).

### Sepolia contract size limits

Sepolia enforces EIP-170 (24,576 byte code size limit). The full RemainderVerifier
contract (135,533 bytes) exceeds this limit. ProgramRegistry, TEEMLVerifier and ExecutionEngine are
deployable on Sepolia. Full GKR/Groth16 verification cannot be deployed on Arbitrum Sepolia
either: it enforces the same 24,576-byte limit (as do Arbitrum One, World Chain, World Chain
Sepolia and Ethereum mainnet), and `deployments/chains.json` marks it `teeOnly`. Only the TEE
path can be deployed there. `deploy.sh` also deploys ProgramRegistry and ExecutionEngine there,
with a `MockRiscZeroVerifier` behind the ExecutionEngine, because `deployments/chains.json` has
no `verifierRouter` for this chain:

```bash
# deploy-multichain.sh fails here ("create initcode size limit"); use deploy.sh
REMAINDER_VERIFIER=0x... DEPLOYER_PRIVATE_KEY=0x... \
  ./scripts/deploy.sh --chain arbitrum-sepolia
```

### RPC rate limiting

If you see HTTP 429 errors, your RPC provider is rate-limiting requests. Solutions:
- Use a paid Alchemy/Infura plan.
- Add `--rpc-url` to point to a different provider.
- Increase poll intervals for event watching (`--poll 30`).

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
