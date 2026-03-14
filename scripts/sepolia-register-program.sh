#!/usr/bin/env bash
# =============================================================================
# Register a Program on Sepolia (ProgramRegistry)
#
# Registers a program in ProgramRegistry. Validates the image-id format,
# checks whether the program is already registered, and verifies after
# registration.
#
# Usage:
#   ./scripts/sepolia-register-program.sh \
#     --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
#     --name "xgboost-rule-engine"
#
#   ./scripts/sepolia-register-program.sh --help
#
# Exit codes:
#   0 -- program registered successfully (or was already registered)
#   1 -- error
# =============================================================================

set -uo pipefail

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
PROGRAM_NAME=""
PROGRAM_URL=""
INPUT_SCHEMA="0x0000000000000000000000000000000000000000000000000000000000000000"
RPC_URL="${ALCHEMY_SEPOLIA_RPC_URL:-https://ethereum-sepolia-rpc.publicnode.com}"
REGISTRY_ADDR=""
PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-}"

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
sepolia-register-program.sh -- Register a program in ProgramRegistry on Sepolia.

USAGE:
  scripts/sepolia-register-program.sh --image-id <hex> --name <name> [OPTIONS]

REQUIRED:
  --image-id <hex>          66-char hex image ID (0x + 64 hex chars)
  --name <name>             Human-readable program name

OPTIONS:
  --url <url>               Program URL (default: empty string)
  --schema <bytes32>        Input schema hash (default: 0x000...000)
  --rpc-url <url>           RPC endpoint (default: ALCHEMY_SEPOLIA_RPC_URL or public node)
  --registry-address <addr> ProgramRegistry address (default: from deployments/11155111.json)
  --private-key <key>       Deployer private key (default: DEPLOYER_PRIVATE_KEY env)
  --help, -h                Show this help message

ENVIRONMENT:
  DEPLOYER_PRIVATE_KEY      Private key for the transaction sender
  ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC URL (fallback if --rpc-url not given)

EXAMPLES:
  # Register with minimal flags
  DEPLOYER_PRIVATE_KEY=0xabc... \
    ./scripts/sepolia-register-program.sh \
      --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
      --name "xgboost-rule-engine"

  # Register with all options
  DEPLOYER_PRIVATE_KEY=0xabc... \
    ./scripts/sepolia-register-program.sh \
      --image-id 0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33 \
      --name "xgboost-rule-engine" \
      --url "https://github.com/worldcoin/world-zk-compute" \
      --rpc-url https://eth-sepolia.g.alchemy.com/v2/KEY \
      --registry-address 0x1234...
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
        --name)
            PROGRAM_NAME="$2"
            shift 2
            ;;
        --url)
            PROGRAM_URL="$2"
            shift 2
            ;;
        --schema)
            INPUT_SCHEMA="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        --registry-address)
            REGISTRY_ADDR="$2"
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

if [[ -z "$PROGRAM_NAME" ]]; then
    err "--name is required"
    MISSING=true
fi

if [[ -z "$PRIVATE_KEY" ]]; then
    err "Private key required. Set DEPLOYER_PRIVATE_KEY or use --private-key."
    MISSING=true
fi

if [[ "$MISSING" == "true" ]]; then
    echo ""
    err "Missing required arguments. Run with --help for details."
    exit 1
fi

# ---------------------------------------------------------------------------
# Validate image ID format (must be 0x + 64 hex chars = 66 chars total)
# ---------------------------------------------------------------------------
if [[ ${#IMAGE_ID} -ne 66 ]]; then
    err "Image ID must be 66 characters (0x + 64 hex digits). Got ${#IMAGE_ID} characters."
    exit 1
fi

if [[ ! "$IMAGE_ID" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
    err "Image ID must be valid hex: 0x followed by 64 hex digits."
    err "Got: $IMAGE_ID"
    exit 1
fi
ok "Image ID format valid (66-char hex)"

# ---------------------------------------------------------------------------
# Resolve registry address from deployment file if not provided
# ---------------------------------------------------------------------------
if [[ -z "$REGISTRY_ADDR" ]]; then
    if [[ -f "$DEPLOY_FILE" ]]; then
        REGISTRY_ADDR=$(jq -r '.contracts.ProgramRegistry.address // empty' "$DEPLOY_FILE" 2>/dev/null)
        if [[ -z "$REGISTRY_ADDR" || "$REGISTRY_ADDR" == "null" ]]; then
            err "ProgramRegistry address not found in $DEPLOY_FILE"
            err "Use --registry-address to specify it manually."
            exit 1
        fi
        info "ProgramRegistry address from deployment file: $REGISTRY_ADDR"
    else
        err "No --registry-address given and deployment file not found: $DEPLOY_FILE"
        err "Deploy contracts first or specify --registry-address."
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
header "  Register Program -- Sepolia"
header "============================================================"
echo ""
info "RPC:              $RPC_URL"
info "Sender:           $SENDER_ADDR"
info "Registry:         $REGISTRY_ADDR"
info "Image ID:         $IMAGE_ID"
info "Name:             $PROGRAM_NAME"
info "URL:              ${PROGRAM_URL:-<empty>}"
info "Schema:           $INPUT_SCHEMA"
echo ""

# ---------------------------------------------------------------------------
# Step 1: Check if program is already registered
# ---------------------------------------------------------------------------
header "Step 1/3: Check existing registration"

IS_ACTIVE=$(cast call "$REGISTRY_ADDR" \
    'isProgramActive(bytes32)(bool)' \
    "$IMAGE_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null) || IS_ACTIVE=""

if [[ "$IS_ACTIVE" == "true" ]]; then
    ok "Program is already registered and active. Nothing to do."

    header "============================================================"
    header "  SUMMARY"
    header "============================================================"
    echo ""
    printf "  ${BOLD}%-20s${RESET} %s\n" "Status:" "Already registered"
    printf "  ${BOLD}%-20s${RESET} %s\n" "Image ID:" "$IMAGE_ID"
    printf "  ${BOLD}%-20s${RESET} %s\n" "Active:" "true"
    printf "  ${BOLD}%-20s${RESET} %s\n" "Registry:" "$REGISTRY_ADDR"
    echo ""
    ok "Program registration verified."
    exit 0
fi

info "Program not yet active. Proceeding with registration."

# ---------------------------------------------------------------------------
# Step 2: Register the program
# ---------------------------------------------------------------------------
header "Step 2/3: Send registerProgram transaction"

TX_OUTPUT=$(cast send "$REGISTRY_ADDR" \
    'registerProgram(bytes32,string,string,bytes32)' \
    "$IMAGE_ID" \
    "$PROGRAM_NAME" \
    "$PROGRAM_URL" \
    "$INPUT_SCHEMA" \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    --json 2>&1) || {
    err "registerProgram transaction failed"
    err "Output: $TX_OUTPUT"
    exit 1
}

TX_STATUS=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo "")
TX_HASH=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('transactionHash',''))" 2>/dev/null || echo "")
TX_GAS=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('gasUsed','0x0'),16))" 2>/dev/null || echo "n/a")

if [[ "$TX_STATUS" != "0x1" ]]; then
    err "Transaction reverted (status=$TX_STATUS)"
    err "TX hash: $TX_HASH"
    exit 1
fi

ok "Transaction succeeded (gas: $TX_GAS)"
info "TX hash: $TX_HASH"

# ---------------------------------------------------------------------------
# Step 3: Verify registration on-chain
# ---------------------------------------------------------------------------
header "Step 3/3: Verify registration"

IS_ACTIVE_AFTER=$(cast call "$REGISTRY_ADDR" \
    'isProgramActive(bytes32)(bool)' \
    "$IMAGE_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null) || IS_ACTIVE_AFTER=""

if [[ "$IS_ACTIVE_AFTER" == "true" ]]; then
    ok "Program is active after registration"
else
    err "Program is NOT active after registration (got: $IS_ACTIVE_AFTER)"
    exit 1
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}%-20s${RESET} %s\n" "Status:" "Registered"
printf "  ${BOLD}%-20s${RESET} %s\n" "Image ID:" "$IMAGE_ID"
printf "  ${BOLD}%-20s${RESET} %s\n" "Name:" "$PROGRAM_NAME"
printf "  ${BOLD}%-20s${RESET} %s\n" "URL:" "${PROGRAM_URL:-<empty>}"
printf "  ${BOLD}%-20s${RESET} %s\n" "Schema:" "$INPUT_SCHEMA"
printf "  ${BOLD}%-20s${RESET} %s\n" "Active:" "$IS_ACTIVE_AFTER"
printf "  ${BOLD}%-20s${RESET} %s\n" "TX Hash:" "$TX_HASH"
printf "  ${BOLD}%-20s${RESET} %s\n" "Gas Used:" "$TX_GAS"
printf "  ${BOLD}%-20s${RESET} %s\n" "Registry:" "$REGISTRY_ADDR"
echo ""

if [[ -n "$TX_HASH" ]]; then
    echo "  Etherscan: https://sepolia.etherscan.io/tx/${TX_HASH}"
    echo ""
fi

ok "Program registration complete."
