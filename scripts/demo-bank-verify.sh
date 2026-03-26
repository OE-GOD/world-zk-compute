#!/usr/bin/env bash
# Bank demo: verify an XGBoost ZKML inference proof via the REST API.
#
# Prerequisites:
#   - verifier service running (docker compose -f docker-compose.verifier.yml up)
#   - OR: PORT=3000 cargo run -p zkml-verifier-service
#
# Usage:
#   ./scripts/demo-bank-verify.sh [verifier-url]
#
# This script:
#   1. Checks the verifier service health
#   2. Registers the XGBoost circuit configuration (warm endpoint)
#   3. Submits the proof for verification
#   4. Displays the result

set -euo pipefail

VERIFIER_URL="${1:-http://localhost:3000}"
FIXTURE_PATH="contracts/test/fixtures/phase1a_dag_fixture.json"

echo "=== ZKML Bank Demo: Proof Verification ==="
echo ""

# Check fixture exists
if [ ! -f "$FIXTURE_PATH" ]; then
    echo "ERROR: Fixture not found at $FIXTURE_PATH"
    echo "Run from the project root directory."
    exit 1
fi

# 1. Health check
echo "1. Checking verifier service at $VERIFIER_URL ..."
HEALTH=$(curl -sf "$VERIFIER_URL/health" 2>/dev/null) || {
    echo "   ERROR: Verifier service not reachable at $VERIFIER_URL"
    echo "   Start it with: cargo run --release -p zkml-verifier-service"
    exit 1
}
echo "   $HEALTH"
echo ""

# 2. Register circuit (warm endpoint)
echo "2. Registering XGBoost circuit config ..."
GENS_HEX=$(jq -r '.gens_hex' "$FIXTURE_PATH")
DAG_DESC=$(jq '.dag_circuit_description' "$FIXTURE_PATH")

REGISTER_RESP=$(curl -sf -X POST "$VERIFIER_URL/circuits" \
    -H "Content-Type: application/json" \
    -d "$(jq -n --arg id "xgboost-sample" --arg gens "$GENS_HEX" --argjson desc "$DAG_DESC" \
        '{circuit_id: $id, gens_hex: $gens, dag_circuit_description: $desc}')" 2>/dev/null) || {
    echo "   ERROR: Failed to register circuit"
    exit 1
}
echo "   $REGISTER_RESP"
echo ""

# 3. Verify proof using warm endpoint
echo "3. Submitting proof for verification (warm endpoint) ..."
PROOF_HEX=$(jq -r '.proof_hex' "$FIXTURE_PATH")
PUB_HEX=$(jq -r '.public_inputs_hex // ""' "$FIXTURE_PATH")

START_TIME=$(date +%s%N 2>/dev/null || date +%s)
VERIFY_RESP=$(curl -sf -X POST "$VERIFIER_URL/circuits/xgboost-sample/verify" \
    -H "Content-Type: application/json" \
    -d "$(jq -n --arg proof "$PROOF_HEX" --arg pub "$PUB_HEX" \
        '{proof_hex: $proof, public_inputs_hex: $pub}')" 2>/dev/null) || {
    echo "   ERROR: Verification request failed"
    exit 1
}
END_TIME=$(date +%s%N 2>/dev/null || date +%s)

echo "   $VERIFY_RESP"
echo ""

# 4. Parse result
VERIFIED=$(echo "$VERIFY_RESP" | jq -r '.verified')
CIRCUIT_HASH=$(echo "$VERIFY_RESP" | jq -r '.circuit_hash')

if [ "$VERIFIED" = "true" ]; then
    echo "=== RESULT: PROOF VERIFIED ==="
    echo "   Circuit hash: $CIRCUIT_HASH"
else
    echo "=== RESULT: VERIFICATION FAILED ==="
    exit 1
fi

echo ""
echo "=== Demo complete ==="
