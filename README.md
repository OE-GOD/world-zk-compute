# World ZK Compute

**A Bonsol-style verifiable computation framework for World Chain.**

Enables developers to run complex computations off-chain with cryptographic proofs of correctness, verified on World Chain.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         WORLD ZK COMPUTE ARCHITECTURE                        │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐       │
│  │ PROGRAM REGISTRY│     │ EXECUTION ENGINE │     │  RISC0 VERIFIER │       │
│  │                 │     │                 │     │                 │       │
│  │ • Image IDs     │     │ • Request queue │     │ • STARK verify  │       │
│  │ • Input schemas │     │ • Claim system  │     │ • Groth16 verify│       │
│  │ • Program URLs  │     │ • Tip mechanism │     │ • Journal decode│       │
│  └────────┬────────┘     └────────┬────────┘     └────────┬────────┘       │
│           │                       │                       │                │
│           └───────────────────────┴───────────────────────┘                │
│                                   │                                        │
│                          WORLD CHAIN (L2)                                  │
│  ─────────────────────────────────┼─────────────────────────────────────── │
│                                   │                                        │
│           ┌───────────────────────┴───────────────────────┐                │
│           │              PROVER NETWORK                    │                │
│           │                                                │                │
│           │  ┌─────────┐  ┌─────────┐  ┌─────────┐        │                │
│           │  │ Prover1 │  │ Prover2 │  │ Prover3 │  ...   │                │
│           │  └─────────┘  └─────────┘  └─────────┘        │                │
│           │                                                │                │
│           │  • Monitor for execution requests              │                │
│           │  • Claim jobs matching criteria                │                │
│           │  • Run zkVM, generate proofs                   │                │
│           │  • Submit proofs, collect rewards              │                │
│           └────────────────────────────────────────────────┘                │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Flow

```
1. DEPLOY PROGRAM
   Developer → deploys zkVM program → registers in Program Registry

2. REQUEST EXECUTION
   User → calls requestExecution(imageId, inputs, tip)
   → Creates pending execution request on-chain

3. PROVER CLAIMS
   Prover → monitors chain → sees request → calls claimExecution(requestId)
   → Has limited time window to submit proof

4. GENERATE PROOF
   Prover → downloads inputs → runs zkVM → generates STARK proof
   → Optionally wraps in Groth16 for smaller on-chain footprint

5. SUBMIT PROOF
   Prover → calls submitProof(requestId, seal, journal)
   → Contract verifies proof → executes callback → pays prover

6. CALLBACK
   Verified results → sent to user's callback contract
   → User's contract can use verified computation results
```

## Components

### Smart Contracts (Solidity)

| Contract | Purpose |
|----------|---------|
| `ProgramRegistry.sol` | Register and manage zkVM programs |
| `ExecutionEngine.sol` | Handle execution requests and claims |
| `RiscZeroVerifier.sol` | Verify RISC Zero proofs on-chain |
| `CallbackRouter.sol` | Route verified results to user contracts |

### Prover Node (Rust)

- Monitors World Chain for execution requests
- Claims jobs matching configured criteria
- Runs RISC Zero zkVM locally
- Submits proofs to chain

### SDK (TypeScript/Rust)

- Easy integration for developers
- Deploy programs, request executions, handle callbacks

## Use Cases

1. **Verifiable Anomaly Detection** - Run detection algorithms, prove correctness
2. **Private Computations** - Compute on private data, reveal only results
3. **Complex DeFi Logic** - Off-load heavy math from smart contracts
4. **AI/ML Inference** - Run models in zkVM, verify on-chain
5. **Cross-Chain Proofs** - Prove state from other chains

## Quick Start

```bash
# Deploy a program
world-zk deploy --image ./target/riscv-guest/release/my-program

# Request execution
world-zk request --image-id 0x... --input ./input.json --tip 0.01

# Run a prover node
world-zk prover --rpc https://worldchain-sepolia.g.alchemy.com/v2/...
```

## License

Apache-2.0
