# Contract Event Indexing

This document describes all events emitted by the TEEMLVerifier contract, how
the indexer service subscribes to them, and which REST/WebSocket endpoints
expose the resulting data.

## 1. TEEMLVerifier Contract Events

The events below are defined in
`contracts/src/tee/ITEEMLVerifier.sol`.

### Core Lifecycle Events

| Event | Solidity Signature | Indexed Parameters | Non-Indexed Parameters |
|-------|-------------------|-------------------|----------------------|
| **ResultSubmitted** | `ResultSubmitted(bytes32 indexed resultId, bytes32 indexed modelHash, bytes32 inputHash, address indexed submitter)` | `resultId`, `modelHash`, `submitter` | `inputHash` |
| **ResultChallenged** | `ResultChallenged(bytes32 indexed resultId, address challenger)` | `resultId` | `challenger` |
| **ResultFinalized** | `ResultFinalized(bytes32 indexed resultId)` | `resultId` | -- |
| **ResultExpired** | `ResultExpired(bytes32 indexed resultId)` | `resultId` | -- |
| **DisputeResolved** | `DisputeResolved(bytes32 indexed resultId, bool proverWon)` | `resultId` | `proverWon` |

### Enclave Management Events

| Event | Solidity Signature | Indexed Parameters | Non-Indexed Parameters |
|-------|-------------------|-------------------|----------------------|
| **EnclaveRegistered** | `EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash)` | `enclaveKey` | `enclaveImageHash` |
| **EnclaveRevoked** | `EnclaveRevoked(address indexed enclaveKey)` | `enclaveKey` | -- |

### Admin / Configuration Events

| Event | Solidity Signature | Indexed Parameters | Non-Indexed Parameters |
|-------|-------------------|-------------------|----------------------|
| **ChallengeBondUpdated** | `ChallengeBondUpdated(uint256 oldAmount, uint256 newAmount)` | -- | `oldAmount`, `newAmount` |
| **ProverStakeUpdated** | `ProverStakeUpdated(uint256 oldAmount, uint256 newAmount)` | -- | `oldAmount`, `newAmount` |
| **RemainderVerifierUpdated** | `RemainderVerifierUpdated(address oldVerifier, address newVerifier)` | -- | `oldVerifier`, `newVerifier` |
| **ConfigUpdated** | `ConfigUpdated(string param, uint256 oldValue, uint256 newValue)` | -- | `param`, `oldValue`, `newValue` |
| **DisputeExtended** | `DisputeExtended(bytes32 indexed resultId, uint256 newDeadline)` | `resultId` | `newDeadline` |

## 2. Events Indexed by the Indexer Service

The indexer service (`services/indexer/`) subscribes to the following five
core lifecycle events via `eth_getLogs` polling. These are the events parsed
by the shared `tee_watcher` crate (`crates/watcher/src/lib.rs`) and the
low-level `tee_events` crate (`crates/events/src/lib.rs`).

| Event | Watcher Enum Variant | Indexer Behavior |
|-------|---------------------|-----------------|
| `ResultSubmitted` | `TEEEvent::ResultSubmitted` | Inserts a new row into the `results` table with status `"submitted"` |
| `ResultChallenged` | `TEEEvent::ResultChallenged` | Updates row status to `"challenged"`, stores challenger address |
| `ResultFinalized` | `TEEEvent::ResultFinalized` | Updates row status to `"finalized"` |
| `ResultExpired` | `TEEEvent::ResultExpired` | Updates row status to `"finalized"` (treated same as finalized) |
| `DisputeResolved` | `TEEEvent::DisputeResolved` | Updates row status to `"resolved"` |

The enclave management events (`EnclaveRegistered`, `EnclaveRevoked`) are
parsed by the `tee_events` crate but are **not** currently stored in the
indexer database. They are available in the `tee_events::TEEEvent` enum for
SDK-level event watching.

The admin events (`ChallengeBondUpdated`, `ProverStakeUpdated`,
`RemainderVerifierUpdated`, `ConfigUpdated`, `DisputeExtended`) are **not**
currently indexed.

## 3. REST API Endpoints

The indexer exposes REST endpoints under `/api/v1/` (with backward-compatible
unversioned aliases). All responses include an `X-API-Version: v1` header.

### `GET /api/v1/results`

List indexed results with optional filters.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | string | Filter by status: `submitted`, `challenged`, `finalized`, `resolved` |
| `submitter` | string | Filter by submitter address (hex, e.g. `0xabc...`) |
| `model_hash` | string | Filter by model hash |
| `limit` | integer | Max results to return (1-1000, default 50) |

**Response:** `200 OK` with JSON array of result objects:

```json
[
  {
    "id": "0x...",
    "model_hash": "0x...",
    "input_hash": "0x...",
    "output": "",
    "submitter": "0x...",
    "status": "submitted",
    "block_number": 12345,
    "timestamp": 0,
    "challenger": null
  }
]
```

Results are ordered by `block_number` descending (newest first).

### `GET /api/v1/results/:id`

Get a single result by its ID.

**Response:** `200 OK` with a single result object, or `404 Not Found`.

### `GET /api/v1/stats`

Get aggregate statistics across all indexed results.

**Response:**

```json
{
  "total_submitted": 10,
  "total_challenged": 2,
  "total_finalized": 7,
  "total_resolved": 1
}
```

### `GET /health`

Health check endpoint (not versioned).

**Response:**

```json
{
  "status": "ok",
  "last_indexed_block": 12345,
  "total_results": 100
}
```

Returns `503 Service Unavailable` with `"status": "degraded"` if the storage
backend is unhealthy (e.g., mutex lock poisoning).

### `GET /ws/events` (WebSocket)

Real-time event streaming via WebSocket. After the connection is established,
the client may optionally send a subscribe message to filter event types:

```json
{"subscribe": ["ResultSubmitted", "ResultChallenged"]}
```

An empty array or no subscribe message means "receive all events".

Events are pushed as JSON:

```json
{
  "event_type": "ResultSubmitted",
  "data": {
    "result_id": "0x...",
    "model_hash": "0x...",
    "input_hash": "0x...",
    "submitter": "0x...",
    "block_number": 12345
  }
}
```

The server sends ping frames every 30 seconds and disconnects clients that
fail to respond with a pong within 10 seconds.

## 4. Event-to-Endpoint Mapping

| Contract Event | Indexer Action | REST Endpoint | WebSocket Event Type |
|----------------|---------------|---------------|---------------------|
| `ResultSubmitted` | Insert row (status=submitted) | `GET /api/v1/results`, `GET /api/v1/results/:id` | `"ResultSubmitted"` |
| `ResultChallenged` | Update status to challenged | `GET /api/v1/results?status=challenged` | `"ResultChallenged"` |
| `ResultFinalized` | Update status to finalized | `GET /api/v1/results?status=finalized` | `"ResultFinalized"` |
| `ResultExpired` | Update status to finalized | `GET /api/v1/results?status=finalized` | `"ResultExpired"` |
| `DisputeResolved` | Update status to resolved | `GET /api/v1/results?status=resolved` | `"DisputeResolved"` |
| `EnclaveRegistered` | Not stored in DB | -- | -- |
| `EnclaveRevoked` | Not stored in DB | -- | -- |
| Admin events | Not indexed | -- | -- |

## 5. Event ABI Signatures for External Integrators

Use these keccak256 topic hashes to filter logs directly via `eth_getLogs`
without depending on the indexer. The topic hash is `keccak256(signature)`.

| Event | ABI Signature | Topic Hash (keccak256) |
|-------|--------------|----------------------|
| ResultSubmitted | `ResultSubmitted(bytes32,bytes32,bytes32,address)` | Computed at runtime; see `tee_events::TOPIC_RESULT_SUBMITTED` |
| ResultChallenged | `ResultChallenged(bytes32,address)` | Computed at runtime; see `tee_events::TOPIC_RESULT_CHALLENGED` |
| ResultFinalized | `ResultFinalized(bytes32)` | Computed at runtime; see `tee_events::TOPIC_RESULT_FINALIZED` |
| ResultExpired | `ResultExpired(bytes32)` | Computed at runtime; see `tee_events::TOPIC_RESULT_EXPIRED` |
| DisputeResolved | `DisputeResolved(bytes32,bool)` | Computed at runtime; see `tee_events::TOPIC_DISPUTE_RESOLVED` |
| EnclaveRegistered | `EnclaveRegistered(address,bytes32)` | Computed at runtime; see `tee_events::TOPIC_ENCLAVE_REGISTERED` |
| EnclaveRevoked | `EnclaveRevoked(address)` | Computed at runtime; see `tee_events::TOPIC_ENCLAVE_REVOKED` |

### Computing Topic Hashes

**Solidity:**
```solidity
bytes32 topic = keccak256("ResultSubmitted(bytes32,bytes32,bytes32,address)");
```

**JavaScript (ethers.js v6):**
```javascript
import { keccak256, toUtf8Bytes } from "ethers";
const topic = keccak256(toUtf8Bytes("ResultSubmitted(bytes32,bytes32,bytes32,address)"));
```

**Python (web3.py):**
```python
from web3 import Web3
topic = Web3.keccak(text="ResultSubmitted(bytes32,bytes32,bytes32,address)").hex()
```

**Rust (tee_events crate):**
```rust
use tee_events::{TOPIC_RESULT_SUBMITTED, event_signature};
// Use the pre-computed constant:
let topic: [u8; 32] = TOPIC_RESULT_SUBMITTED;
// Or compute dynamically:
let topic = event_signature("ResultSubmitted(bytes32,bytes32,bytes32,address)");
```

### Example: Filtering Logs with eth_getLogs

```json
{
  "jsonrpc": "2.0",
  "method": "eth_getLogs",
  "params": [{
    "address": "0x<CONTRACT_ADDRESS>",
    "fromBlock": "0x0",
    "toBlock": "latest",
    "topics": [
      "0x<TOPIC_RESULT_SUBMITTED_HASH>"
    ]
  }],
  "id": 1
}
```

To filter by a specific `resultId` (first indexed parameter), add it as
the second topic:

```json
{
  "topics": [
    "0x<TOPIC_RESULT_SUBMITTED_HASH>",
    "0x<RESULT_ID>"
  ]
}
```

### Log Data Decoding Reference

**ResultSubmitted** log layout:
- `topics[0]`: event selector
- `topics[1]`: `resultId` (bytes32, indexed)
- `topics[2]`: `modelHash` (bytes32, indexed)
- `topics[3]`: `submitter` (address, left-padded to 32 bytes, indexed)
- `data[0:32]`: `inputHash` (bytes32)

**ResultChallenged** log layout:
- `topics[0]`: event selector
- `topics[1]`: `resultId` (bytes32, indexed)
- `data[0:32]`: `challenger` (address, left-padded to 32 bytes)

**DisputeResolved** log layout:
- `topics[0]`: event selector
- `topics[1]`: `resultId` (bytes32, indexed)
- `data[0:32]`: `proverWon` (bool, ABI-encoded as uint256)

**ResultFinalized** log layout:
- `topics[0]`: event selector
- `topics[1]`: `resultId` (bytes32, indexed)
- No data

**ResultExpired** log layout:
- `topics[0]`: event selector
- `topics[1]`: `resultId` (bytes32, indexed)
- No data

**EnclaveRegistered** log layout:
- `topics[0]`: event selector
- `topics[1]`: `enclaveKey` (address, left-padded to 32 bytes, indexed)
- `data[0:32]`: `enclaveImageHash` (bytes32)

**EnclaveRevoked** log layout:
- `topics[0]`: event selector
- `topics[1]`: `enclaveKey` (address, left-padded to 32 bytes, indexed)
- No data
