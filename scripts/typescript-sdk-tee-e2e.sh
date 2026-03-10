#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TypeScript SDK -- TEE Verifier E2E Test
#
# Tests the TypeScript SDK TEEVerifier against a live Anvil instance:
#   1. Start Anvil
#   2. Deploy TEEMLVerifier via forge create
#   3. Install TypeScript SDK
#   4. Run TypeScript E2E covering:
#      - owner() read
#      - pause / unpause / paused()
#      - registerEnclave
#      - submitResult (with ECDSA attestation signed in TypeScript)
#      - getResult
#      - isResultValid (before and after finalize)
#      - fast-forward past challenge window + finalize
#      - 2-step ownership transfer
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
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

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

# ── Step 3: Install TypeScript SDK ────────────────────────────────────────────

log "Step 3: Installing TypeScript SDK..."
cd "$SDK_DIR"
npm install --silent 2>/dev/null
ok "TypeScript SDK installed"

# ── Step 4: Run TypeScript E2E tests ──────────────────────────────────────────

log "Step 4: Running TypeScript SDK TEE E2E tests..."

npx tsx src/tee-e2e.ts \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_KEY" \
    --contract "$TEE_VERIFIER"

TS_EXIT=$?
if [ "$TS_EXIT" -ne 0 ]; then
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
echo "  Tested:    owner, paused, pause/unpause, registerEnclave,"
echo "             submitResult, getResult, isResultValid, finalize,"
echo "             2-step ownership transfer, revokeEnclave"
echo "================================================================"
