#!/usr/bin/env bash
set -euo pipefail

# Bank E2E Off-Chain Workflow
#
# Tests the complete off-chain proof lifecycle:
# 1. Start services (gateway, verifier, registry, generator)
# 2. Register a model
# 3. Generate a proof
# 4. Submit proof to registry
# 5. Verify the proof
# 6. Get verification receipt
#
# Usage: ./scripts/bank-e2e-offchain.sh

GATEWAY_URL="${GATEWAY_URL:-http://localhost:8080}"

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    docker compose -f docker-compose.offchain.yml down 2>/dev/null || true
}
trap cleanup EXIT

cd "$(dirname "$0")/.."

echo "=== Bank E2E Off-Chain Workflow ==="
echo ""

# Step 1: Start services
echo "--- Step 1: Starting off-chain stack ---"
docker compose -f docker-compose.offchain.yml up -d
echo "  Waiting for services..."
for i in $(seq 1 30); do
    if curl -sf "$GATEWAY_URL/health" > /dev/null 2>&1; then
        echo "  Gateway ready."
        break
    fi
    [ "$i" -eq 30 ] && { echo "  TIMEOUT"; exit 1; }
    sleep 1
done

# Step 2: Register model
echo ""
echo "--- Step 2: Registering credit scoring model ---"
REGISTER=$(curl -sf -X POST "$GATEWAY_URL/models" \
    -H "Content-Type: application/json" \
    -d '{"id":"credit-v1","name":"Credit Scoring XGBoost","format":"xgboost","num_features":6}')
echo "  $REGISTER"

# Step 3: Generate proof
echo ""
echo "--- Step 3: Generating proof ---"
PROVE=$(curl -sf -X POST "$GATEWAY_URL/prove" \
    -H "Content-Type: application/json" \
    -d '{"model_id":"credit-v1","features":[720,85000,12,0.22,5,1]}')
PROOF_ID=$(echo "$PROVE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('proof_id',''))" 2>/dev/null)
echo "  Proof ID: $PROOF_ID"

# Step 4: Submit to registry
echo ""
echo "--- Step 4: Submitting to registry ---"
BUNDLE=$(echo "$PROVE" | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d.get('proof_bundle',{})))" 2>/dev/null)
SUBMIT=$(curl -sf -X POST "$GATEWAY_URL/proofs" \
    -H "Content-Type: application/json" \
    -d "$BUNDLE")
REGISTRY_ID=$(echo "$SUBMIT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
echo "  Registry ID: $REGISTRY_ID"
echo "  Content hash: $(echo "$SUBMIT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('content_hash',''))" 2>/dev/null)"

# Step 5: Verify
echo ""
echo "--- Step 5: Verifying proof ---"
VERIFY=$(curl -sf -X POST "$GATEWAY_URL/proofs/$REGISTRY_ID/verify")
echo "  $VERIFY"

# Step 6: Stats
echo ""
echo "--- Step 6: Registry stats ---"
STATS=$(curl -sf "$GATEWAY_URL/stats")
echo "  $STATS"

echo ""
echo "=== E2E Workflow Complete ==="
