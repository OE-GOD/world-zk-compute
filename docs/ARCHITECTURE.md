# World ZK Compute -- System Architecture

## Overview

World ZK Compute provides verifiable ML inference on-chain. The system uses a
TEE (Trusted Execution Environment) happy path for cheap, fast results and a
ZK (Zero-Knowledge) dispute path to guarantee correctness without trust.

Two verification modes exist side by side:

1. **TEE + ZK Dispute** -- TEE enclave runs inference and signs an ECDSA
   attestation. Results can be challenged; disputes are resolved by a full
   GKR/Hyrax ZK proof verified on-chain.
2. **RISC Zero zkVM** -- General-purpose verifiable compute via risc0-zkvm v3.0.
   The `ExecutionEngine` contract handles request/claim/prove lifecycle with
   RISC Zero or custom verifiers (Remainder, eZKL, etc.).

```
                           +------------------+
                           |   User / Client  |
                           |  (SDK or script)  |
                           +--------+---------+
                                    |
              +-----------+---------+-----------+----------+
              |           |                     |          |
              v           v                     v          v
        +-----------+ +--------+          +-----------+ +--------+
        | TEE       | |Operator|          | Warm      | | Anvil  |
        | Enclave   | |Service |          | Prover    | | / Node |
        | (ML infer)| |(watch +|          | (GKR/     | +---+----+
        |           | | submit)|          |  Hyrax)   |     |
        +-----+-----+ +---+----+          +-----+-----+     |
              |            |                     |           |
              |   ECDSA    |  resolve_dispute()  |  ZK proof |
              |   attest.  |  finalize()         |           |
              |            |                     |           |
              v            v                     v           v
        +----------------------------------------------------+
        |                  Smart Contracts                    |
        |                                                    |
        |  TEEMLVerifier          RemainderVerifier           |
        |  - submitResult()       - verifyDAGProof()          |
        |  - challenge()          - GKRDAGVerifier (library)  |
        |  - resolveDispute()     - DAGBatchVerifier          |
        |  - finalize()           - PoseidonSponge            |
        |                         - HyraxVerifier             |
        |  ExecutionEngine        - SumcheckVerifier          |
        |  - requestExecution()   - Groth16Verifier (optional)|
        |  - claimExecution()                                 |
        |  - submitProof()                                    |
        +----------------------------------------------------+
```

## Happy Path Flow (TEE)

```
  User          Enclave         Operator        TEEMLVerifier     Chain
   |               |               |                |               |
   |-- POST /infer ->              |                |               |
   |               |-- inference --|                |               |
   |               |-- sign(model, input, result)   |               |
   |<- attestation-|               |                |               |
   |               |               |                |               |
   |------- submitResult(modelHash, inputHash, result, attestation) -->
   |               |               |                |-- verify sig  |
   |               |               |                |-- store result|
   |               |               |                |-- start 1h    |
   |               |               |                |   window      |
   |               |               |                |               |
   |            ... 1 hour passes, no challenge ...                 |
   |               |               |                |               |
   |------- finalize(resultId) --->|                |               |
   |               |               |                |-- return stake|
   |               |               |                |-- emit event  |
```

1. User sends feature vector to TEE Enclave via HTTP (`POST /infer`).
2. Enclave runs XGBoost/LightGBM inference inside a TEE (AWS Nitro).
3. Enclave ECDSA-signs `keccak256(modelHash || inputHash || resultHash)`.
4. Submitter calls `submitResult()` with ETH stake (default 0.1 ETH).
   Contract verifies the signature against a registered enclave key.
5. A 1-hour challenge window opens. If nobody challenges, the submitter
   calls `finalize()` to reclaim their stake.

## Dispute Path Flow

```
  Challenger      Operator       WarmProver    TEEMLVerifier   RemainderVerifier
   |                |               |               |               |
   |-- challenge(resultId, bond) ------------------>|               |
   |                |               |               |-- 24h window  |
   |                |               |               |-- emit event  |
   |                |<-- ResultChallenged event -----|               |
   |                |               |               |               |
   |                |-- POST /prove ->              |               |
   |                |<- proof_hex, circuit_hash, pub_inputs --------|
   |                |               |               |               |
   |                |-- resolveDispute(proof, circuitHash, ...) --->|
   |                |               |               |-- staticcall ->
   |                |               |               |    verifyDAGProof()
   |                |               |               |<-- bool valid -|
   |                |               |               |               |
   |                |               |               |-- settle:     |
   |                |               |               |   valid=true  |
   |                |               |               |   -> submitter|
   |                |               |               |     gets both |
   |                |               |               |   valid=false |
   |                |               |               |   -> challenger|
   |                |               |               |     gets both |
```

1. Challenger calls `challenge(resultId)` with ETH bond (default 0.1 ETH).
   A 24-hour dispute window opens (extendable once by 30 minutes).
2. The Operator Service detects the `ResultChallenged` event via polling.
3. Operator requests a ZK proof from the Warm Prover (pre-built GKR circuit).
4. Operator calls `resolveDispute()` with proof data.
5. `TEEMLVerifier` delegates to `RemainderVerifier.verifyDAGProof()` via
   `staticcall`. This runs GKR sumcheck + Hyrax commitment verification.
6. If the proof is valid, the submitter wins both stakes. Otherwise, the
   challenger wins both stakes.
7. If no proof is submitted within the dispute window, the challenger can
   call `resolveDisputeByTimeout()` to win by default.

## Component Descriptions

### TEE Enclave (`tee/enclave/`)

HTTP server running inside a TEE (AWS Nitro Enclave). Loads an XGBoost or
LightGBM model, runs inference on feature vectors, and returns an ECDSA-signed
attestation. Supports hot model reload, rate limiting, and Nitro attestation
document generation for on-chain registration.

- Entry point: `tee/enclave/src/main.rs`
- Endpoints: `GET /health`, `POST /infer`, `GET /info`, `GET /attestation`
- Config: `MODEL_PATH`, `ENCLAVE_PRIVATE_KEY`, `PORT`

### Operator Service (`services/operator/`)

Long-running daemon that bridges off-chain services with on-chain contracts.
Three modes: `submit` (single inference), `watch` (event polling loop), and
`run` (submit + watch combined). Watches for `ResultChallenged` events,
loads pre-computed proofs or generates them on demand, and calls
`resolveDispute()` automatically. Also auto-finalizes unchallenged results.

- Entry point: `services/operator/src/main.rs`
- Subcommands: `submit`, `watch`, `run`, `register`, `models`
- Config: `OPERATOR_RPC_URL`, `OPERATOR_PRIVATE_KEY`, `ENCLAVE_URL`,
  `PROVER_URL`, `PROOFS_DIR`

### Warm Prover (`examples/xgboost-remainder/`)

HTTP server that pre-builds the GKR circuit and Pedersen generators on
startup, then serves ZK proof requests with sub-second latency. Supports
single and batch proving, API key auth, rate limiting, CORS, and request
timeouts.

- Entry point: `examples/xgboost-remainder/src/server.rs`
- Endpoints: `GET /health`, `POST /prove`, `POST /prove/batch`
- Circuit: `src/circuit.rs` -- XGBoost tree inference circuit (leaf
  selection + comparison verification)

### TEEMLVerifier Contract (`contracts/src/tee/TEEMLVerifier.sol`)

Solidity contract implementing the TEE attestation + ZK dispute protocol.
Manages enclave registration, result submission with stake, challenge bonds,
dispute resolution via RemainderVerifier, and timeout-based resolution.
Uses OpenZeppelin Ownable2Step, Pausable, and ReentrancyGuard.

### RemainderVerifier Contract (`contracts/src/remainder/RemainderVerifier.sol`)

On-chain verifier for Remainder GKR+Hyrax proofs. Supports both simple
circuits and DAG circuits (88-layer XGBoost). Includes optional Groth16
hybrid mode where expensive EC operations are wrapped in a succinct proof.

Key libraries:
- `GKRDAGVerifier.sol` -- DAG circuit verification (multi-layer, multi-claim)
- `DAGBatchVerifier.sol` -- Multi-tx batched verification (15 transactions
  for 88-layer XGBoost, each under 30M gas)
- `PoseidonSponge.sol` -- Poseidon hash (Fiat-Shamir transcript)
- `HyraxVerifier.sol` -- Hyrax polynomial commitment verification
- `SumcheckVerifier.sol` -- Sumcheck protocol verification

### ExecutionEngine Contract (`contracts/src/ExecutionEngine.sol`)

General-purpose verifiable compute engine using risc0-zkvm. Handles the full
request lifecycle: request (with tip) -> claim (prover) -> prove (verify
on-chain) -> callback. Supports custom verifiers per program (Remainder,
eZKL, etc.) via `ProgramRegistry`, tip decay over time, protocol fees, and
prover reputation tracking.

### Rust SDK (`sdk/`)

Client library for interacting with both `TEEMLVerifier` and
`RemainderVerifier` contracts. Provides `TEEVerifier` (submit, challenge,
finalize, resolve dispute, admin) and `DAGVerifier` (single-tx and multi-tx
batch verification). Also includes hash utilities for computing model, input,
and result hashes.

- `sdk/src/tee.rs` -- TEEVerifier (TEEMLVerifier wrapper)
- `sdk/src/verifier.rs` -- DAGVerifier (RemainderVerifier wrapper)
- `sdk/src/hash.rs` -- Hash computation utilities
- `sdk/src/client.rs` -- Alloy-based RPC client

## Data Flow

### Identifiers

| Field | Type | Computation |
|-------|------|-------------|
| `modelHash` | `bytes32` | `keccak256(model_file_bytes)` |
| `inputHash` | `bytes32` | `keccak256(abi.encode(features))` |
| `resultHash` | `bytes32` | `keccak256(result_bytes)` |
| `resultId` | `bytes32` | `keccak256(submitter, modelHash, inputHash, blockNumber)` |
| `circuitHash` | `bytes32` | Poseidon hash of the GKR circuit structure |

### Attestation Format

The TEE enclave produces an ECDSA signature (EIP-191) over:

```
message = keccak256(modelHash || inputHash || resultHash)
signature = sign(keccak256("\x19Ethereum Signed Message:\n32" || message))
```

The contract recovers the signer via `ecrecover` and checks it against
registered enclave keys.

### ZK Proof Format

The GKR+Hyrax proof is ABI-encoded and contains:

- Per-layer sumcheck proofs (polynomials, claims)
- Hyrax commitment openings (Pedersen commitments, evaluation proofs)
- Public input values and MLE evaluations
- Circuit description (topology, oracle expressions, point templates)

## Service Topology (Docker Compose)

```
+----------+     +----------+     +-----------+     +----------+
|  Anvil   |<----|Deployer  |     |  Enclave  |     |  Warm    |
|  :8545   |     |(run-once)|     |  :8080    |     |  Prover  |
+----+-----+     +----------+     +-----+-----+     |  :3000   |
     |                                  |            +----+-----+
     |           +----------+           |                 |
     +-----------| Operator |<----------+-----------------+
                 |  :9090   |
                 +----------+
```

- **anvil**: Local Ethereum node (gas limit 500M for large proofs)
- **deployer**: One-shot container that deploys contracts and writes
  addresses to a shared volume
- **enclave**: TEE enclave simulator (port 8080)
- **warm-prover**: ZK proof server (port 3000)
- **operator**: Event watcher + auto-dispute resolver (port 9090, metrics)

## Cost Profile

| Operation | Gas Cost | Notes |
|-----------|----------|-------|
| `submitResult()` | ~50K | ECDSA verification + storage |
| `challenge()` | ~50K | Bond escrow + storage update |
| `finalize()` | ~30K | Stake return |
| `resolveDispute()` | ~254M | Full GKR DAG proof (needs Arbitrum or batching) |
| Batch verify (15 txs) | ~13-28M each | Fits within 30M block gas limit |
| Groth16 hybrid | ~51M | Groth16 verification of EC checks |

## Sepolia Testnet Deployment Topology

```
                    Ethereum Sepolia (Chain ID: 11155111)
                    ┌──────────────────────────────────┐
                    │  TEEMLVerifier                    │
                    │  ExecutionEngine                  │
                    │  ProgramRegistry                  │
                    │  (verifier router: 0x925d...187)  │
                    └────────────┬─────────────────────┘
                                 │
            ┌────────────────────┼────────────────────┐
            │                    │                    │
   ┌────────▼──────┐   ┌────────▼──────┐   ┌────────▼──────┐
   │   Operator    │   │   Enclave     │   │  Warm Prover  │
   │   :9090       │──▶│   :8080       │   │  :3000        │
   │  (event poll) │   │  (inference)  │   │  (ZK proofs)  │
   └───────────────┘   └──────────────┘   └───────────────┘
            │
   ┌────────▼──────┐
   │   Indexer     │
   │   :8081       │
   │  (query API)  │
   └───────────────┘
```

**Sepolia-specific constraints:**

- TEE-only mode: RemainderVerifier exceeds Sepolia's 24KB contract size limit
- ZK dispute path available only on Arbitrum Sepolia (421614) or via batch verification
- Verifier router `0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187` supports risc0 v3.0.x selector
- Operator polls Sepolia at ~12s intervals (matching block time)

**Deployment methods:**

| Method | Command |
|--------|---------|
| Docker Compose | `docker compose -f docker-compose.sepolia.yml --env-file .env.sepolia up -d` |
| Helm | `helm install worldzk deploy/helm/worldzk -f deploy/helm/worldzk/values-sepolia.yaml` |
| Kustomize | `kubectl apply -k deploy/k8s/overlays/sepolia/` |
