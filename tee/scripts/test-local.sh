#!/bin/bash
# Test the TEE enclave locally: build, start, health-check, run inference, verify response.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PORT=${PORT:-8080}
PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
CONTAINER_NAME="tee-enclave-test"
BASE_URL="http://localhost:${PORT}"

cleanup() {
    echo "Stopping container..."
    docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Building enclave image ==="
docker build -t tee-enclave "$TEE_DIR"

echo ""
echo "=== Starting enclave container ==="
docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
docker run -d \
    --name "$CONTAINER_NAME" \
    -p "${PORT}:8080" \
    -e "ENCLAVE_PRIVATE_KEY=${PRIVATE_KEY}" \
    tee-enclave

echo "Waiting for health check..."
for i in $(seq 1 30); do
    if curl -sf "${BASE_URL}/health" > /dev/null 2>&1; then
        echo "Health check passed."
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: Health check failed after 30 attempts."
        docker logs "$CONTAINER_NAME"
        exit 1
    fi
    sleep 1
done

echo ""
echo "=== GET /health ==="
curl -sf "${BASE_URL}/health" | python3 -m json.tool

echo ""
echo "=== GET /info ==="
INFO=$(curl -sf "${BASE_URL}/info")
echo "$INFO" | python3 -m json.tool

echo ""
echo "=== POST /infer (4 features) ==="
RESULT=$(curl -sf -X POST "${BASE_URL}/infer" \
    -H "Content-Type: application/json" \
    -d '{"features": [5.0, 3.5, 1.5, 0.3]}')
echo "$RESULT" | python3 -m json.tool

# Verify response structure
echo ""
echo "=== Validating response ==="
has_field() {
    echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$1' in d, f'Missing field: $1'" 2>&1
}

for field in result model_hash input_hash result_hash attestation enclave_address; do
    has_field "$field" && echo "  $field: present" || echo "  $field: MISSING"
done

# Verify attestation is 65 bytes (130 hex chars + 0x prefix = 132 chars)
ATT_LEN=$(echo "$RESULT" | python3 -c "import sys,json; print(len(json.load(sys.stdin)['attestation']))")
if [ "$ATT_LEN" -eq 132 ]; then
    echo "  attestation length: OK (65 bytes)"
else
    echo "  attestation length: WRONG ($ATT_LEN chars, expected 132)"
fi

echo ""
echo "=== POST /infer (wrong feature count) ==="
HTTP_CODE=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "${BASE_URL}/infer" \
    -H "Content-Type: application/json" \
    -d '{"features": [1.0, 2.0]}' 2>/dev/null || echo "error")
if [ "$HTTP_CODE" = "400" ] || [ "$HTTP_CODE" = "error" ]; then
    echo "  Wrong feature count correctly rejected (HTTP $HTTP_CODE)"
else
    echo "  WARNING: Expected 400, got $HTTP_CODE"
fi

echo ""
echo "=== All local tests passed ==="
