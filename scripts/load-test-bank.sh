#!/usr/bin/env bash
# Load test the zkml-verifier REST API.
# Measures throughput and latency for /health, /verify (real proof), and /verify (error path).
#
# Usage: ./scripts/load-test-bank.sh [OPTIONS]
#
# Options:
#   --url URL            Verifier URL (default: http://localhost:3000)
#   --concurrency N      Concurrent workers per test (default: 10)
#   --requests N         Total requests per test (default: 100)
#   --fixture PATH       Proof fixture JSON (default: contracts/test/fixtures/phase1a_dag_fixture.json)
#   --api-key KEY        API key for x-api-key header (optional)
#   --skip-verify        Skip the real-proof /verify test (saves time when server has no circuit loaded)
#   --help, -h           Show this help message
#
# Requires: curl, python3
# The script uses parallel curl with background jobs to drive concurrency.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERIFIER_URL="http://localhost:3000"
CONCURRENCY=10
TOTAL=100
FIXTURE="${PROJECT_ROOT}/contracts/test/fixtures/phase1a_dag_fixture.json"
API_KEY=""
SKIP_VERIFY=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --url) VERIFIER_URL="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --requests) TOTAL="$2"; shift 2 ;;
        --fixture) FIXTURE="$2"; shift 2 ;;
        --api-key) API_KEY="$2"; shift 2 ;;
        --skip-verify) SKIP_VERIFY=true; shift ;;
        --help|-h)
            head -17 "$0" | tail -16
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ──────────────────────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────────────────────

# do_curl: wrapper around curl that injects API key header if set.
# All arguments are passed through to curl.
do_curl() {
    if [[ -n "$API_KEY" ]]; then
        curl -H "x-api-key: ${API_KEY}" "$@"
    else
        curl "$@"
    fi
}

# compute_percentiles: read newline-delimited floats (seconds) from stdin,
# print "min avg p50 p95 p99 max" in milliseconds.
compute_percentiles() {
    python3 -c "
import sys
times = sorted(float(l.strip()) for l in sys.stdin if l.strip())
if not times:
    print('N/A N/A N/A N/A N/A N/A')
    sys.exit(0)
n = len(times)
mn  = times[0]
mx  = times[-1]
avg = sum(times) / n
p50 = times[int(n * 0.50)]
p95 = times[min(int(n * 0.95), n - 1)]
p99 = times[min(int(n * 0.99), n - 1)]
# Convert seconds to milliseconds
fmt = lambda v: f'{v*1000:.1f}'
print(f'{fmt(mn)} {fmt(avg)} {fmt(p50)} {fmt(p95)} {fmt(p99)} {fmt(mx)}')
"
}

# run_load_test LABEL METHOD URL [BODY_FILE]
# Sends $TOTAL requests at $CONCURRENCY concurrency.
# Writes per-request results (http_code time_total_seconds) to a temp dir,
# then computes and prints stats.
run_load_test() {
    local LABEL="$1"
    local METHOD="$2"
    local URL="$3"
    local BODY_FILE="${4:-}"

    echo ""
    echo "--- ${LABEL} ---"

    local TMPDIR
    TMPDIR=$(mktemp -d)
    local TIMES_FILE="${TMPDIR}/times.txt"
    local CODES_FILE="${TMPDIR}/codes.txt"
    touch "$TIMES_FILE" "$CODES_FILE"

    # Build the base curl args (without auth -- do_curl adds that)
    local CURL_BASE=(-s -o /dev/null -w "%{http_code} %{time_total}\n")
    if [[ "$METHOD" == "POST" ]]; then
        CURL_BASE+=(-X POST -H "Content-Type: application/json")
        if [[ -n "$BODY_FILE" ]]; then
            CURL_BASE+=(-d "@${BODY_FILE}")
        fi
    fi

    local START_NS
    START_NS=$(python3 -c "import time; print(int(time.time()*1e9))")

    local SENT=0
    local -a PIDS
    PIDS=()
    for ((i = 1; i <= TOTAL; i++)); do
        (
            RESP=$(do_curl "${CURL_BASE[@]}" "$URL" 2>&1 || echo "000 0.0")
            echo "$RESP" >> "${TMPDIR}/out_${i}.txt"
        ) &
        PIDS+=($!)
        SENT=$((SENT + 1))

        # Throttle: once we hit CONCURRENCY in-flight, wait for them all
        if (( i % CONCURRENCY == 0 )); then
            for pid in "${PIDS[@]}"; do
                wait "$pid" 2>/dev/null || true
            done
            PIDS=()
        fi
    done
    # Wait for any remaining
    if [[ ${#PIDS[@]} -gt 0 ]]; then
        for pid in "${PIDS[@]}"; do
            wait "$pid" 2>/dev/null || true
        done
    fi

    local END_NS
    END_NS=$(python3 -c "import time; print(int(time.time()*1e9))")

    # Aggregate results
    local OK=0 CLIENT_ERR=0 SERVER_ERR=0 CONN_ERR=0 OTHER=0
    for f in "${TMPDIR}"/out_*.txt; do
        [ -f "$f" ] || continue
        while read -r code time_s; do
            echo "$time_s" >> "$TIMES_FILE"
            echo "$code" >> "$CODES_FILE"
            case "$code" in
                2[0-9][0-9]) OK=$((OK + 1)) ;;
                4[0-9][0-9]) CLIENT_ERR=$((CLIENT_ERR + 1)) ;;
                5[0-9][0-9]) SERVER_ERR=$((SERVER_ERR + 1)) ;;
                000)         CONN_ERR=$((CONN_ERR + 1)) ;;
                *)           OTHER=$((OTHER + 1)) ;;
            esac
        done < "$f"
    done

    local WALL_S
    WALL_S=$(python3 -c "print(f'{($END_NS - $START_NS) / 1e9:.3f}')")
    local RPS
    RPS=$(python3 -c "t = ($END_NS - $START_NS) / 1e9; print(f'{$SENT / t:.1f}' if t > 0 else 'N/A')")

    # Percentiles
    local PSTATS
    PSTATS=$(sort -n "$TIMES_FILE" | compute_percentiles)
    local P_MIN P_AVG P_P50 P_P95 P_P99 P_MAX
    read -r P_MIN P_AVG P_P50 P_P95 P_P99 P_MAX <<< "$PSTATS"

    # Determine "success" count based on test type
    # For error-path tests, 400 is the expected response, so we count 4xx as success
    local SUCCESS_COUNT="$OK"
    local SUCCESS_LABEL="2xx"
    if [[ "$LABEL" == *"error"* || "$LABEL" == *"invalid"* ]]; then
        SUCCESS_COUNT=$((OK + CLIENT_ERR))
        SUCCESS_LABEL="2xx+4xx (expected)"
    fi
    local SUCCESS_PCT
    SUCCESS_PCT=$(python3 -c "print(f'{$SUCCESS_COUNT / $SENT * 100:.1f}' if $SENT > 0 else 'N/A')")

    printf "  Requests:     %d sent, %d %s, %d server-err, %d conn-err\n" \
        "$SENT" "$SUCCESS_COUNT" "$SUCCESS_LABEL" "$SERVER_ERR" "$CONN_ERR"
    printf "  Wall time:    %ss\n" "$WALL_S"
    printf "  Throughput:   %s req/sec\n" "$RPS"
    printf "  Latency min:  %sms\n" "$P_MIN"
    printf "  Latency avg:  %sms\n" "$P_AVG"
    printf "  Latency p50:  %sms\n" "$P_P50"
    printf "  Latency p95:  %sms\n" "$P_P95"
    printf "  Latency p99:  %sms\n" "$P_P99"
    printf "  Latency max:  %sms\n" "$P_MAX"
    printf "  Success rate: %s%%\n" "$SUCCESS_PCT"

    # Store results for the summary
    echo "${LABEL}|${RPS}|${P_P50}|${P_P99}|${SUCCESS_PCT}" >> "${RESULTS_SUMMARY_FILE}"

    rm -rf "$TMPDIR"
}

# ──────────────────────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────────────────────

echo "========================================="
echo "  Verifier API Load Test"
echo "========================================="
echo "URL:         ${VERIFIER_URL}"
echo "Concurrency: ${CONCURRENCY}"
echo "Total/test:  ${TOTAL}"
echo "Fixture:     ${FIXTURE}"
echo ""

# Health check
echo "Checking verifier is reachable..."
HTTP_CODE=$(do_curl -s -o /dev/null -w "%{http_code}" "${VERIFIER_URL}/health" 2>/dev/null || true)
if [[ "$HTTP_CODE" != "200" ]]; then
    echo "ERROR: Verifier not running at ${VERIFIER_URL} (HTTP ${HTTP_CODE})"
    echo "Start the verifier first:"
    echo "  cargo run -p zkml-verifier-service --release"
    exit 1
fi
echo "Verifier is up (HTTP 200)."

# Summary file (temp)
RESULTS_SUMMARY_FILE=$(mktemp)
trap "rm -f '$RESULTS_SUMMARY_FILE'" EXIT

# ──────────────────────────────────
# Test 1: Health endpoint baseline
# ──────────────────────────────────
run_load_test "Health endpoint (GET /health)" "GET" "${VERIFIER_URL}/health"

# ──────────────────────────────────
# Test 2: Verify with real proof
# ──────────────────────────────────
if [[ "$SKIP_VERIFY" == true ]]; then
    echo ""
    echo "--- Verify endpoint (real proof) --- SKIPPED (--skip-verify)"
else
    if [[ ! -f "$FIXTURE" ]]; then
        echo ""
        echo "WARNING: Fixture not found at ${FIXTURE}"
        echo "Skipping real-proof verify test."
    else
        # Build a ProofBundle JSON from the fixture.
        # The fixture has extra keys (dag_layers, claim_routing, etc.) that are
        # not part of ProofBundle. We extract only the relevant fields.
        VERIFY_BODY_FILE=$(mktemp)
        python3 -c "
import json, sys
with open('${FIXTURE}') as f:
    d = json.load(f)
bundle = {
    'proof_hex': d['proof_hex'],
    'public_inputs_hex': d.get('public_inputs_hex', ''),
    'gens_hex': d['gens_hex'],
    'dag_circuit_description': d['dag_circuit_description'],
}
json.dump(bundle, sys.stdout)
" > "$VERIFY_BODY_FILE"

        VERIFY_BODY_SIZE=$(wc -c < "$VERIFY_BODY_FILE" | tr -d ' ')
        echo ""
        echo "Proof bundle payload: ${VERIFY_BODY_SIZE} bytes"

        run_load_test "Verify endpoint (real proof, POST /verify)" "POST" "${VERIFIER_URL}/verify" "$VERIFY_BODY_FILE"

        rm -f "$VERIFY_BODY_FILE"
    fi
fi

# ──────────────────────────────────
# Test 3: Verify with invalid proof (error path)
# ──────────────────────────────────
INVALID_BODY_FILE=$(mktemp)
cat > "$INVALID_BODY_FILE" <<'ENDJSON'
{
  "proof_hex": "0xdeadbeef",
  "gens_hex": "0x00",
  "dag_circuit_description": {},
  "public_inputs_hex": ""
}
ENDJSON

run_load_test "Verify endpoint (invalid proof, error path, POST /verify)" "POST" "${VERIFIER_URL}/verify" "$INVALID_BODY_FILE"

rm -f "$INVALID_BODY_FILE"

# ──────────────────────────────────
# Summary table
# ──────────────────────────────────
echo ""
echo "========================================="
echo "  Results Summary"
echo "========================================="
echo ""
printf "%-55s %12s %10s %10s %10s\n" "Test" "Throughput" "p50" "p99" "Success"
printf "%-55s %12s %10s %10s %10s\n" "----" "----------" "---" "---" "-------"
while IFS='|' read -r label rps p50 p99 success; do
    printf "%-55s %10s/s %8sms %8sms %8s%%\n" "$label" "$rps" "$p50" "$p99" "$success"
done < "$RESULTS_SUMMARY_FILE"
echo ""
echo "Config: concurrency=${CONCURRENCY}, requests/test=${TOTAL}"
echo "Done."
