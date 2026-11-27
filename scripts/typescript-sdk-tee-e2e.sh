#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TypeScript SDK -- TEE Verifier E2E Test
#
# Tests the TypeScript SDK TEEVerifier against a live Anvil instance:
#   1. Start Anvil
#   2. Deploy TEEMLVerifier behind its UUPS proxy (forge script)
#   3. Install TypeScript SDK
#   4. Run TypeScript E2E covering:
#      - admin() read
#      - pause / unpause / paused()
#      - registerEnclave
#      - submitResult (with EIP-712 attestation signed in TypeScript)
#      - getResult
#      - isResultValid (before and after finalize)
#      - fast-forward past challenge window + finalize
#      - admin transfer (changeAdmin)
#      - revokeEnclave
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"
SDK_DIR="$ROOT_DIR/sdk/typescript"

ANVIL_PORT=8554
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

# Anvil account #0 -- admin / submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

log() { echo "==> $*"; }
ok()  { echo "  [OK] $*"; }
err() { echo "  [FAIL] $*" >&2; }

cleanup() {
    if [ -n "${ANVIL_PID:-}" ]; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

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
if ! DEPLOY_OUTPUT=$(DEPLOYER_KEY="$ADMIN_KEY" REMAINDER_VERIFIER="$REMAINDER_VERIFIER" \
    forge script script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
    --rpc-url "$RPC_URL" --broadcast 2>&1); then
    err "Failed to deploy TEEMLVerifier"
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
fi

# The script prints the proxy address; the broadcast entry named "TEEMLVerifier" is the
# implementation, which must not be used directly.
TEE_VERIFIER=$(awk '/TEEMLVerifier deployed at:/ { print $NF; exit }' <<< "$DEPLOY_OUTPUT")

if [ -z "$TEE_VERIFIER" ]; then
    err "TEEMLVerifier proxy address not found in deploy output"
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
fi
ok "TEEMLVerifier deployed at: $TEE_VERIFIER"

# ── Step 3: Install TypeScript SDK ────────────────────────────────────────────

log "Step 3: Installing TypeScript SDK..."
cd "$SDK_DIR"
npm install --silent 2>/dev/null
ok "TypeScript SDK installed"

# ── Step 4: Run TypeScript E2E tests ──────────────────────────────────────────

log "Step 4: Running TypeScript SDK TEE E2E tests..."

if ! npx tsx src/tee-e2e.ts \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_KEY" \
    --contract "$TEE_VERIFIER"; then
    err "TypeScript SDK TEE E2E tests FAILED"
    exit 1
fi
ok "TypeScript SDK TEE E2E tests passed"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "================================================================"
echo "  TypeScript SDK TEE Verifier E2E -- PASSED"
echo "================================================================"
echo "  Contract:  $TEE_VERIFIER"
echo "  Tested:    admin, paused, pause/unpause, registerEnclave,"
echo "             submitResult, getResult, isResultValid, finalize,"
echo "             admin transfer, revokeEnclave"
echo "================================================================"
