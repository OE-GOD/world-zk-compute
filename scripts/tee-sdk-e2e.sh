#!/usr/bin/env bash
# =============================================================================
# TEE SDK E2E Test
#
# Starts Anvil, deploys TEEMLVerifier, and runs the Rust SDK integration tests.
#
# Usage:
#   ./scripts/tee-sdk-e2e.sh
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Anvil pre-funded account #0 (deployer/admin)
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
RPC_PORT=8549
RPC_URL="http://127.0.0.1:$RPC_PORT"
ANVIL_PID=""

cleanup() {
    if [ -n "$ANVIL_PID" ]; then
        echo "Stopping Anvil (PID $ANVIL_PID)..."
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "=== Step 1: Start Anvil ==="
anvil --port "$RPC_PORT" --gas-limit 500000000 --code-size-limit 200000 &>/dev/null &
ANVIL_PID=$!

# Wait for Anvil to be ready
for i in $(seq 1 30); do
    if cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
        echo "Anvil ready on port $RPC_PORT (PID $ANVIL_PID)"
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: Anvil failed to start"
        exit 1
    fi
    sleep 0.5
done

echo ""
echo "=== Step 2: Deploy TEEMLVerifier ==="
cd "$ROOT_DIR/contracts"

# Deploy TEEMLVerifier behind its UUPS proxy.
# TEEMLVerifier.initialize rejects address(0). This flow never resolves a ZK dispute,
# so a non-zero placeholder is enough unless a real RemainderVerifier is supplied.
REMAINDER_VERIFIER="${REMAINDER_VERIFIER:-0xDDdDddDdDdddDDddDDddDDDDdDdDDdDDdDDDDDDd}"
export REMAINDER_VERIFIER
# The deploy script reads DEPLOYER_KEY as uint via vm.envUint
export DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOY_OUTPUT=$(forge script script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
    --rpc-url "$RPC_URL" \
    --broadcast \
    --code-size-limit 200000 2>&1)

# The script prints the proxy address; the broadcast entry named "TEEMLVerifier" is the
# implementation, which must not be used directly.
TEE_CONTRACT=$(echo "$DEPLOY_OUTPUT" | grep "TEEMLVerifier deployed at:" | head -1 | awk '{print $NF}')
if [ -z "$TEE_CONTRACT" ]; then
    echo "ERROR: TEEMLVerifier proxy address not found in deploy output"
    echo "$DEPLOY_OUTPUT"
    exit 1
fi

echo "TEEMLVerifier deployed at: $TEE_CONTRACT"

echo ""
echo "=== Step 3: Run SDK E2E Tests ==="
cd "$ROOT_DIR"

RPC_URL="$RPC_URL" \
TEE_CONTRACT_ADDRESS="$TEE_CONTRACT" \
cargo test --manifest-path sdk/Cargo.toml --test e2e_tee_verify -- --ignored --nocapture --test-threads=1

echo ""
echo "=== All TEE SDK E2E tests passed! ==="
