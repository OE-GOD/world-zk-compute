# Detection Algorithm Examples

This directory contains example detection algorithms that run on World ZK Compute.

## How It Works

```
┌─────────────────────────────────────────────────────────────────┐
│                    YOUR DETECTION ALGORITHM                      │
│                                                                  │
│  1. Write your algorithm as a RISC Zero guest program           │
│  2. Register it with ProgramRegistry                            │
│  3. Submit detection jobs via ExecutionEngine                   │
│  4. Provers execute and generate cryptographic proofs           │
│  5. Results are verified on-chain                               │
│                                                                  │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │  Input   │ →  │  Guest   │ →  │  Proof   │ →  │ On-chain │  │
│  │  Data    │    │ Program  │    │  (seal)  │    │ Verified │  │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Examples

### 1. Anomaly Detector (`anomaly-detector/`)

Statistical anomaly detection using z-score analysis.

**Use cases:**
- Detect unusual registration patterns
- Flag suspicious operator behavior
- Identify temporal anomalies

**Precompiles used:** SHA-256 (accelerated)

**Run the example:**
```bash
cd anomaly-detector
cargo run --bin anomaly-detector-host
```

### 2. Signature-Verified Detector (`signature-verified/`)

Detection with cryptographic verification of data source.

**Use cases:**
- Verify orb operator signed the biometric data
- Chain of custody verification
- Trusted data source validation

**Precompiles used:** SHA-256 + secp256k1 ECDSA (both accelerated, ~100x faster)

**Key features:**
- Verifies Ethereum ECDSA signature before processing
- Derives signer address from public key
- Only processes data from verified sources

**Run the example:**
```bash
cd signature-verified
cargo run --bin signature-verified-host
```

## Writing Your Own Detection Algorithm

### Step 1: Create Guest Program

The guest program runs inside the zkVM. It receives private input and produces public output.

```rust
// methods/guest/src/main.rs
#![no_main]
#![no_std]

use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

fn main() {
    // Read private input
    let input: YourInput = env::read();

    // Run your detection algorithm
    let result = your_detection_algorithm(&input);

    // Commit public output (this goes in the proof journal)
    env::commit(&result);
}
```

### Step 2: Create Host Program

The host program prepares input and submits jobs to the network.

```rust
// host/src/main.rs
async fn main() {
    // 1. Prepare detection input
    let input = prepare_input();

    // 2. Upload input data
    let input_url = upload_to_ipfs(&input).await;
    let input_hash = sha256(&input);

    // 3. Submit to World ZK Compute
    let engine = ExecutionEngine::new(ENGINE_ADDRESS);
    let request_id = engine
        .requestExecution(
            IMAGE_ID,      // Your program's image ID
            input_hash,    // Hash of input data
            input_url,     // Where to fetch input
            callback,      // Contract to receive results
            max_delay,     // Maximum time to wait
        )
        .value(bounty)     // Payment for provers
        .send()
        .await?;

    // 4. Wait for proof...
    // 5. Results are automatically verified on-chain!
}
```

### Step 3: Register Your Program

```bash
# Compute image ID from your guest ELF
IMAGE_ID=$(risc0-zkvm compute-image-id target/riscv-guest/release/your-guest)

# Register with ProgramRegistry
cast send $REGISTRY "registerProgram(bytes32,string,string,bytes32)" \
    $IMAGE_ID \
    "Your Detection Algorithm" \
    "https://your-storage/program.elf" \
    0x0 \
    --rpc-url $RPC_URL \
    --private-key $PRIVATE_KEY
```

### Step 4: Submit Detection Jobs

```bash
# Request execution
cast send $ENGINE "requestExecution(bytes32,bytes32,string,address,uint256)" \
    $IMAGE_ID \
    $INPUT_HASH \
    "ipfs://your-input-cid" \
    $CALLBACK_ADDRESS \
    3600 \
    --value 0.01ether \
    --rpc-url $RPC_URL \
    --private-key $PRIVATE_KEY
```

## Detection Algorithm Templates

### Sybil Detection
```rust
fn detect_sybil(registrations: &[Registration]) -> Vec<SybilCluster> {
    // Cluster by:
    // - Registration time patterns
    // - Geographic proximity
    // - Device fingerprints
    // - Behavioral patterns
}
```

### Presentation Attack Detection
```rust
fn detect_presentation_attack(biometric: &BiometricData) -> AttackScore {
    // Check for:
    // - Liveness indicators
    // - Image manipulation
    // - Replay attacks
    // - Synthetic media
}
```

### Operator Fraud Detection
```rust
fn detect_operator_fraud(operator_data: &OperatorData) -> FraudScore {
    // Analyze:
    // - Verification success rates
    // - Geographic patterns
    // - Temporal patterns
    // - Device consistency
}
```

## Input/Output Format Standards

### Input Format
```rust
struct DetectionInput<T> {
    // Data to analyze
    data: Vec<T>,
    // Detection parameters
    params: DetectionParams,
    // Threshold for flagging
    threshold: f64,
}
```

### Output Format
```rust
struct DetectionOutput {
    // Summary statistics
    total_analyzed: usize,
    anomalies_found: usize,
    // Flagged items (by ID hash, not raw data)
    flagged_ids: Vec<[u8; 32]>,
    // Overall risk score
    risk_score: f64,
    // Input hash for verification
    input_hash: [u8; 32],
}
```

## Privacy Guarantees

1. **Input data stays private** - Provers execute without seeing raw data
2. **Only results are public** - The proof journal contains only flagged IDs and scores
3. **Cryptographic verification** - Results are mathematically proven correct
4. **No trust required** - Anyone can verify the proof on-chain

## Performance Tips

1. **Use Precompiles** - 10-100x faster crypto operations (see `sdk/PRECOMPILES.md`)
2. **Use Bonsai** - 10-100x faster than local proving
3. **Use GPU** - Build with `--features cuda` or `--features metal`
4. **Batch inputs** - Process multiple items in one proof
5. **Optimize algorithm** - Fewer cycles = faster proofs
6. **Use SNARK conversion** - 90% cheaper on-chain verification

## Precompiles Guide

**IMPORTANT**: Use RISC Zero's accelerated crypto crates for 10-100x speedup!

See **[sdk/PRECOMPILES.md](../sdk/PRECOMPILES.md)** for the complete guide.

Quick example - in your guest's `Cargo.toml`:

```toml
# SLOW: Standard SHA-256 (~500K cycles)
# sha2 = "0.10"

# FAST: Accelerated SHA-256 (~5K cycles) - 100x faster!
sha2 = { git = "https://github.com/risc0/RustCrypto-hashes", tag = "sha2-v0.10.8-risczero.0" }

# FAST: Accelerated Ethereum ECDSA - 100x faster!
k256 = { git = "https://github.com/risc0/RustCrypto-elliptic-curves", tag = "k256-v0.13.4-risczero.1", features = ["ecdsa", "unstable"] }
```
