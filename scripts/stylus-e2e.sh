#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Stylus GKR Verifier E2E Test (Full Adapter Pipeline)
# =============================================================================
#
# Deploys the Stylus GKR verifier to a local Arbitrum nitro-devnode,
# deploys RemainderVerifier (Solidity), wires them together via
# setDAGStylusVerifier(), then verifies the 88-layer XGBoost DAG proof
# through both the Stylus and pure-Solidity paths.
#
# Prerequisites:
#   - Docker running
#   - cargo-stylus installed (cargo install cargo-stylus)
#   - Foundry installed (forge, cast)
#
# Usage:
#   ./scripts/stylus-e2e.sh [--skip-devnode] [--skip-deploy] [--contract ADDRESS]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERIFIER_DIR="$PROJECT_ROOT/contracts/stylus/gkr-verifier"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"

# Nitro devnode config
DEVNODE_DIR="$PROJECT_ROOT/.nitro-devnode"
RPC_URL="http://localhost:8547"
# Pre-funded dev account on nitro-devnode
DEV_PRIVATE_KEY="0xb6b15c8cb491557369f3c7d2c287b053eb229daa9c22138887752191c9520659"
# shellcheck disable=SC2034
DEV_ADDRESS="0x3f1Eae7D46d88F08fc2F8ed27FCb2AB183EB2d0E"

# Parse args
SKIP_DEVNODE=false
SKIP_DEPLOY=false
CONTRACT_ADDRESS=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-devnode) SKIP_DEVNODE=true; shift ;;
        --skip-deploy)  SKIP_DEPLOY=true; shift ;;
        --contract)     CONTRACT_ADDRESS="$2"; SKIP_DEPLOY=true; shift 2 ;;
        *)              echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "============================================="
echo "  Stylus GKR Verifier E2E Test"
echo "============================================="
echo ""

# =============================================================================
# Phase 1: Start nitro-devnode
# =============================================================================

NITRO_NODE_VERSION="${NITRO_NODE_VERSION:-v3.7.1-926f1ab}"
NITRO_IMAGE="offchainlabs/nitro-node:${NITRO_NODE_VERSION}"
CONTAINER_NAME="nitro-stylus-e2e"
STARTED_DEVNODE=false

cleanup() {
    if [ "$STARTED_DEVNODE" = true ]; then
        echo ""
        echo "Cleaning up devnode container..."
        docker stop -t 5 "$CONTAINER_NAME" >/dev/null 2>&1 || true
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

if [ "$SKIP_DEVNODE" = false ]; then
    echo "[Phase 1] Starting Arbitrum nitro-devnode..."

    # Check if already running
    if cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
        echo "  Devnode already running at $RPC_URL. Block: $(cast block-number --rpc-url "$RPC_URL")"
    else
        # Remove stale container
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true

        echo "  Starting container ($NITRO_IMAGE)..."
        docker run --rm -d --name "$CONTAINER_NAME" \
            -p 8547:8547 \
            "$NITRO_IMAGE" \
            --dev --http.addr 0.0.0.0 --http.api=net,web3,eth,debug &>/dev/null
        STARTED_DEVNODE=true

        # Wait for RPC
        echo "  Waiting for RPC at $RPC_URL..."
        for i in $(seq 1 120); do
            if cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
                echo "  Devnode ready! Block: $(cast block-number --rpc-url "$RPC_URL")"
                break
            fi
            if [ "$i" -eq 120 ]; then
                echo "  ERROR: Devnode failed to start after 120s"
                echo "  Docker logs:"
                docker logs "$CONTAINER_NAME" --tail 20
                exit 1
            fi
            sleep 1
        done

        # Setup: become chain owner + deploy cache manager + deployer
        echo "  Setting up chain owner and deployers..."
        if [ -d "$DEVNODE_DIR" ]; then
            cd "$DEVNODE_DIR"
            # Become chain owner
            cast send 0x00000000000000000000000000000000000000FF \
                "becomeChainOwner()" \
                --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" &>/dev/null
            # Set L1 data fee to 0
            cast send --rpc-url "$RPC_URL" --private-key "$DEV_PRIVATE_KEY" \
                0x0000000000000000000000000000000000000070 \
                'setL1PricePerUnit(uint256)' 0x0 &>/dev/null

            # Deploy CREATE2 factory
            CREATE2_FACTORY=0x4e59b44847b379578588920ca78fbf26c0b4956c
            SALT=0x0000000000000000000000000000000000000000000000000000000000000000
            cast send --rpc-url "$RPC_URL" --private-key "$DEV_PRIVATE_KEY" \
                --value "1 ether" 0x3fab184622dc19b6109349b94811493bf2a45362 &>/dev/null
            cast publish --rpc-url "$RPC_URL" \
                0xf8a58085174876e800830186a08080b853604580600e600039806000f350fe7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf31ba02222222222222222222222222222222222222222222222222222222222222222a02222222222222222222222222222222222222222222222222222222222222222 &>/dev/null || true

            # Deploy Cache Manager
            CM_OUTPUT=$(cast send --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" \
                --create 0x60a06040523060805234801561001457600080fd5b50608051611d1c61003060003960006105260152611d1c6000f3fe 2>&1)
            CM_ADDR=$(echo "$CM_OUTPUT" | awk '/contractAddress/ {print $2}')
            if [ -n "$CM_ADDR" ]; then
                cast send --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" \
                    0x0000000000000000000000000000000000000070 \
                    "addWasmCacheManager(address)" "$CM_ADDR" &>/dev/null
                echo "  Cache Manager deployed: $CM_ADDR"
            fi

            # Deploy StylusDeployer
            if [ -f "./stylus-deployer-bytecode.txt" ]; then
                DEPLOYER_CODE=$(cat ./stylus-deployer-bytecode.txt)
                DEPLOYER_ADDR=$(cast create2 --salt "$SALT" --init-code "$DEPLOYER_CODE" 2>/dev/null || true)
                if [ -n "$DEPLOYER_ADDR" ]; then
                    cast send --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" \
                        "$CREATE2_FACTORY" "${SALT}${DEPLOYER_CODE}" &>/dev/null || true
                    echo "  StylusDeployer deployed: $DEPLOYER_ADDR"
                fi
            fi
        else
            echo "  WARNING: nitro-devnode dir not found, skipping deployer setup"
            echo "  Run: git clone --depth 1 https://github.com/OffchainLabs/nitro-devnode.git $DEVNODE_DIR"
        fi
    fi
else
    echo "[Phase 1] Skipping devnode startup (--skip-devnode)"
    # Verify it's running
    if ! cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
        echo "  ERROR: No devnode running at $RPC_URL"
        exit 1
    fi
    echo "  Devnode running. Block: $(cast block-number --rpc-url "$RPC_URL")"
fi

echo ""

# =============================================================================
# Phase 2: Deploy Stylus contract
# =============================================================================

if [ "$SKIP_DEPLOY" = false ]; then
    echo "[Phase 2] Deploying Stylus GKR verifier..."

    cd "$VERIFIER_DIR"

    # Check the contract against the devnode
    echo "  Running cargo stylus check..."
    if ! cargo stylus check --endpoint "$RPC_URL" --no-verify 2>&1; then
        echo "  ERROR: Contract check failed"
        exit 1
    fi

    # Deploy
    echo ""
    echo "  Deploying contract..."
    DEPLOY_OUTPUT=$(cargo stylus deploy \
        --endpoint "$RPC_URL" \
        --private-key "$DEV_PRIVATE_KEY" \
        --no-verify \
        2>&1)

    echo "$DEPLOY_OUTPUT"

    # Extract contract address from deploy output
    CONTRACT_ADDRESS=$(echo "$DEPLOY_OUTPUT" | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)

    if [ -z "$CONTRACT_ADDRESS" ]; then
        echo "  ERROR: Could not extract contract address from deploy output"
        exit 1
    fi

    echo ""
    echo "  Stylus contract deployed at: $CONTRACT_ADDRESS"
else
    echo "[Phase 2] Skipping deployment (--skip-deploy)"
    if [ -z "$CONTRACT_ADDRESS" ]; then
        echo "  ERROR: No contract address provided. Use --contract ADDRESS"
        exit 1
    fi
    echo "  Using Stylus contract: $CONTRACT_ADDRESS"
fi

echo ""

# =============================================================================
# Phase 3: Run Forge E2E script
# =============================================================================

echo "[Phase 3] Running Forge E2E (Solidity adapter → Stylus verifier)..."
echo ""

cd "$CONTRACTS_DIR"

FORGE_OUTPUT=$(DEPLOYER_KEY="$DEV_PRIVATE_KEY" \
    STYLUS_VERIFIER="$CONTRACT_ADDRESS" \
    forge script script/StylusDAGE2E.s.sol:StylusDAGE2E \
        --rpc-url "$RPC_URL" \
        --broadcast \
        --gas-limit 500000000 \
        -vvv 2>&1) || {
    echo "  Forge script failed!"
    echo "$FORGE_OUTPUT"
    exit 1
}

echo "$FORGE_OUTPUT"

echo ""

# =============================================================================
# Phase 4: Parse and display results
# =============================================================================

echo "============================================="
echo "  E2E Results Summary"
echo "============================================="
echo ""

# Extract gas numbers from Forge output
STYLUS_GAS=$(echo "$FORGE_OUTPUT" | grep -A1 "Stylus path:" | grep -oE '[0-9]+' | head -1 || true)
SOLIDITY_GAS=$(echo "$FORGE_OUTPUT" | grep -A1 "Solidity path:" | grep -oE '[0-9]+' | head -1 || true)

echo "  Stylus contract:    $CONTRACT_ADDRESS"

if [ -n "$STYLUS_GAS" ] && [ -n "$SOLIDITY_GAS" ]; then
    STYLUS_M=$(echo "scale=1; $STYLUS_GAS / 1000000" | bc)
    SOLIDITY_M=$(echo "scale=1; $SOLIDITY_GAS / 1000000" | bc)
    echo "  Stylus gas:         $STYLUS_GAS (${STYLUS_M}M)"
    echo "  Solidity gas:       $SOLIDITY_GAS (${SOLIDITY_M}M)"

    if [ "$SOLIDITY_GAS" -gt "$STYLUS_GAS" ]; then
        SAVINGS=$(echo "scale=1; ($SOLIDITY_GAS - $STYLUS_GAS) * 100 / $SOLIDITY_GAS" | bc)
        echo "  Gas savings:        ${SAVINGS}%"
    else
        EXTRA=$(echo "scale=1; ($STYLUS_GAS - $SOLIDITY_GAS) * 100 / $SOLIDITY_GAS" | bc)
        echo "  Gas overhead:       ${EXTRA}%"
    fi
else
    echo "  (Could not parse gas numbers from output)"
fi

echo ""

# Check for PASSED marker
if echo "$FORGE_OUTPUT" | grep -q "STYLUS DAG E2E PASSED"; then
    echo "=== STYLUS E2E PASSED ==="
else
    echo "=== STYLUS E2E FAILED ==="
    exit 1
fi
