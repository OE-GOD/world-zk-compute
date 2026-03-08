#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Stylus GKR Verifier Gas Benchmark
# =============================================================================
#
# Deploys the Stylus GKR verifier to a local Arbitrum nitro-devnode and
# measures gas costs for verifying the 88-layer XGBoost DAG proof.
#
# Prerequisites:
#   - Docker running
#   - cargo-stylus installed (cargo install cargo-stylus)
#   - Foundry installed (cast)
#
# Usage:
#   ./scripts/stylus-benchmark.sh [--skip-devnode] [--skip-deploy] [--contract ADDRESS]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERIFIER_DIR="$PROJECT_ROOT/contracts/stylus/gkr-verifier"

# Nitro devnode config
DEVNODE_DIR="$PROJECT_ROOT/.nitro-devnode"
RPC_URL="http://localhost:8547"
# Pre-funded dev account on nitro-devnode
DEV_PRIVATE_KEY="0xb6b15c8cb491557369f3c7d2c287b053eb229daa9c22138887752191c9520659"
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
echo "  Stylus GKR Verifier Gas Benchmark"
echo "============================================="
echo ""

# =============================================================================
# Phase 1: Start nitro-devnode
# =============================================================================

NITRO_NODE_VERSION="${NITRO_NODE_VERSION:-v3.7.1-926f1ab}"
NITRO_IMAGE="offchainlabs/nitro-node:${NITRO_NODE_VERSION}"
CONTAINER_NAME="nitro-stylus-bench"
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
        DEVNODE_DIR="$PROJECT_ROOT/.nitro-devnode"
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
# Phase 2: Deploy contract
# =============================================================================

if [ "$SKIP_DEPLOY" = false ]; then
    echo "[Phase 2] Deploying Stylus GKR verifier..."

    cd "$VERIFIER_DIR"

    # Build WASM
    echo "  Building WASM..."
    cargo build --release --lib --target wasm32-unknown-unknown 2>&1

    WASM_RAW="target/wasm32-unknown-unknown/release/gkr_verifier.wasm"
    WASM_OPT="target/wasm32-unknown-unknown/release/gkr_verifier_opt.wasm"

    # Strip debug sections with wasm-opt (required to fit under 24KB Brotli limit)
    if command -v wasm-opt &>/dev/null; then
        echo "  Stripping WASM debug sections..."
        wasm-opt --strip-debug --strip-producers --enable-bulk-memory "$WASM_RAW" -o "$WASM_OPT" 2>&1
        WASM_FILE="$WASM_OPT"
    else
        echo "  WARNING: wasm-opt not found, WASM may exceed 24KB Brotli limit"
        WASM_FILE="$WASM_RAW"
    fi

    RAW_SIZE=$(wc -c < "$WASM_FILE" | tr -d ' ')
    echo "  WASM size: ${RAW_SIZE} bytes"

    # Check the contract against the devnode
    echo "  Running cargo stylus check..."
    if ! cargo stylus check --endpoint "$RPC_URL" --no-verify --wasm-file "$WASM_FILE" 2>&1; then
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
        --wasm-file "$WASM_FILE" \
        2>&1)

    echo "$DEPLOY_OUTPUT"

    # Extract contract address from deploy output
    # cargo stylus deploy prints: "deployed code at address: 0x..."
    # or: "contract address: 0x..."
    CONTRACT_ADDRESS=$(echo "$DEPLOY_OUTPUT" | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)

    if [ -z "$CONTRACT_ADDRESS" ]; then
        echo "  ERROR: Could not extract contract address from deploy output"
        exit 1
    fi

    echo ""
    echo "  Contract deployed at: $CONTRACT_ADDRESS"
else
    echo "[Phase 2] Skipping deployment (--skip-deploy)"
    if [ -z "$CONTRACT_ADDRESS" ]; then
        echo "  ERROR: No contract address provided. Use --contract ADDRESS"
        exit 1
    fi
    echo "  Using contract: $CONTRACT_ADDRESS"
fi

echo ""

# =============================================================================
# Phase 3: Generate calldata
# =============================================================================

echo "[Phase 3] Generating calldata..."

cd "$VERIFIER_DIR"
CALLDATA_FILE="$VERIFIER_DIR/calldata.hex"

# Build and run the calldata generator
cargo run --release --no-default-features \
    --target aarch64-apple-darwin \
    --example gen_calldata 2>&1 | grep -v "^0x" >&2 || true

# Run again to capture just the hex output
CALLDATA=$(cargo run --release --no-default-features \
    --target aarch64-apple-darwin \
    --example gen_calldata 2>/dev/null)

if [ -z "$CALLDATA" ]; then
    # Fall back to file
    if [ -f "$CALLDATA_FILE" ]; then
        CALLDATA=$(cat "$CALLDATA_FILE")
    else
        echo "  ERROR: No calldata generated"
        exit 1
    fi
fi

CALLDATA_SIZE=${#CALLDATA}
CALLDATA_BYTES=$(( (CALLDATA_SIZE - 2) / 2 ))  # subtract "0x" prefix
echo "  Calldata: ${CALLDATA_BYTES} bytes ($(echo "scale=1; $CALLDATA_BYTES / 1024" | bc) KB)"
echo "  Calldata gas (16/byte): ~$(echo "scale=1; $CALLDATA_BYTES * 16 / 1000000" | bc)M"

echo ""

# =============================================================================
# Phase 4: Benchmark
# =============================================================================

echo "[Phase 4] Running gas benchmark..."
echo ""

# First try gas estimation
echo "  Estimating gas..."
GAS_ESTIMATE=$(cast estimate \
    --rpc-url "$RPC_URL" \
    --from "$DEV_ADDRESS" \
    "$CONTRACT_ADDRESS" \
    "$CALLDATA" \
    2>&1) || true

if echo "$GAS_ESTIMATE" | grep -qE '^[0-9]+$'; then
    echo "  Gas estimate: $GAS_ESTIMATE ($(echo "scale=1; $GAS_ESTIMATE / 1000000" | bc)M)"
else
    echo "  Gas estimation returned: $GAS_ESTIMATE"
    echo "  (This may fail if gas exceeds block limit; proceeding with send)"
fi

echo ""

# Send the actual transaction
echo "  Sending verification transaction..."
TX_OUTPUT=$(cast send \
    --rpc-url "$RPC_URL" \
    --private-key "$DEV_PRIVATE_KEY" \
    --gas-limit 500000000 \
    "$CONTRACT_ADDRESS" \
    "$CALLDATA" \
    2>&1)

echo "$TX_OUTPUT"

# Extract gas used
GAS_USED=$(echo "$TX_OUTPUT" | grep -i "gasUsed" | grep -oE '[0-9]+' | head -1)
TX_HASH=$(echo "$TX_OUTPUT" | grep -i "transactionHash" | grep -oE '0x[0-9a-fA-F]{64}' | head -1)
TX_STATUS=$(echo "$TX_OUTPUT" | grep -i "status" | head -1)

echo ""

# =============================================================================
# Phase 5: Results
# =============================================================================

echo "============================================="
echo "  Benchmark Results"
echo "============================================="
echo ""
echo "  Contract:         $CONTRACT_ADDRESS"
echo "  Transaction:      $TX_HASH"
echo "  Status:           $TX_STATUS"
echo "  Calldata size:    ${CALLDATA_BYTES} bytes ($(echo "scale=1; $CALLDATA_BYTES / 1024" | bc) KB)"

if [ -n "$GAS_USED" ]; then
    GAS_M=$(echo "scale=1; $GAS_USED / 1000000" | bc)
    echo "  Gas used:         $GAS_USED (${GAS_M}M)"
    echo ""
    echo "  --- Comparison with Solidity ---"
    echo "  Solidity DAG:     252M gas (15 transactions)"
    echo "  Stylus:           ${GAS_M}M gas (1 transaction)"
    SAVINGS=$(echo "scale=1; (252000000 - $GAS_USED) * 100 / 252000000" | bc)
    echo "  Gas savings:      ${SAVINGS}%"
    echo ""

    # Breakdown estimate
    CALLDATA_GAS=$(echo "$CALLDATA_BYTES * 16" | bc)
    EXEC_GAS=$(echo "$GAS_USED - $CALLDATA_GAS" | bc)
    echo "  --- Gas Breakdown (estimated) ---"
    echo "  Calldata:         ~$(echo "scale=1; $CALLDATA_GAS / 1000000" | bc)M"
    echo "  Execution:        ~$(echo "scale=1; $EXEC_GAS / 1000000" | bc)M"
    echo "  EC precompiles:   ~200M (expected, same as Solidity)"
else
    echo "  Gas used:         Could not extract"
    echo ""
    echo "  Raw output:"
    echo "$TX_OUTPUT"
fi

echo ""
echo "============================================="

# =============================================================================
# Phase 6: Optional trace (if available)
# =============================================================================

if [ -n "$TX_HASH" ] && command -v cargo-stylus &>/dev/null; then
    echo ""
    echo "[Optional] Attempting Stylus trace..."
    cargo stylus trace --tx "$TX_HASH" --endpoint "$RPC_URL" 2>&1 || echo "  Trace not available on this devnode version"
fi
