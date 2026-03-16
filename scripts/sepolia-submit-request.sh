#!/usr/bin/env bash
# =============================================================================
# Submit an Execution Request on Sepolia (ExecutionEngine)
#
# Submits an execution request to the ExecutionEngine contract. Parses the
# request ID from emitted events and prints a summary with the tx hash.
#
# Usage:
#   ./scripts/sepolia-submit-request.sh \
#     --image-id 0xf85c...33 \
#     --input-hash 0xabcd...ef
#
#   ./scripts/sepolia-submit-request.sh --help
#
# Exit codes:
#   0 -- request submitted successfully
#   1 -- error
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' RESET=''
fi

info()    { printf "${CYAN}[INFO]${RESET}  %s\n" "$*"; }
ok()      { printf "${GREEN}[PASS]${RESET}  %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[FAIL]${RESET}  %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOY_FILE="$ROOT_DIR/deployments/11155111.json"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
IMAGE_ID=""
INPUT_HASH=""
INPUT_URL=""
VALUE="0.001ether"
TIMEOUT="3600"
RPC_URL="${ALCHEMY_SEPOLIA_RPC_URL:-https://ethereum-sepolia-rpc.publicnode.com}"
ENGINE_ADDR=""
PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-${REQUESTER_PRIVATE_KEY:-}}"

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
sepolia-submit-request.sh -- Submit an execution request on Sepolia.

USAGE:
  scripts/sepolia-submit-request.sh --image-id <hex> --input-hash <hex> [OPTIONS]

REQUIRED:
  --image-id <hex>          Program image ID (0x + 64 hex chars)
  --input-hash <hex>        Input digest / hash (0x + 64 hex chars)

OPTIONS:
  --input-url <url>         URL where prover can fetch inputs (default: empty)
  --value <amount>          ETH to send as tip (default: 0.001ether)
  --timeout <seconds>       Expiration timeout in seconds (default: 3600)
  --rpc-url <url>           RPC endpoint (default: ALCHEMY_SEPOLIA_RPC_URL or public node)
  --engine-address <addr>   ExecutionEngine address (default: from deployments/11155111.json)
  --private-key <key>       Requester private key (default: DEPLOYER_PRIVATE_KEY or
                            REQUESTER_PRIVATE_KEY env)
  --help, -h                Show this help message

ENVIRONMENT:
  DEPLOYER_PRIVATE_KEY      Fallback private key for the transaction sender
  REQUESTER_PRIVATE_KEY     Alternative private key for the transaction sender
  ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC URL (fallback if --rpc-url not given)

EXAMPLES:
  # Submit with minimal flags
  DEPLOYER_PRIVATE_KEY=0xabc... \
    ./scripts/sepolia-submit-request.sh \
      --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
      --input-hash 0x3333333333333333333333333333333333333333333333333333333333333333

  # Submit with custom value and timeout
  REQUESTER_PRIVATE_KEY=0xdef... \
    ./scripts/sepolia-submit-request.sh \
      --image-id 0xf85c...33 \
      --input-hash 0xabcd...ef \
      --value 0.01ether \
      --timeout 7200 \
      --input-url "https://example.com/inputs/123"
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --image-id)
            IMAGE_ID="$2"
            shift 2
            ;;
        --input-hash)
            INPUT_HASH="$2"
            shift 2
            ;;
        --input-url)
            INPUT_URL="$2"
            shift 2
            ;;
        --value)
            VALUE="$2"
            shift 2
            ;;
        --timeout)
            TIMEOUT="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        --engine-address)
            ENGINE_ADDR="$2"
            shift 2
            ;;
        --private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate prerequisites
# ---------------------------------------------------------------------------
header "Checking prerequisites..."

for tool in cast jq python3; do
    if ! command -v "$tool" &>/dev/null; then
        err "$tool is required. Install Foundry: curl -L https://foundry.paradigm.xyz | bash"
        exit 1
    fi
done
ok "Required tools found (cast, jq, python3)"

# ---------------------------------------------------------------------------
# Validate required arguments
# ---------------------------------------------------------------------------
MISSING=false

if [[ -z "$IMAGE_ID" ]]; then
    err "--image-id is required"
    MISSING=true
fi

if [[ -z "$INPUT_HASH" ]]; then
    err "--input-hash is required"
    MISSING=true
fi

if [[ -z "$PRIVATE_KEY" ]]; then
    err "Private key required. Set DEPLOYER_PRIVATE_KEY, REQUESTER_PRIVATE_KEY, or use --private-key."
    MISSING=true
fi

if [[ "$MISSING" == "true" ]]; then
    echo ""
    err "Missing required arguments. Run with --help for details."
    exit 1
fi

# ---------------------------------------------------------------------------
# Resolve engine address from deployment file if not provided
# ---------------------------------------------------------------------------
if [[ -z "$ENGINE_ADDR" ]]; then
    if [[ -f "$DEPLOY_FILE" ]]; then
        ENGINE_ADDR=$(jq -r '.contracts.ExecutionEngine.address // empty' "$DEPLOY_FILE" 2>/dev/null)
        if [[ -z "$ENGINE_ADDR" || "$ENGINE_ADDR" == "null" ]]; then
            err "ExecutionEngine address not found in $DEPLOY_FILE"
            err "Use --engine-address to specify it manually."
            exit 1
        fi
        info "ExecutionEngine address from deployment file: $ENGINE_ADDR"
    else
        err "No --engine-address given and deployment file not found: $DEPLOY_FILE"
        err "Deploy contracts first or specify --engine-address."
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Derive sender address
# ---------------------------------------------------------------------------
SENDER_ADDR=$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null) || {
    err "Invalid private key. Could not derive address."
    exit 1
}

# ---------------------------------------------------------------------------
# Banner
# ---------------------------------------------------------------------------
header "============================================================"
header "  Submit Execution Request -- Sepolia"
header "============================================================"
echo ""
info "RPC:              $RPC_URL"
info "Sender:           $SENDER_ADDR"
info "Engine:           $ENGINE_ADDR"
info "Image ID:         $IMAGE_ID"
info "Input Hash:       $INPUT_HASH"
info "Input URL:        ${INPUT_URL:-<empty>}"
info "Value:            $VALUE"
info "Timeout:          ${TIMEOUT}s"
echo ""

# ---------------------------------------------------------------------------
# Step 1: Check sender balance
# ---------------------------------------------------------------------------
header "Step 1/3: Check sender balance"

BALANCE_WEI=$(cast balance "$SENDER_ADDR" --rpc-url "$RPC_URL" 2>/dev/null) || {
    warn "Could not check sender balance (RPC may be unreachable)"
    BALANCE_WEI="0"
}
BALANCE_ETH=$(cast from-wei "$BALANCE_WEI" 2>/dev/null || echo "unknown")
info "Sender balance: $BALANCE_ETH ETH"

# ---------------------------------------------------------------------------
# Step 2: Submit the execution request
# ---------------------------------------------------------------------------
header "Step 2/3: Submit execution request"

info "Calling requestExecution..."

TX_OUTPUT=$(cast send "$ENGINE_ADDR" \
    'requestExecution(bytes32,bytes32,string,address,uint256)' \
    "$IMAGE_ID" \
    "$INPUT_HASH" \
    "$INPUT_URL" \
    "0x0000000000000000000000000000000000000000" \
    "$TIMEOUT" \
    --value "$VALUE" \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    --json 2>&1) || {
    err "requestExecution transaction failed"
    err "Output: $TX_OUTPUT"
    exit 1
}

TX_STATUS=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo "")
TX_HASH=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('transactionHash',''))" 2>/dev/null || echo "")
TX_GAS=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('gasUsed','0x0'),16))" 2>/dev/null || echo "n/a")
TX_BLOCK=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('blockNumber','0x0'),16))" 2>/dev/null || echo "n/a")

if [[ "$TX_STATUS" != "0x1" ]]; then
    err "Transaction reverted (status=$TX_STATUS)"
    err "TX hash: $TX_HASH"
    exit 1
fi

ok "Transaction succeeded (gas: $TX_GAS)"
info "TX hash: $TX_HASH"
info "Block:   $TX_BLOCK"

# ---------------------------------------------------------------------------
# Step 3: Parse request ID from event logs
# ---------------------------------------------------------------------------
header "Step 3/3: Parse request ID from events"

# The ExecutionRequested event signature:
#   ExecutionRequested(uint256 indexed requestId, address indexed requester, bytes32 indexed imageId, ...)
# Topic[0] = event signature hash
# Topic[1] = requestId (indexed uint256)
REQUEST_ID=$(echo "$TX_OUTPUT" | python3 -c "
import sys, json
tx = json.load(sys.stdin)
for log in tx.get('logs', []):
    topics = log.get('topics', [])
    # ExecutionRequested has at least 3 indexed params (requestId, requester, imageId)
    if len(topics) >= 4:
        # requestId is topics[1], as a hex uint256
        rid = int(topics[1], 16)
        print(rid)
        break
" 2>/dev/null || echo "")

if [[ -z "$REQUEST_ID" ]]; then
    # Fallback: query nextRequestId and subtract 1
    NEXT_ID=$(cast call "$ENGINE_ADDR" \
        'nextRequestId()(uint256)' \
        --rpc-url "$RPC_URL" 2>/dev/null) || NEXT_ID=""

    if [[ -n "$NEXT_ID" ]]; then
        # Strip any cast annotation like " [1e3]"
        NEXT_ID="${NEXT_ID%% \[*\]}"
        REQUEST_ID=$((NEXT_ID - 1))
        info "Request ID inferred from nextRequestId: $REQUEST_ID"
    else
        warn "Could not determine request ID from events or contract state"
        REQUEST_ID="unknown"
    fi
else
    ok "Request ID parsed from event: $REQUEST_ID"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}%-20s${RESET} %s\n" "Status:" "Submitted"
printf "  ${BOLD}%-20s${RESET} %s\n" "Request ID:" "$REQUEST_ID"
printf "  ${BOLD}%-20s${RESET} %s\n" "Image ID:" "$IMAGE_ID"
printf "  ${BOLD}%-20s${RESET} %s\n" "Input Hash:" "$INPUT_HASH"
printf "  ${BOLD}%-20s${RESET} %s\n" "Input URL:" "${INPUT_URL:-<empty>}"
printf "  ${BOLD}%-20s${RESET} %s\n" "Value:" "$VALUE"
printf "  ${BOLD}%-20s${RESET} %s\n" "Timeout:" "${TIMEOUT}s"
printf "  ${BOLD}%-20s${RESET} %s\n" "TX Hash:" "$TX_HASH"
printf "  ${BOLD}%-20s${RESET} %s\n" "Block:" "$TX_BLOCK"
printf "  ${BOLD}%-20s${RESET} %s\n" "Gas Used:" "$TX_GAS"
printf "  ${BOLD}%-20s${RESET} %s\n" "Engine:" "$ENGINE_ADDR"
printf "  ${BOLD}%-20s${RESET} %s\n" "Sender:" "$SENDER_ADDR"
echo ""

if [[ -n "$TX_HASH" ]]; then
    echo "  Etherscan: https://sepolia.etherscan.io/tx/${TX_HASH}"
    echo ""
fi

ok "Execution request submitted."
