# tee-events

Strongly-typed, serde-compatible event decoding for the `TEEMLVerifier` smart contract.

## Purpose

This crate provides the low-level event data types and ABI decoding logic for
TEEMLVerifier contract events. It uses plain byte arrays (`[u8; 32]`, `[u8; 20]`)
instead of Alloy types, making it lightweight and suitable for serialization.

The higher-level `tee-watcher` crate wraps these types with Alloy primitives
and adds on-chain polling via `eth_getLogs`.

## Key Types

| Type | Description |
|------|-------------|
| `TEEEvent` | Enum over all seven parsed event variants |
| `ResultSubmitted` | New ML inference result submitted (result_id, model_hash, input_hash, submitter, stake) |
| `ResultChallenged` | Existing result challenged (result_id, challenger, bond) |
| `ResultFinalized` | Result finalized after challenge window |
| `ResultExpired` | Result expired without challenge |
| `DisputeResolved` | Dispute resolved on-chain (prover_won flag) |
| `EnclaveRegistered` | New TEE enclave registered (enclave address, image_hash) |
| `EnclaveRevoked` | TEE enclave revoked |

## Core API

- **`parse_log(topics, data) -> Option<TEEEvent>`** -- Decodes raw EVM log topics
  and data bytes into a typed event. Returns `None` for unknown or malformed logs.
- **`event_signature(name) -> [u8; 32]`** -- Computes keccak256 topic hash at runtime.
- **`TOPIC_*` constants** -- Compile-time keccak256 topic hashes for each event
  (e.g., `TOPIC_RESULT_SUBMITTED`). Computed via a `const fn` Keccak-f[1600] implementation.

## Usage

```rust
use tee_events::{parse_log, TEEEvent, TOPIC_RESULT_SUBMITTED};

let event = parse_log(&topics, &data);
match event {
    Some(TEEEvent::ResultSubmitted(e)) => {
        println!("result_id: {:?}, submitter: {:?}", e.result_id, e.submitter);
    }
    _ => {}
}
```

## Relationship to Other Crates

- **`tee-watcher`** depends on this crate; re-exports it and converts between
  byte-array types and Alloy `B256`/`Address` types.
- **`services/operator`** and **`services/indexer`** consume events through `tee-watcher`.

## Dependencies

- `alloy-primitives` -- keccak256 (runtime helper only; compile-time hashing is self-contained)
- `serde` -- Serialize/Deserialize for all event structs
