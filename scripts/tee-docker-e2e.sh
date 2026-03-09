#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TEE Docker E2E Integration Test
#
# Full pipeline: Docker container → XGBoost inference → ECDSA sign →
#   submit on-chain → fast-forward → finalize → verify isResultValid
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"

ANVIL_PORT=8553
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"
CONTAINER_NAME="tee-docker-e2e-$$"
DOCKER_IMAGE="tee-enclave-e2e"

# Anvil account #0 — admin + submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Anvil account #1 — enclave signer
ENCLAVE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"

PROVER_STAKE="0.1ether"

log() { echo "==> $*"; }
ok()  { echo "  ✓ $*"; }
err() { echo "  ✗ $*" >&2; }

cleanup() {
    docker stop "$CONTAINER_NAME" 2>/dev/null || true
    docker rm "$CONTAINER_NAME" 2>/dev/null || true
    if [ -n "${ANVIL_PID:-}" ]; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# compute_result_id <sender> <modelHash> <inputHash> <blockNumber>
compute_result_id() {
    local sender="${1#0x}"
    local model_hash="${2#0x}"
    local input_hash="${3#0x}"
    local block_num="$4"
    local block_hex
    block_hex=$(printf '%064x' "$block_num")
    local packed="0x${sender}${model_hash}${input_hash}${block_hex}"
    cast keccak "$packed"
}

# ── Step 1: Build Docker image ────────────────────────────────────────────────

log "Step 1: Building Docker image..."
docker build -t "$DOCKER_IMAGE" "$ROOT_DIR/tee/" > /dev/null 2>&1
ok "Docker image built: $DOCKER_IMAGE"

# ── Step 2: Start Anvil ──────────────────────────────────────────────────────

log "Step 2: Starting Anvil on port $ANVIL_PORT..."
anvil --port "$ANVIL_PORT" --silent &
ANVIL_PID=$!
sleep 2

if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil failed to start on port $ANVIL_PORT"
    exit 1
fi
ok "Anvil running (PID: $ANVIL_PID)"

# ── Step 3: Deploy TEEMLVerifier ──────────────────────────────────────────────

log "Step 3: Deploying TEEMLVerifier..."
cd "$CONTRACTS_DIR"

TEE_VERIFIER=$(forge create src/tee/TEEMLVerifier.sol:TEEMLVerifier \
    --broadcast --json \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_KEY" \
    --constructor-args "$ADMIN_ADDR" "0x0000000000000000000000000000000000000000" \
    2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['deployedTo'])")

if [ -z "$TEE_VERIFIER" ]; then
    err "Failed to deploy TEEMLVerifier"
    exit 1
fi
ok "TEEMLVerifier deployed at: $TEE_VERIFIER"

cd "$ROOT_DIR"

# ── Step 4: Start Docker container ────────────────────────────────────────────

log "Step 4: Starting TEE enclave container..."
docker run -d --name "$CONTAINER_NAME" \
    -p 8080:8080 \
    --add-host=host.docker.internal:host-gateway \
    -e ENCLAVE_PRIVATE_KEY="$ENCLAVE_KEY" \
    "$DOCKER_IMAGE" > /dev/null

# Wait for container to be ready
for i in $(seq 1 15); do
    if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq 15 ]; then
        err "Container failed to start within 15 seconds"
        docker logs "$CONTAINER_NAME" 2>&1 | tail -20
        exit 1
    fi
    sleep 1
done
ok "Container running: $CONTAINER_NAME"

# ── Step 5: Health check + get enclave info ───────────────────────────────────

log "Step 5: Verifying enclave info..."
HEALTH=$(curl -sf http://localhost:8080/health)
ok "Health: $HEALTH"

INFO=$(curl -sf http://localhost:8080/info)
ENCLAVE_ADDR=$(echo "$INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['enclave_address'])")
ok "Enclave address: $ENCLAVE_ADDR"

# ── Step 6: Register enclave on-chain ─────────────────────────────────────────

log "Step 6: Registering enclave on-chain..."
IMAGE_HASH=$(cast keccak "docker-enclave-v1")

cast send "$TEE_VERIFIER" \
    "registerEnclave(address,bytes32)" "$ENCLAVE_ADDR" "$IMAGE_HASH" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Enclave registered"

# ── Step 7: Call inference ────────────────────────────────────────────────────

log "Step 7: Running inference via Docker container..."
RESPONSE=$(curl -sf -X POST http://localhost:8080/infer \
    -H "Content-Type: application/json" \
    -d '{"features": [5.0, 3.5, 1.5, 0.3]}')

MODEL_HASH=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['model_hash'])")
INPUT_HASH=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['input_hash'])")
RESULT_DATA=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['result'])")
ATTESTATION=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['attestation'])")

ok "Inference complete: result=$RESULT_DATA"
ok "Model hash: $MODEL_HASH"
ok "Input hash: $INPUT_HASH"

# ── Step 8: Submit result on-chain ────────────────────────────────────────────

log "Step 8: Submitting TEE-attested result on-chain..."
cast send "$TEE_VERIFIER" \
    "submitResult(bytes32,bytes32,bytes,bytes)" \
    "$MODEL_HASH" "$INPUT_HASH" "$RESULT_DATA" "$ATTESTATION" \
    --value "$PROVER_STAKE" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Result submitted (stake: $PROVER_STAKE)"

# Compute resultId
BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL")
RESULT_ID=$(compute_result_id "$ADMIN_ADDR" "$MODEL_HASH" "$INPUT_HASH" "$BLOCK_NUM")
ok "Result ID: $RESULT_ID"

# ── Step 9: Fast-forward past challenge window ────────────────────────────────

log "Step 9: Fast-forwarding past 1-hour challenge window..."
cast rpc anvil_increaseTime 3601 --rpc-url "$RPC_URL" > /dev/null
cast rpc anvil_mine 1 --rpc-url "$RPC_URL" > /dev/null
ok "Time advanced by 3601 seconds"

# ── Step 10: Finalize ─────────────────────────────────────────────────────────

log "Step 10: Finalizing result..."
cast send "$TEE_VERIFIER" "finalize(bytes32)" "$RESULT_ID" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Result finalized"

# ── Step 11: Verify ──────────────────────────────────────────────────────────

log "Step 11: Verifying result is valid..."
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
if [ "$IS_VALID" != "true" ]; then
    err "Expected isResultValid=true after finalize, got: $IS_VALID"
    exit 1
fi
ok "Result valid: true"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  TEE Docker E2E Integration Test — PASSED"
echo "════════════════════════════════════════════════════════════"
echo "  Contract:        $TEE_VERIFIER"
echo "  Enclave (Docker): $ENCLAVE_ADDR"
echo "  Result ID:       $RESULT_ID"
echo "  Flow: Docker → infer → sign → submit → finalize → valid"
echo "════════════════════════════════════════════════════════════"
