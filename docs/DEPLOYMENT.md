# Deployment Guide

## Prerequisites

1. **Foundry** - Solidity development toolkit
   ```bash
   curl -L https://foundry.paradigm.xyz | bash
   foundryup
   ```

2. **Rust** - For the prover node
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. **RISC Zero** - zkVM toolkit
   ```bash
   curl -L https://risczero.com/install | bash
   rzup install
   ```

## Network Configuration

### World Chain Sepolia (Testnet)
- RPC URL: `https://worldchain-sepolia.g.alchemy.com/v2/YOUR_KEY`
- Chain ID: `4801`
- Explorer: `https://worldchain-sepolia.explorer.alchemy.com`

### Get Testnet ETH
1. Get Sepolia ETH from a faucet
2. Bridge to World Chain using the official bridge

## Deploy Contracts

### 1. Set Environment Variables

```bash
cp .env.example .env
# Edit .env with your values:
# PRIVATE_KEY=0x...
# RPC_URL=https://worldchain-sepolia.g.alchemy.com/v2/...
# ETHERSCAN_API_KEY=...
```

### 2. Deploy Contracts

```bash
cd contracts

# Install dependencies
forge install

# Deploy MockVerifier (for testing)
forge create src/MockRiscZeroVerifier.sol:MockRiscZeroVerifier \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY

# Note the verifier address, then deploy ProgramRegistry
forge create src/ProgramRegistry.sol:ProgramRegistry \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY

# Note the registry address, then deploy ExecutionEngine
forge create src/ExecutionEngine.sol:ExecutionEngine \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --constructor-args $REGISTRY_ADDRESS $VERIFIER_ADDRESS $FEE_RECIPIENT
```

### 3. Verify Contracts (Optional)

```bash
forge verify-contract $CONTRACT_ADDRESS src/ProgramRegistry.sol:ProgramRegistry \
  --chain-id 4801 \
  --etherscan-api-key $ETHERSCAN_API_KEY
```

## Run Prover Node

### 1. Build Prover

```bash
cd prover
cargo build --release
```

### 2. Configure Prover

```bash
export RPC_URL="https://worldchain-sepolia.g.alchemy.com/v2/..."
export PRIVATE_KEY="0x..."
export ENGINE_ADDRESS="0x..."
```

### 3. Start Prover

```bash
./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --min-tip 0.0001
```

## Register a Program

### 1. Build Your zkVM Program

```bash
cd programs/my-program
cargo build --release
```

### 2. Compute Image ID

```bash
# The image ID is logged when you build with RISC Zero
cargo risczero build
```

### 3. Register On-Chain

```bash
cast send $REGISTRY_ADDRESS \
  "registerProgram(bytes32,string,string,bytes32)" \
  $IMAGE_ID "My Program" "https://example.com/my-program.elf" 0x0 \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY
```

## Request Execution

### 1. Prepare Inputs

```bash
# Upload inputs to IPFS
ipfs add input.json
# Note the CID
```

### 2. Request Execution

```bash
cast send $ENGINE_ADDRESS \
  "requestExecution(bytes32,bytes32,string,address,uint256)" \
  $IMAGE_ID $INPUT_DIGEST "ipfs://QmXXX" $CALLBACK_ADDRESS 3600 \
  --value 0.01ether \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY
```

## Monitoring

### Check Request Status

```bash
./target/release/world-zk-prover status \
  --rpc-url $RPC_URL \
  --engine-address $ENGINE_ADDRESS \
  --request-id 1
```

### List Pending Requests

```bash
./target/release/world-zk-prover list-pending \
  --rpc-url $RPC_URL \
  --engine-address $ENGINE_ADDRESS \
  --limit 20
```

## Production Considerations

1. **Use Real RISC Zero Verifier**
   - Replace MockVerifier with RISC Zero's official verifier contracts
   - See: https://github.com/risc0/risc0-ethereum

2. **Secure Key Management**
   - Use hardware wallets or key management services
   - Never commit private keys

3. **Multiple Provers**
   - Run multiple prover nodes for redundancy
   - Configure different min_tip thresholds

4. **Monitoring**
   - Set up alerts for failed proofs
   - Monitor gas prices and adjust tips accordingly
