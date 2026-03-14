# Production Readiness Checklist

## Pre-Deployment Checklist

- [ ] Security audit completed on `TEEMLVerifier.sol` and `RemainderVerifier.sol`
- [ ] Contract source verified on block explorer (Etherscan / Arbiscan)
- [ ] Admin key stored in hardware wallet or multisig (Safe)
- [ ] 2-step ownership transfer completed (`transferOwnership` + `acceptOwnership`)
- [ ] Emergency contacts documented and shared with on-call team
- [ ] Incident response runbook reviewed by all operators
- [ ] Dry-run test passed on target chain (`DRY_RUN=true`)
- [ ] Load test completed against enclave (target: 120 req/min default limit)

---

## Contract Configuration

### TEEMLVerifier Parameters

| Parameter | Default | Range | Notes |
|-----------|---------|-------|-------|
| `proverStake` | 0.1 ETH | 0 < x <= 100 ETH | Stake locked per `submitResult` |
| `challengeBondAmount` | 0.1 ETH | 0 < x <= 100 ETH | Bond required to challenge |
| `CHALLENGE_WINDOW` | 1 hour | constant | Time for challengers to act |
| `DISPUTE_WINDOW` | 24 hours | constant | Time for prover to submit ZK proof |
| `EXTENSION_PERIOD` | 30 minutes | constant | Per-extension time added |
| `MAX_EXTENSIONS` | 1 | constant | Max dispute extensions allowed |

### Deploy Sequence

```bash
# 1. Deploy RemainderVerifier (DAG proof verifier)
# 2. Deploy TEEMLVerifier with admin + RemainderVerifier address
DEPLOYER_KEY=0x... \
REMAINDER_VERIFIER=0x... \
ADMIN_ADDRESS=0x...  \
forge script contracts/script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
  --rpc-url $RPC_URL \
  --private-key $DEPLOYER_KEY \
  --broadcast \
  --verify

# 3. Accept ownership from multisig (if ADMIN_ADDRESS was set)
cast send $TEE_VERIFIER "acceptOwnership()" \
  --rpc-url $RPC_URL \
  --private-key $ADMIN_KEY

# 4. Register enclave key
cast send $TEE_VERIFIER \
  "registerEnclave(address,bytes32)" \
  $ENCLAVE_PUBLIC_KEY $ENCLAVE_IMAGE_HASH \
  --rpc-url $RPC_URL \
  --private-key $ADMIN_KEY

# 5. Adjust economic parameters if needed
cast send $TEE_VERIFIER "setProverStake(uint256)" $STAKE_WEI \
  --rpc-url $RPC_URL --private-key $ADMIN_KEY
cast send $TEE_VERIFIER "setChallengeBondAmount(uint256)" $BOND_WEI \
  --rpc-url $RPC_URL --private-key $ADMIN_KEY
```

### Verify Deployment

```bash
bash scripts/verify-deployment.sh $RPC_URL $TEE_VERIFIER_ADDRESS
```

This runs 8 checks: contract deployed, owner set, not paused, proverStake > 0, challengeBondAmount > 0, remainderVerifier set, CHALLENGE_WINDOW > 0, DISPUTE_WINDOW > 0.

---

## Operator Setup

### Configuration Reference

The operator loads config from a TOML file and/or environment variables. Env vars always override TOML values.

**Required fields:**

| Env Var | TOML Key | Description |
|---------|----------|-------------|
| `OPERATOR_PRIVATE_KEY` | `private_key` | Operator wallet private key |
| `TEE_VERIFIER_ADDRESS` | `tee_verifier_address` | Deployed TEEMLVerifier address |

**Optional fields (with defaults):**

| Env Var | TOML Key | Default | Description |
|---------|----------|---------|-------------|
| `OPERATOR_RPC_URL` | `rpc_url` | `http://127.0.0.1:8545` | Chain RPC endpoint |
| `ENCLAVE_URL` | `enclave_url` | `http://127.0.0.1:8080` | TEE enclave HTTP address |
| `MODEL_PATH` | `model_path` | `./model.json` | Legacy single-model path |
| `PROOFS_DIR` | `proofs_dir` | `./proofs` | Directory for cached proofs |
| `PROVER_STAKE` | `prover_stake` | `100000000000000000` (0.1 ETH) | Wei sent with submitResult |
| `PRECOMPUTE_BIN` | `precompute_bin` | `precompute_proof` | Path to ZK proof binary |
| `NITRO_VERIFICATION` | `nitro_verification` | `false` | Verify Nitro attestation |
| `EXPECTED_PCR0` | `expected_pcr0` | none | Expected enclave PCR0 value |
| `ATTESTATION_CACHE_TTL` | `attestation_cache_ttl` | `300` (5 min) | Attestation cache TTL (seconds) |
| `PROVER_URL` | `prover_url` | none | Warm prover HTTP service URL |
| `METRICS_PORT` | `metrics_port` | `9090` | Health/metrics HTTP port |
| `MAX_PROOF_RETRIES` | `max_proof_retries` | `3` | Proof generation retry count |
| `PROOF_RETRY_DELAY_SECS` | `proof_retry_delay_secs` | `10` | Base retry delay (exponential backoff) |
| `WEBHOOK_URL` | `webhook_url` | none | Slack-compatible webhook for alerts |
| `DRY_RUN` | `dry_run` | `false` | Simulate without sending txs |

### Multi-Model Registry (TOML)

```toml
# operator.toml
rpc_url = "https://arb-sepolia.g.alchemy.com/v2/YOUR_KEY"
private_key = "0x..."
tee_verifier_address = "0x..."
enclave_url = "http://enclave:8080"
webhook_url = "https://hooks.slack.com/services/T00/B00/XXXX"
metrics_port = 9090
nitro_verification = true
expected_pcr0 = "abcdef..."

[[models]]
name = "xgboost-iris"
path = "/models/iris.json"
model_format = "xgboost"
model_hash = "0x..."

[[models]]
name = "lgbm-credit"
path = "/models/credit.json"
model_format = "lightgbm"
```

### Dry-Run Test

```bash
# Test the full operator flow without spending gas
DRY_RUN=true \
OPERATOR_PRIVATE_KEY=0x... \
TEE_VERIFIER_ADDRESS=0x... \
OPERATOR_RPC_URL=https://... \
cargo run --release -p operator -- watch --metrics-port 9090
```

### Production Startup

```bash
# With TOML config file
cargo run --release -p operator -- --config operator.toml watch --metrics-port 9090

# With env vars only
OPERATOR_PRIVATE_KEY=0x... \
TEE_VERIFIER_ADDRESS=0x... \
OPERATOR_RPC_URL=https://... \
ENCLAVE_URL=http://enclave:8080 \
WEBHOOK_URL=https://hooks.slack.com/... \
NITRO_VERIFICATION=true \
EXPECTED_PCR0=abcdef... \
RUST_LOG=info \
cargo run --release -p operator -- watch --metrics-port 9090
```

---

## TEE Enclave Setup

### Configuration Reference

| Env Var | Default | Description |
|---------|---------|-------------|
| `MODEL_PATH` | `/app/model/model.json` | Path to model JSON inside enclave |
| `MODEL_FORMAT` | `auto` | `auto`, `xgboost`, or `lightgbm` |
| `PORT` | `8080` | HTTP server port |
| `ENCLAVE_PRIVATE_KEY` | none (random generated) | Hex secp256k1 key; omit for Nitro-bound key |
| `NITRO_ENABLED` | `false` | Enable AWS Nitro attestation |
| `CHAIN_ID` | `1` | Chain ID for replay protection |
| `ADMIN_API_KEY` | none | API key for admin endpoints (model reload) |
| `MAX_REQUESTS_PER_MINUTE` | `120` | Rate limit for inference requests |

### Build Nitro Enclave Image

```bash
# Build Docker image for Nitro EIF
docker build -f tee/Dockerfile.nitro -t tee-enclave-nitro tee/

# Build EIF (on Nitro-enabled EC2 instance)
nitro-cli build-enclave \
  --docker-uri tee-enclave-nitro \
  --output-file tee-enclave.eif

# Record PCR0 from build output -- this is the enclaveImageHash
# PCR0: abcdef1234...
```

### Run Enclave

```bash
# Local development (no Nitro)
MODEL_PATH=/path/to/model.json \
PORT=8080 \
ENCLAVE_PRIVATE_KEY=0x... \
CHAIN_ID=421614 \
ADMIN_API_KEY=my-secret \
MAX_REQUESTS_PER_MINUTE=60 \
cargo run --release -p tee-enclave

# AWS Nitro production
nitro-cli run-enclave \
  --eif-path tee-enclave.eif \
  --cpu-count 2 \
  --memory 512
```

### Health Check

```bash
curl -sf http://localhost:8080/health
# {"status":"ok"}
```

---

## Monitoring

### Operator Metrics Endpoints

| Endpoint | Format | Description |
|----------|--------|-------------|
| `GET /health` | JSON | `{"status":"ok","uptime_secs":N,"last_block_polled":N}` |
| `GET /metrics` | Prometheus text | Scrape target for Prometheus |
| `GET /metrics/json` | JSON | Machine-readable metrics |

### Prometheus Scrape Config

```yaml
scrape_configs:
  - job_name: "world-zk-operator"
    scrape_interval: 15s
    static_configs:
      - targets: ["operator:9090"]
```

### Key Metrics to Alert On

| Metric | Type | Alert Condition |
|--------|------|-----------------|
| `operator_active_disputes` | gauge | > 0 for > 30 min |
| `operator_disputes_failed` | counter | any increase |
| `operator_errors_total` | counter | rate > 1/min |
| `operator_last_block_polled` | gauge | stale > 5 min |
| `operator_uptime_seconds` | gauge | reset (restart detected) |
| `operator_challenges_detected` | counter | any increase (informational) |

### Webhook Alerts (Slack-Compatible)

The operator sends JSON payloads to `WEBHOOK_URL` for these events:

- **`dispute`** -- result challenged, includes challenger address and deadline
- **`proof_submitted`** -- ZK proof submitted, includes tx hash
- **`dispute_resolved`** -- dispute settled, includes winner

### Deadline Monitor

The operator's built-in `DeadlineMonitor` logs warnings at 5 min and errors at 1 min before dispute deadlines expire. These appear in structured logs (`RUST_LOG=info`):

```
WARN  deadline_monitor: WARNING: Dispute deadline approaching in <300 seconds
ERROR deadline_monitor: CRITICAL: Dispute deadline in <60 seconds -- submit proof NOW
ERROR deadline_monitor: DISPUTE DEADLINE EXPIRED -- challenger wins by timeout
```

### Log Aggregation

```bash
# Structured JSON logs (recommended for production)
RUST_LOG=info RUST_LOG_FORMAT=json cargo run --release -p operator -- watch
```

Pipe to your log aggregator (Datadog, Grafana Loki, CloudWatch, etc.).

---

## Post-Deployment Verification

### 1. Run Health Check Script

```bash
bash scripts/verify-deployment.sh $RPC_URL $TEE_VERIFIER_ADDRESS
# Expected: 8/8 checks passed
```

### 2. Verify Enclave is Responsive

```bash
curl -sf http://$ENCLAVE_URL/health
```

### 3. Verify Operator is Running

```bash
curl -sf http://$OPERATOR_URL:9090/health
# Check last_block_polled is advancing
curl -sf http://$OPERATOR_URL:9090/metrics/json | jq .last_block_polled
```

### 4. Submit Test Result (Testnet)

```bash
# Use the E2E demo script
docker compose up  # starts anvil, deployer, enclave, warm-prover, operator

# Or manually:
cast send $TEE_VERIFIER "submitResult(bytes32,bytes32,bytes,bytes)" \
  $MODEL_HASH $INPUT_HASH $RESULT $ATTESTATION \
  --value 0.1ether \
  --rpc-url $RPC_URL \
  --private-key $PROVER_KEY
```

### 5. Verify Operator Detects Events

Check operator logs for `ResultSubmitted` event detection:
```bash
curl -sf http://$OPERATOR_URL:9090/metrics/json | jq .total_submissions
```

### 6. Full E2E Test

```bash
# Local (Anvil)
bash scripts/e2e-test.sh --example rule-engine --network local

# Sepolia
SEPOLIA_RPC_URL=https://... \
PRIVATE_KEY=0x... \
PROVER_PRIVATE_KEY=0x... \
REQUESTER_PRIVATE_KEY=0x... \
bash scripts/e2e-test.sh --example rule-engine --network sepolia
```

---

## Incident Response

### Pause Contract (Stop All Submissions)

```bash
cast send $TEE_VERIFIER "pause()" \
  --rpc-url $RPC_URL \
  --private-key $ADMIN_KEY
```

Verify paused state:
```bash
cast call $TEE_VERIFIER "paused()(bool)" --rpc-url $RPC_URL
# true
```

### Unpause Contract

```bash
cast send $TEE_VERIFIER "unpause()" \
  --rpc-url $RPC_URL \
  --private-key $ADMIN_KEY
```

### Revoke Compromised Enclave

```bash
cast send $TEE_VERIFIER "revokeEnclave(address)" $COMPROMISED_ENCLAVE_KEY \
  --rpc-url $RPC_URL \
  --private-key $ADMIN_KEY
```

Verify revocation:
```bash
cast call $TEE_VERIFIER "enclaves(address)" $COMPROMISED_ENCLAVE_KEY --rpc-url $RPC_URL
# active field should be false
```

### Replace RemainderVerifier (Bug in ZK Verifier)

```bash
cast send $TEE_VERIFIER "setRemainderVerifier(address)" $NEW_VERIFIER_ADDRESS \
  --rpc-url $RPC_URL \
  --private-key $ADMIN_KEY
```

### Recover Stuck Funds

Active disputes lock `proverStake + challengeBond`. If the operator is down and cannot submit a proof:

1. **Extend deadline** (submitter only, once):
   ```bash
   cast send $TEE_VERIFIER "extendDisputeWindow(bytes32)" $RESULT_ID \
     --rpc-url $RPC_URL --private-key $SUBMITTER_KEY
   ```

2. **Resolve by timeout** (anyone, after deadline):
   ```bash
   cast send $TEE_VERIFIER "resolveDisputeByTimeout(bytes32)" $RESULT_ID \
     --rpc-url $RPC_URL --private-key $ANY_KEY
   ```

3. **Finalize unchallenged results** (anyone, after challenge window):
   ```bash
   cast send $TEE_VERIFIER "finalize(bytes32)" $RESULT_ID \
     --rpc-url $RPC_URL --private-key $ANY_KEY
   ```

### Contact Escalation

| Severity | Response Time | Action |
|----------|---------------|--------|
| P0 -- Funds at risk / enclave compromised | < 15 min | Pause contract, revoke enclave, notify team |
| P1 -- Operator down / dispute deadline approaching | < 1 hour | Restart operator, extend dispute if needed |
| P2 -- Webhook failures / metrics gaps | < 4 hours | Investigate connectivity, check logs |
| P3 -- Test failures / non-prod issues | Next business day | Investigate and fix |

---

## Docker Compose (Local Full Stack)

```bash
# Build all services
docker compose build

# Start full stack: anvil + deployer + enclave + warm-prover + operator
docker compose up

# Verify all services healthy
docker compose ps
# anvil:        healthy (port 8545)
# enclave:      healthy (port 8080)
# warm-prover:  healthy (port 3000)
# operator:     healthy (port 9090)

# Tear down
docker compose down -v
```

---

## Security Checklist

- [ ] Admin key is NOT the deployer key in production
- [ ] `OPERATOR_PRIVATE_KEY` is not committed to version control
- [ ] `ENCLAVE_PRIVATE_KEY` is omitted in Nitro mode (key generated inside enclave)
- [ ] `ADMIN_API_KEY` is set on enclave for model reload endpoint
- [ ] `NITRO_VERIFICATION=true` and `EXPECTED_PCR0` set on operator
- [ ] Rate limiting configured (`MAX_REQUESTS_PER_MINUTE`)
- [ ] Enclave not accessible from public internet (only via operator)
- [ ] Webhook URL uses HTTPS
- [ ] Contract uses Ownable2Step (prevents accidental ownership transfer)
- [ ] No replay attack vector: `CHAIN_ID` set correctly on enclave

---

## Sepolia Testnet Deployment Checklist

### Pre-Deployment

- [ ] Alchemy/Infura API key provisioned for Sepolia
- [ ] Deployer wallet funded with ~0.06 ETH on Sepolia
- [ ] `.env.sepolia` created from `.env.sepolia.example` with all keys set
- [ ] Private keys are NOT committed to version control
- [ ] Foundry installed (`forge`, `cast` available)

### Deployment

- [ ] Run `source .env.sepolia && ./scripts/deploy-sepolia-tee.sh`
- [ ] Contract addresses saved to `.env.sepolia` (TEE_VERIFIER_ADDRESS, EXECUTION_ENGINE_ADDRESS)
- [ ] Etherscan verification completed (if ETHERSCAN_API_KEY set)
- [ ] Enclave key registered on TEEMLVerifier
- [ ] Program image ID registered on ProgramRegistry

### Post-Deployment Validation

- [ ] Run `./scripts/check-sepolia-balances.sh` — all wallets funded
- [ ] Run `./scripts/sepolia-status.sh` — RPC connected, contracts readable
- [ ] Run `./scripts/sepolia-e2e.sh` — E2E inference + submission works
- [ ] Start services: `docker compose -f docker-compose.sepolia.yml --env-file .env.sepolia up -d`
- [ ] All services healthy: `docker compose -f docker-compose.sepolia.yml ps`
- [ ] Operator polling events: check logs for `ResultSubmitted` detection

### Monitoring

- [ ] Start monitoring: `docker compose -f docker-compose.sepolia.yml -f docker-compose.monitoring.yml --env-file .env.sepolia up -d`
- [ ] Grafana accessible at `http://localhost:3001`
- [ ] Import `monitoring/grafana-sepolia.json` dashboard
- [ ] Verify Prometheus scraping operator/enclave metrics

### Sepolia-Specific Notes

- RemainderVerifier exceeds Sepolia's 24KB code size limit — only TEE path is deployable
- Verifier router address: `0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187` (risc0 v3.0.x)
- Sepolia block time: ~12 seconds (same as mainnet)
- Faucet: https://www.alchemy.com/faucets/ethereum-sepolia
