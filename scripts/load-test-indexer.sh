#!/usr/bin/env bash
# Load test the indexer REST and WebSocket endpoints.
# Usage: ./scripts/load-test-indexer.sh [OPTIONS]
#
# Options:
#   --url URL              Indexer URL (default: http://127.0.0.1:8081)
#   --concurrency N        Concurrent workers (default: 5)
#   --requests N           Total requests (default: 20)
#   --mode MODE            Test mode: rest-health | ws-scale (default: rest-health)
#   --ws-connections N     WebSocket connections for ws-scale (default: 10)
#   --ws-duration N        WebSocket hold duration in seconds (default: 30)
#   --health-only          Only test /health endpoint
#
# Requires: curl, bc (standard on macOS/Linux)
# Optional: hey (https://github.com/rakyll/hey) for better stats
# ws-scale mode requires: python3 with websockets OR websocat

set -euo pipefail

URL="http://127.0.0.1:8081"
CONCURRENCY=5
REQUESTS=20
MODE="rest-health"
WS_CONNECTIONS=10
WS_DURATION=30
HEALTH_ONLY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --url) URL="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --requests) REQUESTS="$2"; shift 2 ;;
        --mode) MODE="$2"; shift 2 ;;
        --ws-connections) WS_CONNECTIONS="$2"; shift 2 ;;
        --ws-duration) WS_DURATION="$2"; shift 2 ;;
        --health-only) HEALTH_ONLY=true; shift ;;
        --help|-h)
            head -16 "$0" | tail -15
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "========================================"
echo "  Indexer Load Test"
echo "========================================"
echo "URL:         $URL"
echo "Concurrency: $CONCURRENCY"
echo "Requests:    $REQUESTS"
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

if [ "$HEALTH_ONLY" = true ]; then
    echo "Health-only mode, done."
    exit 0
fi

echo ""

# ============================================================
# Mode: rest-health
# ============================================================
if [ "$MODE" = "rest-health" ]; then

    ENDPOINTS=("/results" "/stats")

    if command -v hey &>/dev/null; then
        echo "--- Load Test (using hey) ---"

        for EP in "${ENDPOINTS[@]}"; do
            echo ""
            echo ">> GET $EP"
            hey -n "$REQUESTS" -c "$CONCURRENCY" \
                -m GET \
                "${URL}${EP}"
        done
    else
        echo "--- Load Test (using curl) ---"
        echo "'hey' not found, using parallel curl (install hey for better stats)"
        echo ""

        for EP in "${ENDPOINTS[@]}"; do
            echo ">> GET $EP"

            TMPDIR=$(mktemp -d)
            SENT=0
            SUCCESS=0
            ERRORS=0
            RATE_LIMITED=0
            TIMES_FILE="$TMPDIR/times.txt"
            touch "$TIMES_FILE"

            START_ALL=$(date +%s%N 2>/dev/null || python3 -c "import time; print(int(time.time()*1e9))")

            for ((batch=0; batch < REQUESTS; batch+=CONCURRENCY)); do
                BATCH_SIZE=$((REQUESTS - batch))
                if [ "$BATCH_SIZE" -gt "$CONCURRENCY" ]; then
                    BATCH_SIZE=$CONCURRENCY
                fi

                PIDS=()
                for ((j=0; j<BATCH_SIZE; j++)); do
                    (
                        RESP=$(curl -s -o /dev/null -w "%{http_code} %{time_total}" \
                            -X GET \
                            "${URL}${EP}" 2>&1)
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
            printf "│ %-22s │ %10s │\n" "Endpoint" "$EP"
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
                echo "Reduce --concurrency or increase server rate_limit."
            fi

            rm -rf "$TMPDIR"
            echo ""
        done
    fi

# ============================================================
# Mode: ws-scale
# ============================================================
elif [ "$MODE" = "ws-scale" ]; then

    echo "--- WebSocket Scale Test ---"
    echo "Connections: $WS_CONNECTIONS"
    echo "Duration:    ${WS_DURATION}s"
    echo ""

    # Derive ws:// URL from http:// URL
    WS_URL=$(echo "$URL" | sed 's|^http://|ws://|; s|^https://|wss://|')
    WS_ENDPOINT="${WS_URL}/ws/events"

    # Check for python3 + websockets module
    HAS_PY_WS=false
    if python3 -c "import websockets" 2>/dev/null; then
        HAS_PY_WS=true
    fi

    # Check for websocat
    HAS_WEBSOCAT=false
    if command -v websocat &>/dev/null; then
        HAS_WEBSOCAT=true
    fi

    if [ "$HAS_PY_WS" = false ] && [ "$HAS_WEBSOCAT" = false ]; then
        echo "ERROR: No WebSocket client available."
        echo ""
        echo "Install one of the following:"
        echo "  pip3 install websockets"
        echo "  brew install websocat     (macOS)"
        echo "  cargo install websocat    (any platform)"
        exit 1
    fi

    TMPDIR=$(mktemp -d)

    if [ "$HAS_PY_WS" = true ]; then
        echo "Using: python3 websockets"
        echo ""

        python3 - "$WS_ENDPOINT" "$WS_CONNECTIONS" "$WS_DURATION" "$TMPDIR" <<'PYEOF'
import asyncio
import json
import sys
import os
import time

async def run():
    ws_url = sys.argv[1]
    num_conns = int(sys.argv[2])
    duration = int(sys.argv[3])
    tmpdir = sys.argv[4]

    try:
        import websockets
    except ImportError:
        print("ERROR: websockets module not available")
        sys.exit(1)

    established = 0
    rejected = 0
    disconnected = 0

    async def connect_and_hold(conn_id):
        nonlocal established, rejected, disconnected
        try:
            async with websockets.connect(ws_url, open_timeout=10) as ws:
                established += 1
                # Send subscribe message
                subscribe_msg = json.dumps({"subscribe": ["ResultSubmitted", "ResultChallenged"]})
                await ws.send(subscribe_msg)
                # Hold connection for duration
                deadline = time.time() + duration
                while time.time() < deadline:
                    try:
                        await asyncio.wait_for(ws.recv(), timeout=1.0)
                    except asyncio.TimeoutError:
                        pass
                    except websockets.ConnectionClosed:
                        disconnected += 1
                        return
        except websockets.exceptions.InvalidStatusCode as e:
            if hasattr(e, 'status_code') and e.status_code == 503:
                rejected += 1
            else:
                rejected += 1
        except Exception:
            rejected += 1

    tasks = [connect_and_hold(i) for i in range(num_conns)]
    await asyncio.gather(*tasks)

    # Write results to file
    results_path = os.path.join(tmpdir, "ws_results.txt")
    with open(results_path, "w") as f:
        f.write(f"{established} {rejected} {disconnected}\n")

asyncio.run(run())
PYEOF

    elif [ "$HAS_WEBSOCAT" = true ]; then
        echo "Using: websocat"
        echo ""

        SUBSCRIBE_MSG='{"subscribe":["ResultSubmitted","ResultChallenged"]}'

        for ((i=0; i<WS_CONNECTIONS; i++)); do
            (
                RESULT=$(echo "$SUBSCRIBE_MSG" | timeout "$WS_DURATION" websocat "$WS_ENDPOINT" 2>&1; echo "EXIT:$?")
                EXIT_CODE=$(echo "$RESULT" | grep -o 'EXIT:[0-9]*' | cut -d: -f2)
                if echo "$RESULT" | grep -q "503"; then
                    echo "rejected" >> "$TMPDIR/ws_conn_${i}.txt"
                elif [ "$EXIT_CODE" = "0" ] || [ "$EXIT_CODE" = "124" ]; then
                    echo "established" >> "$TMPDIR/ws_conn_${i}.txt"
                else
                    echo "disconnected" >> "$TMPDIR/ws_conn_${i}.txt"
                fi
            ) &
        done

        wait

        WS_EST=0
        WS_REJ=0
        WS_DISC=0
        for f in "$TMPDIR"/ws_conn_*.txt; do
            [ -f "$f" ] || continue
            STATUS=$(cat "$f")
            case "$STATUS" in
                established) WS_EST=$((WS_EST + 1)) ;;
                rejected) WS_REJ=$((WS_REJ + 1)) ;;
                disconnected) WS_DISC=$((WS_DISC + 1)) ;;
            esac
        done

        echo "$WS_EST $WS_REJ $WS_DISC" > "$TMPDIR/ws_results.txt"
    fi

    # Read and display results
    if [ -f "$TMPDIR/ws_results.txt" ]; then
        read -r WS_EST WS_REJ WS_DISC < "$TMPDIR/ws_results.txt"
    else
        WS_EST=0
        WS_REJ=0
        WS_DISC=0
    fi

    echo ""
    echo "┌────────────────────────┬────────────┐"
    echo "│ Metric                 │ Value      │"
    echo "├────────────────────────┼────────────┤"
    printf "│ %-22s │ %10s │\n" "Target connections" "$WS_CONNECTIONS"
    printf "│ %-22s │ %10s │\n" "Established" "$WS_EST"
    printf "│ %-22s │ %10s │\n" "Rejected (503)" "$WS_REJ"
    printf "│ %-22s │ %10s │\n" "Disconnects" "$WS_DISC"
    printf "│ %-22s │ %8ss │\n" "Hold duration" "$WS_DURATION"
    echo "└────────────────────────┴────────────┘"

    if [ "$WS_REJ" -gt 0 ]; then
        echo ""
        echo "NOTE: $WS_REJ connections were rejected (HTTP 503)."
        echo "The server may have a connection limit. Reduce --ws-connections."
    fi

    rm -rf "$TMPDIR"

else
    echo "ERROR: Unknown mode '$MODE'. Use rest-health or ws-scale."
    exit 1
fi
