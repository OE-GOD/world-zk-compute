#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# Rust SDK E2E Test — Batch Verification
#
# End-to-end test for the Rust SDK DAGVerifier::verify_batch():
# 1. Start Anvil with high gas limit + code-size-limit disabled
# 2. Deploy RemainderVerifier + register DAG circuit via Forge
# 3. Run Rust integration test against the deployed contract
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"
SDK_DIR="$ROOT_DIR/sdk"

ANVIL_PORT=8549
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

# ── Step 3: Run Rust SDK Batch Verification ──────────────────────────────────
log "Step 3: Running Rust SDK batch verification..."
cd "$SDK_DIR"

RPC_URL="$RPC_URL" CONTRACT_ADDRESS="$CONTRACT_ADDR" \
    cargo test --test e2e_batch_verify -- --ignored --nocapture 2>&1 || {
    err "Rust SDK batch verification test failed"
    exit 1
}

ok "Rust SDK batch verification passed"

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════════════════════"
echo "  Rust SDK E2E Test PASSED"
echo "════════════════════════════════════════════════════════════"
echo "  Contract:  $CONTRACT_ADDR"
echo "  Port:      $ANVIL_PORT"
echo "  Status:    Batch verification completed successfully"
echo "════════════════════════════════════════════════════════════"
