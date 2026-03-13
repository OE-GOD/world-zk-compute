# Disaster Recovery Playbook

Practical recovery procedures for every world-zk-compute component.
All commands assume production Linux hosts with `aws`, `cast`, and `curl` available.

---

## 1. Operator State Backup

The operator persists crash-recovery state to a JSON file via `StateStore`
(`services/operator/src/store.rs`). Writes are atomic (write-to-tmp, then
`rename`). The state tracks `last_polled_block`, `active_disputes`, and
`processed_event_ids`.

Proof files are stored as individual JSON files in `PROOFS_DIR` (default
`./proofs`), one per `result_id`, via `ProofStore`.

### Backup (cron)

```bash
# Every 15 minutes, upload operator state + proofs to S3
*/15 * * * * /usr/local/bin/backup-operator.sh

# backup-operator.sh
#!/bin/bash
set -euo pipefail
TIMESTAMP=$(date +\%Y\%m\%d-\%H\%M\%S)
BUCKET=s3://worldzk-backups/operator

aws s3 cp /data/operator-state.json "$BUCKET/state/state-$TIMESTAMP.json"
tar czf /tmp/proofs-$TIMESTAMP.tar.gz -C /data/proofs .
aws s3 cp /tmp/proofs-$TIMESTAMP.tar.gz "$BUCKET/proofs/proofs-$TIMESTAMP.tar.gz"
rm /tmp/proofs-$TIMESTAMP.tar.gz

# Retain 7 days
aws s3 ls "$BUCKET/state/" | awk -v cutoff="$(date -d '-7 days' +%Y-%m-%d)" \
  '$1 < cutoff {print $4}' | xargs -I {} aws s3 rm "$BUCKET/state/{}"
```

### Restore Checklist

- [ ] List backups: `aws s3 ls s3://worldzk-backups/operator/state/ --recursive | sort -k1,2`
- [ ] Download latest: `aws s3 cp s3://worldzk-backups/operator/state/state-LATEST.json /data/operator-state.json`
- [ ] Restore proofs: `aws s3 cp s3://worldzk-backups/operator/proofs/proofs-LATEST.tar.gz - | tar xzf - -C /data/proofs`
- [ ] Restart: `systemctl restart tee-operator`
- [ ] Verify: `curl -s http://localhost:9090/health | jq .`
- [ ] Check logs: `journalctl -u tee-operator --since '5 min ago' | grep -i 'Loaded operator state'`

The operator calls `load_or_default()` on startup. If the state file is missing
or corrupt, it starts fresh from the latest block. Missed events between
`last_polled_block` and the current block are re-polled automatically.

**RPO**: 15 min (cron interval). **RTO**: < 5 min.

---

## 2. Indexer DB Backup

The indexer (`services/indexer/src/main.rs`) uses SQLite at `DB_PATH` (default
`./indexer.db`). Tables: `results` (id, model_hash, input_hash, output,
submitter, status, block_number, timestamp, challenger) and `indexer_state`
(tracks `last_indexed_block`).

### SQLite Daily Backup

```bash
# Daily at 02:00 UTC -- online backup (safe while indexer is running)
0 2 * * * sqlite3 /data/indexer.db ".backup /data/backups/indexer-$(date +\%Y\%m\%d).db" && \
          aws s3 cp /data/backups/indexer-$(date +\%Y\%m\%d).db s3://worldzk-backups/indexer/
```

### PostgreSQL Backup (if migrated)

```bash
# Daily logical backup
0 2 * * * pg_dump -Fc -h localhost -U worldzk indexer_db > /data/backups/indexer-$(date +\%Y\%m\%d).dump && \
          aws s3 cp /data/backups/indexer-$(date +\%Y\%m\%d).dump s3://worldzk-backups/indexer/
```

### Restore from Backup

- [ ] Stop indexer: `systemctl stop tee-indexer`
- [ ] SQLite: `cp /data/backups/indexer-YYYYMMDD.db /data/indexer.db`
- [ ] PostgreSQL: `pg_restore -h localhost -U worldzk -d indexer_db /data/backups/indexer-YYYYMMDD.dump`
- [ ] Start indexer: `systemctl start tee-indexer`
- [ ] Verify: `curl -s http://localhost:8081/health | jq .`

### Full Rebuild from Chain Events

If no backup exists, the indexer can be rebuilt from on-chain events:

- [ ] Delete DB: `rm /data/indexer.db`
- [ ] Set start block to contract deployment block for faster sync
- [ ] Start: `POLL_START_BLOCK=<deploy_block> systemctl start tee-indexer`
- [ ] Monitor: `curl -s http://localhost:8081/health | jq .last_indexed_block`

**RPO**: Zero -- the chain is the authoritative source of truth. **RTO**: < 2 min
from backup, approximately 30 min for full chain rebuild.

---

## 3. Contract State Recovery

Smart contract state is on-chain and cannot be lost. Recovery means
re-establishing off-chain service connections and verifying contract health.

### Post-Incident Checklist

- [ ] Confirm contract addresses from `deployments/chains.json` or deployment broadcast files
- [ ] Check not paused: `cast call $CONTRACT "paused()(bool)" --rpc-url $RPC`
- [ ] Check owner: `cast call $CONTRACT "owner()(address)" --rpc-url $RPC`
- [ ] Check enclave registrations: `cast call $VERIFIER "registeredEnclaves(address)(bool,bytes32)" $ENCLAVE_ADDR --rpc-url $RPC`
- [ ] Check circuit registrations: `cast call $REMAINDER "circuits(bytes32)" $CIRCUIT_HASH --rpc-url $RPC`
- [ ] If owner key compromised: execute `transferOwnership(newOwner)` immediately from current owner
- [ ] If contract paused by attacker: unpause requires owner key -- use multisig or timelock recovery

### Re-Index All Events

```bash
# Dump all contract events from deployment block onward
cast logs --from-block $DEPLOY_BLOCK --address $CONTRACT --rpc-url $RPC
```

---

## 4. Enclave Recovery

The TEE enclave (`tee/enclave/src/main.rs`) auto-generates a secp256k1 keypair
on startup (unless `ENCLAVE_PRIVATE_KEY` is set). On AWS Nitro, the key is
bound to the enclave image via PCR0. The model is loaded from `MODEL_PATH`.

### Re-Attestation Flow

- [ ] Launch new enclave:
  ```bash
  nitro-cli run-enclave --eif-path enclave.eif --cpu-count 2 --memory 512
  ```
- [ ] Verify health: `curl -s http://<enclave-ip>:8080/health | jq .`
- [ ] Get enclave address: `curl -s http://<enclave-ip>:8080/info | jq .enclave_address`
- [ ] Register new enclave on-chain:
  ```bash
  cast send $VERIFIER "registerEnclave(address,bytes32)" $NEW_ADDR $PCR0_HASH \
    --private-key $ADMIN_KEY --rpc-url $RPC
  ```
- [ ] Revoke old enclave (if compromised or decommissioned):
  ```bash
  cast send $VERIFIER "revokeEnclave(address)" $OLD_ADDR \
    --private-key $ADMIN_KEY --rpc-url $RPC
  ```
- [ ] Update operator config: `ENCLAVE_URL=http://<new-enclave-ip>:8080`
- [ ] Verify inference: `curl -s -X POST http://<enclave-ip>:8080/infer -H 'Content-Type: application/json' -d '{"features":[1.0,2.0,3.0,4.0]}' | jq .`

### Model Hot-Reload (No Restart)

If the model file changed but the enclave is still healthy:

```bash
curl -X POST http://<enclave-ip>:8080/admin/reload-model \
  -H "Authorization: Bearer $ADMIN_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model_path": "/app/model/model.json"}'
```

Requires `ADMIN_API_KEY` env var to be set on the enclave.

**RTO**: < 10 min (launch + register + verify).

---

## 5. Failover Procedures

### RPC Endpoint Failover

The operator, indexer, and enclave all depend on an Ethereum RPC endpoint.

- [ ] Detect: health checks fail, logs show `Failed to poll` or connection timeouts
- [ ] Switch operator: `kubectl set env deployment/tee-operator OPERATOR_RPC_URL=https://backup-rpc.example.com`
- [ ] Switch indexer: `kubectl set env deployment/tee-indexer RPC_URL=https://backup-rpc.example.com`
- [ ] Verify: `cast block-number --rpc-url https://backup-rpc.example.com`

Recommended: configure an RPC load balancer (e.g., Infura + Alchemy + self-hosted)
with automatic failover upstream of the services.

### Operator Failover (Multi-Replica)

The operator is near-stateless at runtime (state file is read on start, written
periodically). Multiple replicas can run safely -- duplicate on-chain
transactions will revert harmlessly.

```yaml
# Kubernetes: ensure at least 1 operator replica is always running
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: tee-operator-pdb
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app: tee-operator
```

Manual failover:

- [ ] Scale up: `kubectl scale deployment tee-operator --replicas=2`
- [ ] Kill stuck pod: `kubectl delete pod <pod-name> --force --grace-period=0`
- [ ] Verify new pod healthy: `kubectl get pods -l app=tee-operator`

### Enclave Failover

- [ ] Detect: watchdog reports unhealthy (`/health` returns non-200), or inference latency spike
- [ ] Launch replacement enclave (see Section 4)
- [ ] Register new enclave on-chain, update operator `ENCLAVE_URL`
- [ ] Revoke old enclave only after confirming new one is serving traffic

For zero-downtime, run 2+ enclaves behind a load balancer. Both must be
registered on-chain.

---

## 6. Recovery Time Objectives

| Component | RTO | RPO | Recovery Method |
|-----------|-----|-----|-----------------|
| Operator | < 5 min | 15 min | Restore state JSON from S3 + restart |
| Indexer (from backup) | < 2 min | 24 hours | Restore SQLite/pg_dump + restart |
| Indexer (from chain) | ~30 min | 0 | Delete DB, re-index from deployment block |
| Enclave | < 10 min | N/A | Launch new instance + register on-chain |
| Contracts | N/A | N/A | Immutable on-chain; nothing to recover |
| Warm Prover | < 5 min | N/A | Restart container; stateless |
| **Full Stack** | **< 15 min** | **15 min** | Parallel recovery of all components |

**Dispute windows provide safety margin**: challenge window is 1 hour,
dispute resolution window is 24 hours. Even a 15-min full-stack outage
leaves ample time to respond.

---

## 7. Communication Templates

### Incident Notification -- Service Degraded

```
Subject: [WorldZK] Service Degradation -- <Component>
Severity: P1/P2/P3
Status: Investigating
Impact: <User-facing impact, e.g., "Inference submissions delayed">
Start Time: <YYYY-MM-DD HH:MM UTC>
Affected: <Operator / Indexer / Enclave / Contracts>
Current Actions: <What the on-call engineer is doing>
ETA: <Estimated resolution or "TBD">
Next Update: <Time of next update, e.g., "30 min or on status change">
```

### Incident Notification -- Resolved

```
Subject: [WorldZK] RESOLVED -- <Component>
Status: Resolved
Duration: <Start> to <End> (<total duration>)
Root Cause: <1-2 sentence explanation>
Impact: <Quantified: N requests affected, N disputes delayed>
Data Loss: <None / describe if any>
Prevention: <Action items to prevent recurrence>
```

### Emergency Contract Pause

```
Subject: [URGENT] [WorldZK] Emergency Contract Pause
Action: TEEMLVerifier paused at block <N> by <admin address>
Reason: <Brief description of threat>
Impact: All submissions and challenges suspended until unpause
Tx Hash: <0x...>
Next Steps:
  1. Investigate root cause
  2. Patch if needed
  3. Unpause after team review
Unpause ETA: <Estimate or "TBD pending investigation">
Contact: <On-call engineer name + channel>
```

### Webhook Alert (Automated via Operator)

The operator sends automated Slack notifications via `WEBHOOK_URL` for:
- `dispute` -- result challenged (includes result_id, challenger, deadline)
- `proof_submitted` -- ZK proof submitted for disputed result
- `dispute_resolved` -- dispute settled (includes winner)

Configure: set `WEBHOOK_URL` env var on the operator to a Slack incoming webhook URL.
Monitor failures: check `notification_failures` counter on the operator health endpoint.
