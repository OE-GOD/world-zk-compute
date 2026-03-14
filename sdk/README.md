# World ZK Compute - Rust SDK

Rust SDK for interacting with World ZK Compute contracts on-chain. Provides DAG circuit verification (single-tx and multi-tx batch), TEE result submission and dispute resolution, on-chain event watching, and hash utilities compatible with the TEE enclave.

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
world-zk-sdk = { path = "../sdk" }
tokio = { version = "1.0", features = ["full"] }
anyhow = "1.0"
```

When the crate is published, replace the path dependency with a version:

```toml
[dependencies]
world-zk-sdk = "0.1"
```

## Quick Start

Connect to a local Anvil node and verify a DAG proof in a single transaction:

```rust
use world_zk_sdk::{Client, DAGVerifier, DAGFixture};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Create a client (RPC URL, private key, contract address)
    let client = Client::new(
        "http://localhost:8545",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "0x5FbDB2315678afecb367f032d93F642f64180aa3",
    )?;

    // 2. Load a proof fixture
    let fixture = DAGFixture::load("path/to/fixture.json")?;
    let proof = fixture.to_proof_data()?;

    // 3. Verify
    let verifier = DAGVerifier::new(client);
    let valid = verifier.verify_single_tx(&proof).await?;
    println!("Proof valid: {valid}");

    Ok(())
}
```

## API Reference

### Client

`Client` wraps an Ethereum provider, wallet, and contract address.

```rust
use world_zk_sdk::Client;

let client = Client::new(rpc_url, private_key, contract_address)?;

// Accessors
let addr = client.contract_address();
let signer = client.signer_address();
```

| Method | Description |
|---|---|
| `Client::new(rpc_url, private_key, contract_address)` | Create a new client from RPC URL, hex private key, and contract address |
| `client.contract_address()` | Returns the target contract address |
| `client.signer_address()` | Returns the signer (sender) address |

### DAGVerifier

`DAGVerifier` orchestrates on-chain DAG circuit verification against the `RemainderVerifier` contract.

```rust
use world_zk_sdk::DAGVerifier;

let verifier = DAGVerifier::new(client);
```

| Method | Description |
|---|---|
| `DAGVerifier::new(client)` | Create a verifier from a `Client` |
| `verify_single_tx(&proof)` | Verify a proof in one call (view function, 500M gas) |
| `verify_stylus(&proof)` | Verify via the Stylus (WASM) verifier |
| `verify_batch(&proof, on_progress)` | Multi-tx batch verification with progress callbacks |
| `register_circuit(hash, desc, name, gens)` | Register a DAG circuit on-chain |
| `set_stylus_verifier(hash, address)` | Set the Stylus verifier for a circuit |
| `set_groth16_verifier(hash, address, input_count)` | Set the Groth16 verifier for a circuit |
| `get_session_status(session_id)` | Query batch session status |
| `is_circuit_active(circuit_hash)` | Check if a circuit is registered and active |

### DAGFixture and ProofData

`DAGFixture` loads test fixtures from JSON and converts them to `ProofData` for contract calls.

```rust
use world_zk_sdk::{DAGFixture, ProofData};

let fixture = DAGFixture::load("contracts/test/fixtures/phase1a_dag_fixture.json")?;
let proof: ProofData = fixture.to_proof_data()?;
let desc = fixture.to_dag_description()?;
let encoded = fixture.encode_dag_description()?;
```

| Method | Description |
|---|---|
| `DAGFixture::load(path)` | Load a fixture from a JSON file |
| `to_proof_data()` | Convert to `ProofData` (hex-decoded bytes) |
| `to_dag_description()` | Convert to the ABI `DAGCircuitDescription` struct |
| `encode_dag_description()` | ABI-encode the circuit description |

### BatchSession and BatchProgress

These types track multi-tx batch verification state.

```rust
use world_zk_sdk::{BatchSession, BatchProgress};
```

**`BatchSession`** fields:
- `circuit_hash: B256` -- circuit identifier
- `next_batch_idx: u64` -- next compute batch to process
- `total_batches: u64` -- total number of compute batches
- `finalized: bool` -- whether finalization is complete
- `finalize_input_idx: u64` -- current finalization input index
- `finalize_groups_done: u64` -- number of input groups finalized

**`BatchProgress`** variants:
- `Started { session_id, total_batches }` -- session created
- `Computing { batch, total }` -- processing compute batch
- `Finalizing { step }` -- running finalization step
- `Complete` -- verification finished

### TEEVerifier

`TEEVerifier` wraps the `TEEMLVerifier` contract for TEE-attested ML inference.

| Method | Description |
|---|---|
| `TEEVerifier::new(client)` | Create a TEE verifier |
| `register_enclave(key, image_hash)` | Register a TEE enclave key |
| `revoke_enclave(key)` | Revoke an enclave key |
| `submit_result(model_hash, input_hash, result, attestation, stake)` | Submit a TEE result with stake |
| `challenge(result_id, bond)` | Challenge a submitted result |
| `finalize(result_id)` | Finalize after challenge window |
| `resolve_dispute(result_id, proof, circuit_hash, pub_inputs, gens)` | Resolve dispute with ZK proof |
| `resolve_dispute_by_timeout(result_id)` | Resolve by timeout |
| `get_result(result_id)` | Query result struct |
| `is_result_valid(result_id)` | Check if result is valid |
| `owner()` / `pending_owner()` | Query ownership |
| `transfer_ownership(new_owner)` / `accept_ownership()` | Transfer ownership (Ownable2Step) |
| `pause()` / `unpause()` / `paused()` | Pause control |
| `remainder_verifier()` | Get the linked RemainderVerifier address |

### TEEEventWatcher

`TEEEventWatcher` polls on-chain events emitted by the `TEEMLVerifier` contract.

| Method | Description |
|---|---|
| `TEEEventWatcher::new(rpc_url, contract_address)` | Create a watcher |
| `poll_events(from_block)` | One-shot poll, returns `(Vec<TEEEvent>, next_block)` |
| `watch(callback, poll_interval)` | Start continuous background polling |
| `stop()` | Signal the background loop to stop |
| `is_stopped()` | Check if stop was signaled |
| `contract_address()` | Returns the watched contract address |

**`TEEEvent`** variants:
- `ResultSubmitted { result_id, model_hash, input_hash, submitter, block_number }`
- `ResultChallenged { result_id, challenger, block_number }`
- `ResultFinalized { result_id, block_number }`
- `ResultExpired { result_id, block_number }`

### Hash Utilities

Functions for computing hashes compatible with the TEE enclave.

```rust
use world_zk_sdk::{
    compute_model_hash, compute_model_hash_from_file,
    compute_input_hash, compute_input_hash_from_json,
    compute_result_hash, compute_result_hash_from_bytes,
};
```

| Function | Description |
|---|---|
| `compute_model_hash(bytes)` | `keccak256(raw_file_bytes)` |
| `compute_model_hash_from_file(path)` | Read file and hash contents |
| `compute_input_hash(features)` | `keccak256(serde_json::to_vec(features))` |
| `compute_input_hash_from_json(json_bytes)` | Hash pre-serialized JSON bytes |
| `compute_result_hash(scores)` | `keccak256(serde_json::to_vec(scores))` |
| `compute_result_hash_from_bytes(bytes)` | Hash raw result bytes |

### Retry Utilities

Exponential backoff for resilient RPC communication.

```rust
use world_zk_sdk::{retry_with_backoff, RetryPolicy, is_retryable};
```

| Item | Description |
|---|---|
| `RetryPolicy` | Configure `max_retries`, `base_delay`, `max_delay`, `jitter` |
| `RetryPolicy::default()` | 3 retries, 1s base, 30s max, 0.2 jitter |
| `retry_with_backoff(policy, async_fn)` | Execute with retries on transient errors |
| `is_retryable(err)` | Returns true for connection errors, 429/5xx, timeouts |

### Modules

| Module | Description |
|---|---|
| `abi` | Solidity ABI bindings (`RemainderVerifier`, `TEEMLVerifier`) |
| `client` | `Client` type (provider + wallet + address) |
| `event_watcher` | `TEEEventWatcher` and `TEEEvent` types |
| `fixture` | `DAGFixture` and `ProofData` for loading test data |
| `hash` | Keccak256 hash utilities matching enclave behavior |
| `networks` | Network configuration constants |
| `precompiles` | EVM precompile helpers |
| `retry` | `RetryPolicy` and `retry_with_backoff` |
| `tee` | `TEEVerifier` for TEE result lifecycle |
| `verifier` | `DAGVerifier`, `BatchSession`, `BatchProgress` |

## Examples

### TEE Verification

Submit a TEE-attested ML inference result and handle the challenge lifecycle:

```rust
use alloy::primitives::U256;
use world_zk_sdk::{Client, TEEVerifier, compute_model_hash, compute_input_hash};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::new(
        "http://localhost:8545",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512",
    )?;

    let tee = TEEVerifier::new(client);

    // Compute hashes
    let model_hash = compute_model_hash(b"model file bytes");
    let input_hash = compute_input_hash(&[1.0, 2.5, 3.7]);

    // Submit result with 0.01 ETH stake
    let stake = U256::from(10_000_000_000_000_000u64); // 0.01 ETH
    let tx = tee.submit_result(
        model_hash,
        input_hash,
        b"result bytes",
        b"attestation bytes",
        stake,
    ).await?;
    println!("Submitted: {tx}");

    // Later: finalize after challenge window
    // let tx = tee.finalize(result_id).await?;

    // Query result validity
    // let valid = tee.is_result_valid(result_id).await?;

    Ok(())
}
```

### Event Watching

Watch for on-chain TEE events in real time:

```rust
use alloy::primitives::Address;
use world_zk_sdk::TEEEventWatcher;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let contract: Address = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512".parse()?;
    let watcher = TEEEventWatcher::new("http://localhost:8545", contract);

    // One-shot poll from block 0
    let (events, next_block) = watcher.poll_events(0).await?;
    println!("Found {} events, next block: {}", events.len(), next_block);

    // Continuous background watch
    let handle = watcher.watch(
        |event| println!("Event: {:?}", event),
        Duration::from_secs(2),
    );

    // Run for a while, then stop
    tokio::time::sleep(Duration::from_secs(30)).await;
    watcher.stop();
    handle.await?;

    Ok(())
}
```

### Batch Verification

Verify a large DAG proof across multiple transactions (stays under the 30M gas block limit):

```rust
use world_zk_sdk::{Client, DAGVerifier, DAGFixture, BatchProgress};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::new(
        "http://localhost:8545",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "0x5FbDB2315678afecb367f032d93F642f64180aa3",
    )?;

    let fixture = DAGFixture::load("path/to/fixture.json")?;
    let proof = fixture.to_proof_data()?;

    let verifier = DAGVerifier::new(client);

    // verify_batch handles start -> continue x N -> finalize x M -> cleanup
    let session_id = verifier.verify_batch(&proof, |progress| {
        match progress {
            BatchProgress::Started { session_id, total_batches } => {
                println!("Session {session_id} started ({total_batches} batches)");
            }
            BatchProgress::Computing { batch, total } => {
                println!("Computing batch {batch}/{total}");
            }
            BatchProgress::Finalizing { step } => {
                println!("Finalizing step {step}");
            }
            BatchProgress::Complete => {
                println!("Verification complete");
            }
        }
    }).await?;

    println!("Session ID: {session_id}");
    Ok(())
}
```

### Retry with Backoff

Wrap RPC calls with automatic retry on transient failures:

```rust
use world_zk_sdk::{retry_with_backoff, RetryPolicy};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let policy = RetryPolicy {
        max_retries: 5,
        base_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(10),
        jitter: 0.2,
    };

    let result = retry_with_backoff(&policy, || async {
        // Your RPC call here
        Ok::<_, anyhow::Error>(42)
    }).await?;

    println!("Result: {result}");
    Ok(())
}
```

## Running Tests

Unit tests (no network required):

```bash
cd sdk
cargo test
```

Integration tests require a running Anvil instance with deployed contracts:

```bash
# Terminal 1: start Anvil
anvil

# Terminal 2: deploy contracts and run tests
cd sdk
cargo test -- --ignored
```

## Related SDKs

- **TypeScript SDK**: [`sdk/typescript/`](../sdk/typescript/) -- npm package for browser and Node.js
- **Python SDK**: [`sdk/python/`](../sdk/python/) -- pip package with async support and CLI

## License

Apache-2.0
