# World ZK Compute

A decentralized verifiable computation marketplace for Ethereum and World Chain. Built on RISC Zero zkVM.

**Inspired by [Bonsol](https://bonsol.sh)** - bringing Solana's verifiable compute architecture to the EVM ecosystem.

## What Is This?

World ZK Compute enables **trustless outsourced computation**:

1. **Users** post bounties for zkVM program execution
2. **Provers** compete to execute programs and generate proofs
3. **Smart contracts** verify proofs and pay provers
4. **Results** are cryptographically guaranteed correct

```
User posts bounty → Prover claims job → Prover runs zkVM →
Prover submits proof → Contract verifies → Prover gets paid
```

## Why It Matters

- **Privacy-preserving**: Provers process data without seeing it
- **Trustless**: Mathematical proof, not trust in operators
- **Decentralized**: Anyone can be a prover
- **Scalable**: Unlimited provers can join the network

### Use Cases

- **Fraud Detection**: Run ML models on sensitive data with proven results
- **Oracles**: Verifiable off-chain computation for DeFi
- **Identity**: Privacy-preserving verification (World ID)
- **AI Inference**: Prove ML model outputs are correct

## Deployed Contracts (Sepolia Testnet)

| Contract | Address |
|----------|---------|
| MockRiscZeroVerifier | `0x0D194f172a3a50e0E293d0d8f21774b1a222362E` |
| ProgramRegistry | `0x7F9EFc73E50a4f6ec6Ab7B464f6556a89fDeD3ac` |
| ExecutionEngine | `0x9CFd1CF0e263420e010013373Ec4008d341a483e` |

[View on Etherscan](https://sepolia.etherscan.io/address/0x9CFd1CF0e263420e010013373Ec4008d341a483e)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    EXECUTION ENGINE                          │
│                                                              │
│  requestExecution() → claimExecution() → submitProof()      │
│                                                              │
│  ┌─────────┐    ┌─────────┐    ┌───────────┐               │
│  │ PENDING │ →  │ CLAIMED │ →  │ COMPLETED │               │
│  └─────────┘    └─────────┘    └───────────┘               │
│                                                              │
│  Features:                                                   │
│  • Tip decay (incentivizes fast provers)                    │
│  • Claim windows (prevents front-running)                   │
│  • Callbacks (composable with other contracts)              │
│  • Prover stats (reputation tracking)                       │
└─────────────────────────────────────────────────────────────┘
         │                              │
         ▼                              ▼
┌─────────────────┐          ┌─────────────────────┐
│ PROGRAM REGISTRY│          │ RISC ZERO VERIFIER  │
│                 │          │                     │
│ • Register      │          │ • Groth16 proofs    │
│ • Deactivate    │          │ • STARK proofs      │
│ • Update URL    │          │ • Multi-verifier    │
└─────────────────┘          └─────────────────────┘
```

## Quick Start

### Prerequisites

```bash
# Install Foundry
curl -L https://foundry.paradigm.xyz | bash
foundryup

# Install Rust (for prover)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Deploy Contracts

```bash
cd contracts

# Install dependencies
forge install

# Run tests
forge test

# Deploy to Sepolia
export PRIVATE_KEY=0x...
export FEE_RECIPIENT=0x...

forge script script/Deploy.s.sol:DeployScript \
  --rpc-url https://ethereum-sepolia-rpc.publicnode.com \
  --broadcast
```

### Interact with Contracts

```bash
# Register a program
cast send $REGISTRY "registerProgram(bytes32,string,string,bytes32)" \
  $IMAGE_ID "My Program" "https://example.com/program.elf" 0x0 \
  --rpc-url $RPC_URL --private-key $PRIVATE_KEY

# Request execution
cast send $ENGINE "requestExecution(bytes32,bytes32,string,address,uint256)" \
  $IMAGE_ID $INPUT_HASH "ipfs://inputs" 0x0 3600 \
  --value 0.01ether \
  --rpc-url $RPC_URL --private-key $PRIVATE_KEY

# Check pending requests
cast call $ENGINE "getPendingRequests(uint256,uint256)" 0 10 \
  --rpc-url $RPC_URL
```

## Contracts

### ExecutionEngine.sol

Core lifecycle management for verifiable computation.

**Key Functions:**
- `requestExecution()` - Post a bounty for computation
- `claimExecution()` - Lock a job (provers)
- `submitProof()` - Submit proof and collect bounty
- `cancelExecution()` - Cancel pending request (requesters)

**Tip Decay:**
```solidity
// Bounty decreases from 100% to 50% over 30 minutes
// Incentivizes fast proof generation
effectiveTip = maxTip - (elapsed * maxTip / TIP_DECAY_PERIOD / 2)
```

### ProgramRegistry.sol

Registry for zkVM programs.

**Key Functions:**
- `registerProgram()` - Add a new program
- `updateProgramUrl()` - Update program binary location
- `deactivateProgram()` - Disable a program
- `isProgramActive()` - Check if program can be executed

### RiscZeroVerifierRouter.sol

Routes proofs to appropriate verifiers based on proof type.

**Features:**
- Multi-verifier support (Groth16, STARK, etc.)
- Selector-based routing
- Upgradeable verifier backends

## Prover Node

The Rust prover monitors the blockchain and executes jobs.

```bash
cd prover

# Build
cargo build --release

# Run
./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --min-tip 0.0001
```

### Prover Flow

1. **Monitor** - Watch for `ExecutionRequested` events
2. **Evaluate** - Check if job is profitable
3. **Claim** - Call `claimExecution()` to lock job
4. **Execute** - Run zkVM with inputs
5. **Prove** - Generate RISC Zero proof
6. **Submit** - Call `submitProof()` with seal + journal
7. **Collect** - Receive bounty minus protocol fee

## Testing

```bash
cd contracts

# Run all tests
forge test

# Run with verbosity
forge test -vvv

# Run specific test
forge test --match-test testSubmitProof

# Gas report
forge test --gas-report
```

**Test Coverage:** 27 tests passing

## Security

### Attack Mitigations

| Attack | Protection |
|--------|------------|
| Front-running | Claim mechanism locks jobs |
| Claim abandonment | Claims expire after 10 min |
| Fake proofs | On-chain ZK verification |
| Input tampering | Input digest verification |
| Stuck funds | Requester can cancel |

## Comparison with Bonsol

This project is an **EVM port of Bonsol's architecture**:

| Feature | Bonsol (Solana) | World ZK Compute (EVM) |
|---------|-----------------|------------------------|
| Execution Requests | ✅ | ✅ |
| Claim Mechanism | ✅ | ✅ |
| Tip Decay | ✅ | ✅ |
| Program Registry | ✅ | ✅ |
| Callbacks | ✅ | ✅ |
| Multi-Verifier | ✅ | ✅ |

## Platform: Detection-Agnostic Infrastructure

This system works with **ANY** zkVM program - it's infrastructure, not a specific application:

```
YOUR SYSTEM                          DETECTION TEAM
───────────                          ──────────────

┌─────────────────┐                  ┌─────────────────┐
│ ProgramRegistry │ ←── registers ── │ Sybil Detector  │
│                 │                  │ PAD Model       │
│                 │                  │ Geo Clustering  │
└─────────────────┘                  └─────────────────┘
        │
        ▼
┌─────────────────┐                  ┌─────────────────┐
│ ExecutionEngine │ ←── requests ─── │ Detection Jobs  │
│                 │                  │ (any algorithm) │
└─────────────────┘                  └─────────────────┘
        │
        ▼
┌─────────────────┐
│ Prover Network  │ ──── runs ANY program
└─────────────────┘       returns PROVEN results
```

## Roadmap

- [x] Core contracts (Engine, Registry, Verifier)
- [x] Tip decay mechanism
- [x] Comprehensive tests (27 passing)
- [x] Testnet deployment (Sepolia)
- [x] Verifier router for multi-proof support
- [ ] Production RISC Zero verifier integration
- [ ] World Chain mainnet deployment
- [ ] Prover network incentives
- [ ] SDK for developers

## License

Apache-2.0

## Links

- [Bonsol](https://bonsol.sh) - Inspiration
- [RISC Zero](https://risczero.com) - zkVM
- [World Chain](https://world.org) - Target L2
- [Foundry](https://book.getfoundry.sh) - Development framework
