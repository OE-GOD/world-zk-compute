#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# BatchVerifier SDK E2E Test
#
# End-to-end test for the TypeScript BatchVerifier SDK:
# 1. Start Anvil with code-size-limit disabled
# 2. Deploy RemainderVerifier + register DAG circuit via Forge
# 3. Run TypeScript BatchVerifier SDK against the deployed contract
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"
SDK_DIR="$ROOT_DIR/sdk/typescript"
FIXTURE_PATH="$CONTRACTS_DIR/test/fixtures/phase1a_dag_fixture.json"

ANVIL_PORT=8548
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

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

# ── Step 1: Start Anvil ──────────────────────────────────────────────────────
log "Step 1: Starting Anvil on port $ANVIL_PORT..."
anvil --port "$ANVIL_PORT" --silent --disable-code-size-limit --gas-limit 500000000 &
ANVIL_PID=$!
sleep 2

# Verify Anvil is up
if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil failed to start on port $ANVIL_PORT"
    exit 1
fi
ok "Anvil running on port $ANVIL_PORT (PID: $ANVIL_PID)"

# ── Step 2: Deploy & Register ────────────────────────────────────────────────
log "Step 2: Deploying RemainderVerifier and registering DAG circuit..."
cd "$CONTRACTS_DIR"

DEPLOYER_KEY="$DEPLOYER_KEY" \
forge script script/DeployRemainderDAG.s.sol:DeployRemainderDAG \
    --rpc-url "$RPC_URL" \
    --broadcast \
    --disable-code-size-limit \
    -vvv 2>&1

# Extract deployed contract address from broadcast
CONTRACT_ADDR=$(python3 -c "
import json, sys
data = json.load(open('broadcast/DeployRemainderDAG.s.sol/31337/run-latest.json'))
for tx in data['transactions']:
    if tx.get('transactionType') == 'CREATE':
        print(tx['contractAddress'])
        sys.exit(0)
print('NOT_FOUND', file=sys.stderr)
sys.exit(1)
")

if [ -z "$CONTRACT_ADDR" ]; then
    err "Failed to extract contract address from broadcast"
    exit 1
fi
ok "Contract deployed at: $CONTRACT_ADDR"

# Validate registration
CIRCUIT_HASH=$(python3 -c "
import json
d = json.load(open('test/fixtures/phase1a_dag_fixture.json'))
print(d['circuit_hash_raw'])
")

IS_ACTIVE=$(cast call "$CONTRACT_ADDR" "isDAGCircuitActive(bytes32)(bool)" "$CIRCUIT_HASH" --rpc-url "$RPC_URL")
if [ "$IS_ACTIVE" != "true" ]; then
    err "Circuit not registered as active"
    exit 1
fi
ok "DAG circuit is active on-chain"

# ── Step 3: Run TypeScript BatchVerifier ──────────────────────────────────────
log "Step 3: Running TypeScript BatchVerifier SDK..."
cd "$SDK_DIR"

npm install --silent 2>&1

OUTPUT=$(npx tsx src/batch-verifier.ts \
    --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    --contract "$CONTRACT_ADDR" \
    --fixture "$FIXTURE_PATH" 2>&1) || {
    err "BatchVerifier SDK failed"
    echo "$OUTPUT"
    exit 1
}

echo "$OUTPUT"

# ── Step 4: Validate output ──────────────────────────────────────────────────
log "Step 4: Validating results..."

if echo "$OUTPUT" | grep -q "Batch Verification Complete"; then
    ok "Batch verification completed successfully"
else
    err "Output missing 'Batch Verification Complete'"
    exit 1
fi

# Count transactions from output
TX_COUNT=$(echo "$OUTPUT" | grep -c '^\[' || true)
if [ "$TX_COUNT" -lt 10 ]; then
    err "Expected at least 10 transactions, got $TX_COUNT"
    exit 1
fi
ok "Completed $TX_COUNT transaction steps"

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════════════════════"
echo "  BatchVerifier SDK E2E Test PASSED"
echo "════════════════════════════════════════════════════════════"
echo "  Contract:     $CONTRACT_ADDR"
echo "  Transactions: $TX_COUNT steps"
echo "  Status:       All phases completed successfully"
echo "════════════════════════════════════════════════════════════"
