#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TEE Happy Path E2E
#
# Deploy TEEMLVerifier → register enclave → submit TEE-attested result
# → fast-forward time → finalize → verify isResultValid == true
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"

ANVIL_PORT=8551
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

# Anvil account #0 — admin + submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Anvil account #1 — enclave signer
ENCLAVE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
ENCLAVE_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"

PROVER_STAKE="0.1ether"

log() { echo "==> $*"; }
ok()  { echo "  ✓ $*"; }
err() { echo "  ✗ $*" >&2; }

cleanup() {
    if [ -n "${ANVIL_PID:-}" ]; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Helpers ───────────────────────────────────────────────────────────────────

# sign_attestation <modelHash> <inputHash> <resultData> <privateKey>
# Mirrors the contract: message = keccak256(encodePacked(modelHash, inputHash, keccak256(result)))
# cast wallet sign adds the EIP-191 "\x19Ethereum Signed Message:\n32" prefix automatically.
sign_attestation() {
    local model_hash="${1#0x}"
    local input_hash="${2#0x}"
    local result_data="$3"
    local private_key="$4"

    local result_hash
    result_hash=$(cast keccak "$result_data")
    result_hash="${result_hash#0x}"

    local packed="0x${model_hash}${input_hash}${result_hash}"
    local message
    message=$(cast keccak "$packed")

    cast wallet sign --private-key "$private_key" "$message"
}

# compute_result_id <sender> <modelHash> <inputHash> <blockNumber>
# resultId = keccak256(abi.encodePacked(sender, modelHash, inputHash, blockNumber))
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

# ── Step 1: Start Anvil ──────────────────────────────────────────────────────

log "Step 1: Starting Anvil on port $ANVIL_PORT..."
anvil --port "$ANVIL_PORT" --silent &
ANVIL_PID=$!
sleep 2

if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil failed to start on port $ANVIL_PORT"
    exit 1
fi
ok "Anvil running (PID: $ANVIL_PID)"

# ── Step 2: Deploy TEEMLVerifier ──────────────────────────────────────────────

log "Step 2: Deploying TEEMLVerifier..."
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

# ── Step 3: Register enclave ─────────────────────────────────────────────────

log "Step 3: Registering test enclave..."
IMAGE_HASH=$(cast keccak "test-enclave-image-v1")

cast send "$TEE_VERIFIER" \
    "registerEnclave(address,bytes32)" "$ENCLAVE_ADDR" "$IMAGE_HASH" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

ok "Enclave registered: $ENCLAVE_ADDR"

# ── Step 4: Submit TEE-attested result ────────────────────────────────────────

log "Step 4: Submitting TEE-attested result..."

MODEL_HASH=$(cast keccak "xgboost-model-weights")
INPUT_HASH=$(cast keccak "test-input-data")
RESULT_DATA="0xdeadbeef"

ATTESTATION=$(sign_attestation "$MODEL_HASH" "$INPUT_HASH" "$RESULT_DATA" "$ENCLAVE_KEY")
ok "Attestation signed"

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

# Verify not yet valid
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
if [ "$IS_VALID" != "false" ]; then
    err "Expected isResultValid=false during challenge window"
    exit 1
fi
ok "Result not yet valid (challenge window open)"

# ── Step 5: Fast-forward past challenge window ────────────────────────────────

log "Step 5: Fast-forwarding past 1-hour challenge window..."
cast rpc anvil_increaseTime 3601 --rpc-url "$RPC_URL" > /dev/null
cast rpc anvil_mine 1 --rpc-url "$RPC_URL" > /dev/null
ok "Time advanced by 3601 seconds"

# ── Step 6: Finalize ─────────────────────────────────────────────────────────

log "Step 6: Finalizing result..."
cast send "$TEE_VERIFIER" "finalize(bytes32)" "$RESULT_ID" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Result finalized"

# ── Step 7: Verify ────────────────────────────────────────────────────────────

log "Step 7: Verifying result is valid..."
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
if [ "$IS_VALID" != "true" ]; then
    err "Expected isResultValid=true after finalize, got: $IS_VALID"
    exit 1
fi
ok "Result valid: true"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  TEE Happy Path E2E — PASSED"
echo "════════════════════════════════════════════════════════════"
echo "  Contract:  $TEE_VERIFIER"
echo "  Result ID: $RESULT_ID"
echo "  Flow:      submit → wait → finalize → valid ✓"
echo "════════════════════════════════════════════════════════════"
