#!/usr/bin/env bash
set -euo pipefail

# ===============================================================================
# Docker Compose Smoke Test
#
# Quick validation that the Docker Compose stack starts, deploys contracts,
# and all services become healthy within a bounded time window.
#
# Services tested (from docker-compose.yml):
#   anvil         Local Ethereum node (port 8545)
#   deployer      One-shot contract deployment (exits after success)
#   warm-prover   ZK proof server (port 3000)
#   operator      Orchestrator service (port 9090)
#
# Usage:
#   ./scripts/docker-smoke-test.sh              Run full smoke test
#   ./scripts/docker-smoke-test.sh --no-cleanup  Leave containers running on exit
#
# Total timeout: 120 seconds
# ===============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- Constants ----------------------------------------------------------------

TOTAL_TIMEOUT=120
WARM_PROVER_PORT=3000
OPERATOR_PORT=9090
ANVIL_PORT=8545

# --- Color output -------------------------------------------------------------

if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    BOLD=''
    RESET=''
fi

# --- Parse flags --------------------------------------------------------------

NO_CLEANUP=false
for arg in "$@"; do
    case "$arg" in
        --no-cleanup)
            NO_CLEANUP=true
            ;;
        -h|--help)
            echo "Usage: $0 [--no-cleanup]"
            echo "  --no-cleanup   Leave containers running for debugging"
            exit 0
            ;;
        *)
            echo "Unknown flag: $arg" >&2
            exit 1
            ;;
    esac
done

# --- Logging helpers ----------------------------------------------------------

log()  { echo -e "${BOLD}==> $*${RESET}"; }
ok()   { echo -e "  ${GREEN}[OK]${RESET} $*"; }
fail() { echo -e "  ${RED}[FAIL]${RESET} $*" >&2; }
warn() { echo -e "  ${YELLOW}[WARN]${RESET} $*"; }

# --- Result tracking ----------------------------------------------------------

PASS_COUNT=0
FAIL_COUNT=0
RESULTS=()

record_pass() {
    PASS_COUNT=$((PASS_COUNT + 1))
    RESULTS+=("${GREEN}[OK]${RESET}   $1")
}

record_fail() {
    FAIL_COUNT=$((FAIL_COUNT + 1))
    RESULTS+=("${RED}[FAIL]${RESET} $1")
}

# --- Timer --------------------------------------------------------------------

START_TIME=$(date +%s)

elapsed() {
    local now
    now=$(date +%s)
    echo $((now - START_TIME))
}

check_timeout() {
    if [ "$(elapsed)" -ge "$TOTAL_TIMEOUT" ]; then
        fail "Total timeout of ${TOTAL_TIMEOUT}s exceeded"
        record_fail "Total timeout"
        print_summary
        exit 1
    fi
}

# --- Cleanup trap -------------------------------------------------------------

cleanup() {
    local exit_code=$?
    if [ "$NO_CLEANUP" = true ]; then
        log "Skipping cleanup (--no-cleanup). To tear down: docker compose down -v"
        return
    fi
    log "Cleaning up..."
    cd "$ROOT_DIR"
    docker compose down -v 2>/dev/null || true
    if [ "$exit_code" -ne 0 ] && [ "${SUMMARY_PRINTED:-}" != "true" ]; then
        echo ""
        echo -e "${RED}================================================================${RESET}"
        echo -e "${RED}  Docker Smoke Test -- FAILED (exit code $exit_code)${RESET}"
        echo -e "${RED}================================================================${RESET}"
    fi
}
trap cleanup EXIT

# --- Print summary ------------------------------------------------------------

print_summary() {
    SUMMARY_PRINTED=true
    local total_time
    total_time=$(elapsed)
    echo ""
    echo "================================================================"
    echo "  Docker Smoke Test Results"
    echo "================================================================"
    for result in "${RESULTS[@]}"; do
        echo -e "  $result"
    done
    echo "----------------------------------------------------------------"
    echo -e "  Passed: ${GREEN}${PASS_COUNT}${RESET}   Failed: ${RED}${FAIL_COUNT}${RESET}   Time: ${total_time}s"
    echo "================================================================"
    echo ""

    if [ "$FAIL_COUNT" -gt 0 ]; then
        echo -e "${RED}RESULT: FAIL${RESET}"
    else
        echo -e "${GREEN}RESULT: PASS${RESET}"
    fi
}

# --- Prerequisite checks -----------------------------------------------------

log "Checking prerequisites..."
if ! command -v docker &>/dev/null; then
    fail "docker is not installed or not in PATH"
    exit 1
fi
if ! docker compose version &>/dev/null; then
    fail "docker compose plugin is not available"
    exit 1
fi
if ! docker info &>/dev/null 2>&1; then
    fail "Docker daemon is not running"
    exit 1
fi
ok "Prerequisites satisfied"

cd "$ROOT_DIR"

# --- Wait helper --------------------------------------------------------------
# wait_for_url URL TIMEOUT_SECS LABEL
# Polls a URL until it returns HTTP 200 (via curl -sf), or times out.
wait_for_url() {
    local url="$1"
    local timeout_secs="$2"
    local label="$3"
    local i

    for i in $(seq 1 "$timeout_secs"); do
        check_timeout
        if curl -sf "$url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# ==============================================================================
# Step 1: Start Anvil + Deployer
# ==============================================================================

log "Step 1/7: Starting anvil and deployer..."
docker compose up -d anvil deployer
ok "anvil + deployer containers started"

# ==============================================================================
# Step 2: Wait for deployer to finish
# ==============================================================================

log "Step 2/7: Waiting for deployer to finish contract deployment..."

DEPLOYER_TIMEOUT=90
DEPLOYER_DONE=false

for i in $(seq 1 "$DEPLOYER_TIMEOUT"); do
    check_timeout

    # Check if deployer container has exited
    DEPLOYER_STATUS=$(docker compose ps deployer --format '{{.Status}}' 2>/dev/null || echo "unknown")

    # Check for "Exited (0)" in the status string
    if echo "$DEPLOYER_STATUS" | grep -qi "exited"; then
        # Verify exit code is 0
        DEPLOYER_EXIT=$(docker compose ps deployer --format '{{.Status}}' 2>/dev/null | grep -o '([0-9]*)' | tr -d '()' || echo "1")
        if [ "$DEPLOYER_EXIT" = "0" ]; then
            DEPLOYER_DONE=true
            break
        else
            fail "Deployer exited with non-zero code: $DEPLOYER_EXIT"
            docker compose logs deployer 2>&1 | tail -20
            record_fail "Deployer exit code"
            print_summary
            exit 1
        fi
    fi
    sleep 2
done

if [ "$DEPLOYER_DONE" = true ]; then
    ok "Deployer finished successfully"
    record_pass "Deployer completed (contracts deployed)"
else
    fail "Deployer did not finish within ${DEPLOYER_TIMEOUT}s"
    docker compose logs deployer 2>&1 | tail -20
    record_fail "Deployer timeout"
    print_summary
    exit 1
fi

# ==============================================================================
# Step 3: Start warm-prover and operator
# ==============================================================================

log "Step 3/7: Starting warm-prover and operator..."
docker compose up -d warm-prover operator
ok "warm-prover + operator containers started"

# ==============================================================================
# Step 4: Wait for health checks
# ==============================================================================

log "Step 4/7: Waiting for service health checks..."

# -- 4a: warm-prover health ---------------------------------------------------
PROVER_URL="http://localhost:${WARM_PROVER_PORT}/health"
if wait_for_url "$PROVER_URL" 60 "warm-prover"; then
    PROVER_HEALTH=$(curl -sf "$PROVER_URL" 2>/dev/null || echo "(no response)")
    ok "warm-prover healthy: $PROVER_HEALTH"
    record_pass "warm-prover health check"
else
    fail "warm-prover not healthy within timeout"
    docker compose logs warm-prover 2>&1 | tail -20
    record_fail "warm-prover health check"
fi

# -- 4b: operator health ------------------------------------------------------
OPERATOR_URL="http://localhost:${OPERATOR_PORT}/health"
# The operator has start_period: 60s in docker-compose.yml, give it generous time
if wait_for_url "$OPERATOR_URL" 80 "operator"; then
    OPERATOR_HEALTH=$(curl -sf "$OPERATOR_URL" 2>/dev/null || echo "(no response)")
    ok "operator healthy: $OPERATOR_HEALTH"
    record_pass "operator health check"
else
    warn "operator health check did not pass (may need additional runtime config)"
    docker compose logs operator 2>&1 | tail -15
    record_fail "operator health check"
fi

# ==============================================================================
# Step 5: Submit a test inference request to warm-prover
# ==============================================================================

log "Step 5/7: Submitting test inference to warm-prover..."

# The sample model has num_features=5 (see sample_model.json / Dockerfile)
TEST_FEATURES='{"features": [1.0, 2.0, 3.0, 4.0, 5.0]}'

PROVE_RESPONSE=$(curl -sf -X POST \
    "http://localhost:${WARM_PROVER_PORT}/prove" \
    -H "Content-Type: application/json" \
    -d "$TEST_FEATURES" \
    --max-time 30 2>/dev/null || echo "")

if [ -n "$PROVE_RESPONSE" ]; then
    # Validate response has expected fields
    HAS_PREDICTED_CLASS=false
    HAS_PROOF_HEX=false

    if echo "$PROVE_RESPONSE" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'predicted_class' in d" 2>/dev/null; then
        HAS_PREDICTED_CLASS=true
    fi
    if echo "$PROVE_RESPONSE" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'proof_hex' in d and len(d['proof_hex']) > 0" 2>/dev/null; then
        HAS_PROOF_HEX=true
    fi

    if [ "$HAS_PREDICTED_CLASS" = true ] && [ "$HAS_PROOF_HEX" = true ]; then
        PREDICTED=$(echo "$PROVE_RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['predicted_class'])" 2>/dev/null || echo "?")
        PROOF_SIZE=$(echo "$PROVE_RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('proof_size_bytes', '?'))" 2>/dev/null || echo "?")
        PROVE_TIME=$(echo "$PROVE_RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('prove_time_ms', '?'))" 2>/dev/null || echo "?")
        ok "Inference succeeded: predicted_class=$PREDICTED, proof_size=${PROOF_SIZE}B, prove_time=${PROVE_TIME}ms"
        record_pass "Test inference request"
    else
        fail "Inference response missing expected fields"
        echo "  Response: $(echo "$PROVE_RESPONSE" | head -c 200)"
        record_fail "Test inference request"
    fi
else
    fail "No response from warm-prover /prove endpoint"
    docker compose logs warm-prover 2>&1 | tail -10
    record_fail "Test inference request"
fi

# ==============================================================================
# Step 6: Verify operator is watching events
# ==============================================================================

log "Step 6/7: Checking operator is watching for events..."

# Give operator a moment to start its watch loop
sleep 3

OPERATOR_LOGS=$(docker compose logs operator 2>&1 || echo "")

# Look for signs the operator started its event polling loop
WATCHING=false
if echo "$OPERATOR_LOGS" | grep -qi "watching\|poll\|watcher\|started\|listening\|block"; then
    WATCHING=true
fi

if [ "$WATCHING" = true ]; then
    ok "Operator is watching for events (log patterns found)"
    record_pass "Operator event watching"
else
    # The operator may have started but not yet logged recognizable patterns.
    # Check if the container is at least running.
    OPERATOR_STATE=$(docker compose ps operator --format '{{.State}}' 2>/dev/null || echo "unknown")
    if echo "$OPERATOR_STATE" | grep -qi "running"; then
        warn "Operator is running but no watch-loop log patterns detected yet"
        record_pass "Operator event watching (container running)"
    else
        fail "Operator is not running (state: $OPERATOR_STATE)"
        echo "  Last 10 lines of operator logs:"
        echo "$OPERATOR_LOGS" | tail -10
        record_fail "Operator event watching"
    fi
fi

# ==============================================================================
# Step 7: Cleanup (handled by trap)
# ==============================================================================

log "Step 7/7: Tearing down stack..."

# Print summary before cleanup runs
print_summary

# Exit with appropriate code
if [ "$FAIL_COUNT" -gt 0 ]; then
    exit 1
fi
exit 0
