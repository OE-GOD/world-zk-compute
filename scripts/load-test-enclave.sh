#!/usr/bin/env bash
# Load test the TEE enclave HTTP server.
# Usage: ./scripts/load-test-enclave.sh [OPTIONS]
#
# Options:
#   --url URL            Server URL (default: http://127.0.0.1:8080)
#   --concurrency N      Concurrent workers (default: 5)
#   --requests N         Total requests (default: 20)
#   --features JSON      Feature vector JSON (default: [0.5,0.5,0.5,0.5,0.5])
#   --duration SECS      Time-bounded mode in seconds (overrides --requests)
#   --health-only        Only test /health endpoint
#
# Requires: curl, bc (standard on macOS/Linux)
# Optional: hey (https://github.com/rakyll/hey) for better stats

set -euo pipefail

URL="http://127.0.0.1:8080"
CONCURRENCY=5
REQUESTS=20
FEATURES="[0.5,0.5,0.5,0.5,0.5]"
DURATION=""
HEALTH_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --url) URL="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --requests) REQUESTS="$2"; shift 2 ;;
        --features) FEATURES="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --health-only) HEALTH_ONLY=true; shift ;;
        --help|-h)
            head -14 "$0" | tail -13
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "========================================"
echo "  TEE Enclave Load Test"
echo "========================================"
echo "URL:         $URL"
echo "Concurrency: $CONCURRENCY"
if [ -n "$DURATION" ]; then
    echo "Duration:    ${DURATION}s"
else
    echo "Requests:    $REQUESTS"
fi
echo ""

# 1. Health check
echo "--- Health Check ---"
HEALTH_RESP=$(curl -s -w "\n%{http_code}\n%{time_total}" "${URL}/health" 2>&1)
HEALTH_CODE=$(echo "$HEALTH_RESP" | tail -2 | head -1)
HEALTH_TIME=$(echo "$HEALTH_RESP" | tail -1)
HEALTH_BODY=$(echo "$HEALTH_RESP" | head -1)

if [ "$HEALTH_CODE" != "200" ]; then
    echo "FAIL: /health returned HTTP $HEALTH_CODE"
    echo "Is the enclave running at $URL?"
    exit 1
fi

echo "Status: OK (${HEALTH_TIME}s)"
echo "Response: $HEALTH_BODY"

# Parse health fields: status, model_loaded, model_hash
MODEL_LOADED=$(echo "$HEALTH_BODY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('model_loaded', False))" 2>/dev/null || echo "unknown")
MODEL_HASH=$(echo "$HEALTH_BODY" | python3 -c "import sys,json; print(json.load(sys.stdin).get('model_hash', 'unknown'))" 2>/dev/null || echo "unknown")
echo "Model loaded: $MODEL_LOADED"
echo "Model hash:   $MODEL_HASH"

if [ "$MODEL_LOADED" = "False" ] || [ "$MODEL_LOADED" = "false" ]; then
    echo "FAIL: model_loaded is false. Load a model before running load tests."
    exit 1
fi

if [ "$HEALTH_ONLY" = true ]; then
    echo "Health-only mode, done."
    exit 0
fi

echo ""

# 2. Single warmup request — validate response contains attestation
echo "--- Warmup ---"
BODY="{\"features\": ${FEATURES}}"
WARMUP_RESP=$(curl -s -X POST -H "Content-Type: application/json" -d "$BODY" "${URL}/infer" 2>&1)

HAS_ATTESTATION=$(echo "$WARMUP_RESP" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    att = d.get('attestation', '')
    if isinstance(att, str) and len(att) == 132 and att.startswith('0x'):
        print('yes')
    else:
        print('no')
except:
    print('no')
" 2>/dev/null || echo "unknown")

if [ "$HAS_ATTESTATION" = "yes" ]; then
    echo "Warmup OK: response contains valid attestation (132-char hex)"
else
    echo "WARNING: response missing valid attestation field (expected 132-char hex string)"
    echo "Response: $WARMUP_RESP"
fi

echo ""

# 3. Check if hey is available
if command -v hey &>/dev/null; then
    echo "--- Load Test (using hey) ---"

    if [ -n "$DURATION" ]; then
        hey -z "${DURATION}s" -c "$CONCURRENCY" \
            -m POST \
            -H "Content-Type: application/json" \
            -d "$BODY" \
            "${URL}/infer"
    else
        hey -n "$REQUESTS" -c "$CONCURRENCY" \
            -m POST \
            -H "Content-Type: application/json" \
            -d "$BODY" \
            "${URL}/infer"
    fi
else
    echo "--- Load Test (using curl) ---"
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

    if [ -n "$DURATION" ]; then
        # Time-bounded mode
        END_TIME=$(($(date +%s) + DURATION))
        BATCH_NUM=0
        while [ "$(date +%s)" -lt "$END_TIME" ]; do
            PIDS=()
            for ((j=0; j<CONCURRENCY; j++)); do
                (
                    RESP=$(curl -s -o /dev/null -w "%{http_code} %{time_total}" \
                        -X POST \
                        -H "Content-Type: application/json" \
                        -d "$BODY" \
                        "${URL}/infer" 2>&1)
                    echo "$RESP" >> "$TMPDIR/results_${BATCH_NUM}_${j}.txt"
                ) &
                PIDS+=($!)
            done

            for pid in "${PIDS[@]}"; do
                wait "$pid" 2>/dev/null || true
            done
            BATCH_NUM=$((BATCH_NUM + 1))
        done
    else
        # Request-count mode
        for ((batch=0; batch < REQUESTS; batch+=CONCURRENCY)); do
            BATCH_SIZE=$((REQUESTS - batch))
            if [ "$BATCH_SIZE" -gt "$CONCURRENCY" ]; then
                BATCH_SIZE=$CONCURRENCY
            fi

            PIDS=()
            for ((j=0; j<BATCH_SIZE; j++)); do
                (
                    RESP=$(curl -s -o /dev/null -w "%{http_code} %{time_total}" \
                        -X POST \
                        -H "Content-Type: application/json" \
                        -d "$BODY" \
                        "${URL}/infer" 2>&1)
                    echo "$RESP" >> "$TMPDIR/results_${batch}_${j}.txt"
                ) &
                PIDS+=($!)
            done

            for pid in "${PIDS[@]}"; do
                wait "$pid" 2>/dev/null || true
            done
        done
    fi

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
    printf "│ %-22s │ %10s │\n" "Requests/sec" "$RPS"
    printf "│ %-22s │ %8ss │\n" "Avg latency" "$AVG"
    printf "│ %-22s │ %8ss │\n" "P50 latency" "$P50"
    printf "│ %-22s │ %8ss │\n" "P95 latency" "$P95"
    printf "│ %-22s │ %8ss │\n" "P99 latency" "$P99"
    echo "└────────────────────────┴────────────┘"

    if [ "$RATE_LIMITED" -gt 0 ]; then
        echo ""
        echo "NOTE: $RATE_LIMITED requests were rate-limited (HTTP 429)."
        echo "Reduce --concurrency or increase MAX_REQUESTS_PER_MINUTE env var on the enclave."
    fi

    rm -rf "$TMPDIR"
fi
