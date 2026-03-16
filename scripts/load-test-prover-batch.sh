#!/usr/bin/env bash
# Load test the warm prover /prove/batch endpoint.
# Usage: ./scripts/load-test-prover-batch.sh [OPTIONS]
#
# Options:
#   --url URL              Server URL (default: http://127.0.0.1:3000)
#   --concurrency N        Concurrent workers (default: 3)
#   --requests N           Total batch requests (default: 10)
#   --batch-size N         Items per batch request (default: 4)
#   --features-per-item N  Features per item (default: auto-detect from /health)
#   --api-key KEY          API key for Authorization header
#   --health-only          Only test /health endpoint
#
# Requires: curl, python3 (standard on macOS/Linux)
# Optional: hey (https://github.com/rakyll/hey) for better stats

set -euo pipefail

URL="http://127.0.0.1:3000"
CONCURRENCY=3
REQUESTS=10
BATCH_SIZE=4
FEATURES_PER_ITEM=""
API_KEY=""
HEALTH_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --url) URL="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --requests) REQUESTS="$2"; shift 2 ;;
        --batch-size) BATCH_SIZE="$2"; shift 2 ;;
        --features-per-item) FEATURES_PER_ITEM="$2"; shift 2 ;;
        --api-key) API_KEY="$2"; shift 2 ;;
        --health-only) HEALTH_ONLY=true; shift ;;
        --help|-h)
            head -15 "$0" | tail -14
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "========================================"
echo "  Warm Prover Batch Load Test"
echo "========================================"
echo "URL:         $URL"
echo "Concurrency: $CONCURRENCY"
echo "Requests:    $REQUESTS"
echo "Batch size:  $BATCH_SIZE"
echo ""

# 1. Health check
echo "--- Health Check ---"
HEALTH_RESP=$(curl -s -w "\n%{http_code}\n%{time_total}" "${URL}/health" 2>&1)
HEALTH_CODE=$(echo "$HEALTH_RESP" | tail -2 | head -1)
HEALTH_TIME=$(echo "$HEALTH_RESP" | tail -1)
HEALTH_BODY=$(echo "$HEALTH_RESP" | head -1)

if [ "$HEALTH_CODE" != "200" ]; then
    echo "FAIL: /health returned HTTP $HEALTH_CODE"
    echo "Is the server running at $URL?"
    exit 1
fi

echo "Status: OK (${HEALTH_TIME}s)"
echo "Response: $HEALTH_BODY"

# Auto-detect features per item if not provided
if [ -z "$FEATURES_PER_ITEM" ]; then
    FEATURES_PER_ITEM=$(echo "$HEALTH_BODY" | python3 -c "import sys,json; print(json.load(sys.stdin)['num_features'])" 2>/dev/null || echo "5")
    echo "Auto-detected features per item: $FEATURES_PER_ITEM"
fi

if [ "$HEALTH_ONLY" = true ]; then
    echo "Health-only mode, done."
    exit 0
fi

echo ""

# Build batch request body: {"requests": [{"features": [0.5, ...]}, ...]}
BODY=$(python3 -c "
import json
item = {'features': [0.5] * ${FEATURES_PER_ITEM}}
batch = {'requests': [item] * ${BATCH_SIZE}}
print(json.dumps(batch))
" 2>/dev/null)

if [ -z "$BODY" ]; then
    echo "FAIL: Could not generate batch request body (python3 required)"
    exit 1
fi

# 2. Check if hey is available
if command -v hey &>/dev/null; then
    echo "--- Batch Load Test (using hey) ---"

    hey -n "$REQUESTS" -c "$CONCURRENCY" \
        -m POST \
        -H "Content-Type: application/json" \
        ${API_KEY:+-H "Authorization: Bearer ${API_KEY}"} \
        -d "$BODY" \
        "${URL}/prove/batch"

    echo ""
    echo "--- Derived Throughput ---"
    echo "Batch size:          $BATCH_SIZE items/request"
    echo "Effective vectors/s: multiply Requests/sec above by $BATCH_SIZE"
else
    echo "--- Batch Load Test (using curl) ---"
    echo "'hey' not found, using parallel curl (install hey for better stats)"
    echo ""

    TMPDIR=$(mktemp -d)

    # Send requests in batches
    SENT=0
    SUCCESS=0
    ERRORS=0
    RATE_LIMITED=0
    TIMES_FILE="$TMPDIR/times.txt"
    touch "$TIMES_FILE"

    START_ALL=$(date +%s%N 2>/dev/null || python3 -c "import time; print(int(time.time()*1e9))")

    for ((batch=0; batch < REQUESTS; batch+=CONCURRENCY)); do
        CHUNK_SIZE=$((REQUESTS - batch))
        if [ "$CHUNK_SIZE" -gt "$CONCURRENCY" ]; then
            CHUNK_SIZE=$CONCURRENCY
        fi

        PIDS=()
        for ((j=0; j<CHUNK_SIZE; j++)); do
            (
                RESP=$(curl -s -o /dev/null -w "%{http_code} %{time_total}" \
                    -X POST \
                    -H "Content-Type: application/json" \
                    ${API_KEY:+-H "Authorization: Bearer ${API_KEY}"} \
                    -d "$BODY" \
                    "${URL}/prove/batch" 2>&1)
                echo "$RESP" >> "$TMPDIR/results_${batch}_${j}.txt"
            ) &
            PIDS+=($!)
        done

        for pid in "${PIDS[@]}"; do
            wait "$pid" 2>/dev/null || true
        done
    done

    END_ALL=$(date +%s%N 2>/dev/null || python3 -c "import time; print(int(time.time()*1e9))")

    # Parse results
    for f in "$TMPDIR"/results_*.txt; do
        [ -f "$f" ] || continue
        while read -r code time_s; do
            SENT=$((SENT + 1))
            if [ "$code" = "200" ]; then
                SUCCESS=$((SUCCESS + 1))
            elif [ "$code" = "429" ]; then
                RATE_LIMITED=$((RATE_LIMITED + 1))
            else
                ERRORS=$((ERRORS + 1))
            fi
            echo "$time_s" >> "$TIMES_FILE"
        done < "$f"
    done

    # Calculate stats
    WALL_SECS=$(python3 -c "print(($END_ALL - $START_ALL) / 1e9)" 2>/dev/null || echo "?")
    RPS=$(python3 -c "t=$WALL_SECS; print(f'{$SENT/t:.1f}' if t > 0 else '?')" 2>/dev/null || echo "?")
    THROUGHPUT=$(python3 -c "t=$WALL_SECS; print(f'{$SENT*$BATCH_SIZE/t:.1f}' if t > 0 else '?')" 2>/dev/null || echo "?")

    # Latency percentiles
    LATENCY_STATS=$(sort -n "$TIMES_FILE" | python3 -c "
import sys
times = [float(l.strip()) for l in sys.stdin if l.strip()]
if not times:
    print('N/A N/A N/A N/A')
else:
    times.sort()
    n = len(times)
    p50 = times[int(n*0.5)]
    p95 = times[int(min(n*0.95, n-1))]
    p99 = times[int(min(n*0.99, n-1))]
    avg = sum(times)/n
    print(f'{avg:.3f} {p50:.3f} {p95:.3f} {p99:.3f}')
" 2>/dev/null || echo "? ? ? ?")

    read -r AVG P50 P95 P99 <<< "$LATENCY_STATS"

    echo ""
    echo "┌────────────────────────┬────────────┐"
    echo "│ Metric                 │ Value      │"
    echo "├────────────────────────┼────────────┤"
    printf "│ %-22s │ %10s │\n" "Total requests" "$SENT"
    printf "│ %-22s │ %10s │\n" "Successful (200)" "$SUCCESS"
    printf "│ %-22s │ %10s │\n" "Rate limited (429)" "$RATE_LIMITED"
    printf "│ %-22s │ %10s │\n" "Errors" "$ERRORS"
    printf "│ %-22s │ %10s │\n" "Wall time (s)" "$WALL_SECS"
    printf "│ %-22s │ %10s │\n" "Batch requests/sec" "$RPS"
    printf "│ %-22s │ %8ss │\n" "Avg batch latency" "$AVG"
    printf "│ %-22s │ %8ss │\n" "P50 batch latency" "$P50"
    printf "│ %-22s │ %8ss │\n" "P95 batch latency" "$P95"
    printf "│ %-22s │ %8ss │\n" "P99 batch latency" "$P99"
    echo "├────────────────────────┼────────────┤"
    printf "│ %-22s │ %10s │\n" "Batch size" "$BATCH_SIZE"
    printf "│ %-22s │ %8s/s │\n" "Vectors throughput" "$THROUGHPUT"
    echo "└────────────────────────┴────────────┘"
    echo ""
    echo "Throughput = batch_size ($BATCH_SIZE) x batches/sec ($RPS) = $THROUGHPUT vectors/sec"

    if [ "$RATE_LIMITED" -gt 0 ]; then
        echo ""
        echo "NOTE: $RATE_LIMITED requests were rate-limited (HTTP 429)."
        echo "Reduce --concurrency or increase server rate_limit."
    fi

    rm -rf "$TMPDIR"
fi
