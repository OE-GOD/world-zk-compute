#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TEE Happy Path E2E (with security feature tests)
#
# Deploy TEEMLVerifier → verify admin → test pause/unpause → test input
# validation → test admin transfer → register enclave →
# submit TEE-attested result → fast-forward time → finalize →
# verify isResultValid == true
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

# Sign a TEE attestation the way TEEMLVerifier.submitResult checks it (EIP-712):
#   structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, keccak256(result)))
#   digest     = keccak256("\x19\x01" || DOMAIN_SEPARATOR || structHash)
# The domain separator binds the signature to this chain and this verifier, so it is
# read from the deployed contract.
# sign_attestation <modelHash> <inputHash> <resultData> <privateKey> <teeVerifierAddress>
sign_attestation() {
    local model_hash="$1"
    local input_hash="$2"
    local result_data="$3"
    local private_key="$4"
    local verifier="$5"

    local result_hash typehash domain struct_hash digest
    result_hash=$(cast keccak "$result_data")
    typehash=$(cast call --rpc-url "$RPC_URL" "$verifier" "RESULT_TYPEHASH()(bytes32)")
    domain=$(cast call --rpc-url "$RPC_URL" "$verifier" "DOMAIN_SEPARATOR()(bytes32)")
    struct_hash=$(cast keccak "$(cast abi-encode "f(bytes32,bytes32,bytes32,bytes32)" \
        "$typehash" "$model_hash" "$input_hash" "$result_hash")")
    digest=$(cast keccak "0x1901${domain#0x}${struct_hash#0x}")

    cast wallet sign --no-hash --private-key "$private_key" "$digest"
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

# TEEMLVerifier.initialize rejects address(0). This flow never resolves a ZK dispute,
# so a non-zero placeholder is enough unless a real RemainderVerifier is supplied.
REMAINDER_VERIFIER="${REMAINDER_VERIFIER:-0xDDdDddDdDdddDDddDDddDDDDdDdDDdDDdDDDDDDd}"

# TEEMLVerifier is UUPS-upgradeable: deploy the implementation behind a proxy and
# initialize it through the proxy (the implementation itself cannot be initialized).
TEE_VERIFIER=$(DEPLOYER_KEY="$ADMIN_KEY" REMAINDER_VERIFIER="$REMAINDER_VERIFIER" \
    forge script script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
    --rpc-url "$RPC_URL" --broadcast 2>/dev/null \
    | grep "TEEMLVerifier deployed at:" | head -1 | awk '{print $NF}')

if [ -z "$TEE_VERIFIER" ]; then
    err "Failed to deploy TEEMLVerifier"
    exit 1
fi
ok "TEEMLVerifier deployed at: $TEE_VERIFIER"

# Define test variables early (needed for pause test in Step 2b)
MODEL_HASH=$(cast keccak "xgboost-model-weights")
INPUT_HASH=$(cast keccak "test-input-data")
RESULT_DATA="0xdeadbeef"

# ── Step 2a: Verify admin ────────────────────────────────────────────────────

log "Step 2a: Verifying contract admin..."
ADMIN_ON_CHAIN=$(cast call "$TEE_VERIFIER" "admin()(address)" --rpc-url "$RPC_URL")
# cast returns lowercase, ADMIN_ADDR has mixed case — compare lowercase
ADMIN_LOWER=$(echo "$ADMIN_ADDR" | tr '[:upper:]' '[:lower:]')
ADMIN_ON_CHAIN_LOWER=$(echo "$ADMIN_ON_CHAIN" | tr '[:upper:]' '[:lower:]')
if [ "$ADMIN_ON_CHAIN_LOWER" != "$ADMIN_LOWER" ]; then
    err "Expected admin=$ADMIN_ADDR, got $ADMIN_ON_CHAIN"
    exit 1
fi
ok "Admin verified: $ADMIN_ON_CHAIN"

# ── Step 2b: Test pause/unpause ──────────────────────────────────────────────

log "Step 2b: Testing pause/unpause..."
cast send "$TEE_VERIFIER" "pause()" --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Contract paused"

# submitResult should revert while paused
PAUSE_RESULT=$(cast send "$TEE_VERIFIER" \
    "submitResult(bytes32,bytes32,bytes,bytes)" \
    "$MODEL_HASH" "$INPUT_HASH" "0xdeadbeef" "0x00" \
    --value "0.1ether" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" 2>&1 || true)

if echo "$PAUSE_RESULT" | grep -qi "pause\|revert"; then
    ok "submitResult correctly reverted while paused"
else
    err "submitResult should have reverted while paused"
    exit 1
fi

cast send "$TEE_VERIFIER" "unpause()" --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Contract unpaused"

# ── Step 2c: Test input validation ───────────────────────────────────────────

log "Step 2c: Testing input validation..."
BOND_RESULT=$(cast send "$TEE_VERIFIER" "setChallengeBondAmount(uint256)" 0 \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" 2>&1 || true)
if echo "$BOND_RESULT" | grep -qi "zero amount\|revert"; then
    ok "setChallengeBondAmount(0) correctly reverted"
else
    err "setChallengeBondAmount(0) should have reverted"
    exit 1
fi

# ── Step 2d: Test admin transfer ─────────────────────────────────────────────

log "Step 2d: Testing admin transfer..."
# Anvil account #2
NEW_ADMIN_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
NEW_ADMIN_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"

cast send "$TEE_VERIFIER" "changeAdmin(address)" "$NEW_ADMIN_ADDR" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

ADMIN_ON_CHAIN=$(cast call "$TEE_VERIFIER" "admin()(address)" --rpc-url "$RPC_URL")
ADMIN_ON_CHAIN_LOWER=$(echo "$ADMIN_ON_CHAIN" | tr '[:upper:]' '[:lower:]')
NEW_ADMIN_LOWER=$(echo "$NEW_ADMIN_ADDR" | tr '[:upper:]' '[:lower:]')
if [ "$ADMIN_ON_CHAIN_LOWER" != "$NEW_ADMIN_LOWER" ]; then
    err "Expected new admin=$NEW_ADMIN_ADDR, got $ADMIN_ON_CHAIN"
    exit 1
fi
ok "New admin verified: $ADMIN_ON_CHAIN"

# Transfer back to the original admin for the remaining test steps
cast send "$TEE_VERIFIER" "changeAdmin(address)" "$ADMIN_ADDR" \
    --rpc-url "$RPC_URL" --private-key "$NEW_ADMIN_KEY" > /dev/null
ok "Admin transferred back to $ADMIN_ADDR"

# ── Step 3: Register enclave ─────────────────────────────────────────────────

log "Step 3: Registering test enclave..."
IMAGE_HASH=$(cast keccak "test-enclave-image-v1")

cast send "$TEE_VERIFIER" \
    "registerEnclave(address,bytes32)" "$ENCLAVE_ADDR" "$IMAGE_HASH" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

ok "Enclave registered: $ENCLAVE_ADDR"

# ── Step 4: Submit TEE-attested result ────────────────────────────────────────

log "Step 4: Submitting TEE-attested result..."

ATTESTATION=$(sign_attestation "$MODEL_HASH" "$INPUT_HASH" "$RESULT_DATA" "$ENCLAVE_KEY" "$TEE_VERIFIER")
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
echo "  TEE Happy Path E2E (with security tests) — PASSED"
echo "════════════════════════════════════════════════════════════"
echo "  Contract:  $TEE_VERIFIER"
echo "  Result ID: $RESULT_ID"
echo "  Tested:    admin, pause/unpause, input validation,"
echo "             admin transfer, submit, finalize, valid"
echo "════════════════════════════════════════════════════════════"
