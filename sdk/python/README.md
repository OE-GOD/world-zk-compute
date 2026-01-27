# World ZK Compute Python SDK

Python client library for the World ZK Compute decentralized proving network.

## Installation

```bash
pip install worldzk
```

## Quick Start

```python
from worldzk import Client

# Initialize client
client = Client(api_key="your-api-key")

# Submit a computation request
request = client.requests.create(
    image_id="0x1234567890abcdef...",
    input_data=b"hello world",
)

print(f"Request ID: {request.id}")
print(f"Status: {request.status}")

# Wait for completion
result = client.requests.wait(request.id, timeout=300)
print(f"Proof: {result.proof}")
```

## Async Support

```python
import asyncio
from worldzk import AsyncClient

async def main():
    async with AsyncClient(api_key="your-api-key") as client:
        # Check health
        health = await client.health()
        print(f"Status: {health.status}")

        # List pending requests
        requests = await client.requests.list(status="pending")
        for req in requests.items:
            print(f"Request {req.id}: {req.status}")

asyncio.run(main())
```

## API Reference

### Client

```python
from worldzk import Client

client = Client(
    base_url="https://api.worldzk.compute/v1",  # API base URL
    api_key="your-api-key",                      # Optional API key
    timeout=30.0,                                # Request timeout (seconds)
    max_retries=3,                               # Max retry attempts
)
```

### Requests

```python
# List requests
requests = client.requests.list(
    status="pending",  # Filter by status
    image_id="0x...",  # Filter by program
    limit=20,
    offset=0,
)

# Create request
request = client.requests.create(
    image_id="0x...",
    input_data=b"...",           # Raw input bytes
    # OR
    input_url="ipfs://...",      # URL to fetch input
    input_hash="0x...",          # Required with input_url
    callback_contract="0x...",   # Optional callback
    expiration_seconds=3600,
    tip=1000000000000000,        # Wei
)

# Get request
request = client.requests.get(request_id=123)

# Cancel request
request = client.requests.cancel(request_id=123)

# Wait for completion
request = client.requests.wait(
    request_id=123,
    timeout=300.0,       # Max wait time
    poll_interval=2.0,   # Check interval
)
```

### Programs

```python
# List programs
programs = client.programs.list(active=True)

# Get program
program = client.programs.get(image_id="0x...")
```

### Provers

```python
# List provers
provers = client.provers.list(tier="gold")

# Get prover
prover = client.provers.get(address="0x...")

# Register as prover
prover = client.provers.register(
    name="My Prover",
    endpoint="https://my-prover.example.com",
)
```

## Error Handling

```python
from worldzk import Client, ApiError, RateLimitError, NotFoundError

client = Client(api_key="your-api-key")

try:
    request = client.requests.get(12345)
except NotFoundError as e:
    print(f"Request not found: {e.message}")
except RateLimitError as e:
    print(f"Rate limited, retry after {e.retry_after_seconds}s")
except ApiError as e:
    print(f"API error [{e.code}]: {e.message}")
    if e.is_retryable:
        print(f"Retry after {e.retry_after_ms}ms")
```

### Error Codes

| Code | Description |
|------|-------------|
| WZK-1xxx | Client errors (validation, auth) |
| WZK-2xxx | Server errors |
| WZK-3xxx | Proof errors |
| WZK-4xxx | Contract/chain errors |
| WZK-5xxx | Network errors |

## Models

### ExecutionRequest

```python
request.id              # int: Request ID
request.requester       # str: Requester address
request.image_id        # str: Program image ID
request.input_hash      # str: SHA256 of input
request.status          # RequestStatus: pending/claimed/completed/expired/cancelled
request.created_at      # datetime
request.expires_at      # datetime
request.proof           # ProofResult (if completed)
request.is_pending      # bool
request.is_completed    # bool
request.is_terminal     # bool
```

### Prover

```python
prover.address                    # str: Ethereum address
prover.reputation.score           # int: 0-10000
prover.reputation.tier            # ReputationTier
prover.reputation.is_good_standing  # bool
prover.stats.total_jobs           # int
prover.stats.completed_jobs       # int
prover.stats.success_rate         # float: percentage
prover.stats.avg_proof_time_ms    # int
```

## License

Apache-2.0
