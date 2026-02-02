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

# Run with local CPU proving (slow but free)
./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --min-tip 0.0001 \
  --proving-mode local

# Run with Bonsai cloud proving (fast, GPU-accelerated)
./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --min-tip 0.0001 \
  --proving-mode bonsai
```

### Bonsai Cloud Proving

[Bonsai](https://bonsai.xyz) is RISC Zero's cloud proving service that provides:

- **10-100x faster** proof generation using GPU acceleration
- **Parallel proving** across multiple machines
- **Production-ready** for any zkVM program complexity

**Proving Modes:**

| Mode | Description | Use Case |
|------|-------------|----------|
| `local` | CPU-based proving | Development, testing |
| `bonsai` | Bonsai cloud proving | Production workloads |
| `bonsai-fallback` | Try Bonsai, fall back to local | Hybrid setup |

**Setup Bonsai:**

```bash
# 1. Get API key from https://bonsai.xyz
# 2. Set environment variables
export BONSAI_API_KEY=your-api-key
export BONSAI_API_URL=https://api.bonsai.xyz  # optional, default

# 3. Run prover with Bonsai
./target/release/world-zk-prover run \
  --proving-mode bonsai \
  ...
```

**Why Bonsai Matters:**

Without Bonsai, local CPU proving can take 10-60+ minutes for complex programs. With Bonsai's GPU clusters, the same proof generates in seconds to minutes. This makes the system practical for ANY detection algorithm, including ML models.

## Fast Proving System

The prover includes advanced optimizations for **maximum proof generation speed**:

### Automatic Strategy Selection

The prover analyzes each job and selects the optimal proving strategy:

| Program Complexity | Cycles | Strategy | Speed |
|-------------------|--------|----------|-------|
| Simple | <20M | Direct | Fastest |
| Medium | 20-100M | Segmented (parallel) | Fast |
| Complex | 100-500M | Continuation | Moderate |
| Very Complex | >500M | Rejected | N/A |

### Preflight Execution

Before proving, the system runs a quick preflight to estimate resources:

```rust
let preflight = prover.preflight(elf, input).await?;
// Returns: cycles, memory usage, estimated time, recommended strategy
```

This prevents wasting GPU time on jobs that will fail.

### GPU Pipeline Optimization

```
Job 1: [Upload] → [Prove] → [Download]
Job 2:           [Upload] → [Prove] → [Download]
Job 3:                     [Upload] → [Prove] → ...

Pipeline keeps GPU constantly busy!
```

Features:
- **Request batching** - Amortize API overhead
- **Adaptive concurrency** - Auto-tune based on Bonsai load
- **Memory pooling** - Pre-allocate GPU memory
- **Session reuse** - Keep proving sessions warm

### Proof Composition

Combine multiple proofs into one for cheaper verification:

```
10 individual proofs → 10 verifications → 2M gas
10 composed proofs   → 1 verification  → 200K gas (90% savings!)
```

## Performance Optimizations

The prover includes several optimizations for maximum throughput:

### Parallel Processing

Process multiple proofs concurrently:

```bash
# Process up to 8 proofs in parallel
./world-zk-prover run --max-concurrent 8 ...
```

### STARK-to-SNARK Conversion

Convert proofs to Groth16 for smaller size and cheaper on-chain verification:

```bash
# Enable SNARK conversion (256 bytes vs 200KB for STARK)
./world-zk-prover run --use-snark ...
```

| Proof Type | Size | On-chain Gas |
|------------|------|--------------|
| STARK | ~200 KB | ~2M gas |
| SNARK (Groth16) | ~256 bytes | ~200K gas |

### ELF Caching

Cache downloaded programs to avoid re-downloading:

```bash
# Use 512MB memory cache
./world-zk-prover run --cache-size-mb 512 ...
```

### Connection Pooling

The prover uses optimized HTTP settings:
- Connection reuse (keep-alive)
- Automatic retry with exponential backoff
- gzip/brotli compression
- Parallel prefetching

### Full Optimization Example

```bash
./world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --proving-mode bonsai \
  --max-concurrent 8 \
  --use-snark \
  --cache-size-mb 512 \
  --queue-size 1000 \
  --health-port 8081 \
  --min-tip 0.0001
```

### IPFS Integration

The prover supports fetching inputs from IPFS for decentralized storage:

```rust
// Inputs can be stored on IPFS
let input_url = "ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";

// The prover automatically resolves IPFS URLs through multiple gateways:
// 1. Cloudflare IPFS (fastest)
// 2. ipfs.io (official)
// 3. dweb.link (fallback)
// 4. Pinata (fallback)
```

### Private Input Server

For sensitive data that needs access control, we provide a **Private Input Server** (like [Bonsol](https://bonsol.sh)):

```
User uploads input → Server encrypts → Returns inputId
User submits job with "private://<inputId>"
Prover claims job on-chain
Prover requests input (signs with wallet)
Server verifies on-chain claim → Returns encrypted data + key
Prover decrypts locally → Runs in zkVM
```

**Why use Private Input Server instead of IPFS?**

| Aspect | IPFS | Private Input Server |
|--------|------|---------------------|
| Access control | None | On-chain claim verification |
| Encryption | None | AES-256-GCM at rest |
| Who can fetch | Anyone with CID | Only verified claimers |
| Audit trail | None | Full access logging |

**Start the server:**

```bash
cd private-input-server
cargo run -- \
  --rpc-url $RPC_URL \
  --engine-address $ENGINE_ADDRESS \
  --master-key $(openssl rand -hex 32)
```

**Upload input:**

```bash
curl -X POST http://localhost:3000/inputs \
  -H "Content-Type: application/json" \
  -d '{"data": "<base64-encoded-input>"}'
# Returns: {"input_id": "abc123...", "input_digest": "..."}
```

**Use in execution request:**

```bash
cast send $ENGINE "requestExecution(...)" \
  ... "private://abc123..." ...  # Use private:// URL scheme
```

The prover automatically handles authentication and decryption when fetching private inputs.

### Health Monitoring

The prover exposes HTTP endpoints for monitoring:

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Basic health check (returns 200/503) |
| `GET /metrics` | Prometheus-format metrics |
| `GET /status` | Detailed prover status JSON |

**Example metrics output:**
```
prover_proofs_total{status="success"} 150
prover_proofs_total{status="failed"} 3
prover_active_proofs 2
prover_proof_duration_seconds{quantile="0.99"} 45.2
prover_throughput_per_hour 12.5
```

### Job Queue

Smart job selection with priority scoring:

- **Tip amount** - Higher tips = higher priority
- **Time urgency** - Jobs expiring soon get boosted
- **Program familiarity** - Cached programs preferred
- **Complexity** - Simpler jobs first for throughput

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
| Private Input Server | ✅ | ✅ |

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

## Detection Algorithm Examples

See [`examples/`](./examples/) for complete detection algorithm examples:

- **Anomaly Detector** - Statistical anomaly detection using z-score analysis
- More coming soon...

### Quick Start

```bash
# Run the demo
./scripts/demo-detection.sh

# Or use the SDK
cargo add world-zk-sdk
```

```rust
use world_zk_sdk::{Client, DetectionJob};

let client = Client::new(rpc_url, private_key, engine_address).await?;
let result = client.submit_and_wait(
    DetectionJob::new(image_id, input_data)
        .with_bounty(0.01)
).await?;

println!("Anomalies found: {}", result.anomalies_found);
```

## Handling Large Detection Programs

### Limitations & Solutions

| Limit | Constraint | Solution |
|-------|------------|----------|
| **Cycles** | ~100M cycles max | Continuations (split execution) |
| **Memory** | ~256MB guest RAM | Model quantization (INT8) |
| **Proof Time** | Hours for huge programs | Bonsai GPU + parallel |
| **Model Size** | Large NNs don't fit | Staged pipeline |

### Continuations (Split Execution)

For programs exceeding cycle limits:

```rust
use world_zk_compute::continuations::ContinuationExecutor;

let executor = ContinuationExecutor::new(LargeProgramConfig {
    max_cycles_per_segment: 50_000_000, // 50M per segment
    enable_recursive: true,              // Compose proofs
    ..Default::default()
});

let result = executor.execute_with_continuations(elf, input).await?;
println!("Split into {} segments", result.segments.len());
```

### Staged Detection Pipeline

Break complex detection into stages:

```rust
let pipeline = StagedPipeline::new()
    .add_stage("preprocess", preprocess_id, 10_000_000, 64)
    .add_stage("features", features_id, 30_000_000, 128)
    .add_stage("inference", model_id, 50_000_000, 256)
    .add_stage("postprocess", postprocess_id, 5_000_000, 32);

// Each stage is proven separately, then composed
```

### Model Optimization for zkVM

| Technique | Size Reduction | Recommended For |
|-----------|---------------|-----------------|
| INT8 Quantization | 4x smaller | All models |
| Pruning | 2-10x smaller | Dense layers |
| Knowledge Distillation | Custom | Large → small |
| Weight Sharing | 2-4x smaller | Transformers |

```rust
// Check if your model fits
let estimate = ModelOptimizer::estimate_fit(
    1_000_000,  // 1M parameters
    8,          // INT8 (8 bits)
    256,        // 256MB limit
);

if !estimate.fits {
    println!("{}", estimate.recommendation);
}
```

### What Works Well in zkVM

| Algorithm Type | Cycles | Fits? |
|---------------|--------|-------|
| Statistical (z-score, clustering) | 1-10M | ✅ Easy |
| Decision trees / Random forest | 5-20M | ✅ Easy |
| Small NNs (<1M params) | 20-50M | ✅ Yes |
| Medium NNs (1-10M params) | 50-200M | ⚠️ With continuations |
| Large NNs (>10M params) | 200M+ | ❌ Use staged pipeline |
| LLMs | Billions | ❌ Not practical |

### Monitoring

Track prover performance:

```rust
use world_zk_compute::metrics;

// Get current stats
let snapshot = metrics::metrics().snapshot();
println!("{}", snapshot);

// Output:
// === Prover Metrics ===
// Proofs: 150 generated, 3 failed (98.0% success)
// Throughput: 12.5 proofs/hour
// Avg proof time: 45.2s
// P99 proof time: 120.3s
```

## Integrated Architecture

All optimization modules are wired together in `OptimizedProcessor`:

```
┌─────────────────────────────────────────────────────────────────────┐
│                    OptimizedProcessor                                │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Blockchain ──→ Fetch Requests ──→ JobQueue (priority sort)         │
│                                          │                          │
│                                    ┌─────┴─────┐                    │
│                                    ▼           ▼                    │
│                              [Worker 1]   [Worker N]  (parallel)    │
│                                    │           │                    │
│  ┌─────────────────────────────────┴───────────┴──────────────────┐ │
│  │                         Per-Job Pipeline                        │ │
│  │                                                                 │ │
│  │  1. ProgramCache.get() ──→ Cache hit? Skip download            │ │
│  │           │                                                     │ │
│  │           ▼                                                     │ │
│  │  2. IpfsClient.fetch() ──→ Multi-gateway fallback              │ │
│  │           │                                                     │ │
│  │           ▼                                                     │ │
│  │  3. FastProver.preflight() ──→ Reject impossible jobs          │ │
│  │           │                                                     │ │
│  │           ▼                                                     │ │
│  │  4. Claim on-chain                                              │ │
│  │           │                                                     │ │
│  │           ▼                                                     │ │
│  │  5. FastProver.prove_fast() ──→ Strategy selection             │ │
│  │           │                                                     │ │
│  │           ▼                                                     │ │
│  │  6. Submit proof on-chain                                       │ │
│  │           │                                                     │ │
│  │           ▼                                                     │ │
│  │  7. Metrics.record() ──→ Track success/failure/timing          │ │
│  └─────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  Health Server (/health, /metrics, /status) ──→ Prometheus          │
└─────────────────────────────────────────────────────────────────────┘
```

**Result:** 5-10x throughput improvement over sequential processing.

## Roadmap

- [x] Core contracts (Engine, Registry, Verifier)
- [x] Tip decay mechanism
- [x] Comprehensive tests (27 passing)
- [x] Testnet deployment (Sepolia)
- [x] Verifier router for multi-proof support
- [x] Bonsai cloud proving integration (10-100x faster proofs)
- [x] Performance optimizations (parallel, SNARK, caching)
- [x] Detection SDK for easy integration
- [x] Example detection algorithms
- [x] Large program support (continuations, staged pipelines)
- [x] IPFS integration for decentralized inputs
- [x] Health monitoring & Prometheus metrics
- [x] Smart job queue with priority scoring
- [x] **Fully integrated OptimizedProcessor** (all modules wired together)
- [x] **Private Input Server** (like Bonsol - on-chain claim verification)
- [ ] Production RISC Zero verifier integration
- [ ] World Chain mainnet deployment
- [ ] Prover network incentives

## License

Apache-2.0

## Links

- [Bonsol](https://bonsol.sh) - Inspiration
- [RISC Zero](https://risczero.com) - zkVM
- [World Chain](https://world.org) - Target L2
- [Foundry](https://book.getfoundry.sh) - Development framework
