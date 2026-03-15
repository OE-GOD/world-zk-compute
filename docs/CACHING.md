# Caching Strategies

Production caching recommendations for the indexer, operator, and prover services.

---

## 1. Indexer Query Caching

The indexer serves several HTTP endpoints with different caching characteristics.

| Endpoint | Pattern | Recommended TTL | Rationale |
|----------|---------|-----------------|-----------|
| `GET /stats` | Aggregate counts across 4 status categories | 5-10s | Full table scans; result changes only on new blocks (~12s) |
| `GET /results?status=X` | Filtered list with common filters | 3-5s | Frequent dashboard queries with repeated filter combos |
| `GET /results/:id` | Single-row lookup by primary key | 30s | Immutable once finalized; short TTL covers status transitions |
| `GET /health` | Live status check | No cache | Must reflect real-time liveness |
| `GET /ready` | Readiness probe | No cache | Must reflect real-time readiness |

**Implementation approach.** Add an in-process response cache keyed on `(endpoint, query_params)`. The `/stats` endpoint benefits most: it runs four `SELECT COUNT(*)` queries per call. A 5-second TTL eliminates redundant scans under load without stale data risk, since the indexer only ingests events every `POLL_INTERVAL_SECS` (default 12s).

For `/results` list queries, cache by the full normalized query string (status + submitter + model_hash + limit + offset/after_id). Invalidate on any new event ingestion rather than relying solely on TTL.

---

## 2. RPC Response Caching

Both the indexer and operator poll the chain via JSON-RPC. Key cacheable calls:

| RPC Call | Service | Recommended TTL | Notes |
|----------|---------|-----------------|-------|
| `eth_getBlockNumber` | Both | 1-2s | Called every poll cycle; safe to serve slightly stale |
| `eth_getLogs` | Both | Per-block-range, immutable | Logs for finalized block ranges never change |
| `eth_getChainId` | Operator | Indefinite | Chain ID is constant for a given network |

**eth_getLogs caching.** The indexer polls `eth_getLogs` with `(contract_address, from_block, to_block)`. Responses for block ranges below the finalized block are immutable and can be cached indefinitely. Use the tuple `(contract_address, from_block, to_block)` as the cache key. This eliminates redundant RPC calls on operator restarts or re-indexing.

**Approach.** Use an RPC-aware caching proxy (e.g., a middleware in the alloy provider layer) or an external proxy like [Cacheth](https://github.com/ethereumjs/ethereumjs-monorepo) in front of the RPC endpoint. For simpler setups, cache at the application level using the block range as key.

---

## 3. Proof Caching

The operator already caches proofs on disk (`PROOF_CACHE_DIR`). Additional caching layers:

| Cache Layer | What | Key | TTL |
|-------------|------|-----|-----|
| **Disk (existing)** | Serialized proof files | `result_id` | Indefinite (proofs are deterministic) |
| **Verification result** | Boolean pass/fail per proof | `keccak256(proof_bytes)` | Indefinite (deterministic) |
| **Attestation (existing)** | Nitro attestation document | Singleton | `attestation_cache_ttl` config (default 300s) |

**Verification result caching.** When the operator or a verifier contract checks a proof, the result is deterministic for a given proof. Cache the verification outcome keyed on the proof hash to avoid re-verifying the same proof during retries or multi-step batch verification flows.

**Proof pre-warming.** The operator already spawns proof generation as a background task after `submit`. No additional caching needed here, but ensure the proof cache directory is on a persistent volume (see `docs/VOLUMES.md`).

---

## 4. Cache Backends

| Environment | Backend | Use Case |
|-------------|---------|----------|
| **Development** | In-process `HashMap` + `Instant` TTL | Zero external dependencies; sufficient for single-instance dev |
| **Production (single node)** | In-process `moka` or `mini-moka` crate | Lock-free concurrent cache with TTL, max-size eviction, ~2MB overhead |
| **Production (multi-node)** | Redis 7+ | Shared cache across indexer replicas; pub/sub for invalidation |

**Redis configuration for production:**

```yaml
# docker-compose.yml addition
services:
  redis:
    image: redis:7-alpine
    command: redis-server --maxmemory 128mb --maxmemory-policy allkeys-lru
    ports:
      - "6379:6379"
```

```bash
# Indexer env vars
CACHE_BACKEND=redis          # or "memory" (default)
CACHE_REDIS_URL=redis://redis:6379/0
CACHE_DEFAULT_TTL_SECS=5
```

For single-replica deployments (most common initially), the in-process cache is sufficient and avoids the Redis dependency.

---

## 5. Cache Invalidation Strategies

| Data Type | Strategy | Trigger |
|-----------|----------|---------|
| **Stats** | TTL-based (5s) | Expires naturally; acceptable staleness within one poll interval |
| **Result lists** | Event-driven + TTL fallback | Invalidate all list caches when `apply_event()` processes a new event; 5s TTL as safety net |
| **Single result** | Event-driven | Invalidate `results:{id}` entry when that result's status changes |
| **RPC logs** | Immutable (finalized ranges) | Never invalidate for block ranges below chain finality |
| **Block number** | TTL-based (1-2s) | Short-lived; always slightly stale is acceptable |

**Event-driven invalidation in the indexer.** The `poll_and_index` function already processes events sequentially. After each `storage.apply_event(event)` call, broadcast a cache-invalidation signal:

```
// Pseudocode addition to poll_and_index
for event in &events {
    storage.apply_event(event)?;
    cache.invalidate_for_event(event);  // clears affected keys
    broadcaster.broadcast(tee_event_to_ws_event(event));
}
cache.invalidate_key("stats");  // always clear stats after any event batch
```

For Redis-backed caches across multiple replicas, use Redis pub/sub or keyspace notifications to propagate invalidation.

---

## 6. Estimated Impact

### Memory

| Cache | Estimated Size | Max Entries | Per-Entry Size |
|-------|---------------|-------------|----------------|
| Stats | < 1 KB | 1 | 32 bytes (4 x u64) |
| Result lists | 1-5 MB | ~100 unique query combos | ~50 KB per page (50 results x 1 KB) |
| Single results | 0.5-2 MB | ~2000 hot entries | ~1 KB each |
| RPC logs | 1-10 MB | ~1000 block ranges | Variable |
| **Total** | **3-18 MB** | | |

### Performance

| Endpoint | Uncached p99 | Cached p99 | Improvement |
|----------|-------------|-----------|-------------|
| `GET /stats` | 5-15ms (4 COUNT queries) | < 1ms | 5-15x |
| `GET /results` | 2-10ms (filtered query) | < 1ms | 2-10x |
| `GET /results/:id` | 1-3ms (PK lookup) | < 1ms | Marginal |
| RPC `eth_getLogs` | 50-200ms (network) | < 1ms (cache hit) | 50-200x |

The biggest wins come from RPC response caching (eliminates network round-trips on restarts) and stats caching (eliminates repeated full-table scans under concurrent load).

---

## 7. Configuration Recommendations

### Minimal (Development)

No configuration needed. In-process caches with sensible defaults.

### Production Single-Node

```bash
# Indexer
CACHE_BACKEND=memory
CACHE_STATS_TTL_SECS=5
CACHE_RESULTS_TTL_SECS=3
CACHE_MAX_ENTRIES=5000

# Operator (already has attestation cache)
ATTESTATION_CACHE_TTL=300
```

### Production Multi-Node

```bash
# Indexer (all replicas)
CACHE_BACKEND=redis
CACHE_REDIS_URL=redis://redis:6379/0
CACHE_STATS_TTL_SECS=5
CACHE_RESULTS_TTL_SECS=3

# Redis server
# maxmemory: 128MB (sufficient for indexer cache workload)
# eviction: allkeys-lru
# persistence: disabled (cache is ephemeral; DB is source of truth)
```

### Monitoring

Add these metrics to track cache effectiveness:

- `cache_hits_total` / `cache_misses_total` -- per cache namespace (stats, results, rpc)
- `cache_evictions_total` -- indicates if max-size is too low
- `cache_size_bytes` -- current memory usage

The indexer's existing `/metrics` endpoint (Prometheus format) is the natural place to expose these.
