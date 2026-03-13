# SDK Quickstart Guide

World ZK Compute provides SDKs in **Rust**, **Python**, and **TypeScript** for interacting with the TEEMLVerifier contract on-chain.

## Installation

### Rust

```toml
# Cargo.toml
[dependencies]
world-zk-sdk = { path = "../sdk" }
tokio = { version = "1.0", features = ["full"] }
```

### Python

```bash
pip install worldzk[web3]
```

Requires Python 3.9+. The `[web3]` extra installs `web3` and `eth-account`.

### TypeScript

```bash
npm install @worldzk/sdk
# or
pnpm add @worldzk/sdk
```

Requires Node.js 18.3+. Uses `viem` internally.

---

## Connect to Contract

### Rust

```rust
use world_zk_sdk::{Client, TEEVerifier};

let client = Client::new(
    "http://localhost:8545",                              // RPC URL
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80", // Private key
    "0x5FbDB2315678afecb367f032d93F642f64180aa3",        // Contract address
)?;

let verifier = TEEVerifier::new(client);
```

### Python

```python
from worldzk import TEEVerifier

verifier = TEEVerifier(
    rpc_url="http://localhost:8545",
    private_key="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    contract_address="0x5FbDB2315678afecb367f032d93F642f64180aa3",
)
```

### TypeScript

```typescript
import { TEEVerifier } from '@worldzk/sdk';

const verifier = new TEEVerifier({
  rpcUrl: 'http://localhost:8545',
  privateKey: '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80',
  contractAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
  chainId: 31337, // optional, defaults to 31337 (Anvil)
});
```

---

## Register an Enclave (Admin)

### Rust

```rust
use alloy::primitives::{Address, B256};

let enclave_key: Address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8".parse()?;
let image_hash = B256::ZERO; // TEE enclave image hash (PCR0)

let tx_hash = verifier.register_enclave(enclave_key, image_hash).await?;
println!("Registered enclave: {tx_hash}");
```

### Python

```python
tx_hash = verifier.register_enclave(
    enclave_key="0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    image_hash=b"\x00" * 32,
)
print(f"Registered: {tx_hash}")
```

### TypeScript

```typescript
const txHash = await verifier.registerEnclave(
  '0x70997970C51812dc3A010C7d01b50e0d17dc79C8',
  '0x0000000000000000000000000000000000000000000000000000000000000000',
);
console.log('Registered:', txHash);
```

---

## Submit a Result

Submit a TEE-attested ML inference result with a prover stake.

### Rust

```rust
use alloy::primitives::{B256, U256, keccak256};

let model_hash = keccak256(b"my-model-v1");
let input_hash = keccak256(b"input-data");
let result = b"prediction: 0.95";
let attestation = b"tee-signature-bytes";
let stake = U256::from(100_000_000_000_000_000u64); // 0.1 ETH

let tx_hash = verifier
    .submit_result(model_hash, input_hash, result, attestation, stake)
    .await?;
println!("Submitted: {tx_hash}");
```

### Python

```python
from hashlib import sha256

model_hash = sha256(b"my-model-v1").digest()
input_hash = sha256(b"input-data").digest()
result = b"prediction: 0.95"
attestation = b"tee-signature-bytes"

result_id = verifier.submit_result(
    model_hash=model_hash,
    input_hash=input_hash,
    result=result,
    attestation=attestation,
    stake_wei=100_000_000_000_000_000,  # 0.1 ETH
)
print(f"Result ID: {result_id}")
```

### TypeScript

```typescript
import { keccak256, toHex } from 'viem';

const modelHash = keccak256(toHex('my-model-v1'));
const inputHash = keccak256(toHex('input-data'));
const result = '0x70726564696374696f6e3a20302e3935';  // hex-encoded
const attestation = '0xdeadbeef';

const txHash = await verifier.submitResult(
  modelHash,
  inputHash,
  result,
  attestation,
  '0.1', // stake in ETH (string)
);
console.log('Submitted:', txHash);
```

---

## Challenge a Result

Challenge a result you believe is incorrect by posting a bond.

### Rust

```rust
let result_id: B256 = /* from ResultSubmitted event */;
let bond = U256::from(100_000_000_000_000_000u64); // 0.1 ETH

let tx_hash = verifier.challenge(result_id, bond).await?;
```

### Python

```python
tx_hash = verifier.challenge(
    result_id=result_id,
    bond_wei=100_000_000_000_000_000,
)
```

### TypeScript

```typescript
const txHash = await verifier.challenge(resultId, '0.1');
```

---

## Finalize a Result

After the challenge window (default: 1 hour) passes without a challenge, finalize to release the prover's stake.

### Rust

```rust
let tx_hash = verifier.finalize(result_id).await?;
```

### Python

```python
tx_hash = verifier.finalize(result_id)
```

### TypeScript

```typescript
const txHash = await verifier.finalize(resultId);
```

---

## Query Results

### Rust

```rust
// Check if a result is valid
let valid = verifier.is_result_valid(result_id).await?;
println!("Valid: {valid}");

// Get full result details
let result = verifier.get_result(result_id).await?;
println!("Submitter: {:?}", result.submitter);
println!("Finalized: {}", result.finalized);
```

### Python

```python
valid = verifier.is_result_valid(result_id)
print(f"Valid: {valid}")

result = verifier.get_result(result_id)
print(f"Submitter: {result['submitter']}")
print(f"Finalized: {result['finalized']}")
```

### TypeScript

```typescript
const valid = await verifier.isResultValid(resultId);
console.log('Valid:', valid);

const result = await verifier.getResult(resultId);
console.log('Submitter:', result.submitter);
console.log('Finalized:', result.finalized);
```

---

## Watch Events

### Rust

The Rust SDK uses the operator's `EventWatcher` for event polling:

```rust
use world_zk_sdk::EventWatcher;
use alloy::primitives::Address;

let contract: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3".parse()?;
let watcher = EventWatcher::new("http://localhost:8545", contract);

let mut from_block = 0u64;
loop {
    let (events, next_block) = watcher.poll_events(from_block).await?;
    for event in &events {
        match event {
            TEEEvent::ResultSubmitted { result_id, .. } => {
                println!("New result: {result_id}");
            }
            TEEEvent::ResultChallenged { result_id, challenger } => {
                println!("Challenged: {result_id} by {challenger}");
            }
            TEEEvent::ResultFinalized { result_id } => {
                println!("Finalized: {result_id}");
            }
            TEEEvent::ResultExpired { result_id } => {
                println!("Expired: {result_id}");
            }
        }
    }
    from_block = next_block;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}
```

### Python

```python
events = verifier.get_past_events("ResultSubmitted", from_block=0)
for event in events:
    print(f"Result: {event['args']['resultId'].hex()}")
```

### TypeScript

```typescript
import { TEEEventWatcher } from '@worldzk/sdk';

const watcher = new TEEEventWatcher({
  rpcUrl: 'http://localhost:8545',
  contractAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
});

const events = await watcher.getPastEvents('ResultSubmitted', {
  fromBlock: 0n,
});
for (const event of events) {
  console.log('Result ID:', event.args.resultId);
}
```

---

## Owner / Pause Operations

### Rust

```rust
let owner = verifier.owner().await?;
verifier.pause().await?;    // Emergency stop
verifier.unpause().await?;  // Resume
verifier.transfer_ownership(new_owner).await?; // Ownable2Step
```

### Python

```python
owner = verifier.owner()
verifier.pause()
verifier.unpause()
verifier.transfer_ownership(new_owner)
```

### TypeScript

```typescript
const owner = await verifier.owner();
await verifier.pause();
await verifier.unpause();
await verifier.transferOwnership(newOwner);
```

---

## Full Example: Happy Path

```typescript
// TypeScript — full lifecycle example
import { TEEVerifier } from '@worldzk/sdk';

const verifier = new TEEVerifier({
  rpcUrl: 'http://localhost:8545',
  privateKey: '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80',
  contractAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
});

// 1. Register enclave (admin)
await verifier.registerEnclave(enclaveKey, imageHash);

// 2. Submit result (prover)
const txHash = await verifier.submitResult(modelHash, inputHash, result, attestation, '0.1');

// 3. Wait for challenge window (1 hour)...

// 4. Finalize (anyone)
await verifier.finalize(resultId);

// 5. Verify
const valid = await verifier.isResultValid(resultId);
console.log('Result is valid:', valid); // true
```
