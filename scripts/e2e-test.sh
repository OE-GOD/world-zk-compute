#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# World ZK Compute - E2E Test Script
#
# Runs a full end-to-end test: deploy contracts, register program, submit
# execution request, run prover, verify completion.
#
# Usage:
#   ./scripts/e2e-test.sh --example anomaly-detector
#   ./scripts/e2e-test.sh --example signature-verified
#   ./scripts/e2e-test.sh --example sybil-detector
#   ./scripts/e2e-test.sh --example rule-engine
#   ./scripts/e2e-test.sh --example xgboost-inference
#   ./scripts/e2e-test.sh --example all
#   ./scripts/e2e-test.sh --example anomaly-detector --gpu
#   ./scripts/e2e-test.sh --example anomaly-detector --network sepolia
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────
EXAMPLE=""
GPU=false
NETWORK="local"
ANVIL_PID=""
PROVER_PID=""
ANVIL_PORT=8545
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

# Known RISC Zero Verifier Router addresses
SEPOLIA_VERIFIER_ADDRESS="0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187"
MAINNET_VERIFIER_ADDRESS="0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319"

# Anvil deterministic accounts (default mnemonic)
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOYER_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
PROVER_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
PROVER_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
REQUESTER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
REQUESTER_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"

# Deterministic contract addresses on fresh Anvil (deployer nonce 0,1,2)
VERIFIER_ADDR="0x5FbDB2315678afecb367f032d93F642f64180aa3"
REGISTRY_ADDR="0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
ENGINE_ADDR="0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0"

# Image IDs
ANOMALY_DETECTOR_IMAGE_ID="0x24c3af8225689d633ce0b02a61cb6a58fe656db1f31185eedd69f656a982bc95"
SIGNATURE_VERIFIED_IMAGE_ID="0x28d93899974adcfe07ccad0c251b65e4308f265b6e296b9b81f1267bbf3ddd34"
SYBIL_DETECTOR_IMAGE_ID="0xee666bd16e310391f57cc2c65f301b06fcc018573913edf699ac3ad65db146e4"
RULE_ENGINE_IMAGE_ID="0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33"
XGBOOST_INFERENCE_IMAGE_ID="0x1b1e0e6ea0bbefbbb2ccfe269a687b2e46efaa243f36664776b49dc15716e2ac"

# Timeouts
CPU_TIMEOUT=300
GPU_TIMEOUT=120

# ── Helpers ───────────────────────────────────────────────────────────────────

log() { echo "==> $*"; }
err() { echo "ERROR: $*" >&2; }
ok()  { echo "  OK: $*"; }

cleanup() {
    log "Cleaning up..."
    if [ -n "$PROVER_PID" ] && kill -0 "$PROVER_PID" 2>/dev/null; then
        kill "$PROVER_PID" 2>/dev/null || true
        wait "$PROVER_PID" 2>/dev/null || true
    fi
    if [ -n "$ANVIL_PID" ] && kill -0 "$ANVIL_PID" 2>/dev/null; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

usage() {
    echo "Usage: $0 --example <anomaly-detector|signature-verified|sybil-detector|rule-engine|xgboost-inference|all> [--gpu] [--network <local|sepolia>]"
    echo ""
    echo "Options:"
    echo "  --example  Which example to test (required)"
    echo "  --gpu      Use GPU acceleration (Metal on macOS, CUDA on Linux)"
    echo "  --network  Network to deploy to (default: local)"
    echo "             local   - Anvil + MockVerifier (default)"
    echo "             sepolia - Deploy to Sepolia with real RISC Zero verifier"
    exit 1
}

# ── Parse args ────────────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case $1 in
        --example) EXAMPLE="$2"; shift 2 ;;
        --gpu)     GPU=true; shift ;;
        --network) NETWORK="$2"; shift 2 ;;
        -h|--help) usage ;;
        *) err "Unknown option: $1"; usage ;;
    esac
done

if [ -z "$EXAMPLE" ]; then
    err "Missing required --example flag"
    usage
fi

if [[ "$EXAMPLE" != "anomaly-detector" && "$EXAMPLE" != "signature-verified" && "$EXAMPLE" != "sybil-detector" && "$EXAMPLE" != "rule-engine" && "$EXAMPLE" != "xgboost-inference" && "$EXAMPLE" != "all" ]]; then
    err "Invalid example: $EXAMPLE (must be anomaly-detector, signature-verified, sybil-detector, rule-engine, xgboost-inference, or all)"
    usage
fi

if [[ "$NETWORK" != "local" && "$NETWORK" != "sepolia" ]]; then
    err "Invalid network: $NETWORK (must be local or sepolia)"
    usage
fi

# ── 1. Prerequisites ─────────────────────────────────────────────────────────

log "Checking prerequisites..."

for cmd in anvil forge cast cargo python3; do
    if ! command -v "$cmd" &>/dev/null; then
        err "Required command not found: $cmd"
        case $cmd in
            anvil|forge|cast)
                echo "  Install Foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup" ;;
            cargo)
                echo "  Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" ;;
            python3)
                echo "  Install Python 3: https://www.python.org/downloads/" ;;
        esac
        exit 1
    fi
done

# Check ecdsa package for signature-verified example
if [[ "$EXAMPLE" == "signature-verified" || "$EXAMPLE" == "all" ]]; then
    if ! python3 -c "import ecdsa" 2>/dev/null; then
        err "Python 'ecdsa' package required for signature-verified example"
        echo "  Install with: pip install ecdsa"
        exit 1
    fi
fi

ok "All prerequisites met"

# ── 2. Start Anvil (local only) ──────────────────────────────────────────────

if [[ "$NETWORK" == "local" ]]; then
    log "Starting Anvil on port ${ANVIL_PORT}..."

    # Kill any existing anvil on that port
    if lsof -ti:${ANVIL_PORT} &>/dev/null; then
        log "Killing existing process on port ${ANVIL_PORT}"
        kill $(lsof -ti:${ANVIL_PORT}) 2>/dev/null || true
        sleep 1
    fi

    anvil --port ${ANVIL_PORT} --silent &
    ANVIL_PID=$!
    sleep 2

    if ! kill -0 "$ANVIL_PID" 2>/dev/null; then
        err "Failed to start Anvil"
        exit 1
    fi
    ok "Anvil running (PID: $ANVIL_PID)"
elif [[ "$NETWORK" == "sepolia" ]]; then
    RPC_URL="${SEPOLIA_RPC_URL:?SEPOLIA_RPC_URL env var must be set}"
    DEPLOYER_KEY="${PRIVATE_KEY:?PRIVATE_KEY env var must be set for testnet deployment}"
    DEPLOYER_ADDR=$(cast wallet address "$DEPLOYER_KEY" 2>/dev/null)
    PROVER_KEY="${PROVER_PRIVATE_KEY:?PROVER_PRIVATE_KEY env var must be set for testnet deployment}"
    PROVER_ADDR=$(cast wallet address "$PROVER_KEY" 2>/dev/null)
    REQUESTER_KEY="${REQUESTER_PRIVATE_KEY:?REQUESTER_PRIVATE_KEY env var must be set for testnet deployment}"
    REQUESTER_ADDR=$(cast wallet address "$REQUESTER_KEY" 2>/dev/null)

    log "Using Sepolia network"
    log "RPC URL: $RPC_URL"
    log "Deployer: $DEPLOYER_ADDR"
fi

# ── 3. Deploy contracts ──────────────────────────────────────────────────────

log "Deploying contracts (network: $NETWORK)..."

cd "$ROOT_DIR/contracts"

# Install forge dependencies if needed
if [ ! -d "lib/forge-std" ]; then
    log "Installing forge-std..."
    forge install foundry-rs/forge-std --no-git
fi
if [ ! -d "lib/risc0-ethereum" ]; then
    log "Installing risc0-ethereum..."
    forge install risc0/risc0-ethereum --no-git
fi

if [[ "$NETWORK" == "local" ]]; then
    PRIVATE_KEY="$DEPLOYER_KEY" FEE_RECIPIENT="$DEPLOYER_ADDR" ETHERSCAN_API_KEY="dummy" \
        forge script script/DeployLocal.s.sol:DeployLocalScript \
        --rpc-url "$RPC_URL" \
        --broadcast \
        --silent 2>&1 | tail -5
elif [[ "$NETWORK" == "sepolia" ]]; then
    FORGE_OUTPUT=""
    FORGE_EXIT=0
    FORGE_OUTPUT=$(PRIVATE_KEY="$DEPLOYER_KEY" FEE_RECIPIENT="$DEPLOYER_ADDR" \
        VERIFIER_ADDRESS="$SEPOLIA_VERIFIER_ADDRESS" \
        ETHERSCAN_API_KEY="${ETHERSCAN_API_KEY:-dummy}" \
        forge script script/DeployTestnet.s.sol:DeployTestnetScript \
        --rpc-url "$RPC_URL" \
        --broadcast \
        2>&1) || FORGE_EXIT=$?

    echo "$FORGE_OUTPUT" | tail -20

    if [ "$FORGE_EXIT" -ne 0 ]; then
        err "Forge deployment failed (exit code: $FORGE_EXIT)"
        echo "$FORGE_OUTPUT" >&2
        exit 1
    fi

    REGISTRY_ADDR=$(echo "$FORGE_OUTPUT" | grep "ProgramRegistry deployed at:" | awk '{print $NF}')
    ENGINE_ADDR=$(echo "$FORGE_OUTPUT" | grep "ExecutionEngine deployed at:" | awk '{print $NF}')
    VERIFIER_ADDR="$SEPOLIA_VERIFIER_ADDRESS"

    if [ -z "$REGISTRY_ADDR" ] || [ -z "$ENGINE_ADDR" ]; then
        err "Failed to extract deployed contract addresses from forge output"
        echo "$FORGE_OUTPUT" >&2
        exit 1
    fi
fi

cd "$ROOT_DIR"

# Verify deployment
DEPLOYED_CODE=$(cast code --rpc-url "$RPC_URL" "$ENGINE_ADDR" 2>/dev/null)
if [ "$DEPLOYED_CODE" = "0x" ] || [ -z "$DEPLOYED_CODE" ]; then
    err "Contract deployment failed - no code at expected address"
    exit 1
fi
ok "Contracts deployed (Verifier=$VERIFIER_ADDR, Registry=$REGISTRY_ADDR, Engine=$ENGINE_ADDR)"

# Fund requester and prover wallets from deployer (testnet only)
if [[ "$NETWORK" != "local" ]]; then
    log "Checking wallet balances..."
    DEPLOYER_BAL=$(cast balance --rpc-url "$RPC_URL" "$DEPLOYER_ADDR" --ether 2>/dev/null || echo "0")
    REQUESTER_BAL=$(cast balance --rpc-url "$RPC_URL" "$REQUESTER_ADDR" --ether 2>/dev/null || echo "0")
    PROVER_BAL=$(cast balance --rpc-url "$RPC_URL" "$PROVER_ADDR" --ether 2>/dev/null || echo "0")
    log "Balances: deployer=$DEPLOYER_BAL, requester=$REQUESTER_BAL, prover=$PROVER_BAL"

    # Fund requester (needs gas for requestExecution + tip)
    log "Funding requester wallet..."
    if cast send --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" \
        "$REQUESTER_ADDR" --value 0.01ether >/dev/null 2>&1; then
        ok "Transferred 0.01 ETH to requester"
    else
        log "Warning: Could not fund requester (deployer may be low on ETH)"
    fi

    # Wait for first tx to confirm before sending second (avoid nonce conflict)
    sleep 5

    # Fund prover (needs gas for submitResult)
    log "Funding prover wallet..."
    if cast send --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY" \
        "$PROVER_ADDR" --value 0.01ether >/dev/null 2>&1; then
        ok "Transferred 0.01 ETH to prover"
    else
        log "Warning: Could not fund prover (deployer may be low on ETH)"
    fi
fi

# ── Run a single example ─────────────────────────────────────────────────────

run_example() {
    local example_name="$1"
    local image_id

    case "$example_name" in
        anomaly-detector)      image_id="$ANOMALY_DETECTOR_IMAGE_ID" ;;
        signature-verified)    image_id="$SIGNATURE_VERIFIED_IMAGE_ID" ;;
        sybil-detector)        image_id="$SYBIL_DETECTOR_IMAGE_ID" ;;
        rule-engine)           image_id="$RULE_ENGINE_IMAGE_ID" ;;
        xgboost-inference)     image_id="$XGBOOST_INFERENCE_IMAGE_ID" ;;
    esac

    log "━━━ Running E2E test: $example_name ━━━"
    log "Image ID: $image_id"

    # ── 4. Register program ───────────────────────────────────────────────
    log "Registering program in ProgramRegistry..."

    cast send --rpc-url "$RPC_URL" \
        --private-key "$DEPLOYER_KEY" \
        "$REGISTRY_ADDR" \
        "registerProgram(bytes32,string,string,bytes32)" \
        "$image_id" \
        "$example_name" \
        "file://local" \
        "0x0000000000000000000000000000000000000000000000000000000000000000" \
        >/dev/null 2>&1

    # Verify registration
    IS_ACTIVE=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" \
        "isProgramActive(bytes32)(bool)" "$image_id" 2>/dev/null)
    if [ "$IS_ACTIVE" != "true" ]; then
        err "Program registration failed"
        return 1
    fi
    ok "Program registered"

    # ── 5. Generate test input ────────────────────────────────────────────
    log "Generating test input..."

    INPUT_OUTPUT=$(python3 "$SCRIPT_DIR/generate-test-input.py" "$example_name")
    DATA_URL=$(echo "$INPUT_OUTPUT" | grep "^DATA_URL=" | cut -d'=' -f2-)
    INPUT_DIGEST=$(echo "$INPUT_OUTPUT" | grep "^INPUT_DIGEST=" | cut -d'=' -f2-)

    if [ -z "$DATA_URL" ] || [ -z "$INPUT_DIGEST" ]; then
        err "Failed to generate test input"
        return 1
    fi
    ok "Input generated (digest: ${INPUT_DIGEST:0:18}...)"

    # ── 6. Submit execution request ───────────────────────────────────────
    log "Submitting execution request..."

    TX_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
        --private-key "$REQUESTER_KEY" \
        "$ENGINE_ADDR" \
        "requestExecution(bytes32,bytes32,string,address,uint256)" \
        "$image_id" \
        "$INPUT_DIGEST" \
        "$DATA_URL" \
        "0x0000000000000000000000000000000000000000" \
        3600 \
        --value 0.001ether \
        --gas-limit 1500000 \
        2>&1)

    # Check for failure
    if echo "$TX_OUTPUT" | grep -qi "revert\|error\|fail"; then
        err "Execution request transaction failed"
        echo "$TX_OUTPUT" >&2
        return 1
    fi

    # Extract tx hash (try multiple field names)
    TX_HASH=$(echo "$TX_OUTPUT" | grep -i "transactionHash\|hash" | head -1 | awk '{print $NF}')
    log "TX output (last 5 lines):"
    echo "$TX_OUTPUT" | tail -5

    # Wait until the request is visible on-chain (handles block confirmation delays)
    if [[ "$NETWORK" != "local" ]]; then
        log "Waiting for request to appear on-chain..."
        for i in $(seq 1 12); do
            NEXT_ID=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" "nextRequestId()(uint256)" 2>/dev/null || echo "1")
            if [ "$NEXT_ID" != "1" ] && [ -n "$NEXT_ID" ]; then
                log "Request confirmed on-chain (nextRequestId=$NEXT_ID)"
                break
            fi
            sleep 5
        done
        if [ "$NEXT_ID" = "1" ] || [ -z "$NEXT_ID" ]; then
            err "Request not found on-chain after 60s"
            return 1
        fi
    fi
    ok "Execution request submitted and confirmed"

    # Verify getPendingRequests works
    NEXT_ID=${NEXT_ID:-$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" "nextRequestId()(uint256)" 2>/dev/null || echo "?")}
    PENDING=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" "getPendingRequests(uint256,uint256)(uint256[])" 0 50 2>/dev/null || echo "[]")
    log "Contract state: nextRequestId=$NEXT_ID, pending=$PENDING"

    # ── 7. Run prover ─────────────────────────────────────────────────────
    log "Starting prover..."

    local PROVE_FEATURES=""
    local TIMEOUT=$CPU_TIMEOUT
    local PROVING_MODE="local"
    local SNARK_FLAG=""

    if [ "$GPU" = true ]; then
        TIMEOUT=$GPU_TIMEOUT
        PROVING_MODE="gpu"
        case "$(uname -s)" in
            Darwin*) PROVE_FEATURES="--features metal" ;;
            Linux*)  PROVE_FEATURES="--features cuda" ;;
        esac
    fi

    # On testnet, enable SNARK (Groth16) for real on-chain verification
    if [[ "$NETWORK" != "local" ]]; then
        SNARK_FLAG="--use-snark"
        TIMEOUT=600  # Groth16 proving takes longer
        log "SNARK mode enabled (Groth16 for on-chain verification)"

        # Use Bonsai cloud proving if API key is available (works on any arch)
        if [[ -n "${BONSAI_API_KEY:-}" ]]; then
            PROVING_MODE="bonsai"
            log "Bonsai cloud proving enabled (API key detected)"
        fi
    fi

    # Build prover (prover has its own Cargo.toml, not a workspace member)
    log "Building prover (this may take a while on first run)..."
    cargo build --release --manifest-path "$ROOT_DIR/prover/Cargo.toml" $PROVE_FEATURES 2>&1 | tail -3

    # Run prover in background (run from repo root so ./programs/ is accessible)
    log "Running prover (mode: $PROVING_MODE, snark: ${SNARK_FLAG:-off}, timeout: ${TIMEOUT}s)..."

    "$ROOT_DIR/prover/target/release/world-zk-prover" run \
        --rpc-url "$RPC_URL" \
        --private-key "$PROVER_KEY" \
        --engine-address "$ENGINE_ADDR" \
        --registry-address "$REGISTRY_ADDR" \
        --proving-mode "$PROVING_MODE" \
        --min-tip 0 \
        --skip-profitability-check \
        --health-port 0 \
        $SNARK_FLAG \
        2>&1 &
    PROVER_PID=$!

    ok "Prover started (PID: $PROVER_PID)"

    # ── 8. Wait for completion ────────────────────────────────────────────
    log "Waiting for proof completion (timeout: ${TIMEOUT}s)..."

    local ELAPSED=0
    local POLL_INTERVAL=5
    local COMPLETED=false

    while [ $ELAPSED -lt $TIMEOUT ]; do
        sleep $POLL_INTERVAL
        ELAPSED=$((ELAPSED + POLL_INTERVAL))

        # Check if prover is still running
        if ! kill -0 "$PROVER_PID" 2>/dev/null; then
            log "Prover process exited"
            break
        fi

        # Check request status via raw hex (RequestStatus enum: 0=Pending, 1=Claimed, 2=Completed)
        # Status is field 10 in the struct: skip 0x(2) + offset_ptr(64) + 10*field(640) = char 706, 64 chars wide
        RAW_HEX=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" "getRequest(uint256)" 1 2>/dev/null || echo "")
        STATUS_WORD=$(echo "$RAW_HEX" | tr -d '[:space:]' | cut -c707-770)
        STATUS=$(echo "$STATUS_WORD" | sed 's/^0*//')
        STATUS=${STATUS:-0}

        case "$STATUS" in
            0) printf "\r  [%3ds] Status: Pending   " "$ELAPSED" ;;
            1) printf "\r  [%3ds] Status: Claimed   " "$ELAPSED" ;;
            2)
                printf "\r  [%3ds] Status: Completed!\n" "$ELAPSED"
                COMPLETED=true
                break
                ;;
            *)
                printf "\r  [%3ds] Status: Unknown (%s)" "$ELAPSED" "$STATUS"
                ;;
        esac
    done

    # Kill prover
    if [ -n "$PROVER_PID" ] && kill -0 "$PROVER_PID" 2>/dev/null; then
        kill "$PROVER_PID" 2>/dev/null || true
        wait "$PROVER_PID" 2>/dev/null || true
    fi
    PROVER_PID=""

    # ── 9. Verify ─────────────────────────────────────────────────────────
    if [ "$COMPLETED" = true ]; then
        echo ""
        ok "E2E test PASSED: $example_name"
        log "Proof was generated and verified on-chain successfully"
        return 0
    else
        echo ""
        err "E2E test FAILED: $example_name (timed out after ${TIMEOUT}s)"
        return 1
    fi
}

# ── Main execution ────────────────────────────────────────────────────────────

FAILED=0

if [[ "$EXAMPLE" == "all" ]]; then
    for ex in anomaly-detector signature-verified sybil-detector rule-engine xgboost-inference; do
        if ! run_example "$ex"; then
            FAILED=1
        fi
        echo ""
    done
else
    if ! run_example "$EXAMPLE"; then
        FAILED=1
    fi
fi

echo ""
if [ $FAILED -eq 0 ]; then
    log "All E2E tests passed!"
else
    err "Some E2E tests failed"
fi

exit $FAILED
