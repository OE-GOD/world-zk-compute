# tee-watcher

Shared TEE event watching library for polling `TEEMLVerifier` contract events
via `eth_getLogs`. Used by both the **operator** and **indexer** services.

## Purpose

This crate wraps the low-level `tee-events` types with Alloy primitives
(`B256`, `Address`) and provides `EventWatcher`, an async poller that tracks
new on-chain events across block ranges. It serves as the single event-ingestion
layer so services do not duplicate RPC/parsing logic.

## Key Types

| Type | Description |
|------|-------------|
| `TEEEvent` | Enum with Alloy-typed fields: `ResultSubmitted`, `ResultChallenged`, `ResultFinalized`, `ResultExpired`, `DisputeResolved` |
| `TaggedEvent` | A `TEEEvent` paired with the `contract_address` that emitted it |
| `EventWatcher` | Async poller wrapping one or more contract addresses and an RPC URL |

## Core API

- **`parse_log(log) -> Option<TEEEvent>`** -- Parses an Alloy `Log` into a typed event.
- **`parse_log_tagged(log) -> Option<TaggedEvent>`** -- Same, but includes emitting contract address.
- **`EventWatcher::new(rpc_url, address)`** / **`new_multi(rpc_url, addresses)`** -- Watch one or more contracts.
- **`poll_events(from_block)`** -- Returns `(Vec<TEEEvent>, next_block)`.
- **`poll_events_tagged(from_block)`** -- Returns `(Vec<TaggedEvent>, next_block)`.
- **`get_challenges(from, to)`** / **`get_submissions(from, to)`** -- Filtered event queries.
- **Topic hash functions** -- `topic_result_submitted()`, etc. Derived from `tee-events` constants.

## Usage

```rust
use tee_watcher::{EventWatcher, TEEEvent};

let watcher = EventWatcher::new("https://rpc.example.com", contract_addr);
let (events, next_block) = watcher.poll_events(from_block).await?;
for event in &events {
    match event {
        TEEEvent::ResultSubmitted { result_id, .. } => { /* handle */ }
        TEEEvent::ResultChallenged { result_id, .. } => { /* handle */ }
        _ => {}
    }
}
```

## Relationship to Services

- **Operator** (`services/operator`) re-exports `EventWatcher` and `TEEEvent`
  via its internal `watcher` module.
- **Indexer** (`services/indexer`) imports `parse_log` and `TEEEvent` directly
  for its own polling loop.

## Dependencies

- `tee-events` -- Low-level byte-array event types and compile-time topic hashes
- `alloy` -- RPC provider, log types, `B256`/`Address` primitives
- `anyhow`, `tracing` -- Error handling and instrumentation
