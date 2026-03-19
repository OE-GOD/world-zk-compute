#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Stylus + Groth16 Hybrid Verification E2E Test
# =============================================================================
#
# Full end-to-end test of the hybrid verification pipeline on Arbitrum
# nitro-devnode:
#   1. Stylus WASM verifier (transcript replay + Fr arithmetic, ~2-5M gas)
#   2. Groth16 proof (EC arithmetic verification, ~230K pairing)
#   3. Combined verification via HybridStylusGroth16Verifier (~3-6M total)
#
# Prerequisites:
#   - Docker running
#   - cargo-stylus installed (cargo install cargo-stylus)
#   - Foundry installed (forge, cast)
#   - gnark-wrapper built (cd examples/xgboost-remainder/gnark-wrapper && make)
#
# Usage:
#   ./scripts/stylus-groth16-hybrid-e2e.sh [OPTIONS]
#
# Options:
#   --skip-devnode       Don't start nitro-devnode (assume already running)
#   --skip-deploy        Skip Stylus deployment (requires --contract)
#   --skip-groth16-gen   Skip Groth16 proof generation (use existing fixture)
#   --contract ADDRESS   Use existing Stylus contract address
#   --fixture FILE       Path to E2E fixture JSON (default: phase1a_dag_fixture.json)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERIFIER_DIR="$PROJECT_ROOT/contracts/stylus/gkr-verifier"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
GNARK_DIR="$PROJECT_ROOT/examples/xgboost-remainder/gnark-wrapper"
RUST_EXAMPLE_DIR="$PROJECT_ROOT/examples/xgboost-remainder"

# Nitro devnode config
DEVNODE_DIR="$PROJECT_ROOT/.nitro-devnode"
RPC_URL="http://localhost:8547"
# Pre-funded dev account on nitro-devnode
DEV_PRIVATE_KEY="0xb6b15c8cb491557369f3c7d2c287b053eb229daa9c22138887752191c9520659"
DEV_ADDRESS="0x3f1Eae7D46d88F08fc2F8ed27FCb2AB183EB2d0E"

# Parse args
SKIP_DEVNODE=false
SKIP_DEPLOY=false
SKIP_GROTH16_GEN=false
CONTRACT_ADDRESS=""
FIXTURE_PATH="$CONTRACTS_DIR/test/fixtures/phase1a_dag_fixture.json"

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-devnode)      SKIP_DEVNODE=true; shift ;;
        --skip-deploy)       SKIP_DEPLOY=true; shift ;;
        --skip-groth16-gen)  SKIP_GROTH16_GEN=true; shift ;;
        --contract)          CONTRACT_ADDRESS="$2"; SKIP_DEPLOY=true; shift 2 ;;
        --fixture)           FIXTURE_PATH="$2"; shift 2 ;;
        *)                   echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "============================================="
echo "  Stylus + Groth16 Hybrid E2E Test"
echo "============================================="
echo ""
echo "  Target: Single-tx hybrid verification (~3-6M gas)"
echo "  Pipeline: Stylus WASM (Fr) → Groth16 (EC) → On-chain"
echo ""

# =============================================================================
# Phase 1: Start nitro-devnode
# =============================================================================

NITRO_NODE_VERSION="${NITRO_NODE_VERSION:-v3.7.1-926f1ab}"
NITRO_IMAGE="offchainlabs/nitro-node:${NITRO_NODE_VERSION}"
CONTAINER_NAME="nitro-hybrid-e2e"
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
    if ! cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
        echo "  ERROR: No devnode running at $RPC_URL"
        exit 1
    fi
    echo "  Devnode running. Block: $(cast block-number --rpc-url "$RPC_URL")"
fi

echo ""

# =============================================================================
# Phase 2: Deploy Stylus contract (with hybrid feature)
# =============================================================================

if [ "$SKIP_DEPLOY" = false ]; then
    echo "[Phase 2] Deploying Stylus GKR verifier (hybrid-enabled)..."

    cd "$VERIFIER_DIR"

    # Build WASM with hybrid feature enabled
    echo "  Building WASM with hybrid feature..."
    cargo build --release --lib --target wasm32-unknown-unknown --features hybrid 2>&1

    WASM_RAW="target/wasm32-unknown-unknown/release/gkr_verifier.wasm"
    WASM_OPT="target/wasm32-unknown-unknown/release/gkr_verifier_opt.wasm"

    # Strip debug sections with wasm-opt
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

    # Extract contract address
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
# Phase 3: Generate EC witness + Groth16 proof
# =============================================================================

EC_WITNESS_FILE="$PROJECT_ROOT/target/ec_witness.json"
EC_GROTH16_PROOF_FILE="$PROJECT_ROOT/target/ec_groth16_proof.json"

if [ "$SKIP_GROTH16_GEN" = false ]; then
    echo "[Phase 3] Generating EC Groth16 witness and proof..."

    # Step 3a: Generate EC witness from the GKR proof fixture
    echo "  Generating EC witness (Rust)..."
    cd "$RUST_EXAMPLE_DIR"

    if [ -f "src/bin/gen_ec_groth16_witness.rs" ]; then
        cargo run --release --bin gen_ec_groth16_witness -- \
            --trees 3 --depth 4 --features 4 \
            --output "$EC_WITNESS_FILE" 2>&1

        if [ ! -f "$EC_WITNESS_FILE" ]; then
            echo "  ERROR: EC witness file not generated"
            exit 1
        fi
        echo "  EC witness generated: $(wc -c < "$EC_WITNESS_FILE" | tr -d ' ') bytes"
    else
        echo "  WARNING: gen_ec_groth16_witness not found (T479 not complete)"
        echo "  Using mock EC witness for testing..."
        # Create a minimal mock witness for pipeline testing
        echo '{"transcriptDigest":"0x0000000000000000000000000000000000000000000000000000000000000001","circuitHash":"0x0000000000000000000000000000000000000000000000000000000000000002","ecOperations":[],"frOutputs":{"rlcBetas":[],"zDotJStars":[],"lTensorFlat":[],"zDotRs":[],"mleEvals":[]},"stats":{"totalEcMul":0,"totalEcAdd":0,"totalMsm":0,"totalMsmPoints":0}}' > "$EC_WITNESS_FILE"
    fi

    # Step 3b: Generate Groth16 proof via gnark-wrapper
    echo "  Generating EC Groth16 proof (gnark)..."
    cd "$GNARK_DIR"

    if [ -x "./gnark-wrapper" ]; then
        # Check if prove-ec-json command exists
        if ./gnark-wrapper help 2>&1 | grep -q "prove-ec-json"; then
            cat "$EC_WITNESS_FILE" | ./gnark-wrapper prove-ec-json --config-json > "$EC_GROTH16_PROOF_FILE" 2>/dev/null

            if [ ! -f "$EC_GROTH16_PROOF_FILE" ] || [ ! -s "$EC_GROTH16_PROOF_FILE" ]; then
                echo "  ERROR: Groth16 proof generation failed"
                exit 1
            fi
            echo "  Groth16 proof generated: $(wc -c < "$EC_GROTH16_PROOF_FILE" | tr -d ' ') bytes"
        else
            echo "  WARNING: prove-ec-json command not available (T480 not complete)"
            echo "  Generating mock Groth16 proof for pipeline testing..."
            # 8 uint256 values for A (2), B (4), C (2) points
            echo '{"proof":[0,0,0,0,0,0,0,0],"publicInputs":[]}' > "$EC_GROTH16_PROOF_FILE"
        fi
    else
        echo "  WARNING: gnark-wrapper not built"
        echo "  Build with: cd $GNARK_DIR && make"
        echo "  Generating mock Groth16 proof for pipeline testing..."
        echo '{"proof":[0,0,0,0,0,0,0,0],"publicInputs":[]}' > "$EC_GROTH16_PROOF_FILE"
    fi
else
    echo "[Phase 3] Skipping Groth16 proof generation (--skip-groth16-gen)"
    if [ ! -f "$EC_GROTH16_PROOF_FILE" ]; then
        echo "  WARNING: No existing EC Groth16 proof file found"
    fi
fi

echo ""

# =============================================================================
# Phase 4: Deploy Solidity contracts + configure hybrid
# =============================================================================

echo "[Phase 4] Deploying Solidity contracts for hybrid verification..."

cd "$CONTRACTS_DIR"

# Deploy the hybrid E2E Forge script
FORGE_OUTPUT=$(DEPLOYER_KEY="$DEV_PRIVATE_KEY" \
    STYLUS_VERIFIER="$CONTRACT_ADDRESS" \
    EC_GROTH16_PROOF_FILE="$EC_GROTH16_PROOF_FILE" \
    forge script script/StylusHybridE2E.s.sol:StylusHybridE2E \
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
# Phase 5: Run verification comparison
# =============================================================================

echo "[Phase 5] Verification gas comparison..."
echo ""

# Extract gas numbers from Forge output
HYBRID_GAS=$(echo "$FORGE_OUTPUT" | grep -i "Hybrid.*gas:" | grep -oE '[0-9]+' | head -1 || true)
STYLUS_GAS=$(echo "$FORGE_OUTPUT" | grep -i "Stylus.*gas:" | grep -oE '[0-9]+' | head -1 || true)
SOLIDITY_GAS=$(echo "$FORGE_OUTPUT" | grep -i "Solidity.*gas:" | grep -oE '[0-9]+' | head -1 || true)

echo "============================================="
echo "  Gas Comparison Results"
echo "============================================="
echo ""
echo "  Stylus contract:    $CONTRACT_ADDRESS"
echo ""

# Known baseline values
KNOWN_SOLIDITY_GAS=254000000  # ~254M (88-layer XGBoost, pure Solidity)
KNOWN_STYLUS_GAS=207000000    # ~207M (88-layer XGBoost, Stylus full)

if [ -n "$HYBRID_GAS" ]; then
    HYBRID_M=$(echo "scale=1; $HYBRID_GAS / 1000000" | bc)
    echo "  Hybrid gas:         $HYBRID_GAS (${HYBRID_M}M)"
else
    echo "  Hybrid gas:         (not available)"
fi

if [ -n "$STYLUS_GAS" ]; then
    STYLUS_M=$(echo "scale=1; $STYLUS_GAS / 1000000" | bc)
    echo "  Stylus (full) gas:  $STYLUS_GAS (${STYLUS_M}M)"
else
    echo "  Stylus (full) gas:  ~207M (baseline)"
fi

if [ -n "$SOLIDITY_GAS" ]; then
    SOLIDITY_M=$(echo "scale=1; $SOLIDITY_GAS / 1000000" | bc)
    echo "  Solidity gas:       $SOLIDITY_GAS (${SOLIDITY_M}M)"
else
    echo "  Solidity gas:       ~254M (baseline)"
fi

echo ""
echo "  --- Reference ---"
echo "  Solidity (direct):  ~254M gas (15 transactions, batch verifier)"
echo "  Stylus (full):      ~207M gas (1 transaction, but exceeds 30M limit)"
echo "  Hybrid (target):    ~3-6M gas (1 transaction, within 30M limit)"
echo "    - Stylus WASM:    ~2-5M (transcript replay + Fr arithmetic)"
echo "    - Groth16:        ~230K (pairing verification)"
echo "    - Overhead:       ~50K (calldata + orchestration)"

if [ -n "$HYBRID_GAS" ]; then
    echo ""
    # Check if within target
    if [ "$HYBRID_GAS" -le 6000000 ]; then
        echo "  >>> HYBRID GAS WITHIN TARGET (<=6M) <<<"
    elif [ "$HYBRID_GAS" -le 30000000 ]; then
        echo "  >>> HYBRID GAS WITHIN BLOCK LIMIT (<=30M) <<<"
    else
        echo "  >>> WARNING: HYBRID GAS EXCEEDS BLOCK LIMIT (>30M) <<<"
    fi

    # Compute savings vs Solidity
    SAVINGS_VS_SOLIDITY=$(echo "scale=1; ($KNOWN_SOLIDITY_GAS - $HYBRID_GAS) * 100 / $KNOWN_SOLIDITY_GAS" | bc)
    SAVINGS_VS_STYLUS=$(echo "scale=1; ($KNOWN_STYLUS_GAS - $HYBRID_GAS) * 100 / $KNOWN_STYLUS_GAS" | bc)
    echo "  Savings vs Solidity: ${SAVINGS_VS_SOLIDITY}%"
    echo "  Savings vs Stylus:   ${SAVINGS_VS_STYLUS}%"
fi

echo ""

# =============================================================================
# Phase 6: Verify outcome
# =============================================================================

if echo "$FORGE_OUTPUT" | grep -q "HYBRID E2E PASSED"; then
    echo "=== STYLUS + GROTH16 HYBRID E2E PASSED ==="
else
    echo "=== STYLUS + GROTH16 HYBRID E2E FAILED ==="
    exit 1
fi
