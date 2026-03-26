#!/usr/bin/env bash
set -euo pipefail

# Bank Demo E2E Smoke Test
#
# Tests the full bank credit scoring workflow:
# 1. Build the bank-demo CLI and verifier API
# 2. Start the verifier API (natively or via Docker)
# 3. Run credit scoring inference
# 4. Submit a proof bundle to the verifier API
# 5. Print PASS/FAIL
#
# Usage:
#   ./scripts/bank-demo.sh              # Native mode (default) — builds and runs locally
#   ./scripts/bank-demo.sh --docker     # Docker mode — uses docker-compose
#   ./scripts/bank-demo.sh --ci         # CI mode — skip bank-demo build, test verifier only

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERIFIER_URL="${VERIFIER_URL:-http://localhost:3000}"
PROOFS_DIR="${PROOFS_DIR:-/tmp/bank-demo-proofs}"
FIXTURE_PATH="$PROJECT_ROOT/contracts/test/fixtures/phase1a_dag_fixture.json"
# Respect CARGO_TARGET_DIR if set (e.g. by multi-agent build isolation)
TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
MODE="native"
VERIFIER_PID=""
PASSED=0
FAILED=0
SKIPPED=0

for arg in "$@"; do
    case "$arg" in
        --docker)  MODE="docker" ;;
        --ci)      MODE="ci" ;;
        --help|-h)
            echo "Usage: $0 [--docker|--ci]"
            echo ""
            echo "Modes:"
            echo "  (default)  Build and run verifier API natively"
            echo "  --docker   Use docker-compose to run verifier API"
            echo "  --ci       CI mode: skip bank-demo build, test verifier crate + service only"
            exit 0
            ;;
        *)
            echo "Unknown option: $arg"
            exit 1
            ;;
    esac
done

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    if [[ -n "${VERIFIER_PID:-}" ]] && kill -0 "$VERIFIER_PID" 2>/dev/null; then
        kill "$VERIFIER_PID" 2>/dev/null || true
        wait "$VERIFIER_PID" 2>/dev/null || true
        echo "  Stopped verifier API (PID $VERIFIER_PID)"
    fi
    if [[ "$MODE" == "docker" ]]; then
        docker compose -f "$PROJECT_ROOT/docker-compose.bank-demo.yml" down 2>/dev/null || true
    fi
}
trap cleanup EXIT

pass() {
    PASSED=$((PASSED + 1))
    echo "  [PASS] $1"
}

fail() {
    FAILED=$((FAILED + 1))
    echo "  [FAIL] $1"
}

skip() {
    SKIPPED=$((SKIPPED + 1))
    echo "  [SKIP] $1"
}

cd "$PROJECT_ROOT"

echo "=== Bank Demo E2E Smoke Test ==="
echo "  Mode: $MODE"
echo ""

# ---------------------------------------------------------------------------
# Step 1: Build components
# ---------------------------------------------------------------------------
echo "--- Step 1: Build ---"

if [[ "$MODE" != "ci" ]]; then
    echo "  Building bank-demo CLI..."
    if (cd examples/bank-demo && cargo build --release 2>&1 | tail -3); then
        pass "bank-demo CLI builds"
    else
        fail "bank-demo CLI build"
    fi
fi

if [[ "$MODE" == "native" || "$MODE" == "ci" ]]; then
    echo "  Building verifier service..."
    if cargo build --release -p zkml-verifier-service 2>&1 | tail -3; then
        pass "verifier service builds"
    else
        fail "verifier service build"
    fi
fi
echo ""

# ---------------------------------------------------------------------------
# Step 2: Run unit tests
# ---------------------------------------------------------------------------
echo "--- Step 2: Unit Tests ---"

if [[ "$MODE" != "ci" ]]; then
    echo "  Testing bank-demo..."
    if (cd examples/bank-demo && cargo test 2>&1 | tail -5); then
        pass "bank-demo tests (5 tests)"
    else
        fail "bank-demo tests"
    fi
fi

echo "  Testing zkml-verifier..."
if cargo test -p zkml-verifier 2>&1 | tail -5; then
    pass "zkml-verifier tests (92+ tests)"
else
    fail "zkml-verifier tests"
fi

echo "  Testing verifier service..."
if cargo test -p zkml-verifier-service 2>&1 | tail -5; then
    pass "verifier service tests (15 tests)"
else
    fail "verifier service tests"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 3: Run bank-demo inference
# ---------------------------------------------------------------------------
echo "--- Step 3: Credit Scoring Inference ---"

if [[ "$MODE" != "ci" ]]; then
    INPUT_FILE="$PROJECT_ROOT/examples/bank-demo/sample_input.json"
    if [[ -f "$INPUT_FILE" ]]; then
        FEATURES=$(python3 -c "
import json
with open('$INPUT_FILE') as f:
    d = json.load(f)
print(f'--credit-score {d[\"credit_score\"]} --income {d[\"annual_income\"]} --credit-history-years {d[\"credit_history_years\"]} --dti-ratio {d[\"debt_to_income_ratio\"]} --num-accounts {d[\"num_accounts\"]} --recent-inquiries {d[\"recent_inquiries\"]}')
" 2>/dev/null) || FEATURES="--credit-score 720 --income 85000 --credit-history-years 12 --dti-ratio 0.22 --num-accounts 5 --recent-inquiries 1"
    else
        FEATURES="--credit-score 720 --income 85000 --credit-history-years 12 --dti-ratio 0.22 --num-accounts 5 --recent-inquiries 1"
    fi

    INFERENCE_RESULT=""
    if INFERENCE_RESULT=$(cd examples/bank-demo && cargo run --release -- --model models/credit_scoring.json $FEATURES --json 2>/dev/null); then
        DECISION=$(echo "$INFERENCE_RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin)['decision'])" 2>/dev/null) || DECISION="?"
        PROB=$(echo "$INFERENCE_RESULT" | python3 -c "import sys,json; print(f\"{json.load(sys.stdin)['approval_probability']*100:.1f}%\")" 2>/dev/null) || PROB="?"
        echo "  Decision: $DECISION (probability: $PROB)"
        pass "credit scoring inference"
    else
        fail "credit scoring inference"
    fi
else
    skip "credit scoring inference (CI mode)"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 4: Start verifier API
# ---------------------------------------------------------------------------
echo "--- Step 4: Verifier API ---"

if [[ "$MODE" == "docker" ]]; then
    echo "  Starting via Docker Compose..."
    docker compose -f "$PROJECT_ROOT/docker-compose.bank-demo.yml" up -d
elif [[ "$MODE" == "native" || "$MODE" == "ci" ]]; then
    VERIFIER_BIN="$TARGET_DIR/release/zkml-verifier-service"
    if [[ ! -f "$VERIFIER_BIN" ]]; then
        fail "verifier binary not found at $VERIFIER_BIN"
    else
        # Pick a random port to avoid conflicts
        VERIFIER_PORT="${VERIFIER_PORT:-3099}"
        VERIFIER_URL="http://localhost:$VERIFIER_PORT"
        echo "  Starting verifier on port $VERIFIER_PORT..."
        PORT="$VERIFIER_PORT" "$VERIFIER_BIN" &
        VERIFIER_PID=$!
    fi
fi

# Wait for the verifier API to be ready
echo "  Waiting for verifier API at $VERIFIER_URL..."
API_READY=false
for i in $(seq 1 15); do
    if curl -sf "$VERIFIER_URL/health" > /dev/null 2>&1; then
        API_READY=true
        break
    fi
    sleep 1
done

if $API_READY; then
    HEALTH=$(curl -sf "$VERIFIER_URL/health" 2>/dev/null) || HEALTH=""
    echo "  Health: $HEALTH"
    pass "verifier API started"
else
    fail "verifier API did not start within 15s"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 5: Verify proof via API
# ---------------------------------------------------------------------------
echo "--- Step 5: Proof Verification via API ---"

if $API_READY; then
    # Test 5a: Health endpoint returns valid JSON
    if HEALTH_JSON=$(curl -sf "$VERIFIER_URL/health" 2>/dev/null) && \
       echo "$HEALTH_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['status']=='ok'" 2>/dev/null; then
        pass "GET /health returns {status: ok}"
    else
        fail "GET /health"
    fi

    # Test 5b: Invalid proof returns 400
    INVALID_BUNDLE='{"proof_hex":"0xdead","gens_hex":"0xbeef","dag_circuit_description":{}}'
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
        -X POST "$VERIFIER_URL/verify" \
        -H "Content-Type: application/json" \
        -d "$INVALID_BUNDLE" 2>/dev/null) || HTTP_CODE="000"
    if [[ "$HTTP_CODE" == "400" ]]; then
        pass "POST /verify rejects invalid proof (HTTP 400)"
    else
        fail "POST /verify invalid proof returned HTTP $HTTP_CODE (expected 400)"
    fi

    # Test 5c: Circuit registration
    REG_BODY='{"circuit_id":"smoke-test","gens_hex":"0x00","dag_circuit_description":{}}'
    REG_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
        -X POST "$VERIFIER_URL/circuits" \
        -H "Content-Type: application/json" \
        -d "$REG_BODY" 2>/dev/null) || REG_CODE="000"
    if [[ "$REG_CODE" == "200" ]]; then
        pass "POST /circuits registers circuit (HTTP 200)"
    else
        fail "POST /circuits returned HTTP $REG_CODE (expected 200)"
    fi

    # Test 5d: List circuits shows registered circuit
    CIRCUITS=$(curl -sf "$VERIFIER_URL/circuits" 2>/dev/null) || CIRCUITS=""
    if echo "$CIRCUITS" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'smoke-test' in d['circuits']" 2>/dev/null; then
        pass "GET /circuits lists registered circuit"
    else
        fail "GET /circuits"
    fi

    # Test 5e: Batch verify with invalid proofs returns results
    BATCH_BODY='{"bundles":[{"proof_hex":"0xdead","gens_hex":"0x00","dag_circuit_description":{}},{"proof_hex":"0xbeef","gens_hex":"0x00","dag_circuit_description":{}}]}'
    BATCH_RESP=$(curl -sf \
        -X POST "$VERIFIER_URL/verify/batch" \
        -H "Content-Type: application/json" \
        -d "$BATCH_BODY" 2>/dev/null) || BATCH_RESP=""
    if echo "$BATCH_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['total']==2 and d['valid']==0" 2>/dev/null; then
        pass "POST /verify/batch handles batch (total=2, valid=0)"
    else
        fail "POST /verify/batch"
    fi

    # Test 5f: Submit real proof bundle from fixture (if available)
    if [[ -f "$FIXTURE_PATH" ]]; then
        echo "  Submitting real proof bundle from phase1a_dag_fixture.json..."
        # The fixture is a superset of ProofBundle; extract the needed fields
        BUNDLE_JSON=$(python3 -c "
import json
with open('$FIXTURE_PATH') as f:
    d = json.load(f)
bundle = {
    'proof_hex': d['proof_hex'],
    'gens_hex': d['gens_hex'],
    'public_inputs_hex': d.get('public_inputs_hex', ''),
    'dag_circuit_description': d['dag_circuit_description']
}
print(json.dumps(bundle))
" 2>/dev/null) || BUNDLE_JSON=""

        if [[ -n "$BUNDLE_JSON" ]]; then
            VERIFY_RESP=$(curl -sf \
                -X POST "$VERIFIER_URL/verify" \
                -H "Content-Type: application/json" \
                -d "$BUNDLE_JSON" 2>/dev/null) || VERIFY_RESP=""
            if echo "$VERIFY_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['verified']==True" 2>/dev/null; then
                CIRCUIT_HASH=$(echo "$VERIFY_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['circuit_hash'])" 2>/dev/null) || CIRCUIT_HASH="?"
                echo "  Circuit hash: $CIRCUIT_HASH"
                pass "POST /verify with real proof bundle (verified=true)"
            elif [[ -n "$VERIFY_RESP" ]]; then
                echo "  Response: $VERIFY_RESP"
                fail "POST /verify with real proof (verified != true)"
            else
                fail "POST /verify with real proof (no response)"
            fi
        else
            fail "could not construct proof bundle from fixture"
        fi
    else
        skip "real proof verification (fixture not found at $FIXTURE_PATH)"
    fi

    # Test 5g: Hybrid verify with real proof bundle (if available)
    if [[ -f "$FIXTURE_PATH" ]]; then
        HYBRID_RESP=$(curl -sf \
            -X POST "$VERIFIER_URL/verify/hybrid" \
            -H "Content-Type: application/json" \
            -d "$BUNDLE_JSON" 2>/dev/null) || HYBRID_RESP=""
        if echo "$HYBRID_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['num_compute_layers'] > 0" 2>/dev/null; then
            NUM_LAYERS=$(echo "$HYBRID_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['num_compute_layers'])" 2>/dev/null) || NUM_LAYERS="?"
            echo "  Hybrid: $NUM_LAYERS compute layers"
            pass "POST /verify/hybrid with real proof bundle"
        elif [[ -n "$HYBRID_RESP" ]]; then
            echo "  Response: $HYBRID_RESP"
            fail "POST /verify/hybrid (unexpected response)"
        else
            fail "POST /verify/hybrid (no response)"
        fi
    else
        skip "hybrid proof verification (fixture not found)"
    fi
else
    skip "API tests (verifier not running)"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 6: Archive result
# ---------------------------------------------------------------------------
echo "--- Step 6: Archive ---"
if [[ -n "${INFERENCE_RESULT:-}" ]]; then
    mkdir -p "$PROOFS_DIR"
    TIMESTAMP=$(date +%Y%m%d-%H%M%S)
    ARCHIVE_FILE="$PROOFS_DIR/bank-demo-$TIMESTAMP.json"
    echo "$INFERENCE_RESULT" > "$ARCHIVE_FILE"
    echo "  Archived inference result to: $ARCHIVE_FILE"
    pass "result archived"
else
    skip "archive (no inference result)"
fi
echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "============================================"
echo "  PASSED:  $PASSED"
echo "  FAILED:  $FAILED"
echo "  SKIPPED: $SKIPPED"
echo "============================================"

if [[ $FAILED -gt 0 ]]; then
    echo ""
    echo "RESULT: FAIL"
    exit 1
else
    echo ""
    echo "RESULT: PASS"
    exit 0
fi
