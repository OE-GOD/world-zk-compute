#!/usr/bin/env bash
# Testnet E2E: verify a DAG proof on a deployed RemainderVerifier.
#
# Required env vars:
#   DEPLOYER_KEY           -- private key for sending the verification tx
#
# One of the following (to locate the verifier):
#   REMAINDER_VERIFIER     -- explicit address of the RemainderVerifier (proxy)
#   DEPLOYMENT_FILE        -- path to deployment JSON (from deploy-testnet.sh)
#
# Optional env vars:
#   RPC_URL                -- RPC URL (default: https://sepolia-rollup.arbitrum.io/rpc)
#   FIXTURE_PATH           -- fixture file (default: test/fixtures/phase1a_dag_fixture.json)
#   VERIFY_MODE            -- "direct" (default) or "batch"
#   REGISTER_IF_NEEDED     -- "true" to auto-register the circuit if not registered
#   GAS_LIMIT              -- gas limit for Anvil (default: 500000000)
#   CODE_SIZE_LIMIT        -- code size limit for forge (default: 200000)
#
# Usage:
#   # Against a known deployed verifier
#   DEPLOYER_KEY=0x... REMAINDER_VERIFIER=0x... RPC_URL=http://127.0.0.1:8545 \
#     bash scripts/testnet-e2e.sh
#
#   # Using deployment file from deploy-testnet.sh
#   DEPLOYER_KEY=0x... DEPLOYMENT_FILE=deployments/anvil-local-testnet.json \
#     RPC_URL=http://127.0.0.1:8545 bash scripts/testnet-e2e.sh
#
#   # Batch verification mode
#   DEPLOYER_KEY=0x... REMAINDER_VERIFIER=0x... VERIFY_MODE=batch \
#     RPC_URL=http://127.0.0.1:8545 bash scripts/testnet-e2e.sh
#
#   # Full local flow (deploy + verify):
#   # 1. Start Anvil:    anvil --gas-limit 500000000 --code-size-limit 200000
#   # 2. Deploy:         DEPLOYER_PRIVATE_KEY=0xac0974... ARBITRUM_SEPOLIA_RPC_URL=http://127.0.0.1:8545 bash scripts/deploy-testnet.sh
#   # 3. Verify:         DEPLOYER_KEY=0xac0974... DEPLOYMENT_FILE=deployments/anvil-local-testnet.json RPC_URL=http://127.0.0.1:8545 bash scripts/testnet-e2e.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"

# ── Validate required env vars ────────────────────────────────────────────

: "${DEPLOYER_KEY:?Set DEPLOYER_KEY (private key for sending txs)}"

RPC_URL="${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}"
VERIFY_MODE="${VERIFY_MODE:-direct}"
GAS_LIMIT="${GAS_LIMIT:-500000000}"
CODE_SIZE_LIMIT="${CODE_SIZE_LIMIT:-200000}"

echo "=== Testnet E2E Verification ==="
echo ""

# ── Resolve REMAINDER_VERIFIER address ────────────────────────────────────

if [ -z "${REMAINDER_VERIFIER:-}" ]; then
    # Try to read from deployment file
    DEPLOYMENT_FILE="${DEPLOYMENT_FILE:-}"
    if [ -z "$DEPLOYMENT_FILE" ]; then
        echo "ERROR: Set REMAINDER_VERIFIER or DEPLOYMENT_FILE"
        echo ""
        echo "  REMAINDER_VERIFIER=0x... -- explicit verifier address"
        echo "  DEPLOYMENT_FILE=path.json -- from deploy-testnet.sh output"
        exit 1
    fi

    if [ ! -f "$DEPLOYMENT_FILE" ]; then
        echo "ERROR: Deployment file not found: $DEPLOYMENT_FILE"
        exit 1
    fi

    REMAINDER_VERIFIER=$(jq -r '.contracts.RemainderVerifier // empty' "$DEPLOYMENT_FILE")
    if [ -z "$REMAINDER_VERIFIER" ]; then
        echo "ERROR: No RemainderVerifier address in $DEPLOYMENT_FILE"
        exit 1
    fi

    NETWORK=$(jq -r '.network // "unknown"' "$DEPLOYMENT_FILE")
    echo "Loaded from deployment: $DEPLOYMENT_FILE"
    echo "  Network: $NETWORK"
fi

echo "  Verifier:  $REMAINDER_VERIFIER"
echo "  RPC:       $RPC_URL"
echo "  Mode:      $VERIFY_MODE"
echo ""

# ── Pre-flight: verify contract has code ──────────────────────────────────

echo "Checking contract..."
CODE=$(cast code "$REMAINDER_VERIFIER" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")
if [ "$CODE" = "0x" ] || [ -z "$CODE" ]; then
    echo "ERROR: No contract deployed at $REMAINDER_VERIFIER"
    echo "  Did you run deploy-testnet.sh first?"
    exit 1
fi
CODE_LEN=${#CODE}
echo "  Contract found: $((CODE_LEN / 2 - 1)) bytes of bytecode"
echo ""

# ── Run verification via forge script ─────────────────────────────────────

cd "$CONTRACTS_DIR"

FORGE_ARGS=(
    script script/VerifyOnChain.s.sol:VerifyOnChain
    --rpc-url "$RPC_URL"
    --private-key "$DEPLOYER_KEY"
    --broadcast
    --code-size-limit "$CODE_SIZE_LIMIT"
    -vvv
)

# Export env vars for the forge script
export REMAINDER_VERIFIER
export DEPLOYER_KEY
export VERIFY_MODE

if [ -n "${FIXTURE_PATH:-}" ]; then
    export FIXTURE_PATH
fi

if [ "${REGISTER_IF_NEEDED:-}" = "true" ]; then
    export REGISTER_IF_NEEDED=true
fi

echo "Running forge script..."
echo "  forge ${FORGE_ARGS[*]}"
echo ""

forge "${FORGE_ARGS[@]}"

echo ""
echo "=== TESTNET E2E PASSED ==="
