#!/usr/bin/env bash
set -euo pipefail

# Real Proof E2E Off-Chain Test
#
# Submits the actual phase1a_dag_fixture.json GKR+Hyrax proof through the
# off-chain service stack and verifies the full lifecycle:
#   1. Health check (registry)
#   2. Submit proof bundle to registry
#   3. Verify proof via verifier API
#   4. Get verification receipt
#   5. Check transparency log
#
# Usage:
#   ./scripts/test-offchain-real.sh
#   REGISTRY_URL=http://host:3001 VERIFIER_URL=http://host:3000 ./scripts/test-offchain-real.sh
#
# Environment:
#   REGISTRY_URL   - Proof registry URL (default: http://localhost:3001)
#   VERIFIER_URL   - Verifier API URL (default: http://localhost:3000)
#   API_KEY        - Optional API key for authenticated endpoints
#   TIMEOUT        - Health-check wait timeout in seconds (default: 15)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REGISTRY_URL="${REGISTRY_URL:-http://localhost:3001}"
VERIFIER_URL="${VERIFIER_URL:-http://localhost:3000}"
API_KEY="${API_KEY:-}"
TIMEOUT="${TIMEOUT:-15}"

FIXTURE="$PROJECT_ROOT/contracts/test/fixtures/phase1a_dag_fixture.json"

PASS=0
FAIL=0

# ---- Helpers ----

pass() {
    PASS=$((PASS + 1))
    echo "  PASS: $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo "  FAIL: $1"
}

auth_header() {
    if [ -n "$API_KEY" ]; then
        echo "-H" "x-api-key: $API_KEY"
    fi
}

json_field() {
    python3 -c "import sys,json; print(json.load(sys.stdin).get('$1',''))" 2>/dev/null
}

json_field_nested() {
    python3 -c "
import sys, json
data = json.load(sys.stdin)
keys = '$1'.split('.')
for k in keys:
    if isinstance(data, dict):
        data = data.get(k, '')
    else:
        data = ''
        break
print(data)
" 2>/dev/null
}

# ---- Pre-flight checks ----

echo "=== Real Proof E2E Off-Chain Test ==="
echo "Registry: $REGISTRY_URL"
echo "Verifier: $VERIFIER_URL"
echo "Fixture:  $FIXTURE"
echo ""

if [ ! -f "$FIXTURE" ]; then
    echo "ERROR: Fixture not found at $FIXTURE"
    echo "Run 'cargo test gen_phase1a_fixture' to generate it."
    exit 1
fi

# ---- Step 1: Health Check ----

echo "--- Step 1: Health check ---"

HEALTHY=false
for i in $(seq 1 "$TIMEOUT"); do
    if RESP=$(curl -sf "$REGISTRY_URL/health" 2>/dev/null); then
        STATUS=$(echo "$RESP" | json_field "status") || STATUS=""
        if [ "$STATUS" = "ok" ]; then
            HEALTHY=true
            break
        fi
    fi
    sleep 1
done

if [ "$HEALTHY" = "true" ]; then
    pass "Registry health check"
else
    fail "Registry health check (not reachable after ${TIMEOUT}s)"
    echo ""
    echo "=== Results: $PASS passed, $FAIL failed ==="
    exit 1
fi

# ---- Step 2: Submit proof to registry ----

echo ""
echo "--- Step 2: Submit proof to registry ---"

# Build a proof bundle JSON from the fixture. The fixture contains the raw
# proof, gens, circuit description, etc. We wrap it in the format expected
# by the registry's POST /proofs endpoint.
BUNDLE=$(python3 -c "
import json, sys, hashlib

with open('$FIXTURE') as f:
    fix = json.load(f)

proof_hex = fix.get('proof_hex', fix.get('proof', ''))
gens_hex = fix.get('gens_hex', fix.get('gens', ''))

bundle = {
    'proof_hex': proof_hex,
    'public_inputs_hex': fix.get('public_inputs_hex', ''),
    'gens_hex': gens_hex,
    'dag_circuit_description': fix.get('dag_circuit_description', fix.get('circuit_description', {})),
    'model_hash': '0x' + hashlib.sha256(b'test-model-real-e2e').hexdigest()[:16],
    'timestamp': 1700000000,
    'prover_version': 'e2e-test-real',
    'circuit_hash': fix.get('circuit_hash', '0x0000'),
}
print(json.dumps(bundle))
")

SUBMIT_RESP=$(curl -sf -X POST "$REGISTRY_URL/proofs" \
    -H "Content-Type: application/json" \
    $(auth_header) \
    -d "$BUNDLE" 2>/dev/null) || SUBMIT_RESP=""

if [ -z "$SUBMIT_RESP" ]; then
    fail "Submit proof (no response)"
else
    PROOF_ID=$(echo "$SUBMIT_RESP" | json_field "id")
    PROOF_STATUS=$(echo "$SUBMIT_RESP" | json_field "status")
    TLOG_INDEX=$(echo "$SUBMIT_RESP" | json_field "transparency_index")

    if [ -n "$PROOF_ID" ] && [ "$PROOF_STATUS" = "stored" ]; then
        pass "Submit proof (id=$PROOF_ID, tlog_index=$TLOG_INDEX)"
    else
        fail "Submit proof (unexpected response: $SUBMIT_RESP)"
        PROOF_ID=""
    fi
fi

# ---- Step 3: Verify proof via verifier API ----

echo ""
echo "--- Step 3: Verify proof via verifier API ---"

if [ -n "${PROOF_ID:-}" ]; then
    VERIFY_RESP=$(curl -sf -X POST "$REGISTRY_URL/proofs/$PROOF_ID/verify" \
        $(auth_header) \
        2>/dev/null) || VERIFY_RESP=""

    if [ -z "$VERIFY_RESP" ]; then
        fail "Verify proof (no response)"
    else
        VERIFIED=$(echo "$VERIFY_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(str(d.get('verified', d.get('status', ''))).lower())" 2>/dev/null) || VERIFIED=""

        if [ "$VERIFIED" = "true" ] || [ "$VERIFIED" = "verified" ]; then
            pass "Verify proof (verified=true)"
        else
            # Verification may fail for real proofs if the verifier is not configured
            # with the correct circuit. Report the result but don't hard-fail.
            echo "  INFO: Verification returned: $VERIFY_RESP"
            fail "Verify proof (verified=$VERIFIED)"
        fi
    fi
else
    fail "Verify proof (skipped: no proof ID from submit step)"
fi

# ---- Step 4: Get verification receipt ----

echo ""
echo "--- Step 4: Get verification receipt ---"

if [ -n "${PROOF_ID:-}" ]; then
    RECEIPT_RESP=$(curl -sf "$REGISTRY_URL/proofs/$PROOF_ID/receipt" \
        $(auth_header) \
        2>/dev/null) || RECEIPT_RESP=""

    if [ -z "$RECEIPT_RESP" ]; then
        fail "Get receipt (no response)"
    else
        RECEIPT_ID=$(echo "$RECEIPT_RESP" | json_field "proof_id") || RECEIPT_ID=""
        SIGNATURE=$(echo "$RECEIPT_RESP" | json_field "signature") || SIGNATURE=""

        if [ -n "$RECEIPT_ID" ]; then
            pass "Get receipt (proof_id=$RECEIPT_ID, signed=$([ -n "$SIGNATURE" ] && echo yes || echo no))"
        else
            fail "Get receipt (unexpected response: $(echo "$RECEIPT_RESP" | head -c 200))"
        fi
    fi
else
    fail "Get receipt (skipped: no proof ID from submit step)"
fi

# ---- Step 5: Check transparency log ----

echo ""
echo "--- Step 5: Check transparency log ---"

TLOG_ROOT=$(curl -sf "$REGISTRY_URL/transparency/root" \
    $(auth_header) \
    2>/dev/null) || TLOG_ROOT=""

if [ -z "$TLOG_ROOT" ]; then
    fail "Transparency log root (no response)"
else
    TREE_SIZE=$(echo "$TLOG_ROOT" | json_field "tree_size") || TREE_SIZE=""
    ROOT_HASH=$(echo "$TLOG_ROOT" | json_field "root") || ROOT_HASH=""

    if [ -n "$TREE_SIZE" ] && [ "$TREE_SIZE" -gt 0 ] 2>/dev/null; then
        pass "Transparency log root (tree_size=$TREE_SIZE)"
    else
        fail "Transparency log root (tree_size=$TREE_SIZE, root=$ROOT_HASH)"
    fi
fi

# Also check inclusion proof if we have a tlog index.
if [ -n "${TLOG_INDEX:-}" ] && [ "$TLOG_INDEX" != "None" ] && [ "$TLOG_INDEX" != "" ]; then
    INCLUSION_RESP=$(curl -sf "$REGISTRY_URL/transparency/proof/$TLOG_INDEX" \
        $(auth_header) \
        2>/dev/null) || INCLUSION_RESP=""

    if [ -z "$INCLUSION_RESP" ]; then
        fail "Transparency inclusion proof (no response)"
    else
        INC_INDEX=$(echo "$INCLUSION_RESP" | json_field "index") || INC_INDEX=""
        if [ "$INC_INDEX" = "$TLOG_INDEX" ]; then
            pass "Transparency inclusion proof (index=$INC_INDEX)"
        else
            fail "Transparency inclusion proof (expected index=$TLOG_INDEX, got=$INC_INDEX)"
        fi
    fi
fi

# ---- Summary ----

echo ""
echo "==========================================="
echo "  Results: $PASS passed, $FAIL failed"
echo "==========================================="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
