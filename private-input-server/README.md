# Private Input Server

Reference implementation for hosting private data behind an authenticated server. Provers authenticate with a wallet-signed request, and the server verifies the prover has claimed the job on-chain before releasing data.

## API Endpoints

### `POST /inputs/:request_id/upload`

Upload private input data for a request ID (admin endpoint).

- **Content-Type**: `application/octet-stream`
- **Body**: Raw bytes
- **Response**: `{ "request_id": 42, "digest": "0x...", "size": 1024 }`

```bash
curl -X POST http://localhost:8090/inputs/42/upload \
  -H "Content-Type: application/octet-stream" \
  --data-binary @input.bin
```

### `POST /inputs/:request_id`

Fetch private input (authenticated prover endpoint).

- **Content-Type**: `application/json`
- **Body**: `AuthRequest` JSON (see below)
- **Response**: Raw bytes (`application/octet-stream`)

```bash
curl -X POST http://localhost:8090/inputs/42 \
  -H "Content-Type: application/json" \
  -d '{"request_id": 42, "prover_address": "0x...", "timestamp": 1710000000, "signature": "0x..."}'
```

### `GET /health`

Health check endpoint.

- **Response**: `{ "status": "healthy", "inputs_stored": 5 }`

```bash
curl http://localhost:8090/health
```

## Authentication

Provers authenticate using EIP-191 signed messages:

1. **Message format**: `world-zk-compute:fetch-input:{request_id}:{prover_address}:{timestamp}`
2. **Timestamp**: Must be within 5 minutes (300s) of server time
3. **Signature**: 65-byte EIP-191 signature, hex-encoded with `0x` prefix
4. **On-chain verification**: Server calls `ExecutionEngine.getRequest(requestId)` to verify the prover has claimed the job (status == Claimed, claimedBy == prover)

## Configuration

| Env Var | CLI Flag | Default | Description |
|---------|----------|---------|-------------|
| `HOST` | `--host` | `0.0.0.0` | Server bind address |
| `PORT` | `--port` | `8090` | Server port |
| `RPC_URL` | `--rpc-url` | *required* | Ethereum RPC URL for on-chain verification |
| `ENGINE_ADDRESS` | `--engine-address` | *required* | ExecutionEngine contract address |
| `STORAGE_DIR` | `--storage-dir` | *(none)* | Directory for persistent storage (in-memory if unset) |
| `SKIP_CHAIN_VERIFICATION` | `--skip-chain-verification` | `false` | Skip on-chain checks (testing only) |

Supports `.env` file via `dotenvy`.

## Storage

- **In-memory** (default): Fast, but data is lost on restart
- **File-backed** (`STORAGE_DIR`): Inputs persisted as `{request_id}.bin` files with in-memory cache

## Rate Limiting

No built-in rate limiting. Use a reverse proxy (nginx, Caddy) or cloud WAF for production deployments.

## Running

```bash
# Development (skip on-chain verification)
cargo run -- --rpc-url http://localhost:8545 --engine-address 0x... --skip-chain-verification

# Production (with persistent storage)
STORAGE_DIR=/data/inputs \
RPC_URL=https://eth-mainnet.g.alchemy.com/v2/KEY \
ENGINE_ADDRESS=0x... \
cargo run --release

# Docker
docker build -t private-input-server .
docker run -p 8090:8090 -e RPC_URL=... -e ENGINE_ADDRESS=... private-input-server
```

## Testing

```bash
cargo test
```
