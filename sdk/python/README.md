# World ZK Compute Python SDK

Python clients for the World ZK Compute services and contracts.

Read the status note at the top of the [repository README](../../README.md)
before relying on anything these clients report as "verified".

## Installation

The package is not published on PyPI; install it from a checkout:

```bash
pip install -e sdk/python            # HTTP clients, converters
pip install -e 'sdk/python[web3]'    # + on-chain clients and the CLI
```

Python 3.9 or newer. `httpx` is the only required dependency. The on-chain
clients need the `web3` extra. The hash helpers need a keccak256
implementation, which the `web3` extra brings in (or `pip install pycryptodome`).

## What is in the package

| Import | Talks to | Extra |
|--------|----------|-------|
| `IndexerClient`, `AsyncIndexerClient` | TEE indexer REST API (`services/indexer`) | - |
| `WorldZK` | API gateway (`services/gateway`) | - |
| `Client` | verifier, proof-registry and proof-generator services directly | - |
| `TEEVerifier`, `AsyncTEEVerifier` | `TEEMLVerifier` contract | `web3` |
| `TEEEventWatcher`, `AsyncEventWatcher` | `TEEMLVerifier` events | `web3` |
| `BatchVerifier`, `StepBatchVerifier` | DAG batch verification contract | `web3` |
| `ExecutionEngineClient`, `ProverRegistryClient` | `ExecutionEngine`, `ProverRegistry` contracts | `web3` |
| `XGBoostConverter`, `LightGBMConverter` | model JSON to guest-program input | - |
| `compute_model_hash`, `compute_input_hash`, `compute_result_hash` | keccak256 hashing helpers | keccak256 backend |

Everything is importable from `worldzk`. Names that need `web3` are loaded on
first use and raise `ImportError` with an install hint when it is missing.

## Indexer client

```python
from worldzk import IndexerClient

with IndexerClient("http://localhost:8081", api_key="your-api-key") as client:
    health = client.health()
    print(health.status, health.last_indexed_block, health.total_results)

    for row in client.list_results(status="finalized", limit=10):
        print(row.id, row.status, row.submitter)

    row = client.get_result("0x" + "ab" * 32)   # None when the indexer has no such result
    stats = client.stats()
    print(stats.total_finalized, stats.total)
```

`api_key` is only needed when the indexer runs with `API_KEY` set. The async
client has the same methods:

```python
import asyncio
from worldzk import AsyncIndexerClient

async def main():
    async with AsyncIndexerClient("http://localhost:8081") as client:
        health = await client.health()
        results = await client.list_results(status="challenged")
        print(health.status, len(results))

asyncio.run(main())
```

### Error handling

The indexer clients make up to `max_retries` attempts for retryable failures
and then raise:

```python
from worldzk import ApiError, IndexerClient, NetworkError, TimeoutError

client = IndexerClient(timeout=30.0, max_retries=3)
try:
    client.list_results(status="finalized")
except TimeoutError as e:
    print(f"timed out after {e.timeout_seconds}s")
except NetworkError as e:
    print(f"could not reach the indexer: {e.message}")
except ApiError as e:
    print(f"HTTP {e.http_status}: {e.message}")
finally:
    client.close()
```

`worldzk.TimeoutError` is the SDK's own class, not the builtin. `NetworkError`
is a subclass of `ApiError`, so catch it first.

## Gateway client

```python
from worldzk import WorldZK

zk = WorldZK("http://localhost:8080", api_key="my-key")
print(zk.health().status)

result = zk.prove("model-id", [1.0, 2.0, 3.0])
verification = zk.verify(result.proof_id)
receipt = zk.get_receipt(result.proof_id)
```

`WorldZK` and `Client` use the standard library only and raise `RuntimeError`
for HTTP errors and `ConnectionError` when the service is unreachable.

Known gaps (November 2026): `WorldZK.upload_model()` and
`WorldZK.transparency_root()` do not match what the proof-generator and
proof-registry services accept today, and the gateway client has not been run
against a live gateway from this repository.

## On-chain clients (`web3` extra)

```python
from worldzk import TEEVerifier

verifier = TEEVerifier(
    rpc_url="http://localhost:8545",
    private_key="0x...",
    contract_address="0x...",
)
result_id = verifier.submit_result(
    model_hash="0x" + "00" * 32,
    input_hash="0x" + "00" * 32,
    result=b"\xde\xad\xbe\xef",
    attestation=b"...",
    stake_wei=100_000_000_000_000_000,  # 0.1 ETH
)
print(verifier.get_result(result_id).finalized)
```

```python
from worldzk import TEEEventWatcher

watcher = TEEEventWatcher(contract_address="0x...", rpc_url="http://localhost:8545")
events, next_block = watcher.poll_events(from_block=0)
```

`e2e_batch_verify.py` in this directory drives `BatchVerifier` against a
deployed verifier contract; CI runs it against Anvil.

## Model converters and hashes

```python
from worldzk import XGBoostConverter, compute_input_hash, compute_model_hash

converter = XGBoostConverter.from_model_file("model.json")
converter.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0])
converter.set_threshold(0.5)
input_bytes = converter.serialize()

model_hash = compute_model_hash("model.json")
input_hash = compute_input_hash([1.0, 2.0, 3.0])
```

## Command line

Installing the package adds a `worldzk` command for `TEEMLVerifier`
operations (`submit-result`, `challenge`, `finalize`, `status`, `is-valid`,
`watch`). It needs the `web3` extra:

```bash
worldzk --rpc-url http://localhost:8545 --contract 0x... is-valid --result-id 0x...
```

## Development

```bash
cd sdk/python
pip install -e '.[dev,web3]'
ruff check .
python -m pytest tests/ --ignore=tests/test_anvil_integration.py --ignore=tests/test_sepolia_integration.py
```

## License

Apache-2.0
