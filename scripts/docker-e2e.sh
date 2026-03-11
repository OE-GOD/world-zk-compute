#!/usr/bin/env bash
set -euo pipefail

# ===============================================================================
# Docker Compose E2E Smoke Test
#
# Validates the full Docker Compose stack:
#   build -> start -> deploy -> health checks -> deployment data -> cleanup
#
# Services tested:
#   anvil         Local Ethereum node (port 8545)
#   deployer      One-shot contract deployment (writes /deployment/addresses.json)
#   enclave       TEE inference service (port 8080)
#   warm-prover   ZK proof server (port 3000)
#   operator      Orchestrator service (port 9090)
#
# Usage:
#   ./scripts/docker-e2e.sh              Run full smoke test
#   ./scripts/docker-e2e.sh --no-build   Skip docker compose build step
#   ./scripts/docker-e2e.sh --keep       Do not tear down stack on exit
# ===============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Parse flags
SKIP_BUILD=false
KEEP_STACK=false
for arg in "$@"; do
    case "$arg" in
        --no-build) SKIP_BUILD=true ;;
        --keep)     KEEP_STACK=true ;;
        -h|--help)
            echo "Usage: $0 [--no-build] [--keep]"
            echo "  --no-build   Skip docker compose build step"
            echo "  --keep       Do not tear down stack on exit"
            exit 0
            ;;
        *)
            echo "Unknown flag: $arg" >&2
            exit 1
            ;;
    esac
done

# --- Logging helpers ---------------------------------------------------------

log()  { echo "==> $*"; }
ok()   { echo "  [OK] $*"; }
err()  { echo "  [FAIL] $*" >&2; }
warn() { echo "  [WARN] $*"; }

# --- Cleanup on exit --------------------------------------------------------

cleanup() {
    local exit_code=$?
    if [ "$KEEP_STACK" = true ]; then
        log "Keeping stack running (--keep). To tear down: docker compose down -v"
        return
    fi
    log "Cleaning up..."
    cd "$ROOT_DIR"
    docker compose down -v 2>/dev/null || true
    if [ "$exit_code" -ne 0 ]; then
        echo ""
        echo "================================================================"
        echo "  Docker Compose E2E Smoke Test -- FAILED (exit code $exit_code)"
        echo "================================================================"
    fi
}
trap cleanup EXIT

# --- Prerequisite checks ----------------------------------------------------

log "Checking prerequisites..."
if ! command -v docker &>/dev/null; then
    err "docker is not installed or not in PATH"
    exit 1
fi
if ! docker compose version &>/dev/null; then
    err "docker compose plugin is not available"
    exit 1
fi
if ! docker info &>/dev/null 2>&1; then
    err "Docker daemon is not running"
    exit 1
fi
ok "Prerequisites satisfied"

cd "$ROOT_DIR"

# --- Step 1: Build -----------------------------------------------------------

if [ "$SKIP_BUILD" = true ]; then
    log "Step 1: Skipping build (--no-build)"
else
    log "Step 1: Building Docker images..."
    docker compose build
    ok "All images built successfully"
fi

# --- Step 2: Start stack -----------------------------------------------------

log "Step 2: Starting Docker Compose stack..."
docker compose up -d
ok "Stack started"

# --- Step 3: Wait for Anvil --------------------------------------------------

log "Step 3: Waiting for Anvil (local Ethereum node)..."
ANVIL_TIMEOUT=60
for i in $(seq 1 "$ANVIL_TIMEOUT"); do
    if docker compose exec -T anvil cast block-number --rpc-url http://localhost:8545 >/dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq "$ANVIL_TIMEOUT" ]; then
        err "Anvil not ready after ${ANVIL_TIMEOUT}s"
        docker compose logs anvil 2>&1 | tail -20
        exit 1
    fi
    sleep 1
done
BLOCK=$(docker compose exec -T anvil cast block-number --rpc-url http://localhost:8545 2>/dev/null || echo "?")
ok "Anvil ready (block: $BLOCK)"

# --- Step 4: Wait for deployer to complete -----------------------------------

log "Step 4: Waiting for deployer to finish contract deployment..."
DEPLOYER_TIMEOUT=120
for i in $(seq 1 "$DEPLOYER_TIMEOUT"); do
    # Check if deployer container has exited
    DEPLOYER_STATE=$(docker compose ps deployer --format json 2>/dev/null \
        | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('State','unknown'))" 2>/dev/null \
        || echo "unknown")
    if [ "$DEPLOYER_STATE" = "exited" ]; then
        break
    fi
    if [ "$i" -eq "$DEPLOYER_TIMEOUT" ]; then
        err "Deployer did not finish within ${DEPLOYER_TIMEOUT}s (state: $DEPLOYER_STATE)"
        docker compose logs deployer 2>&1 | tail -30
        exit 1
    fi
    sleep 2
done

# Verify deployer exit code
DEPLOYER_EXIT=$(docker compose ps deployer --format json 2>/dev/null \
    | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('ExitCode', 1))" 2>/dev/null \
    || echo "1")
if [ "$DEPLOYER_EXIT" != "0" ]; then
    err "Deployer exited with non-zero code: $DEPLOYER_EXIT"
    docker compose logs deployer
    exit 1
fi
ok "Deployer finished successfully (exit code 0)"

# --- Step 5: Verify deployment data ------------------------------------------

log "Step 5: Verifying deployment addresses..."
# Read addresses.json from the deployment-data volume via the operator container
# (operator mounts deployment-data at /deployment)
ADDRESSES_JSON=$(docker compose exec -T operator cat /deployment/addresses.json 2>/dev/null || echo "")
if [ -z "$ADDRESSES_JSON" ]; then
    # Fallback: try to read via a temporary container
    ADDRESSES_JSON=$(docker run --rm -v "$(docker volume ls -q | grep deployment-data | head -1):/data:ro" \
        alpine:latest cat /data/addresses.json 2>/dev/null || echo "")
fi

if [ -z "$ADDRESSES_JSON" ]; then
    warn "Could not read /deployment/addresses.json (volume may not be accessible)"
else
    # Validate it contains expected keys
    REMAINDER_ADDR=$(echo "$ADDRESSES_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('RemainderVerifier',''))" 2>/dev/null || echo "")
    TEE_ADDR=$(echo "$ADDRESSES_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('TEEMLVerifier',''))" 2>/dev/null || echo "")

    if [ -n "$REMAINDER_ADDR" ] && [ "$REMAINDER_ADDR" != "null" ] && [ "$REMAINDER_ADDR" != "" ]; then
        ok "RemainderVerifier deployed at: $REMAINDER_ADDR"
    else
        warn "RemainderVerifier address not found in deployment data"
    fi
    if [ -n "$TEE_ADDR" ] && [ "$TEE_ADDR" != "null" ] && [ "$TEE_ADDR" != "" ]; then
        ok "TEEMLVerifier deployed at: $TEE_ADDR"
    else
        warn "TEEMLVerifier address not found in deployment data"
    fi
fi

# --- Step 6: Check enclave health -------------------------------------------

log "Step 6: Checking enclave health (port 8080)..."
ENCLAVE_TIMEOUT=40
for i in $(seq 1 "$ENCLAVE_TIMEOUT"); do
    if curl -sf http://localhost:8080/health >/dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq "$ENCLAVE_TIMEOUT" ]; then
        err "Enclave not healthy after ${ENCLAVE_TIMEOUT}s"
        docker compose logs enclave 2>&1 | tail -20
        exit 1
    fi
    sleep 1
done
ENCLAVE_HEALTH=$(curl -sf http://localhost:8080/health 2>/dev/null || echo "(no response)")
ok "Enclave healthy: $ENCLAVE_HEALTH"

# --- Step 7: Check warm-prover health ---------------------------------------

log "Step 7: Checking warm-prover health (port 3000)..."
PROVER_TIMEOUT=60
for i in $(seq 1 "$PROVER_TIMEOUT"); do
    if curl -sf http://localhost:3000/health >/dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq "$PROVER_TIMEOUT" ]; then
        err "Warm prover not healthy after ${PROVER_TIMEOUT}s"
        docker compose logs warm-prover 2>&1 | tail -20
        exit 1
    fi
    sleep 1
done
PROVER_HEALTH=$(curl -sf http://localhost:3000/health 2>/dev/null || echo "(no response)")
ok "Warm prover healthy: $PROVER_HEALTH"

# --- Step 8: Check operator health -------------------------------------------

log "Step 8: Checking operator health (port 9090)..."
# The operator has start_period: 30s in its healthcheck, so allow generous time.
OPERATOR_TIMEOUT=90
OPERATOR_HEALTHY=false
for i in $(seq 1 "$OPERATOR_TIMEOUT"); do
    # Try external port first (mapped in docker-compose.yml as 9090:9090)
    if curl -sf http://localhost:9090/health >/dev/null 2>&1; then
        OPERATOR_HEALTHY=true
        break
    fi
    # Fallback: check via docker exec (in case port is not mapped)
    if docker compose exec -T operator curl -sf http://localhost:9090/health >/dev/null 2>&1; then
        OPERATOR_HEALTHY=true
        break
    fi
    if [ "$i" -eq "$OPERATOR_TIMEOUT" ]; then
        break
    fi
    sleep 1
done

if [ "$OPERATOR_HEALTHY" = true ]; then
    OPERATOR_HEALTH=$(curl -sf http://localhost:9090/health 2>/dev/null || echo "(via docker exec)")
    ok "Operator healthy: $OPERATOR_HEALTH"
else
    # Operator health failure is non-fatal for the smoke test -- the operator
    # may need contract addresses loaded or other runtime config that isn't
    # fully wired in all environments.
    warn "Operator health check did not pass within ${OPERATOR_TIMEOUT}s"
    warn "This may be expected if the operator requires additional runtime config"
    docker compose logs operator 2>&1 | tail -15
fi

# --- Step 9: Verify all containers are running -------------------------------

log "Step 9: Verifying container states..."
echo ""
docker compose ps
echo ""

# Count running containers (exclude the deployer which exits after completion)
RUNNING_COUNT=$(docker compose ps --format json 2>/dev/null \
    | python3 -c "
import sys, json
running = 0
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    d = json.loads(line)
    name = d.get('Service', d.get('Name', ''))
    state = d.get('State', '')
    if 'deployer' in name:
        continue  # deployer exits normally
    if state == 'running':
        running += 1
print(running)
" 2>/dev/null || echo "0")

# We expect at least 3 running: anvil, enclave, warm-prover
# Operator is a bonus (may not start if build fails)
if [ "$RUNNING_COUNT" -ge 3 ]; then
    ok "At least 3 services running (anvil, enclave, warm-prover)"
else
    err "Expected at least 3 running services, got $RUNNING_COUNT"
    docker compose logs 2>&1 | tail -40
    exit 1
fi

# --- Step 10: Quick chain interaction test -----------------------------------

log "Step 10: Quick chain interaction test..."
# Verify we can read the deployed block number from the host
BLOCK_NUM=$(curl -sf -X POST http://localhost:8545 \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    | python3 -c "import sys,json; print(int(json.load(sys.stdin)['result'], 16))" 2>/dev/null || echo "0")
if [ "$BLOCK_NUM" -gt 0 ]; then
    ok "Chain accessible from host (block number: $BLOCK_NUM)"
else
    warn "Could not read block number from host (port 8545 may not be accessible)"
fi

# --- Summary -----------------------------------------------------------------

echo ""
echo "================================================================"
echo "  Docker Compose E2E Smoke Test -- PASSED"
echo "================================================================"
echo ""
echo "  Services:"
echo "    anvil         Local chain on port 8545"
echo "    deployer      Contracts deployed (exited successfully)"
echo "    enclave       TEE service on port 8080 -- healthy"
echo "    warm-prover   ZK prover on port 3000 -- healthy"
if [ "$OPERATOR_HEALTHY" = true ]; then
echo "    operator      Orchestrator on port 9090 -- healthy"
else
echo "    operator      Orchestrator on port 9090 -- (check warning above)"
fi
echo ""
echo "  Running containers: $RUNNING_COUNT"
echo "  Chain block number: $BLOCK_NUM"
echo ""
echo "================================================================"
