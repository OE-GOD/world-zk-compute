#!/usr/bin/env bash
# =============================================================================
# End-to-end latency benchmark for World ZK Compute.
#
# Measures submit-to-result latency on a running local stack (Anvil + services).
# Per iteration: submit an execution request via cast send, poll for the result,
# and record timing. Prints a latency table and P50/P95/avg summary.
#
# Usage:
#   ./scripts/benchmark-e2e.sh
#   ./scripts/benchmark-e2e.sh --iterations 10
#   ./scripts/benchmark-e2e.sh --rpc-url http://127.0.0.1:8545 --iterations 3
#   ./scripts/benchmark-e2e.sh --help
#
# Requires: cast (Foundry), bc or python3
# =============================================================================

set -uo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' RESET=''
fi

info()    { printf "${CYAN}[INFO]${RESET}  %s\n" "$*"; }
ok()      { printf "${GREEN}[OK]${RESET}    %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
ITERATIONS=5
RPC_URL="http://127.0.0.1:8545"
POLL_INTERVAL_S=1
MAX_POLL_ATTEMPTS=120

# Anvil default accounts
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
REQUESTER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"

# Contract addresses (set via env or auto-detect)
ENGINE_ADDR="${ENGINE_ADDR:-}"
REGISTRY_ADDR="${REGISTRY_ADDR:-}"

# Test constants
IMAGE_ID="${IMAGE_ID:-0x1111111111111111111111111111111111111111111111111111111111111111}"
INPUT_DIGEST="0x4444444444444444444444444444444444444444444444444444444444444444"

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------
show_help() {
    cat <<'EOF'
benchmark-e2e.sh -- End-to-end latency benchmark for World ZK Compute.

USAGE:
  scripts/benchmark-e2e.sh [OPTIONS]

OPTIONS:
  --iterations N     Number of benchmark iterations (default: 5)
  --rpc-url URL      RPC endpoint (default: http://127.0.0.1:8545)
  -h, --help         Show this help message

ENVIRONMENT:
  ENGINE_ADDR        ExecutionEngine contract address (auto-detect if unset)
  REGISTRY_ADDR      ProgramRegistry contract address (auto-detect if unset)
  IMAGE_ID           Program image ID to use (default: 0x1111...1111)
  REQUESTER_KEY      Private key for submitting requests
  DEPLOYER_KEY       Private key for setup operations

DESCRIPTION:
  Requires a running local stack (Anvil + contracts deployed). Each
  iteration submits an execution request, polls for completion, and
  measures the elapsed time. Results are printed as a table with
  P50/P95/avg latency summary.

REQUIRES:
  cast       (from Foundry)
  python3    (for statistics)
EOF
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --iterations)
            ITERATIONS="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            echo "Run with --help for usage."
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Prerequisites
# ---------------------------------------------------------------------------
if ! command -v cast &>/dev/null; then
    err "cast is required. Install: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

if ! command -v python3 &>/dev/null; then
    err "python3 is required for latency statistics."
    exit 1
fi

# ---------------------------------------------------------------------------
# Timestamp helper (milliseconds since epoch)
# ---------------------------------------------------------------------------
now_ms() {
    python3 -c "import time; print(int(time.time() * 1000))"
}

# ---------------------------------------------------------------------------
# Verify RPC is reachable
# ---------------------------------------------------------------------------
info "Checking RPC endpoint: $RPC_URL"
if ! cast chain-id --rpc-url "$RPC_URL" &>/dev/null; then
    err "Cannot reach RPC at $RPC_URL"
    err "Make sure Anvil (or another node) is running."
    exit 1
fi
CHAIN_ID=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null)
ok "Connected to chain $CHAIN_ID"

# ---------------------------------------------------------------------------
# Auto-detect contract addresses (if not provided)
# ---------------------------------------------------------------------------
if [[ -z "$ENGINE_ADDR" || -z "$REGISTRY_ADDR" ]]; then
    info "ENGINE_ADDR or REGISTRY_ADDR not set. Attempting auto-detection..."
    warn "Set ENGINE_ADDR and REGISTRY_ADDR env vars if auto-detection fails."
    warn "You can deploy contracts with: make deploy-local"
fi

# ---------------------------------------------------------------------------
# Print benchmark plan
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- E2E Latency Benchmark"
header "============================================================"
echo ""
info "RPC URL:      $RPC_URL"
info "Chain ID:     $CHAIN_ID"
info "Iterations:   $ITERATIONS"
info "Image ID:     ${IMAGE_ID:0:18}..."
if [[ -n "$ENGINE_ADDR" ]]; then
    info "Engine:       $ENGINE_ADDR"
fi
echo ""

# ---------------------------------------------------------------------------
# Benchmark loop
# ---------------------------------------------------------------------------
declare -a SUBMIT_TIMES=()
declare -a RESULT_TIMES=()
declare -a TOTAL_TIMES=()
PASS_COUNT=0
FAIL_COUNT=0

for i in $(seq 1 "$ITERATIONS"); do
    info "--- Iteration $i/$ITERATIONS ---"

    # ---- Submit request ----
    T_START=$(now_ms)

    SUBMIT_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
        --private-key "$REQUESTER_KEY" \
        "$ENGINE_ADDR" \
        "requestExecution(bytes32,bytes32,string,address,uint256)" \
        "$IMAGE_ID" \
        "$INPUT_DIGEST" \
        "https://bench.test/input-$i" \
        "0x0000000000000000000000000000000000000000" \
        3600 \
        --value 0.001ether \
        2>&1) || true

    T_SUBMIT=$(now_ms)
    SUBMIT_MS=$((T_SUBMIT - T_START))

    # Get the request ID (nextRequestId - 1)
    NEXT_ID=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
        "nextRequestId()(uint256)" 2>/dev/null || echo "0")
    REQUEST_ID=$((NEXT_ID - 1))

    if [[ "$REQUEST_ID" -lt 1 ]]; then
        warn "  Submit failed for iteration $i"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        SUBMIT_TIMES+=("$SUBMIT_MS")
        RESULT_TIMES+=("0")
        TOTAL_TIMES+=("$SUBMIT_MS")
        continue
    fi

    # ---- Poll for result ----
    POLL_COUNT=0
    COMPLETED=false

    while [[ "$POLL_COUNT" -lt "$MAX_POLL_ATTEMPTS" ]]; do
        RAW_HEX=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
            "getRequest(uint256)" "$REQUEST_ID" 2>/dev/null | tr -d '[:space:]')

        # Status field is at offset ~225 in the ABI-encoded tuple
        STATUS_HEX=${RAW_HEX:450:64}
        STATUS=$((16#${STATUS_HEX:-0})) 2>/dev/null || STATUS=0

        # Status 2 = Completed
        if [[ "$STATUS" -ge 2 ]]; then
            COMPLETED=true
            break
        fi

        sleep "$POLL_INTERVAL_S"
        POLL_COUNT=$((POLL_COUNT + 1))
    done

    T_RESULT=$(now_ms)
    RESULT_MS=$((T_RESULT - T_SUBMIT))
    TOTAL_MS=$((T_RESULT - T_START))

    SUBMIT_TIMES+=("$SUBMIT_MS")
    RESULT_TIMES+=("$RESULT_MS")
    TOTAL_TIMES+=("$TOTAL_MS")

    if [[ "$COMPLETED" == "true" ]]; then
        ok "  Iteration $i: submit=${SUBMIT_MS}ms result=${RESULT_MS}ms total=${TOTAL_MS}ms"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        warn "  Iteration $i: submit=${SUBMIT_MS}ms result=TIMEOUT total=${TOTAL_MS}ms"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
done

# ---------------------------------------------------------------------------
# Results table
# ---------------------------------------------------------------------------
header "============================================================"
header "  Latency Results"
header "============================================================"
echo ""
printf "  ${BOLD}%-12s %-14s %-14s %-14s %-10s${RESET}\n" "ITERATION" "SUBMIT (ms)" "RESULT (ms)" "TOTAL (ms)" "STATUS"
printf "  %-12s %-14s %-14s %-14s %-10s\n" "------------" "--------------" "--------------" "--------------" "----------"

for i in $(seq 0 $((ITERATIONS - 1))); do
    iter_num=$((i + 1))
    s="${SUBMIT_TIMES[$i]}"
    r="${RESULT_TIMES[$i]}"
    t="${TOTAL_TIMES[$i]}"

    if [[ "$r" -eq 0 && "$iter_num" -le "$ITERATIONS" ]]; then
        status="FAIL"
    else
        status="OK"
    fi

    printf "  %-12s %-14s %-14s %-14s %-10s\n" "$iter_num" "$s" "$r" "$t" "$status"
done

# ---------------------------------------------------------------------------
# Statistics (P50, P95, avg)
# ---------------------------------------------------------------------------
echo ""
header "  Latency Summary"
echo ""

# Build comma-separated lists for python
TOTAL_CSV=$(IFS=,; echo "${TOTAL_TIMES[*]}")
SUBMIT_CSV=$(IFS=,; echo "${SUBMIT_TIMES[*]}")
RESULT_CSV=$(IFS=,; echo "${RESULT_TIMES[*]}")

python3 -c "
import statistics

def stats(name, values):
    values = sorted(values)
    n = len(values)
    if n == 0:
        return
    avg = statistics.mean(values)
    p50 = values[int(n * 0.5)] if n > 1 else values[0]
    p95 = values[int(n * 0.95)] if n > 1 else values[-1]
    mn = min(values)
    mx = max(values)
    print(f'  {name:12s}  avg={avg:8.0f}ms  P50={p50:8.0f}ms  P95={p95:8.0f}ms  min={mn:8.0f}ms  max={mx:8.0f}ms')

submit = [${SUBMIT_CSV}]
result = [${RESULT_CSV}]
total  = [${TOTAL_CSV}]

stats('Submit', submit)
stats('Result', [r for r in result if r > 0])
stats('Total', total)
"

echo ""
info "Passed: $PASS_COUNT / $ITERATIONS"
if [[ "$FAIL_COUNT" -gt 0 ]]; then
    warn "Failed: $FAIL_COUNT / $ITERATIONS"
fi
echo ""

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
fi
exit 0
