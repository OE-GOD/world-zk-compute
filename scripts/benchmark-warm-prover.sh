#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Warm Prover Benchmark: Cold (one-shot) vs Warm (HTTP server) Performance
# =============================================================================
#
# Builds the xgboost-remainder prover in release mode and compares:
#   - Cold: repeated one-shot invocations (each builds circuit from scratch)
#   - Warm: HTTP server mode (circuit built once, proof generation per request)
#
# Usage:
#   ./scripts/benchmark-warm-prover.sh [--iterations N] [--model PATH] [--port PORT]
#
# Defaults:
#   --iterations 5
#   --model examples/xgboost-remainder/sample_model.json
#   --port 3099

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BINARY="$PROJECT_ROOT/target/release/xgboost-remainder"
DEFAULT_MODEL="$PROJECT_ROOT/examples/xgboost-remainder/sample_model.json"
DEFAULT_INPUT="$PROJECT_ROOT/examples/xgboost-remainder/sample_input.json"
FEATURES='[0.6, 0.2, 0.8, 0.5, 0.3]'

# Configurable parameters
ITERATIONS=5
MODEL="$DEFAULT_MODEL"
PORT=3099
HOST="127.0.0.1"

while [[ $# -gt 0 ]]; do
    case $1 in
        --iterations) ITERATIONS="$2"; shift 2 ;;
        --model)      MODEL="$2"; shift 2 ;;
        --port)       PORT="$2"; shift 2 ;;
        --help|-h)
            echo "Usage: $0 [--iterations N] [--model PATH] [--port PORT]"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Validate model file exists
if [ ! -f "$MODEL" ]; then
    echo "ERROR: Model file not found: $MODEL"
    exit 1
fi

# Validate input file exists (needed for cold runs)
if [ ! -f "$DEFAULT_INPUT" ]; then
    echo "ERROR: Input file not found: $DEFAULT_INPUT"
    exit 1
fi

echo "============================================="
echo "  Warm Prover Benchmark"
echo "============================================="
echo ""
echo "  Model:      $MODEL"
echo "  Features:   $FEATURES"
echo "  Iterations: $ITERATIONS"
echo "  Port:       $PORT"
echo ""

# =============================================================================
# Phase 1: Build
# =============================================================================

echo "[Phase 1] Building xgboost-remainder in release mode..."
cd "$PROJECT_ROOT"
cargo build --release -p xgboost-remainder 2>&1

if [ ! -x "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    exit 1
fi
echo "  Binary: $BINARY"
echo ""

# =============================================================================
# Phase 2: Cold (one-shot) benchmark
# =============================================================================

echo "[Phase 2] Cold benchmark ($ITERATIONS one-shot runs)..."
echo "  Each run builds circuit + generates proof from scratch."
echo ""

COLD_TIMES=()

for i in $(seq 1 "$ITERATIONS"); do
    PROVE_OUTPUT=$(mktemp)
    START_NS=$(python3 -c 'import time; print(int(time.time() * 1e9))')
    "$BINARY" --model "$MODEL" --input "$DEFAULT_INPUT" --output "$PROVE_OUTPUT" 2>&1 || true
    END_NS=$(python3 -c 'import time; print(int(time.time() * 1e9))')

    ELAPSED_MS=$(python3 -c "print(($END_NS - $START_NS) / 1e6)")
    COLD_TIMES+=("$ELAPSED_MS")
    printf "  Run %d/%d: %.1f ms\n" "$i" "$ITERATIONS" "$ELAPSED_MS"
    rm -f "$PROVE_OUTPUT"
done

echo ""

# =============================================================================
# Phase 3: Warm (HTTP server) benchmark
# =============================================================================

echo "[Phase 3] Warm benchmark ($ITERATIONS HTTP /prove requests)..."
echo "  Starting server, then sending requests to pre-warmed prover."
echo ""

SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        echo ""
        echo "Cleaning up: stopping server (PID $SERVER_PID)..."
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Start the warm prover server in the background
SERVER_LOG=$(mktemp)
"$BINARY" --model "$MODEL" --serve --host "$HOST" --port "$PORT" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

echo "  Server starting (PID $SERVER_PID)..."

# Wait for /health to respond (up to 120 seconds for circuit compilation)
HEALTH_URL="http://${HOST}:${PORT}/health"
echo "  Waiting for server at $HEALTH_URL ..."

for i in $(seq 1 120); do
    if curl -sf "$HEALTH_URL" >/dev/null 2>&1; then
        HEALTH_RESPONSE=$(curl -sf "$HEALTH_URL")
        echo "  Server ready! Health: $HEALTH_RESPONSE"
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "  ERROR: Server process died. Log:"
        cat "$SERVER_LOG"
        exit 1
    fi
    if [ "$i" -eq 120 ]; then
        echo "  ERROR: Server did not become ready within 120s. Log:"
        cat "$SERVER_LOG"
        exit 1
    fi
    sleep 1
done

echo ""

PROVE_URL="http://${HOST}:${PORT}/prove"
WARM_TIMES=()

# Create a temporary file for curl output
CURL_BODY=$(mktemp)

for i in $(seq 1 "$ITERATIONS"); do
    # Use curl -w to get precise timing; body goes to CURL_BODY, timing to stdout
    TIME_TOTAL_S=$(curl -s -X POST "$PROVE_URL" \
        -H "Content-Type: application/json" \
        -d "{\"features\": $FEATURES}" \
        -w "%{time_total}" \
        -o "$CURL_BODY" 2>/dev/null)
    TIME_TOTAL_MS=$(python3 -c "print(float('$TIME_TOTAL_S') * 1000)")

    # Also extract prove_time_ms from the JSON response for pure proving time
    PROVE_TIME_MS=""
    if [ -f "$CURL_BODY" ] && [ -s "$CURL_BODY" ]; then
        PROVE_TIME_MS=$(python3 -c "
import json, sys
try:
    data = json.load(open('$CURL_BODY'))
    print(data.get('prove_time_ms', ''))
except:
    print('')
")
    fi

    WARM_TIMES+=("$TIME_TOTAL_MS")

    if [ -n "$PROVE_TIME_MS" ]; then
        printf "  Run %d/%d: %.1f ms total (%.0f ms proving)\n" "$i" "$ITERATIONS" "$TIME_TOTAL_MS" "$PROVE_TIME_MS"
    else
        printf "  Run %d/%d: %.1f ms total\n" "$i" "$ITERATIONS" "$TIME_TOTAL_MS"
    fi
done

rm -f "$CURL_BODY"

echo ""

# =============================================================================
# Phase 4: Stop server
# =============================================================================

echo "[Phase 4] Stopping server..."
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""
rm -f "$SERVER_LOG"
echo "  Server stopped."
echo ""

# =============================================================================
# Phase 5: Compute statistics and print summary
# =============================================================================

echo "[Phase 5] Results"
echo ""

# Build comma-separated lists for python
COLD_CSV=$(IFS=,; echo "${COLD_TIMES[*]}")
WARM_CSV=$(IFS=,; echo "${WARM_TIMES[*]}")

# Use python3 for statistics (available on macOS and most Linux)
SUMMARY=$(python3 -c "
import sys

cold = [$COLD_CSV]
warm = [$WARM_CSV]

def stats(times):
    avg = sum(times) / len(times)
    mn = min(times)
    mx = max(times)
    return avg, mn, mx

c_avg, c_min, c_max = stats(cold)
w_avg, w_min, w_max = stats(warm)

speedup = c_avg / w_avg if w_avg > 0 else float('inf')

print('============================================================')
print('  Cold vs Warm Prover Benchmark Summary')
print('============================================================')
print()
print(f'  Iterations: {len(cold)}')
print()
print('  -----------------------------------------------------------')
print(f'  {\"Metric\":<20} {\"Cold (one-shot)\":>18} {\"Warm (server)\":>18}')
print('  -----------------------------------------------------------')
print(f'  {\"Average\":.<20} {c_avg:>15.1f} ms {w_avg:>15.1f} ms')
print(f'  {\"Min\":.<20} {c_min:>15.1f} ms {w_min:>15.1f} ms')
print(f'  {\"Max\":.<20} {c_max:>15.1f} ms {w_max:>15.1f} ms')
print('  -----------------------------------------------------------')
print(f'  Speedup (avg):     {speedup:.1f}x')
print()
print('  Cold = full process: load model + build circuit + prove')
print('  Warm = HTTP request: prove only (circuit pre-built)')
print()
print('============================================================')
")

echo "$SUMMARY"
