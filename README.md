# World ZK Compute

A decentralized verifiable computation marketplace for Ethereum and World Chain. Multi-backend proof verification: RISC Zero zkVM, Remainder GKR+Hyrax (on-chain ZKML), and eZKL.

**Inspired by [Bonsol](https://bonsol.sh)** — bringing Solana's verifiable compute architecture to the EVM ecosystem.

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

## Project Structure

```
├── contracts/               Foundry Solidity contracts
│   ├── src/                   Core contracts + Remainder verifier suite
│   ├── test/                  167+ Solidity tests
│   ├── script/                Deployment scripts (Local, Testnet, Remainder)
│   └── stylus/gkr-verifier/  Arbitrum Stylus WASM port (Rust, 85 tests)
├── prover/                  Rust prover node (risc0-zkvm v3.0)
├── services/
│   ├── operator/              TEE dispute operator (Rust, axum)
│   ├── indexer/               On-chain event indexer
│   └── admin-cli/             Admin CLI tool
├── tee/
│   └── enclave/               TEE inference enclave (Rust, axum)
├── examples/
│   └── xgboost-remainder/     XGBoost ZKML circuit (34 tests)
│       └── gnark-wrapper/     Groth16 SNARK wrapper (Go, 14 tests)
├── sdk/
│   ├── rust (sdk/)            Rust SDK (DAG verifier, TEE, events)
│   ├── python/                Python SDK + XGBoost model parser (449 tests)
│   └── typescript/            TypeScript SDK (199 tests)
├── programs/                Pre-compiled guest program binaries
├── deploy/                  Kubernetes manifests + Helm charts
├── monitoring/              Prometheus + Grafana configs
├── scripts/                 E2E, deployment, and utility scripts
└── docs/                    Architecture, runbooks, threat model
```

## TEE + ZK Dispute System

The production path uses TEE inference with ZK dispute fallback:

```
User → TEE Enclave (inference) → On-chain result submission
                                  ↓
                          Challenge window (anyone can dispute)
                                  ↓
              If disputed → ZK proof generated → On-chain verification
              If unchallenged → Result finalized
```

- **TEE happy path cost:** ~$0.0001 per inference (gas only)
- **ZK dispute cost:** ~$0.02 (Groth16) or ~$5 (full GKR)
- **AWS Nitro Enclaves** with P-384 attestation chain verification

### Docker Compose Quickstart

```bash
# Start everything: Anvil + contracts + TEE + prover + operator
docker compose up --build

# Run inference via TEE enclave
curl -X POST http://localhost:8080/infer \
  -H "Content-Type: application/json" \
  -d '{"features": [1.0, 2.0, 3.0, 4.0]}'
```

## Test Coverage

| Component | Tests | What |
|-----------|-------|------|
| Solidity | 167 | Remainder verifier, DAG verifier, batch verifier, Groth16 hybrid, E2E |
| Stylus (Rust) | 85 | BN254 field/EC ops, Poseidon, GKR, Hyrax, sumcheck, proof decoding |
| Remainder (Rust) | 34 | XGBoost circuit, GKR prove-and-verify, model parsing, ABI encoding |
| gnark (Go) | 14 | Groth16 circuit compilation, proving, per-layer num_vars |
| TypeScript SDK | 199 | TEE verifier, event watcher, batch verifier, Anvil integration |
| Python SDK | 449 | Client, verifier, XGBoost, LightGBM, batch, events, CLI |
| **Total** | **948+** | All run in CI |

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    EXECUTION ENGINE                          │
│  requestExecution() → claimExecution() → submitProof()      │
│                                                              │
│  Features: Tip decay, claim windows, callbacks,             │
│  prover stats, multi-backend proof routing                  │
└───────────────┬──────────────────────┬──────────────────────┘
                │                      │
                ▼                      ▼
┌─────────────────────┐    ┌──────────────────────────────────┐
│  PROGRAM REGISTRY   │    │     PROOF VERIFICATION           │
│  • Register/deact   │    │                                  │
│  • Per-program       │    │  ┌────────────────────────────┐  │
│    verifier select  │    │  │ RiscZeroVerifierAdapter     │  │
│                     │    │  │ • Groth16 / STARK           │  │
└─────────────────────┘    │  └────────────────────────────┘  │
                           │  ┌────────────────────────────┐  │
                           │  │ RemainderVerifier           │  │
                           │  │ • GKR+Hyrax (direct)       │  │
                           │  │ • DAG Groth16 (hybrid)     │  │
                           │  │ • Multi-tx batch (15 txs)  │  │
                           │  └────────────────────────────┘  │
                           │  ┌────────────────────────────┐  │
                           │  │ Stylus GKR Verifier (WASM) │  │
                           │  │ • Arbitrum L2 native       │  │
                           │  │ • Single-tx (~24KB)        │  │
                           │  └────────────────────────────┘  │
                           └──────────────────────────────────┘
```

## Remainder On-Chain Verifier (GKR+Hyrax ZKML)

The first EVM verifier for [Remainder_CE](https://github.com/worldcoin/Remainder_CE) — World's GKR+Hyrax proof system optimized for ML inference.

### Verification Modes

| Mode | Gas | Transactions | Use Case |
|------|-----|-------------|----------|
| **Direct GKR** | ~7.6M | 1 | Simple circuits |
| **DAG Direct** | ~213M | 1 (requires >30M gas limit) | Full XGBoost (88 layers) |
| **DAG Batch** | ~13-28M/tx | 15 txs | Production (fits in blocks) |
| **DAG Groth16 Hybrid** | ~252M | 1 | SNARK-compressed |

### Contract Suite (`contracts/src/remainder/`)

| Contract | Purpose |
|----------|---------|
| `RemainderVerifier.sol` | Top-level orchestrator, circuit registry, all verification modes |
| `GKRVerifier.sol` | Layer-by-layer GKR reduction (sumcheck + proof-of-product) |
| `GKRDAGVerifier.sol` | DAG circuit topology (non-linear atom routing, multi-claim RLC) |
| `GKRDAGHybridVerifier.sol` | Transcript replay + Groth16 verification + EC equation checks |
| `DAGBatchVerifier.sol` | Multi-tx batch verification (8 layers/batch, session management) |
| `DAGRemainderGroth16Verifier.sol` | gnark-exported Groth16 verifier (3416 public inputs) |
| `PoseidonSponge.sol` | Fiat-Shamir transcript (t=3, rate=2, 8+57 rounds, BN254) |
| `SumcheckVerifier.sol` | Per-round sumcheck with Lagrange interpolation |
| `HyraxVerifier.sol` | Polynomial commitment via BN254 ecAdd/ecMul precompiles |

### Groth16 Hybrid Pipeline

The hybrid approach moves expensive Fr-field arithmetic off-chain into a Groth16 SNARK:

```
XGBoost Model
  → Rust: build_full_inference_circuit() (88-layer DAG)
  → Rust: gen_dag_groth16_witness (extract challenges + intermediate values)
  → gnark: prove-dag-json (Groth16 proof over Fr arithmetic)
  → Solidity: verifyDAGWithGroth16() (transcript replay + Groth16 + EC checks)
```

### Multi-Tx Batch Verification

For production use within Ethereum's 30M gas block limit:

```
startDAGBatchVerify()         →  Setup + transcript (~17.5M gas)
continueDAGBatchVerify() × 11 →  8 compute layers per batch (~13-28M gas each)
finalizeDAGBatchVerify() × 3   →  Input layer verification (~9-22M gas each)
                                  Total: 15 transactions, all under 30M gas
```

## Stylus GKR Verifier (Arbitrum WASM)

Full port of the GKR DAG verifier to Rust/WASM for Arbitrum Stylus deployment.

- **Location:** `contracts/stylus/gkr-verifier/`
- **Binary size:** ~50KB raw, ~23.7KB Brotli (under 24KB Stylus limit with wasm-opt)
- **Tests:** 85 native tests + WASM build verification
- **Key optimizations:** Shared `#[inline(never)]` field ops, `verify!` macro (strips panic strings on WASM), Knuth's Algorithm D for 512-bit reduction

```bash
# Run tests
cd contracts/stylus/gkr-verifier
cargo test --no-default-features

# Build WASM
cargo build --release --target wasm32-unknown-unknown --lib

# Benchmark on local Arbitrum devnode
./scripts/stylus-benchmark.sh
```

## XGBoost Inference Circuit

Decision tree ensemble inference proved with GKR+Hyrax:

- **Phase 1a:** Leaf selection via path-bit folding + pairwise aggregation
- **Phase 1b:** Comparison verification via bit decomposition (K=18)
- **DAG topology:** 88 compute layers, variable num_vars (0-7) per layer
- **Model import:** `load_xgboost_json()` parses XGBoost's native JSON format

```bash
# Run circuit tests
cd examples/xgboost-remainder
cargo test

# Generate Groth16 witness
cargo run --bin gen_dag_groth16_witness

# Generate Groth16 proof (gnark)
cd gnark-wrapper
cat witness.json | ./gnark-wrapper prove-dag-json --config-json
```

## Quick Start

### Prerequisites

```bash
# Install Foundry
curl -L https://foundry.paradigm.xyz | bash
foundryup

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Run Tests

```bash
# Solidity (167 tests)
cd contracts && forge install foundry-rs/forge-std --no-git && forge install risc0/risc0-ethereum --no-git
forge test -vv

# Remainder Rust (34 tests)
cd examples/xgboost-remainder && cargo test

# Stylus Rust (85 tests)
cd contracts/stylus/gkr-verifier && cargo test --no-default-features

# gnark Go (14 tests)
cd examples/xgboost-remainder/gnark-wrapper && go test -v ./...

# Python SDK (37 tests)
cd sdk/python && pip install -e ".[dev]" && pytest -v
```

### Deploy Contracts

```bash
cd contracts

# Local (Anvil) — uses MockRiscZeroVerifier
PRIVATE_KEY=0x... FEE_RECIPIENT=0x... \
  forge script script/DeployLocal.s.sol:DeployLocalScript --rpc-url http://localhost:8545 --broadcast

# Testnet — uses real RISC Zero verifier router
PRIVATE_KEY=0x... FEE_RECIPIENT=0x... VERIFIER_ADDRESS=0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187 \
  forge script script/DeployTestnet.s.sol:DeployTestnetScript --rpc-url $RPC_URL --broadcast

# Remainder DAG verifier
PRIVATE_KEY=0x... \
  forge script script/DeployRemainder.s.sol:DeployRemainder --rpc-url $RPC_URL --broadcast

# Remainder DAG + on-chain verification
PRIVATE_KEY=0x... VERIFY=true \
  forge script script/DeployRemainder.s.sol:DeployRemainder --rpc-url $RPC_URL --broadcast --gas-limit 5000000000
```

### E2E Tests

```bash
# RISC Zero pipeline (5 examples)
./scripts/e2e-test.sh --example anomaly-detector
./scripts/e2e-test.sh --example rule-engine
./scripts/e2e-test.sh --example xgboost-inference
./scripts/e2e-test.sh --example all
```

## Prover Node

The Rust prover monitors the blockchain and executes jobs:

```bash
cd prover && cargo build --release

./target/release/world-zk-prover run \
  --rpc-url $RPC_URL \
  --private-key $PRIVATE_KEY \
  --engine-address $ENGINE_ADDRESS \
  --min-tip 0.0001 \
  --proving-mode local    # or: bonsai, bonsai-fallback
```

| Proving Mode | Description | Speed |
|-------------|-------------|-------|
| `local` | CPU-based | Development/testing |
| `bonsai` | [Bonsai](https://bonsai.xyz) GPU cloud | Production (10-100x faster) |
| `bonsai-fallback` | Try Bonsai, fall back to local | Hybrid |

## SDKs

| SDK | Install | Docs |
|-----|---------|------|
| **Rust** | `world-zk-sdk` in Cargo.toml | [sdk/README.md](sdk/README.md) |
| **TypeScript** | `npm install @worldzk/sdk` | [sdk/typescript/](sdk/typescript/) |
| **Python** | `pip install worldzk` | [sdk/python/docs/](sdk/python/docs/) |

All SDKs support: TEE verification, event watching, batch verification, hash computation.

## CI

All components tested in GitHub Actions on every push/PR:

| Job | What |
|-----|------|
| `solidity` | forge fmt + forge test (167 tests) |
| `rust` | clippy + cargo test (prover + SDK) |
| `remainder` | fmt + clippy + cargo test (34 tests) |
| `stylus` | fmt + clippy + cargo test + WASM build (85 tests) |
| `gnark` | go test (14 tests) |
| `typescript-sdk` | vitest (199 tests) |
| `python-sdk` | ruff + pytest (449 tests) |
| `e2e-smoke` | Docker Compose E2E |
| `security-audit` | cargo audit + npm audit |

## Deployed Contracts (Sepolia Testnet)

| Contract | Address |
|----------|---------|
| MockRiscZeroVerifier | `0x0D194f172a3a50e0E293d0d8f21774b1a222362E` |
| ProgramRegistry | `0x7F9EFc73E50a4f6ec6Ab7B464f6556a89fDeD3ac` |
| ExecutionEngine | `0x9CFd1CF0e263420e010013373Ec4008d341a483e` |

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — system design and component interaction
- [API Reference](docs/API.md) — REST API for all services
- [Contract Addresses](docs/CONTRACT_ADDRESSES.md) — deployed contract registry
- [SDK Quickstart](docs/SDK_QUICKSTART.md) — getting started with SDKs
- [Gas Optimization](docs/GAS_OPTIMIZATION.md) — gas profiling and optimization notes
- [Threat Model](docs/THREAT_MODEL.md) — security analysis
- [Runbook](docs/RUNBOOK.md) — operational procedures
- [Upgrade Guide](docs/UPGRADE_GUIDE.md) — contract upgrade process
- [Troubleshooting](docs/TROUBLESHOOTING.md) — common issues and fixes

## Roadmap

- [x] Core contracts (Engine, Registry, Verifier routing)
- [x] Tip decay, claim windows, callbacks, prover stats
- [x] Testnet deployment (Sepolia)
- [x] Bonsai cloud proving integration
- [x] Example programs (anomaly detector, sybil detector, rule engine, XGBoost)
- [x] **Remainder on-chain verifier** (GKR+Hyrax — PoseidonSponge, SumcheckVerifier, HyraxVerifier, GKRVerifier)
- [x] **DAG verifier** for multi-layer XGBoost circuits (88 compute layers)
- [x] **Multi-tx batch verification** (15 txs, all under 30M gas block limit)
- [x] **Groth16 hybrid verification** (gnark SNARK wrapping, 3416 public inputs)
- [x] **Stylus WASM port** (full GKR DAG verifier on Arbitrum, ~23.7KB Brotli)
- [x] **TEE + ZK dispute system** (AWS Nitro attestation, operator service)
- [x] **Multi-language SDKs** (Rust, TypeScript, Python — 948+ tests)
- [x] **Production hardening** (CI/CD, monitoring, Docker Compose, Helm charts)
- [ ] Arbitrum Stylus testnet deployment
- [ ] Batch verification orchestrator SDK
- [ ] World Chain mainnet deployment
- [ ] Prover network incentives

## License

Apache-2.0

## Links

- [Bonsol](https://bonsol.sh) — Inspiration
- [RISC Zero](https://risczero.com) — zkVM
- [World Chain](https://world.org) — Target L2
- [Remainder_CE](https://github.com/worldcoin/Remainder_CE) — GKR+Hyrax proof system for ZKML
- [Foundry](https://book.getfoundry.sh) — Development framework
- [Arbitrum Stylus](https://docs.arbitrum.io/stylus/gentle-introduction) — WASM smart contracts
