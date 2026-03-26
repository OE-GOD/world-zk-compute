#!/usr/bin/env bash
set -euo pipefail

# Docker Compose Integration Test for Bank Demo
#
# Validates that the docker-compose.bank-demo.yml setup works end-to-end:
# 1. Builds Docker images
# 2. Starts services
# 3. Waits for health
# 4. Tests API endpoints (health, verify invalid, verify real proof, batch, circuits)
# 5. Cleans up
#
# Usage:
#   ./scripts/test-docker-demo.sh              # Run full Docker integration test
#   VERIFIER_PORT=3001 ./scripts/test-docker-demo.sh  # Use custom port
#
# Exit codes:
#   0 — all tests passed
#   2 — Docker not available (skipped, not a failure)
#   1 — test failure

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERIFIER_PORT="${VERIFIER_PORT:-3000}"
VERIFIER_URL="http://localhost:${VERIFIER_PORT}"
COMPOSE_FILE="$PROJECT_ROOT/docker-compose.bank-demo.yml"
FIXTURE_PATH="$PROJECT_ROOT/contracts/test/fixtures/phase1a_dag_fixture.json"

PASSED=0
FAILED=0
SKIPPED=0

# ---------------------------------------------------------------------------
# Pre-flight: Check Docker availability
# ---------------------------------------------------------------------------

if ! command -v docker &> /dev/null; then
    echo "SKIP: Docker is not installed. Install Docker to run this test."
    exit 2
fi

if ! docker info &> /dev/null; then
    echo "SKIP: Docker daemon is not running. Start Docker to run this test."
    exit 2
fi

if ! docker compose version &> /dev/null; then
    echo "SKIP: Docker Compose plugin is not available."
    exit 2
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

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

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    docker compose -f "$COMPOSE_FILE" down --remove-orphans 2>/dev/null || true
    echo "  Docker services stopped."
}
trap cleanup EXIT

cd "$PROJECT_ROOT"

echo "=== Docker Compose Integration Test ==="
echo "  Compose file: $COMPOSE_FILE"
echo "  Verifier URL: $VERIFIER_URL"
echo ""

# ---------------------------------------------------------------------------
# Step 1: Build Docker images
# ---------------------------------------------------------------------------
echo "--- Step 1: Build Docker images ---"

if VERIFIER_PORT="$VERIFIER_PORT" docker compose -f "$COMPOSE_FILE" build 2>&1; then
    pass "Docker image build"
else
    fail "Docker image build"
    echo "FATAL: Cannot continue without a successful build."
    exit 1
fi
echo ""

# ---------------------------------------------------------------------------
# Step 2: Start services
# ---------------------------------------------------------------------------
echo "--- Step 2: Start services ---"

if VERIFIER_PORT="$VERIFIER_PORT" docker compose -f "$COMPOSE_FILE" up -d 2>&1; then
    pass "Docker services started"
else
    fail "Docker services started"
    echo "FATAL: Cannot continue without running services."
    exit 1
fi
echo ""

# ---------------------------------------------------------------------------
# Step 3: Wait for health
# ---------------------------------------------------------------------------
echo "--- Step 3: Wait for verifier API ---"

HEALTH_TIMEOUT=60
API_READY=false
for i in $(seq 1 "$HEALTH_TIMEOUT"); do
    if curl -sf "$VERIFIER_URL/health" > /dev/null 2>&1; then
        API_READY=true
        echo "  Verifier API ready after ${i}s"
        break
    fi
    if (( i % 10 == 0 )); then
        echo "  Still waiting... (${i}/${HEALTH_TIMEOUT}s)"
    fi
    sleep 1
done

if $API_READY; then
    pass "Verifier API health check"
else
    fail "Verifier API did not become healthy within ${HEALTH_TIMEOUT}s"
    echo ""
    echo "=== Container logs ==="
    docker compose -f "$COMPOSE_FILE" logs 2>/dev/null || true
    exit 1
fi
echo ""

# ---------------------------------------------------------------------------
# Step 4: Test API endpoints
# ---------------------------------------------------------------------------
echo "--- Step 4: Test API endpoints ---"

# 4a: Health endpoint returns valid JSON with status=ok
HEALTH_RESP=$(curl -sf "$VERIFIER_URL/health" 2>/dev/null) || HEALTH_RESP=""
if echo "$HEALTH_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['status']=='ok'" 2>/dev/null; then
    pass "GET /health returns {\"status\":\"ok\"}"
else
    fail "GET /health (response: $HEALTH_RESP)"
fi

# 4b: Invalid proof returns HTTP 400
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

# 4c: Circuit registration
REG_BODY='{"circuit_id":"docker-test","gens_hex":"0x00","dag_circuit_description":{}}'
REG_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST "$VERIFIER_URL/circuits" \
    -H "Content-Type: application/json" \
    -d "$REG_BODY" 2>/dev/null) || REG_CODE="000"
if [[ "$REG_CODE" == "200" ]]; then
    pass "POST /circuits registers circuit (HTTP 200)"
else
    fail "POST /circuits returned HTTP $REG_CODE (expected 200)"
fi

# 4d: List circuits shows registered circuit
CIRCUITS_RESP=$(curl -sf "$VERIFIER_URL/circuits" 2>/dev/null) || CIRCUITS_RESP=""
if echo "$CIRCUITS_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'docker-test' in d['circuits']" 2>/dev/null; then
    pass "GET /circuits lists registered circuit"
else
    fail "GET /circuits (response: $CIRCUITS_RESP)"
fi

# 4e: Health now shows circuits_loaded=1
HEALTH_RESP2=$(curl -sf "$VERIFIER_URL/health" 2>/dev/null) || HEALTH_RESP2=""
if echo "$HEALTH_RESP2" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['circuits_loaded']==1" 2>/dev/null; then
    pass "GET /health shows circuits_loaded=1 after registration"
else
    fail "GET /health circuits_loaded (response: $HEALTH_RESP2)"
fi

# 4f: Batch verify with invalid proofs returns total=2, valid=0
BATCH_BODY='{"bundles":[{"proof_hex":"0xdead","gens_hex":"0x00","dag_circuit_description":{}},{"proof_hex":"0xbeef","gens_hex":"0x00","dag_circuit_description":{}}]}'
BATCH_RESP=$(curl -sf \
    -X POST "$VERIFIER_URL/verify/batch" \
    -H "Content-Type: application/json" \
    -d "$BATCH_BODY" 2>/dev/null) || BATCH_RESP=""
if echo "$BATCH_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['total']==2 and d['valid']==0" 2>/dev/null; then
    pass "POST /verify/batch handles batch (total=2, valid=0)"
else
    fail "POST /verify/batch (response: $BATCH_RESP)"
fi

# 4g: Invalid hybrid verify returns HTTP 400
HYBRID_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST "$VERIFIER_URL/verify/hybrid" \
    -H "Content-Type: application/json" \
    -d "$INVALID_BUNDLE" 2>/dev/null) || HYBRID_CODE="000"
if [[ "$HYBRID_CODE" == "400" ]]; then
    pass "POST /verify/hybrid rejects invalid proof (HTTP 400)"
else
    fail "POST /verify/hybrid invalid proof returned HTTP $HYBRID_CODE (expected 400)"
fi

echo ""

# ---------------------------------------------------------------------------
# Step 5: Real proof verification (if fixture available)
# ---------------------------------------------------------------------------
echo "--- Step 5: Real proof verification ---"

if [[ -f "$FIXTURE_PATH" ]]; then
    echo "  Constructing proof bundle from phase1a_dag_fixture.json..."
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
        # 5a: Full verification
        VERIFY_RESP=$(curl -sf \
            -X POST "$VERIFIER_URL/verify" \
            -H "Content-Type: application/json" \
            -d "$BUNDLE_JSON" 2>/dev/null) || VERIFY_RESP=""
        if echo "$VERIFY_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['verified']==True" 2>/dev/null; then
            CIRCUIT_HASH=$(echo "$VERIFY_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['circuit_hash'])" 2>/dev/null) || CIRCUIT_HASH="?"
            echo "  Circuit hash: $CIRCUIT_HASH"
            pass "POST /verify with real proof (verified=true)"
        elif [[ -n "$VERIFY_RESP" ]]; then
            echo "  Response: $VERIFY_RESP"
            fail "POST /verify with real proof (verified != true)"
        else
            fail "POST /verify with real proof (no response)"
        fi

        # 5b: Hybrid verification
        HYBRID_RESP=$(curl -sf \
            -X POST "$VERIFIER_URL/verify/hybrid" \
            -H "Content-Type: application/json" \
            -d "$BUNDLE_JSON" 2>/dev/null) || HYBRID_RESP=""
        if echo "$HYBRID_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['num_compute_layers'] > 0" 2>/dev/null; then
            NUM_LAYERS=$(echo "$HYBRID_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['num_compute_layers'])" 2>/dev/null) || NUM_LAYERS="?"
            echo "  Hybrid: $NUM_LAYERS compute layers"
            pass "POST /verify/hybrid with real proof"
        elif [[ -n "$HYBRID_RESP" ]]; then
            echo "  Response: $HYBRID_RESP"
            fail "POST /verify/hybrid with real proof"
        else
            fail "POST /verify/hybrid with real proof (no response)"
        fi
    else
        fail "Could not construct proof bundle from fixture"
    fi
else
    skip "Real proof verification (fixture not found at $FIXTURE_PATH)"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 6: Container health check via Docker
# ---------------------------------------------------------------------------
echo "--- Step 6: Container health ---"

CONTAINER_STATUS=$(docker compose -f "$COMPOSE_FILE" ps --format json 2>/dev/null | python3 -c "
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    c = json.loads(line)
    name = c.get('Name', c.get('name', '?'))
    state = c.get('State', c.get('state', '?'))
    health = c.get('Health', c.get('health', 'N/A'))
    print(f'  {name}: state={state} health={health}')
" 2>/dev/null) || CONTAINER_STATUS=""

if [[ -n "$CONTAINER_STATUS" ]]; then
    echo "$CONTAINER_STATUS"
    pass "Container status retrieved"
else
    # Fallback: just check docker compose ps
    docker compose -f "$COMPOSE_FILE" ps 2>/dev/null
    pass "Container status (fallback)"
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
    echo "=== Container logs (for debugging) ==="
    docker compose -f "$COMPOSE_FILE" logs --tail=50 2>/dev/null || true
    echo ""
    echo "RESULT: FAIL"
    exit 1
else
    echo ""
    echo "RESULT: PASS — All Docker integration tests passed"
    exit 0
fi
