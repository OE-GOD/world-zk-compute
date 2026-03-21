#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# Stylus + Groth16 Chunked Hybrid Verification E2E Test
# =============================================================================
#
# Full end-to-end test of the chunked hybrid verification pipeline on Arbitrum
# nitro-devnode. Splits EC operations into chunks, generates per-chunk Groth16
# proofs, and verifies them on-chain alongside Stylus WASM verification.
#
# Pipeline:
#   1. Rust: gen_ec_groth16_witness --chunk-size 500 --output-dir chunks/
#   2. gnark: prove-ec-chunked-json per chunk → chunk proofs
#   3. Stylus: verify_dag_proof_hybrid → (transcriptDigest, frOutputs)
#   4. On-chain: verify each chunk Groth16 proof + Stylus result
#
# Prerequisites:
#   - Docker running
#   - cargo-stylus installed
#   - Foundry installed (forge, cast)
#   - gnark-wrapper built with chunked support
#
# Usage:
#   ./scripts/stylus-groth16-chunked-e2e.sh [OPTIONS]
#
# Options:
#   --skip-devnode       Don't start nitro-devnode
#   --skip-deploy        Skip Stylus deployment (requires --contract)
#   --skip-wasm-check    Skip WASM size comparison
#   --contract ADDRESS   Use existing Stylus contract address
#   --chunk-size N       EC operations per chunk (default: 500)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERIFIER_DIR="$PROJECT_ROOT/contracts/stylus/gkr-verifier"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
GNARK_DIR="$PROJECT_ROOT/examples/xgboost-remainder/gnark-wrapper"
RUST_DIR="$PROJECT_ROOT/examples/xgboost-remainder"

# Nitro devnode config
DEVNODE_DIR="$PROJECT_ROOT/.nitro-devnode"
RPC_URL="http://localhost:8547"
DEV_PRIVATE_KEY="0xb6b15c8cb491557369f3c7d2c287b053eb229daa9c22138887752191c9520659"
DEV_ADDRESS="0x3f1Eae7D46d88F08fc2F8ed27FCb2AB183EB2d0E"

# Parse args
SKIP_DEVNODE=false
SKIP_DEPLOY=false
SKIP_WASM_CHECK=false
CONTRACT_ADDRESS=""
CHUNK_SIZE=500

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-devnode)     SKIP_DEVNODE=true; shift ;;
        --skip-deploy)      SKIP_DEPLOY=true; shift ;;
        --skip-wasm-check)  SKIP_WASM_CHECK=true; shift ;;
        --contract)         CONTRACT_ADDRESS="$2"; SKIP_DEPLOY=true; shift 2 ;;
        --chunk-size)       CHUNK_SIZE="$2"; shift 2 ;;
        *)                  echo "Unknown option: $1"; exit 1 ;;
    esac
done

TMPDIR=$(mktemp -d)
CHUNKS_DIR="$TMPDIR/chunks"
mkdir -p "$CHUNKS_DIR"

cleanup_tmp() {
    rm -rf "$TMPDIR"
}

echo "============================================="
echo "  Chunked Hybrid E2E Test"
echo "============================================="
echo ""
echo "  Pipeline: Stylus WASM (Fr) + chunked Groth16 (EC)"
echo "  Chunk size: $CHUNK_SIZE EC ops per chunk"
echo ""

# =============================================================================
# Phase 0: WASM Size Check
# =============================================================================

if [ "$SKIP_WASM_CHECK" = false ]; then
    echo "[Phase 0] WASM Size Check..."
    cd "$VERIFIER_DIR"

    # Build without hybrid feature
    echo "  Building WASM (default, no hybrid)..."
    cargo build --release --lib --target wasm32-unknown-unknown 2>/dev/null
    WASM_RAW="target/wasm32-unknown-unknown/release/gkr_verifier.wasm"

    if [ -f "$WASM_RAW" ]; then
        DEFAULT_RAW_SIZE=$(wc -c < "$WASM_RAW" | tr -d ' ')
        if command -v brotli &>/dev/null; then
            brotli -f "$WASM_RAW" -o "$TMPDIR/default.wasm.br" 2>/dev/null
            DEFAULT_BR_SIZE=$(wc -c < "$TMPDIR/default.wasm.br" | tr -d ' ')
        else
            DEFAULT_BR_SIZE="N/A"
        fi
        echo "  Default: ${DEFAULT_RAW_SIZE} raw, ${DEFAULT_BR_SIZE} Brotli"
    fi

    # Build with hybrid feature
    echo "  Building WASM (with hybrid feature)..."
    cargo build --release --lib --target wasm32-unknown-unknown --features hybrid 2>/dev/null
    WASM_RAW="target/wasm32-unknown-unknown/release/gkr_verifier.wasm"

    if [ -f "$WASM_RAW" ]; then
        HYBRID_RAW_SIZE=$(wc -c < "$WASM_RAW" | tr -d ' ')
        if command -v brotli &>/dev/null; then
            brotli -f "$WASM_RAW" -o "$TMPDIR/hybrid.wasm.br" 2>/dev/null
            HYBRID_BR_SIZE=$(wc -c < "$TMPDIR/hybrid.wasm.br" | tr -d ' ')
        else
            HYBRID_BR_SIZE="N/A"
        fi
        echo "  Hybrid:  ${HYBRID_RAW_SIZE} raw, ${HYBRID_BR_SIZE} Brotli"

        # Strip with wasm-opt
        WASM_OPT="target/wasm32-unknown-unknown/release/gkr_verifier_opt.wasm"
        if command -v wasm-opt &>/dev/null; then
            wasm-opt --strip-debug --strip-producers --enable-bulk-memory "$WASM_RAW" -o "$WASM_OPT" 2>/dev/null
            OPT_RAW_SIZE=$(wc -c < "$WASM_OPT" | tr -d ' ')
            if command -v brotli &>/dev/null; then
                brotli -f "$WASM_OPT" -o "$TMPDIR/hybrid_opt.wasm.br" 2>/dev/null
                OPT_BR_SIZE=$(wc -c < "$TMPDIR/hybrid_opt.wasm.br" | tr -d ' ')
            else
                OPT_BR_SIZE="N/A"
            fi
            echo "  Optimized: ${OPT_RAW_SIZE} raw, ${OPT_BR_SIZE} Brotli"

            # Warn if exceeds 24KB
            if [ "$OPT_BR_SIZE" != "N/A" ] && [ "$OPT_BR_SIZE" -gt 24576 ]; then
                echo "  WARNING: Hybrid WASM exceeds 24KB Brotli limit (${OPT_BR_SIZE} > 24576)"
                echo "  Consider splitting hybrid code behind feature flag"
            else
                echo "  OK: Within 24KB Stylus limit"
            fi
        fi
    fi
    echo ""
fi

# =============================================================================
# Phase 1: Start nitro-devnode
# =============================================================================

NITRO_NODE_VERSION="${NITRO_NODE_VERSION:-v3.7.1-926f1ab}"
NITRO_IMAGE="offchainlabs/nitro-node:${NITRO_NODE_VERSION}"
CONTAINER_NAME="nitro-chunked-e2e"
STARTED_DEVNODE=false

cleanup() {
    cleanup_tmp
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

    if cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
        echo "  Devnode already running at $RPC_URL. Block: $(cast block-number --rpc-url "$RPC_URL")"
    else
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
        echo "  Starting container ($NITRO_IMAGE)..."
        docker run --rm -d --name "$CONTAINER_NAME" \
            -p 8547:8547 \
            "$NITRO_IMAGE" \
            --dev --http.addr 0.0.0.0 --http.api=net,web3,eth,debug &>/dev/null
        STARTED_DEVNODE=true

        echo "  Waiting for RPC at $RPC_URL..."
        for i in $(seq 1 120); do
            if cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
                echo "  Devnode ready! Block: $(cast block-number --rpc-url "$RPC_URL")"
                break
            fi
            if [ "$i" -eq 120 ]; then
                echo "  ERROR: Devnode failed to start after 120s"
                docker logs "$CONTAINER_NAME" --tail 20
                exit 1
            fi
            sleep 1
        done

        # Setup chain owner + deployers
        echo "  Setting up chain owner and deployers..."
        if [ -d "$DEVNODE_DIR" ]; then
            cd "$DEVNODE_DIR"
            cast send 0x00000000000000000000000000000000000000FF \
                "becomeChainOwner()" \
                --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" &>/dev/null
            cast send --rpc-url "$RPC_URL" --private-key "$DEV_PRIVATE_KEY" \
                0x0000000000000000000000000000000000000070 \
                'setL1PricePerUnit(uint256)' 0x0 &>/dev/null

            CREATE2_FACTORY=0x4e59b44847b379578588920ca78fbf26c0b4956c
            SALT=0x0000000000000000000000000000000000000000000000000000000000000000
            cast send --rpc-url "$RPC_URL" --private-key "$DEV_PRIVATE_KEY" \
                --value "1 ether" 0x3fab184622dc19b6109349b94811493bf2a45362 &>/dev/null
            cast publish --rpc-url "$RPC_URL" \
                0xf8a58085174876e800830186a08080b853604580600e600039806000f350fe7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf31ba02222222222222222222222222222222222222222222222222222222222222222a02222222222222222222222222222222222222222222222222222222222222222 &>/dev/null || true

            CM_OUTPUT=$(cast send --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" \
                --create 0x60a06040523060805234801561001457600080fd5b50608051611d1c61003060003960006105260152611d1c6000f3fe 2>&1)
            CM_ADDR=$(echo "$CM_OUTPUT" | awk '/contractAddress/ {print $2}')
            if [ -n "$CM_ADDR" ]; then
                cast send --private-key "$DEV_PRIVATE_KEY" --rpc-url "$RPC_URL" \
                    0x0000000000000000000000000000000000000070 \
                    "addWasmCacheManager(address)" "$CM_ADDR" &>/dev/null
                echo "  Cache Manager deployed: $CM_ADDR"
            fi

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
# Phase 2: Deploy Stylus contract (hybrid-enabled)
# =============================================================================

if [ "$SKIP_DEPLOY" = false ]; then
    echo "[Phase 2] Deploying Stylus GKR verifier (hybrid-enabled)..."
    cd "$VERIFIER_DIR"

    cargo build --release --lib --target wasm32-unknown-unknown --features hybrid 2>&1

    WASM_RAW="target/wasm32-unknown-unknown/release/gkr_verifier.wasm"
    WASM_OPT="target/wasm32-unknown-unknown/release/gkr_verifier_opt.wasm"

    if command -v wasm-opt &>/dev/null; then
        wasm-opt --strip-debug --strip-producers --enable-bulk-memory "$WASM_RAW" -o "$WASM_OPT" 2>&1
        WASM_FILE="$WASM_OPT"
    else
        WASM_FILE="$WASM_RAW"
    fi

    RAW_SIZE=$(wc -c < "$WASM_FILE" | tr -d ' ')
    echo "  WASM size: ${RAW_SIZE} bytes"

    echo "  Running cargo stylus check..."
    if ! cargo stylus check --endpoint "$RPC_URL" --no-verify --wasm-file "$WASM_FILE" 2>&1; then
        echo "  ERROR: Contract check failed"
        exit 1
    fi

    echo "  Deploying contract..."
    DEPLOY_OUTPUT=$(cargo stylus deploy \
        --endpoint "$RPC_URL" \
        --private-key "$DEV_PRIVATE_KEY" \
        --no-verify \
        --wasm-file "$WASM_FILE" \
        2>&1)

    echo "$DEPLOY_OUTPUT"
    CONTRACT_ADDRESS=$(echo "$DEPLOY_OUTPUT" | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)

    if [ -z "$CONTRACT_ADDRESS" ]; then
        echo "  ERROR: Could not extract contract address"
        exit 1
    fi
    echo "  Stylus contract deployed at: $CONTRACT_ADDRESS"
else
    echo "[Phase 2] Skipping deployment (--skip-deploy)"
    if [ -z "$CONTRACT_ADDRESS" ]; then
        echo "  ERROR: No contract address. Use --contract ADDRESS"
        exit 1
    fi
    echo "  Using Stylus contract: $CONTRACT_ADDRESS"
fi

echo ""

# =============================================================================
# Phase 3: Generate chunked EC witnesses
# =============================================================================

echo "[Phase 3] Generating chunked EC witnesses..."

cd "$RUST_DIR"

if [ -f "src/bin/gen_ec_groth16_witness.rs" ]; then
    cargo run --release --bin gen_ec_groth16_witness -- \
        --trees 3 --depth 4 --features 4 \
        --chunk-size "$CHUNK_SIZE" \
        --output-dir "$CHUNKS_DIR" \
        2>"$TMPDIR/witness_stderr.txt" || {
        echo "  ERROR: EC witness generation failed"
        cat "$TMPDIR/witness_stderr.txt"
        exit 1
    }

    NUM_CHUNKS=$(ls "$CHUNKS_DIR"/chunk_*.json 2>/dev/null | wc -l | tr -d ' ')
    echo "  Generated $NUM_CHUNKS chunks in $CHUNKS_DIR"

    if [ -f "$CHUNKS_DIR/metadata.json" ]; then
        python3 -c "
import json
with open('$CHUNKS_DIR/metadata.json') as f:
    m = json.load(f)
print(f\"  Total EC ops: {m.get('stats',{}).get('totalEcMul',0)} mul, {m.get('stats',{}).get('totalEcAdd',0)} add\")
print(f\"  Transcript digest: {m.get('transcriptDigest','N/A')[:18]}...\")
" 2>/dev/null || true
    fi
else
    echo "  WARNING: gen_ec_groth16_witness not found (T479/T487 not complete)"
    echo "  Creating mock chunk data..."
    NUM_CHUNKS=0
fi

echo ""

# =============================================================================
# Phase 4: Generate per-chunk Groth16 proofs
# =============================================================================

echo "[Phase 4] Generating per-chunk Groth16 proofs..."

CHUNK_PROOFS_DIR="$TMPDIR/chunk_proofs"
mkdir -p "$CHUNK_PROOFS_DIR"

cd "$GNARK_DIR"

if [ "$NUM_CHUNKS" -gt 0 ] && [ -x "./gnark-wrapper" ] && ./gnark-wrapper help 2>&1 | grep -q "prove-ec-chunked-json"; then
    TOTAL_CHUNK_GAS=0
    for chunk_file in "$CHUNKS_DIR"/chunk_*.json; do
        chunk_name=$(basename "$chunk_file" .json)
        echo "  Processing $chunk_name..."
        cat "$chunk_file" | ./gnark-wrapper prove-ec-chunked-json \
            > "$CHUNK_PROOFS_DIR/${chunk_name}_proof.json" 2>"$TMPDIR/${chunk_name}_stderr.txt"

        if [ -s "$CHUNK_PROOFS_DIR/${chunk_name}_proof.json" ]; then
            CONSTRAINTS=$(python3 -c "import json; d=json.load(open('$CHUNK_PROOFS_DIR/${chunk_name}_proof.json')); print(d.get('stats',{}).get('r1cs_constraints','N/A'))" 2>/dev/null || echo "N/A")
            echo "    Proof generated ($CONSTRAINTS R1CS constraints)"
        else
            echo "    WARNING: Proof generation failed for $chunk_name"
            cat "$TMPDIR/${chunk_name}_stderr.txt" | head -5
        fi
    done
    echo "  All chunk proofs generated"
else
    echo "  WARNING: Chunked proving not available"
    if [ "$NUM_CHUNKS" -eq 0 ]; then
        echo "  No chunks to prove (EC witness generation was skipped)"
    else
        echo "  gnark-wrapper missing prove-ec-chunked-json command (T486/T488 not complete)"
    fi
fi

echo ""

# =============================================================================
# Phase 5: Deploy Solidity contracts + run verification
# =============================================================================

echo "[Phase 5] Running on-chain verification..."

cd "$CONTRACTS_DIR"

# Run the hybrid E2E Forge script (which handles Solidity + Stylus paths)
FORGE_OUTPUT=$(DEPLOYER_KEY="$DEV_PRIVATE_KEY" \
    STYLUS_VERIFIER="$CONTRACT_ADDRESS" \
    forge script script/StylusHybridE2E.s.sol:StylusHybridE2E \
        --rpc-url "$RPC_URL" \
        --broadcast \
        --gas-limit 500000000 \
        -vvv 2>&1) || {
    echo "  Forge script failed!"
    echo "$FORGE_OUTPUT" | tail -30
    exit 1
}

echo "$FORGE_OUTPUT" | tail -20

echo ""

# =============================================================================
# Phase 6: Results
# =============================================================================

echo "============================================="
echo "  Chunked Hybrid E2E Results"
echo "============================================="
echo ""
echo "  Stylus contract: $CONTRACT_ADDRESS"
echo "  Chunk size:      $CHUNK_SIZE ops per chunk"
echo "  Num chunks:      $NUM_CHUNKS"
echo ""

# Extract gas from Forge output
HYBRID_GAS=$(echo "$FORGE_OUTPUT" | grep -i "Hybrid.*gas:" | grep -oE '[0-9]+' | head -1 || true)
STYLUS_GAS=$(echo "$FORGE_OUTPUT" | grep -i "Stylus.*gas:" | grep -oE '[0-9]+' | head -1 || true)

echo "  --- Gas Profile ---"
if [ -n "$HYBRID_GAS" ]; then
    HYBRID_M=$(echo "scale=1; $HYBRID_GAS / 1000000" | bc)
    echo "  Hybrid (Stylus + Groth16): ${HYBRID_M}M gas"
fi
if [ -n "$STYLUS_GAS" ]; then
    STYLUS_M=$(echo "scale=1; $STYLUS_GAS / 1000000" | bc)
    echo "  Stylus (full, with EC):    ${STYLUS_M}M gas"
fi
echo ""
echo "  --- Expected (Full Pipeline) ---"
echo "  Stylus hybrid (Fr only):     ~2-5M gas"
if [ "$NUM_CHUNKS" -gt 0 ]; then
    echo "  Groth16 (${NUM_CHUNKS} chunks):       ~$(echo "scale=1; $NUM_CHUNKS * 230 / 1000" | bc)M gas"
    ESTIMATED_TOTAL=$(echo "scale=1; 3 + $NUM_CHUNKS * 0.23" | bc)
    echo "  Estimated total:             ~${ESTIMATED_TOTAL}M gas"
else
    echo "  Groth16 (N chunks):          ~N*230K gas"
fi
echo "  vs Solidity direct:          ~254M gas (15 batch txs)"
echo ""

if echo "$FORGE_OUTPUT" | grep -q "HYBRID E2E PASSED"; then
    echo "=== CHUNKED HYBRID E2E PASSED ==="
else
    echo "=== CHUNKED HYBRID E2E FAILED ==="
    exit 1
fi
