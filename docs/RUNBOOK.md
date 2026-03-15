# Operational Runbook

Production incident response procedures for World ZK Compute.

**Audience:** On-call engineers and operators.
**Contracts:** TEEMLVerifier, ExecutionEngine, RemainderVerifier, ProgramRegistry
**Services:** Operator (`services/operator/`), Enclave (`tee/enclave/`), Indexer (`services/indexer/`), Warm Prover (`examples/xgboost-remainder/`)
**Monitoring:** Prometheus alerts defined in `monitoring/alerting-rules.yml`
**Troubleshooting Reference:** `docs/TROUBLESHOOTING.md`

---

## Table of Contents

1. [Emergency Pause](#1-emergency-pause)
2. [Enclave Key Rotation](#2-enclave-key-rotation)
3. [High Dispute Rate Response](#3-high-dispute-rate-response)
4. [Operator Crash Recovery](#4-operator-crash-recovery)
5. [RPC Endpoint Migration](#5-rpc-endpoint-migration)
6. [Contract Upgrade Procedure](#6-contract-upgrade-procedure)
7. [Monitoring Alert Responses](#7-monitoring-alert-responses)
8. [Scaling](#8-scaling)
9. [Load Testing Procedures](#9-load-testing-procedures)
10. [Database Backup and Restore](#10-database-backup-and-restore)
11. [Rolling Upgrades (Blue/Green)](#11-rolling-upgrades-bluegreen)
12. [Attestation Expiry and Re-Registration](#12-attestation-expiry-and-re-registration)

---

## 1. Emergency Pause

### When to Pause

- Suspected exploit or unauthorized fund movement
- Compromised enclave signing key (before key rotation completes)
- Critical bug discovered in contract logic
- Sustained unresolvable dispute failures (proof generation broken)

### Who Can Pause

Only the contract **owner** (Ownable2Step) can call `pause()` and `unpause()`.

```bash
# Verify the current owner
cast call $TEE_VERIFIER "owner()(address)" --rpc-url $RPC
```

### Resolution

**Option A: admin-cli (recommended)**

```bash
# Dry-run first to confirm access and gas
admin-cli \
  --rpc-url $RPC \
  --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  --dry-run \
  pause

# Execute pause
admin-cli \
  --rpc-url $RPC \
  --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  pause
```

**Option B: cast**

```bash
cast send $TEE_VERIFIER "pause()" \
  --rpc-url $RPC \
  --private-key $OWNER_KEY
```

**Option C: Operator DRY_RUN mode (soft halt)**

If you need to stop the operator from submitting on-chain transactions without pausing the contract itself (e.g., to investigate operator-side issues while the contract remains available for other submitters), set `DRY_RUN=true` on the operator:

```bash
# Docker Compose: set DRY_RUN and restart
docker compose stop operator
# Edit docker-compose.yml: add DRY_RUN: "true" to operator.environment
docker compose up -d operator

# Or set it as an environment override without editing the file
DRY_RUN=true docker compose up -d operator

# Kubernetes
kubectl -n world-zk set env deployment/worldzk-operator DRY_RUN=true
```

When `DRY_RUN=true` (env var or `dry_run = true` in TOML config), the operator simulates the full flow (enclave calls, proof generation) but does not send on-chain transactions. This is useful for:

- Validating a new operator configuration against a live chain without spending gas
- Temporarily halting submissions while keeping the watch loop active for monitoring
- Testing proof generation without dispute resolution

To resume normal operation, set `DRY_RUN=false` (or remove it) and restart.

**Option D: Stop the operator service entirely**

If you need an immediate full halt of all operator activity (no polling, no proof generation, no submissions):

```bash
# Docker Compose -- graceful stop (waits for in-flight tasks up to 60s)
docker compose stop operator

# Kubernetes -- scale to zero
kubectl -n world-zk scale deployment worldzk-operator --replicas=0
```

On SIGTERM/SIGINT, the operator:
1. Stops accepting new events
2. Waits up to 60 seconds for in-flight proof submissions to complete
3. Saves final state to `operator-state.json`
4. Exits cleanly

On restart, it resumes from `last_polled_block` in the state file.

**Option E: Circuit breaker (automatic, no intervention needed)**

The operator has built-in circuit breakers that automatically halt RPC and chain calls after repeated failures:

- **RPC circuit breaker**: Opens after 5 consecutive RPC failures. Blocks polling for 30 seconds, then retries.
- **Chain circuit breaker**: Opens after 3 consecutive chain call failures. Blocks dispute resolution and finalization for 60 seconds.

Monitor via Prometheus metrics:
```bash
curl -s http://localhost:9090/metrics | grep circuit_breaker
```

### Verification

- [ ] Confirm paused:
  ```bash
  cast call $TEE_VERIFIER "paused()(bool)" --rpc-url $RPC
  # Expected: true
  ```
- [ ] Confirm `submitResult` reverts with `EnforcedPause`:
  ```bash
  cast call $TEE_VERIFIER "submitResult(bytes32,bytes32,bytes,bytes)" \
    0x0 0x0 0x 0x --rpc-url $RPC --from $ANY_ADDRESS
  # Expected: revert EnforcedPause
  ```
- [ ] Confirm `challenge` also reverts with `EnforcedPause`
- [ ] Notify the team via webhook/Slack
- [ ] Document the reason and timestamp in an incident log

### Unpause

Only unpause after the root cause is identified and resolved.

```bash
admin-cli \
  --rpc-url $RPC \
  --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  unpause
```

Post-unpause checklist:

- [ ] `cast call $TEE_VERIFIER "paused()(bool)" --rpc-url $RPC` returns `false`
- [ ] Submit a test inference and verify it succeeds
- [ ] Monitor for 30 minutes

---

## 2. Enclave Key Rotation

### When to Rotate

- Scheduled rotation (recommended: every 90 days)
- Suspected key compromise
- Enclave image update (new PCR0 hash)

### Zero-Downtime Procedure

**Phase 1: Deploy new enclave**

- [ ] Build and deploy the new enclave image
- [ ] Record the new enclave signing address and image hash (PCR0)
- [ ] Verify the new enclave is healthy:
  ```bash
  curl -s http://$NEW_ENCLAVE:8080/health
  curl -s http://$NEW_ENCLAVE:8080/info | jq .
  ```
- [ ] If using Nitro attestation, verify an attestation document:
  ```bash
  NONCE=$(openssl rand -hex 32)
  curl -s "http://$NEW_ENCLAVE:8080/attestation?nonce=0x${NONCE}"
  ```

**Phase 2: Register new enclave on-chain**

```bash
admin-cli \
  --rpc-url $RPC \
  --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  register-enclave $NEW_ENCLAVE_ADDRESS $NEW_IMAGE_HASH
```

**Phase 3: Update operator to point to new enclave**

```bash
# Kubernetes
kubectl -n world-zk patch configmap world-zk-config \
  --type merge \
  -p '{"data": {"ENCLAVE_URL": "http://new-enclave:8080"}}'
kubectl -n world-zk rollout restart deployment worldzk-operator

# Docker Compose
# Edit docker-compose.yml: operator.environment.ENCLAVE_URL
docker compose up -d operator
```

**Phase 4: Verify new enclave is working**

- [ ] Submit a test inference through the new enclave
- [ ] Confirm the operator can reach the new enclave (check operator logs)

**Phase 5: Revoke old enclave (after pending results finalize)**

Wait at least 1 hour for all pending results from the old enclave to exit the challenge window, then:

```bash
admin-cli \
  --rpc-url $RPC \
  --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  revoke-enclave $OLD_ENCLAVE_ADDRESS
```

**Phase 6: Decommission old enclave**

- [ ] Stop and remove the old enclave instance

### Emergency Key Compromise

If the key is compromised, **revoke first**, then deploy a new enclave:

- [ ] Revoke immediately (do not wait for pending results):
  ```bash
  admin-cli --rpc-url $RPC --contract $TEE_VERIFIER \
    --private-key $OWNER_KEY revoke-enclave $COMPROMISED_ADDRESS
  ```
- [ ] Pause the contract if unauthorized results were submitted
- [ ] Challenge any suspicious results from the compromised key
- [ ] Deploy and register a new enclave as above

### Verification

- [ ] New enclave submits results successfully
- [ ] Old enclave submissions revert with `TEEMLVerifier: enclave revoked`
- [ ] Operator logs show successful event polling

---

## 3. High Dispute Rate Response

### Symptoms

- **Alert:** `HighDisputeRate` -- `rate(operator_challenges_detected[1h]) > 5`
- **Alert:** `DisputeResolutionFailed` -- disputes failing to resolve
- Multiple webhook notifications for challenges in quick succession

### Investigation

- [ ] Determine if disputes are from one challenger (griefing) or many (systemic):
  ```bash
  # Query indexer for recent challenges
  curl -s "http://$INDEXER:8081/results?status=challenged&limit=20" | jq '.[].challenger'
  # Or query on-chain logs
  cast logs $TEE_VERIFIER "ResultChallenged(bytes32,address)" \
    --from-block $RECENT_BLOCK --rpc-url $RPC
  ```
- [ ] Check enclave health and model integrity:
  ```bash
  curl -s http://$ENCLAVE:8080/health
  curl -s http://$ENCLAVE:8080/info | jq '.model_hash'
  ```
- [ ] Manually run inference and compare:
  ```bash
  curl -s http://$ENCLAVE:8080/infer \
    -X POST -H "Content-Type: application/json" \
    -d '{"features": [5.0, 3.5, 1.5, 0.3]}'
  ```
- [ ] Check if proofs are succeeding (submitter wins valid disputes):
  ```bash
  cast logs $TEE_VERIFIER "DisputeResolved(bytes32,address,bool)" \
    --from-block $RECENT_BLOCK --rpc-url $RPC
  ```
- [ ] Check operator logs:
  ```bash
  # Kubernetes
  kubectl -n world-zk logs -l app=worldzk-operator --tail=200 | grep -i "dispute\|error\|fail"
  # Docker Compose
  docker compose logs operator --tail=200 | grep -i "dispute\|error\|fail"
  ```

### Resolution

**Scenario A: Single griefing challenger (enclave results correct)**

- Disputes are self-penalizing (challenger loses bond per failed challenge)
- No action needed unless volume is extreme
- Monitor that all proof submissions succeed

**Scenario B: Enclave producing wrong results**

- [ ] Pause the contract immediately (Section 1)
- [ ] Check model integrity: `curl -s http://$ENCLAVE:8080/info | jq '.model_hash'`
- [ ] If model is corrupt, reload via admin API:
  ```bash
  curl -X POST http://$ENCLAVE:8080/admin/reload-model \
    -H "Authorization: Bearer $ADMIN_API_KEY" \
    -H "Content-Type: application/json" \
    -d '{"model_path": "/app/model/model.json"}'
  ```
- [ ] Restart enclave:
  ```bash
  kubectl -n world-zk rollout restart deployment worldzk-enclave
  # or: docker compose restart enclave
  ```
- [ ] If enclave image is compromised, follow Enclave Key Rotation (Section 2)

**Scenario C: Proof generation failures**

- [ ] Check warm prover health: `curl -s http://$PROVER:3000/health`
- [ ] Check proof retry settings: `MAX_PROOF_RETRIES` (default: 3), `PROOF_RETRY_DELAY_SECS` (default: 10)
- [ ] Restart prover if unresponsive:
  ```bash
  kubectl -n world-zk rollout restart deployment worldzk-prover
  # or: docker compose restart warm-prover
  ```

### Escalation

If disputes continue after enclave restart:

1. Pause the contract
2. Page the engineering lead
3. Collect all operator, enclave, and prover logs
4. Do NOT unpause until root cause is confirmed

---

## 4. Operator Crash Recovery

### Symptoms

- **Alert:** `OperatorDown` -- `up{job="operator"} == 0` for 5 minutes
- **Alert:** `LowUptime` -- uptime < 1 hour, counter reset
- Disputes going unresolved (no proof submissions)

### Investigation

- [ ] Check operator status:
  ```bash
  # Kubernetes
  kubectl -n world-zk get pods -l app=worldzk-operator
  kubectl -n world-zk describe pod -l app=worldzk-operator
  kubectl -n world-zk logs -l app=worldzk-operator --tail=100
  # Docker Compose
  docker compose ps operator
  docker compose logs operator --tail=100
  ```
- [ ] Check for OOM kills:
  ```bash
  kubectl -n world-zk describe pod -l app=worldzk-operator | grep -A5 "Last State"
  ```
- [ ] Inspect the persisted state file:
  ```bash
  cat $STATE_FILE | jq '.last_polled_block, (.active_disputes | length), (.processed_event_ids | length)'
  ```

### About the State File

The operator persists crash-recovery state to a JSON file (default: `./operator-state.json`, configurable via `STATE_FILE` env var or `state_file` in TOML config).

**Contents:**

| Field | Type | Purpose |
|-------|------|---------|
| `last_polled_block` | `u64` | Next block to poll from on restart |
| `active_disputes` | `map<string, u64>` | Result IDs with deadline timestamps |
| `processed_event_ids` | `set<string>` | Event IDs already handled (deduplication) |

**Safety:** The state file uses atomic writes (write to `.tmp`, then rename), so it is never left half-written even if the process is killed mid-write.

### Resolution

**Step 1: Restart the operator**

```bash
# Kubernetes (also triggered automatically by liveness probe after 3 failures)
kubectl -n world-zk rollout restart deployment worldzk-operator
# Docker Compose
docker compose restart operator
```

On startup, the operator will:
1. Load `operator-state.json` (or start from latest block if missing/corrupt)
2. Resume polling from `last_polled_block`
3. Skip already-processed events via `processed_event_ids`
4. Re-check `active_disputes` deadlines

**Step 2: Verify state was loaded**

```bash
# Look for "Loaded operator state" in logs
kubectl -n world-zk logs -l app=worldzk-operator --tail=20 | grep "Loaded operator state"
```

**Step 3: Check for missed disputes**

```bash
# Compare last polled block with current block
CURRENT=$(cast block-number --rpc-url $RPC)
LAST=$(cat $STATE_FILE | jq '.last_polled_block')
echo "Gap: $LAST -> $CURRENT"

# Search for challenges in the gap
cast logs $TEE_VERIFIER "ResultChallenged(bytes32,address)" \
  --from-block $LAST --to-block $CURRENT --rpc-url $RPC
```

**Step 4: Check dispute deadlines**

For each active dispute, verify the deadline has not passed:

```bash
# Get current timestamp
cast block latest --field timestamp --rpc-url $RPC

# For each active dispute result ID, check deadline
cast call $TEE_VERIFIER "getResult(bytes32)" $RESULT_ID --rpc-url $RPC
```

If a dispute deadline is imminent, the submitter can extend once (30 minutes):

```bash
cast send $TEE_VERIFIER "extendDisputeWindow(bytes32)" $RESULT_ID \
  --rpc-url $RPC --private-key $SUBMITTER_KEY
```

### Handling In-Flight Proofs

When the operator crashes, there may be proofs being generated that have not yet been submitted on-chain. On restart:

1. **Pre-computed proofs are preserved.** Proofs are stored as JSON files in `PROOFS_DIR` (default: `./proofs`). The operator checks this directory when a challenge is detected and uses existing proofs without regeneration.

2. **In-progress proof tasks are lost.** Any `tokio::spawn`-ed proof generation tasks that were running at crash time are abandoned. On the next poll cycle, the operator will re-detect the challenge event (if not in `processed_event_ids`) and trigger proof generation again.

3. **Semaphore-blocked proofs.** The operator limits concurrent proof submissions to 10 via a semaphore. If a challenge was skipped because the semaphore was full, it was NOT marked as processed and will be retried on the next poll.

4. **Graceful shutdown.** On SIGINT/SIGTERM, the operator waits up to 60 seconds for in-flight tasks to complete before saving state. If this timeout expires, remaining tasks are abandoned but the state file is still saved with the last polled block.

### Corrupt State File Recovery

If the state file is corrupt, the operator logs a warning and falls back to `OperatorState::default()` (starts from latest block, missing events in the gap).

To manually recover:

```bash
# Option A: Delete and restart (operator starts from latest block)
rm $STATE_FILE
# Restart operator

# Option B: Manually create state file to resume from a specific block
echo '{"last_polled_block": 12300, "active_disputes": {}, "processed_event_ids": []}' > $STATE_FILE
# Restart operator -- it will replay from block 12300
```

### Verification

- [ ] Operator health returns OK: `curl -s http://$OPERATOR:9090/health`
- [ ] `operator_uptime_seconds` metric is increasing
- [ ] No active disputes past deadline without proof submission
- [ ] State file is being updated (check mtime): `ls -la $STATE_FILE`

---

## 5. RPC Endpoint Migration

### Symptoms (triggering migration)

- RPC provider deprecation or planned outage
- `ProofSubmissionFailed` alert due to RPC connectivity issues
- Rate limiting from current provider

### Investigation (pre-migration)

- [ ] Verify the new RPC endpoint is valid and accessible:
  ```bash
  cast block-number --rpc-url $NEW_RPC
  cast chain-id --rpc-url $NEW_RPC
  ```
- [ ] Confirm the chain ID matches the expected network:
  ```
  31337       = Anvil (local)
  1           = Ethereum Mainnet
  11155111    = Sepolia
  4801        = World Chain Sepolia
  421614      = Arbitrum Sepolia
  ```
- [ ] Confirm the contract is accessible via the new endpoint:
  ```bash
  cast call $TEE_VERIFIER "paused()(bool)" --rpc-url $NEW_RPC
  ```

### Resolution

**Docker Compose**

```bash
# Edit docker-compose.yml: operator.environment.OPERATOR_RPC_URL
docker compose up -d operator
```

**Kubernetes: ConfigMap update**

```bash
kubectl -n world-zk patch configmap world-zk-config \
  --type merge \
  -p '{"data": {"OPERATOR_RPC_URL": "https://new-rpc-provider.com/v1/KEY"}}'
```

**Kubernetes: Secret update (if RPC URL contains API key)**

```bash
kubectl -n world-zk create secret generic world-zk-secrets \
  --from-literal=OPERATOR_PRIVATE_KEY="$PRIVATE_KEY" \
  --from-literal=TEE_VERIFIER_ADDRESS="$CONTRACT_ADDRESS" \
  --dry-run=client -o yaml | kubectl apply -f -
```

**Rolling restart (required to pick up ConfigMap/Secret changes)**

```bash
kubectl -n world-zk rollout restart deployment worldzk-operator
kubectl -n world-zk rollout restart deployment worldzk-indexer
```

### Verification

- [ ] Operator health OK: `curl -s http://$OPERATOR:9090/health`
- [ ] Operator logs show successful polling (no RPC errors):
  ```bash
  kubectl -n world-zk logs -l app=worldzk-operator --tail=50 | grep -i "error\|poll"
  ```
- [ ] Indexer `last_indexed_block` is increasing:
  ```bash
  curl -s http://$INDEXER:8081/health | jq .last_indexed_block
  ```
- [ ] No `ProofSubmissionFailed` alerts firing

---

## 6. Contract Upgrade Procedure

### Upgrade Paths

| Contract | Pattern | Upgrade Method |
|----------|---------|---------------|
| UpgradeableExecutionEngine | UUPS Proxy | `upgradeTo()` / `upgradeToAndCall()` via admin |
| TEEMLVerifier | Non-upgradeable (Ownable2Step) | Fresh deploy + state migration |
| RemainderVerifier | Non-upgradeable | Fresh deploy + re-register circuits |

### UUPS Proxy Upgrade (UpgradeableExecutionEngine)

**Pre-upgrade checklist:**

- [ ] New implementation deployed and verified on block explorer
- [ ] Storage layout compatible (no slot reordering, new vars use `__gap` slots)
- [ ] Tests pass: `cd contracts && forge test --match-contract UpgradeableTest -vvv`
- [ ] Code audited (for production)
- [ ] Admin key available

**Step 1: Deploy new implementation**

```bash
forge create src/Upgradeable.sol:UpgradeableExecutionEngineV2 \
  --rpc-url $RPC \
  --private-key $DEPLOYER_KEY
# Record: $NEW_IMPL
```

**Step 2: Verify admin access**

```bash
cast call $PROXY "admin()(address)" --rpc-url $RPC
# Must match the caller of upgradeTo
```

**Step 3: Execute upgrade**

```bash
# Without re-initialization
cast send $PROXY "upgradeTo(address)" $NEW_IMPL \
  --rpc-url $RPC \
  --private-key $ADMIN_KEY

# With re-initialization
INIT_DATA=$(cast calldata "reinitialize(uint256)" 2)
cast send $PROXY "upgradeToAndCall(address,bytes)" $NEW_IMPL $INIT_DATA \
  --rpc-url $RPC \
  --private-key $ADMIN_KEY
```

**Step 4: Post-upgrade verification**

- [ ] Implementation address updated:
  ```bash
  cast call $PROXY "implementation()(address)" --rpc-url $RPC
  # Expected: $NEW_IMPL
  ```
- [ ] State preserved:
  ```bash
  cast call $PROXY "nextRequestId()(uint256)" --rpc-url $RPC
  cast call $PROXY "protocolFeeBps()(uint256)" --rpc-url $RPC
  cast call $PROXY "admin()(address)" --rpc-url $RPC
  ```
- [ ] New functionality works (if applicable)
- [ ] Monitor for 1 hour

**Rollback:** Deploy the old implementation and call `upgradeTo(oldImpl)`. Note: storage changes from the new implementation persist.

### Non-Upgradeable Contract Upgrade (TEEMLVerifier, RemainderVerifier)

For non-upgradeable contracts, a fresh deployment is required:

- [ ] Deploy new contract instance
- [ ] Transfer ownership: `transferOwnership(newOwnerAddress)` on the old contract, then `acceptOwnership()` on new
- [ ] For TEEMLVerifier: re-register all active enclave keys on the new contract
- [ ] For RemainderVerifier: re-register all active circuits and set Groth16 verifiers
- [ ] Update `TEE_VERIFIER_ADDRESS` in operator/indexer config
- [ ] Update SDK clients to point to the new contract address
- [ ] The old contract continues to function for existing in-flight results

---

## 7. Monitoring Alert Responses

Each alert defined in `monitoring/alerting-rules.yml` with investigation and resolution steps.

### OperatorDown (critical)

**Alert:** `up{job="operator"} == 0` for 5 minutes

| | |
|---|---|
| **Symptoms** | Operator metrics unreachable. Disputes will not be detected or resolved. |
| **Investigation** | Check pod status: `kubectl -n world-zk get pods -l app=worldzk-operator`. Check logs for crash reason. Check for OOM kills: `kubectl describe pod ... \| grep -A5 "Last State"`. |
| **Resolution** | See [Operator Crash Recovery](#4-operator-crash-recovery). Restart: `kubectl -n world-zk rollout restart deployment worldzk-operator`. If OOM, increase memory limit (current: 256Mi). |
| **Verification** | Health endpoint returns OK. `operator_uptime_seconds` increasing. No missed disputes. |

### HighDisputeRate (warning)

**Alert:** `rate(operator_challenges_detected[1h]) > 5`

| | |
|---|---|
| **Symptoms** | More than 5 challenges per hour. May indicate malicious challenger or systemic inference failures. |
| **Investigation** | See [High Dispute Rate Response](#3-high-dispute-rate-response). |
| **Resolution** | Depends on root cause: griefing (no action), bad enclave (pause + fix), proof failures (restart prover). |
| **Verification** | Dispute rate returns below threshold. All disputes resolving successfully. |

### ProofSubmissionFailed (warning)

**Alert:** `increase(operator_errors_total[15m]) > 0`

| | |
|---|---|
| **Symptoms** | Operator errors in last 15 minutes. Could be proof generation failures, RPC issues, or transaction reverts. |
| **Investigation** | Check operator logs for specific error type. Check RPC: `cast block-number --rpc-url $RPC`. Check prover: `curl -s http://$PROVER:3000/health`. Check operator wallet balance: `cast balance $OPERATOR_ADDR --rpc-url $RPC`. |
| **Resolution** | RPC issues: see [RPC Endpoint Migration](#5-rpc-endpoint-migration). Prover failures: restart warm prover. Insufficient funds: top up operator wallet. Tx reverts: check gas limit, nonce (`cast nonce $OPERATOR_ADDR --rpc-url $RPC`). |
| **Verification** | `operator_errors_total` stops incrementing. |

### DisputeResolutionFailed (critical)

**Alert:** `increase(operator_disputes_failed[15m]) > 0` (immediate)

| | |
|---|---|
| **Symptoms** | Dispute resolution tx failed. **Stake at risk of slashing if the 24-hour deadline passes.** |
| **Investigation** | Check revert reason: simulate the `resolveDispute` call with `cast call`. Check if contract is paused. Check if RemainderVerifier is set: `cast call $TEE_VERIFIER "remainderVerifier()(address)" --rpc-url $RPC`. Check proof data integrity in `PROOFS_DIR`. |
| **Resolution** | Fix root cause (proof data, gas, RPC). If time-critical, manually submit. Check deadlines in operator state file `active_disputes`. |
| **Verification** | `cast call $TEE_VERIFIER "disputeResolved(bytes32)(bool)" $RESULT_ID --rpc-url $RPC` returns `true`. |

### WebhookFailures (warning)

**Alert:** `increase(webhook_failures_total[15m]) > 3`

| | |
|---|---|
| **Symptoms** | More than 3 webhook delivery failures in 15 minutes. Team may not be receiving dispute alerts. |
| **Investigation** | Check endpoint reachability: `curl -s -o /dev/null -w "%{http_code}" $WEBHOOK_URL`. Check operator logs for 4xx vs 5xx vs connection errors. Webhook uses retry with exponential backoff (1s, 2s, 4s) for transient failures. |
| **Resolution** | 4xx: verify webhook URL/token. 5xx: endpoint service may be down; wait and retry. Connection error: check network/DNS. |
| **Verification** | `webhook_failures_total` stops incrementing. |

### LowUptime (info)

**Alert:** `operator_uptime_seconds < 3600 and changes(operator_uptime_seconds[1h]) > 0`

| | |
|---|---|
| **Symptoms** | Operator recently restarted. Informational unless frequent. |
| **Investigation** | Check if restart was intentional (deployment, config change). Check for crash loops: `kubectl -n world-zk describe pod -l app=worldzk-operator`. |
| **Resolution** | If crash-looping: check logs for startup errors (missing env vars, invalid config, RPC unreachable). Validate state file. |
| **Verification** | Uptime increases steadily without further resets. |

### EnclaveDown (critical)

**Alert:** `up{job="enclave"} == 0` for 5 minutes

| | |
|---|---|
| **Symptoms** | Enclave health unreachable. Inference requests fail. |
| **Investigation** | Check container/pod status. Check for OOM (memory limit: 512m in Docker Compose). Check model file accessibility. |
| **Resolution** | Restart: `kubectl -n world-zk rollout restart deployment worldzk-enclave` or `docker compose restart enclave`. If lock poisoned (`internal lock error: ... poisoned`), full restart required. |
| **Verification** | `curl -s http://$ENCLAVE:8080/health` returns 200. `curl -s http://$ENCLAVE:8080/info` shows correct model. |

### EnclaveHighLatency (warning)

**Alert:** `enclave_avg_inference_ms > 5000` for 10 minutes

| | |
|---|---|
| **Symptoms** | Average inference latency exceeds 5 seconds. |
| **Investigation** | Check enclave metrics: `curl -s http://$ENCLAVE:8080/metrics`. Check CPU/memory utilization. Check if a large number of concurrent requests. |
| **Resolution** | Restart enclave to clear state. Increase CPU/memory limits if consistently high. Check if model complexity changed (hot-reload). |
| **Verification** | `enclave_avg_inference_ms` drops below 5000. |

### EnclaveHighErrorRate (warning)

**Alert:** `rate(enclave_total_errors[5m]) / rate(enclave_total_inferences[5m]) > 0.05`

| | |
|---|---|
| **Symptoms** | Error rate exceeds 5% over 5 minutes. |
| **Investigation** | Check enclave logs for: malformed requests (wrong feature count), model errors, lock poisoning. |
| **Resolution** | Lock poisoning: restart enclave. Malformed requests: check client SDK version. Model errors: verify model file integrity, use `EXPECTED_MODEL_HASH` env var for validation. |
| **Verification** | Error rate drops below 5%. |

### EnclaveRateLimited (info)

**Alert:** `rate(enclave_total_rate_limited[5m]) > 0`

| | |
|---|---|
| **Symptoms** | Enclave rejecting requests with 429 status. Default limit: 120 requests/minute. |
| **Investigation** | Check if traffic spike is legitimate or an attack. Check `MAX_REQUESTS_PER_MINUTE` config. |
| **Resolution** | Legitimate traffic: increase `MAX_REQUESTS_PER_MINUTE` in enclave config and restart. Attack: keep rate limit, investigate source. |
| **Verification** | Rate-limited count stabilizes. |

---

## 8. Scaling

### Operator Scaling

The operator deployment includes an HPA (Horizontal Pod Autoscaler):

| Setting | Value | Source |
|---------|-------|--------|
| Minimum replicas | 2 | Matches PodDisruptionBudget `minAvailable: 1` |
| Maximum replicas | 5 | HPA spec |
| Scale-up trigger | CPU > 70% average | HPA metrics |
| Scale-up rate | 2 pods per 60 seconds | HPA behavior |
| Scale-down rate | 1 pod per 60 seconds | HPA behavior |
| Scale-down cooldown | 300 seconds | HPA stabilization window |

```bash
# Check current HPA status
kubectl -n world-zk get hpa worldzk-operator-hpa

# Manual temporary scale
kubectl -n world-zk scale deployment worldzk-operator --replicas=4

# Permanently adjust HPA limits
kubectl -n world-zk patch hpa worldzk-operator-hpa \
  --type merge \
  -p '{"spec": {"maxReplicas": 10}}'
```

**Multi-replica considerations:**

- Each replica polls for events independently using its own state file (stored in pod-local `emptyDir`)
- Event deduplication via `processed_event_ids` prevents double-processing within a single instance
- For dispute resolution, ensure only one replica submits `resolveDispute` to avoid nonce conflicts. Use a leader election pattern or designate one replica as the resolver.
- Each replica needs access to proof data (`PROOFS_DIR`)

**When to scale:**

- Spike in on-chain events (many submissions/challenges)
- Watching multiple contracts (`CONTRACT_ADDRESSES` or `contracts` in TOML config)
- Proof generation under high load

### Indexer Scaling

The indexer uses **SQLite** (single-writer) and runs as **1 replica** with `Recreate` strategy.

| Setting | Value |
|---------|-------|
| Replicas | 1 (hard limit due to SQLite) |
| Storage | 1Gi PVC (`worldzk-indexer-data`) |
| Poll interval | 12 seconds (configurable via `POLL_INTERVAL_SECS`) |
| Deployment strategy | `Recreate` (not `RollingUpdate`) |

**To scale the indexer beyond 1 replica:**

1. Migrate from SQLite to PostgreSQL (the `Storage` trait in `services/indexer/src/main.rs` is designed for swappable backends)
2. Update `DB_PATH` to a PostgreSQL connection string
3. Switch deployment strategy to `RollingUpdate`
4. Add replicas

### Enclave Scaling

- Deploy additional enclave instances behind a load balancer
- Each enclave has its own signing key; register each key on-chain via `registerEnclave`
- Set `MAX_REQUESTS_PER_MINUTE` per instance based on capacity (default: 120)
- Monitor per-instance latency via Prometheus `instance` label

### Warm Prover Scaling

- Memory-intensive (2GB limit in Docker Compose): GKR circuit and Pedersen generators are loaded in memory at startup
- For higher throughput, deploy multiple instances behind a load balancer and update `PROVER_URL` in operator config

---

## 9. Load Testing Procedures

### Prerequisites

- Running services (Docker Compose or K8s)
- `curl` and `python3` (required)
- `hey` (recommended, for latency percentiles): `brew install hey`
- `websocat` (optional, for indexer WebSocket tests)

### Quick Load Test

```bash
# Enclave inference — 50 requests, 5 concurrent
./scripts/load-test-enclave.sh --requests 50 --concurrency 5

# Warm prover single proof — 10 requests, 2 concurrent
./scripts/load-test-prover.sh --requests 10 --concurrency 2

# Warm prover batch mode — 20 requests, batch size 8
./scripts/load-test-prover-batch.sh --requests 20 --batch-size 8

# Indexer REST + WebSocket
./scripts/load-test-indexer.sh --mode rest-health --requests 100
./scripts/load-test-indexer.sh --mode ws-scale --ws-connections 20 --ws-duration 60
```

### Docker Compose Load Test

Run the full automated suite:

```bash
docker compose -f docker-compose.loadtest.yml up
# Results written to ./load-test-results/
```

Configure via environment variables:

```bash
CONCURRENCY=10 REQUESTS=100 docker compose -f docker-compose.loadtest.yml up
```

### Interpreting Results

| Metric | Healthy Range | Action if Exceeded |
|--------|--------------|-------------------|
| P50 latency (enclave) | < 50ms | Check CPU, model size |
| P95 latency (enclave) | < 200ms | Reduce concurrency or scale horizontally |
| P50 latency (prover) | < 500ms | Expected — proof generation is CPU-intensive |
| Error rate | < 1% | Check service logs: `docker compose logs <service>` |
| 429 rate | 0% | Increase `MAX_REQUESTS_PER_MINUTE` on enclave |

### Pre-Deployment Validation

Before deploying to Sepolia or production:

1. Run health checks: `./scripts/load-test-enclave.sh --health-only`
2. Run baseline load test with expected concurrency
3. Verify error rate < 1% and P95 latency within SLO
4. Check Prometheus metrics during test: `curl http://localhost:9090/metrics`

### Troubleshooting Load Tests

- **Connection refused**: Service not running. Check `docker compose ps`.
- **HTTP 429**: Rate limit active. Increase `MAX_REQUESTS_PER_MINUTE` or reduce `--concurrency`.
- **Timeouts**: Prover is CPU-bound. Increase `--timeout` or reduce `--concurrency`.
- **"model_loaded: false"**: Enclave hasn't loaded a model. Check `MODEL_PATH` env var.

---

## 10. Database Backup and Restore

### Indexer SQLite Database

**Location:** Configured by `DB_PATH` env var (default: `./indexer.db`). In K8s: `/data/indexer.db` on PVC `worldzk-indexer-data`.

**Tables:**
- `results` -- indexed TEE results (id, model_hash, input_hash, submitter, status, block_number, challenger)
- `indexer_state` -- cursor tracking (`last_indexed_block`)

**Backup:**

```bash
# Kubernetes: copy from running pod
INDEXER_POD=$(kubectl -n world-zk get pod -l app=worldzk-indexer -o jsonpath='{.items[0].metadata.name}')
kubectl -n world-zk cp $INDEXER_POD:/data/indexer.db ./indexer-backup-$(date +%Y%m%d).db

# Docker Compose: copy from container
docker compose cp indexer:/data/indexer.db ./indexer-backup-$(date +%Y%m%d).db

# SQLite online backup (safe while service is running)
sqlite3 /path/to/indexer.db ".backup '/path/to/backup.db'"
```

**Restore:**

```bash
# Kubernetes: scale down, restore, scale up
kubectl -n world-zk scale deployment worldzk-indexer --replicas=0
# Wait for pod to terminate
kubectl -n world-zk wait --for=delete pod -l app=worldzk-indexer --timeout=60s
# Start a temporary pod to copy the file, or use a Job
kubectl -n world-zk scale deployment worldzk-indexer --replicas=1
# Copy backup into the new pod
INDEXER_POD=$(kubectl -n world-zk get pod -l app=worldzk-indexer -o jsonpath='{.items[0].metadata.name}')
kubectl -n world-zk cp ./indexer-backup.db $INDEXER_POD:/data/indexer.db
kubectl -n world-zk rollout restart deployment worldzk-indexer

# Docker Compose
docker compose stop indexer
docker compose cp ./indexer-backup.db indexer:/data/indexer.db
docker compose start indexer
```

**Re-index from scratch:** If the database is lost, delete it and restart. The indexer re-polls from block 0 and rebuilds the index.

```bash
kubectl -n world-zk exec $INDEXER_POD -- rm /data/indexer.db
kubectl -n world-zk rollout restart deployment worldzk-indexer
```

### Migrating from SQLite to PostgreSQL

The indexer implements a `Storage` trait with both SQLite and PostgreSQL backends (`services/indexer/src/pg_storage.rs`). The PostgreSQL backend requires the `postgres` Cargo feature and the `DATABASE_URL` env var.

**Step 1: Provision a PostgreSQL instance**

```bash
# Example: create a local Postgres DB for testing
createdb indexer_db
# Or use a managed service (RDS, Cloud SQL, etc.)
```

**Step 2: Export data from SQLite**

```bash
# Stop the indexer to avoid writes during export
docker compose stop indexer

# Export results table to CSV
sqlite3 ./indexer.db <<SQL
.headers on
.mode csv
.output /tmp/results_export.csv
SELECT * FROM results;
.output /tmp/indexer_state_export.csv
SELECT * FROM indexer_state;
SQL
```

**Step 3: Build the indexer with Postgres support**

```bash
# The indexer Dockerfile already builds with --features postgres
docker compose build indexer
```

**Step 4: Configure and start with PostgreSQL**

```bash
# Set environment variables
export DB_TYPE=postgres
export DATABASE_URL="postgres://user:pass@localhost:5432/indexer_db"

# Docker Compose: update indexer service environment
# environment:
#   DB_TYPE: postgres
#   DATABASE_URL: "postgres://user:pass@postgres:5432/indexer_db"

docker compose up -d indexer
```

The `PgStorage::new()` method auto-creates the required tables (`results`, `indexer_state`, `events`, `sync_state`) on first connection.

**Step 5: Import existing data (optional)**

```bash
# Import into Postgres using psql COPY
psql $DATABASE_URL <<SQL
\copy results FROM '/tmp/results_export.csv' WITH (FORMAT csv, HEADER true);
\copy indexer_state FROM '/tmp/indexer_state_export.csv' WITH (FORMAT csv, HEADER true);
SQL
```

Alternatively, skip the import and let the indexer re-index from block 0. This is simpler but slower for large histories.

**Step 6: Verify**

```bash
curl -s http://localhost:8081/health | jq .
# Confirm last_indexed_block and total_results match expectations
```

### Operator State File

**Location:** Configured by `STATE_FILE` env var (default: `./operator-state.json`).

**Warning:** In K8s, the operator state file lives in an `emptyDir` volume, which is lost on pod restart. For durable state across restarts, mount a PersistentVolumeClaim.

**Backup:**

```bash
# Docker Compose
docker compose cp operator:/app/proofs/operator-state.json ./operator-state-backup-$(date +%Y%m%d).json

# Kubernetes (from running pod)
OPERATOR_POD=$(kubectl -n world-zk get pod -l app=worldzk-operator -o jsonpath='{.items[0].metadata.name}')
kubectl -n world-zk cp $OPERATOR_POD:/app/proofs/operator-state.json ./operator-state-backup.json
```

**Restore:**

```bash
# Copy backup into container before or after restart
docker compose cp ./operator-state-backup.json operator:/app/proofs/operator-state.json
docker compose restart operator
```

**Manual state file creation:** If lost, create a minimal state file to resume from a specific block:

```json
{
  "last_polled_block": 12300,
  "active_disputes": {},
  "processed_event_ids": []
}
```

Set `last_polled_block` to a block before the first unprocessed event. The operator replays and deduplicates from there.

### Proof File Storage

**Location:** Configured by `PROOFS_DIR` env var (default: `./proofs`). In Docker Compose: `/app/proofs` on the `proof-storage` volume.

**Format:** Individual JSON files named `{result_id}.json` containing:

```json
{
  "proof_hex": "0x...",
  "circuit_hash": "0x...",
  "public_inputs_hex": "0x...",
  "gens_hex": "0x..."
}
```

**Backup:**

```bash
# Docker Compose
docker compose cp operator:/app/proofs ./proofs-backup-$(date +%Y%m%d)
```

**Recovery:** Proofs can be regenerated from the warm prover if lost, but regeneration takes time. For disputes with approaching deadlines, having backups is critical.

---

## 11. Rolling Upgrades (Blue/Green)

### Overview

Use blue/green deployment to upgrade services with zero downtime. The key idea: bring up a new ("green") instance alongside the existing ("blue") one, verify it, then switch traffic and tear down the old one.

### Docker Compose Blue/Green Procedure

Docker Compose does not natively support blue/green, but you can achieve it by running two project instances or using service overrides.

**Method: Override file with parallel services**

Create a `docker-compose.green.yml` override that defines green variants of each service:

```yaml
# docker-compose.green.yml
services:
  enclave-green:
    build:
      context: ./tee
    ports:
      - "8082:8080"   # Different host port
    environment:
      MODEL_PATH: /app/model/model.json
      PORT: "8080"
      ENCLAVE_PRIVATE_KEY: "${GREEN_ENCLAVE_KEY}"
    healthcheck:
      test: ["CMD-SHELL", "curl -sf http://localhost:8080/health || exit 1"]
      interval: 5s
      timeout: 3s
      retries: 5
    networks:
      - backend

  operator-green:
    build:
      context: .
      dockerfile: services/operator/Dockerfile
    ports:
      - "9091:9090"   # Different host port
    environment:
      OPERATOR_RPC_URL: "http://anvil:8545"
      OPERATOR_PRIVATE_KEY: "${OPERATOR_PRIVATE_KEY}"
      ENCLAVE_URL: "http://enclave-green:8080"
      PROVER_URL: "http://warm-prover:3000"
      STATE_FILE: "/app/proofs/operator-state-green.json"
    volumes:
      - deployment-data:/deployment:ro
      - proof-storage:/app/proofs
    networks:
      - backend
```

**Step 1: Build and start green services**

```bash
docker compose -f docker-compose.yml -f docker-compose.green.yml build enclave-green operator-green
docker compose -f docker-compose.yml -f docker-compose.green.yml up -d enclave-green operator-green
```

**Step 2: Verify green services**

```bash
# Check health
curl -s http://localhost:8082/health    # green enclave
curl -s http://localhost:9091/health    # green operator

# Submit a test inference through the green enclave
curl -s http://localhost:8082/infer \
  -X POST -H "Content-Type: application/json" \
  -d '{"features": [5.0, 3.5, 1.5, 0.3]}'

# If the green enclave uses a new signing key, register it on-chain first
admin-cli --rpc-url $RPC --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  register-enclave $GREEN_ENCLAVE_ADDRESS $GREEN_IMAGE_HASH
```

**Step 3: Switch traffic (promote green to blue)**

```bash
# Stop the old blue services
docker compose stop enclave operator

# Update docker-compose.yml to point to the green image/config
# Or rename green services to the standard names

# Restart with the new configuration
docker compose up -d enclave operator
```

**Step 4: Clean up**

```bash
# Stop green services
docker compose -f docker-compose.yml -f docker-compose.green.yml stop enclave-green operator-green
docker compose -f docker-compose.yml -f docker-compose.green.yml rm enclave-green operator-green
```

### Service-Specific Upgrade Notes

**Enclave upgrade:**
- New enclave image may have a different PCR0 hash and signing key
- Register the new enclave on-chain BEFORE switching traffic (Section 2)
- Revoke the old enclave after all pending results clear the challenge window (minimum 1 hour)
- The enclave is stateless; no data migration needed

**Operator upgrade:**
- Copy the state file from the old operator to the new one, OR let the new operator start fresh from the latest block
- Ensure only ONE operator is actively submitting transactions at a time to avoid nonce conflicts
- To hand off: stop old operator, copy `operator-state.json`, start new operator

**Indexer upgrade:**
- The indexer uses SQLite with single-writer semantics; only ONE instance can run at a time
- Use `Recreate` strategy (not rolling) to avoid concurrent writes
- Backup the database before upgrade (see Section 10)
- For zero-downtime indexer upgrades, migrate to PostgreSQL first

**Warm prover upgrade:**
- Stateless service; safe to restart or replace at any time
- During transition, both old and new can serve proofs simultaneously behind a load balancer

### Rollback

If the green services are unhealthy:

```bash
# Stop green, restart blue
docker compose -f docker-compose.yml -f docker-compose.green.yml stop enclave-green operator-green
docker compose up -d enclave operator
```

---

## 12. Attestation Expiry and Re-Registration

### How Attestation Works

The operator verifies enclave attestations when `NITRO_VERIFICATION=true`. Attestation documents are fetched from the enclave's `/attestation` endpoint and validated for:

1. **Certificate chain** -- AWS Nitro root CA to enclave certificate
2. **Nonce** -- `keccak256(chainId || blockNumber || enclaveAddress)` prevents replay
3. **PCR0** -- matches expected enclave image hash (if `EXPECTED_PCR0` is set)
4. **Freshness** -- attestation document must be less than a configured age:
   - **On submit:** 600 seconds (10 minutes) (`validate_freshness(&verified, 600)`)
   - **On register:** 300 seconds (5 minutes) (`validate_freshness(&verified, 300)`)

### Attestation Cache TTL

The operator caches verified attestation documents to avoid re-fetching on rapid successive submits. The cache TTL is configured by:

| Setting | Default | Source |
|---------|---------|--------|
| `ATTESTATION_CACHE_TTL` env var | 300 seconds | `services/operator/src/config.rs` |
| `attestation_cache_ttl` in TOML | 300 seconds | Config file |

When the cache expires, the next submit triggers a fresh attestation fetch and verification.

### What Happens When Attestation Expires

1. **Cached attestation expires (normal operation):** The operator transparently re-fetches and re-verifies the attestation. No manual action needed. This happens automatically on the next submit when the cache age exceeds `ATTESTATION_CACHE_TTL`.

2. **Enclave attestation document too old (freshness check fails):** The operator receives a fresh document from the enclave on each fetch request, so freshness failures are rare. If they occur, it means the enclave is returning stale cached documents internally. Resolution: restart the enclave.

3. **PCR0 mismatch (enclave image changed):** This happens after an enclave image update. The new image has a different PCR0 hash. Resolution: update `EXPECTED_PCR0` in the operator config or re-register the enclave.

4. **Certificate chain validation fails:** This can happen if the AWS Nitro CA rotates or the enclave's certificate expires. Resolution: rebuild and redeploy the enclave image to get a fresh certificate.

### Re-Registering an Enclave

Use the operator's `register` subcommand or the admin-cli to re-register:

**Using the operator CLI (recommended for Nitro enclaves):**

```bash
# This fetches a fresh attestation, verifies it, and registers on-chain
tee-operator register \
  --config operator-config.toml \
  --expected-pcr0 $EXPECTED_PCR0

# In dev mode (skip attestation verification)
tee-operator register \
  --config operator-config.toml \
  --skip-verify
```

The `register` command:
1. Calls `GET /info` on the enclave to get its address
2. Computes a chain-bound nonce from `keccak256(chainId || blockNumber || enclaveAddress)`
3. Fetches attestation with the nonce via `GET /attestation?nonce=<hex>`
4. Verifies: certificate chain, nonce match, PCR0, freshness (5 minutes)
5. Calls `registerEnclave(enclaveAddress, imageHash)` on TEEMLVerifier

**Using admin-cli (when you already have the address and image hash):**

```bash
admin-cli \
  --rpc-url $RPC \
  --contract $TEE_VERIFIER \
  --private-key $OWNER_KEY \
  register-enclave $ENCLAVE_ADDRESS $IMAGE_HASH
```

### Monitoring Attestation Health

Check the enclave's attestation status:

```bash
# Fetch a fresh attestation document
NONCE=$(openssl rand -hex 32)
curl -s "http://$ENCLAVE:8080/attestation?nonce=0x${NONCE}" | jq '{is_nitro, pcr0}'

# Check enclave identity
curl -s http://$ENCLAVE:8080/info | jq '{enclave_address, model_hash}'

# Verify enclave health with detailed watchdog status
curl -s http://$ENCLAVE:8080/health/detailed | jq .
```

### Enclave Watchdog

The enclave runs a background watchdog (configurable via `WATCHDOG_ENABLED`, default: `true`) that monitors:

| Check | Warn Threshold | Critical Threshold |
|-------|---------------|-------------------|
| Error rate | > 5% | > 20% |
| Avg latency | > 10,000 ms | > 30,000 ms |
| Memory (RSS) | > 512 MB | > 1024 MB |

The watchdog runs every 30 seconds and updates `GET /health/detailed`. If the watchdog reports `unhealthy`, the enclave's inference quality may be degraded and attestation-backed results may be unreliable.

---

## Quick Reference

### Key Environment Variables

| Variable | Service | Default |
|----------|---------|---------|
| `OPERATOR_RPC_URL` | Operator | `http://127.0.0.1:8545` |
| `OPERATOR_PRIVATE_KEY` | Operator | (required) |
| `TEE_VERIFIER_ADDRESS` | Operator | (required) |
| `ENCLAVE_URL` | Operator | `http://127.0.0.1:8080` |
| `PROVER_URL` | Operator | (optional) |
| `STATE_FILE` | Operator | `./operator-state.json` |
| `DRY_RUN` | Operator | `false` |
| `WEBHOOK_URL` | Operator | (optional) |
| `MAX_PROOF_RETRIES` | Operator | `3` |
| `PROOF_RETRY_DELAY_SECS` | Operator | `10` |
| `NITRO_VERIFICATION` | Operator | `false` |
| `ATTESTATION_CACHE_TTL` | Operator | `300` |
| `EXPECTED_PCR0` | Operator | (optional) |
| `METRICS_PORT` | Operator | `9090` |
| `MODEL_PATH` | Enclave | `/app/model/model.json` |
| `ENCLAVE_PRIVATE_KEY` | Enclave | (auto-generated) |
| `MAX_REQUESTS_PER_MINUTE` | Enclave | `120` |
| `CHAIN_ID` | Enclave | `1` |
| `ADMIN_API_KEY` | Enclave | (optional) |
| `WATCHDOG_ENABLED` | Enclave | `true` |
| `DB_PATH` | Indexer | `./indexer.db` |
| `DB_TYPE` | Indexer | `sqlite` |
| `DATABASE_URL` | Indexer | (required if DB_TYPE=postgres) |
| `POLL_INTERVAL_SECS` | Indexer | `12` |

### Essential Commands

```bash
# Contract status (admin-cli)
admin-cli --rpc-url $RPC --contract $CONTRACT status

# Pause / unpause
admin-cli --rpc-url $RPC --contract $CONTRACT --private-key $KEY pause
admin-cli --rpc-url $RPC --contract $CONTRACT --private-key $KEY unpause

# Enclave management
admin-cli --rpc-url $RPC --contract $CONTRACT --private-key $KEY \
  register-enclave $ADDR $HASH
admin-cli --rpc-url $RPC --contract $CONTRACT --private-key $KEY \
  revoke-enclave $ADDR

# Contract queries
cast call $CONTRACT "paused()(bool)" --rpc-url $RPC
cast call $CONTRACT "owner()(address)" --rpc-url $RPC
cast call $CONTRACT "remainderVerifier()(address)" --rpc-url $RPC
cast call $CONTRACT "challengeBondAmount()(uint256)" --rpc-url $RPC
cast call $CONTRACT "proverStake()(uint256)" --rpc-url $RPC

# Service health
curl -s http://localhost:8080/health   # enclave
curl -s http://localhost:9090/health   # operator
curl -s http://localhost:8081/health   # indexer
curl -s http://localhost:3000/health   # warm prover

# Kubernetes
kubectl -n world-zk get pods
kubectl -n world-zk logs -l app=worldzk-operator --tail=100
kubectl -n world-zk rollout restart deployment worldzk-operator
kubectl -n world-zk get hpa worldzk-operator-hpa

# Docker Compose
docker compose ps
docker compose logs operator --tail=100
docker compose restart operator

# Anvil time manipulation (local dev)
cast rpc anvil_increaseTime 3601 --rpc-url http://127.0.0.1:8545
cast rpc anvil_mine 1 --rpc-url http://127.0.0.1:8545
```
