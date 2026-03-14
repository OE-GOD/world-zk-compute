# Load Testing Scripts

Scripts for load testing individual World ZK Compute services.

## Available Scripts

| Script | Service | Default URL | Endpoint |
|--------|---------|-------------|----------|
| `load-test-prover.sh` | Warm Prover | `http://127.0.0.1:3000` | `POST /prove` |
| `load-test-prover-batch.sh` | Warm Prover (batch) | `http://127.0.0.1:3000` | `POST /prove/batch` |
| `load-test-enclave.sh` | TEE Enclave | `http://127.0.0.1:8080` | `POST /infer` |
| `load-test-indexer.sh` | Indexer | `http://127.0.0.1:8081` | `GET /results`, `GET /stats` |

## Prerequisites

- **Required**: `curl`, `bash`, `python3`
- **Recommended**: [`hey`](https://github.com/rakyll/hey) (better latency stats)
- **Optional**: `websocat` or `python3 -m websockets` (for indexer WebSocket tests)

Install `hey`:
```sh
# macOS
brew install hey

# Linux
go install github.com/rakyll/hey@latest
```

## Quick Start

```sh
# Start the full stack
docker compose up -d

# Test enclave inference
./scripts/load-test-enclave.sh --concurrency 5 --requests 50

# Test prover with batch mode
./scripts/load-test-prover-batch.sh --batch-size 8 --requests 20

# Test indexer REST endpoints
./scripts/load-test-indexer.sh --mode rest-health --requests 100

# Test indexer WebSocket connections
./scripts/load-test-indexer.sh --mode ws-scale --ws-connections 20 --ws-duration 60

# Health check only (all scripts)
./scripts/load-test-enclave.sh --health-only
./scripts/load-test-prover.sh --health-only
```

## Docker Compose Load Test

Run the full stack with automated load testing:

```sh
docker compose -f docker-compose.loadtest.yml up
```

Results are written to `./load-test-results/` on the host.

Configure via environment variables:
```sh
CONCURRENCY=10 REQUESTS=100 docker compose -f docker-compose.loadtest.yml up
```

## Interpreting Results

| Metric | Healthy Range | Action if Exceeded |
|--------|--------------|-------------------|
| P50 latency (enclave) | < 50ms | Check model size, CPU |
| P95 latency (enclave) | < 200ms | Reduce concurrency |
| P50 latency (prover) | < 500ms | Expected — proof generation is CPU-intensive |
| Error rate | < 1% | Check service logs |
| 429 rate | 0% | Increase `MAX_REQUESTS_PER_MINUTE` on enclave |

## Troubleshooting

- **Connection refused**: Service not running. Check `docker compose ps`.
- **HTTP 429 (rate limited)**: Enclave rate limiter active. Set `MAX_REQUESTS_PER_MINUTE=1000` on the enclave or reduce `--concurrency`.
- **Timeouts**: Increase `--timeout` or reduce `--concurrency`. Prover is CPU-bound.
- **Empty responses**: Check service health with `--health-only` first.
- **"model_loaded: false"**: Enclave hasn't loaded a model. Check `MODEL_PATH` env var.
