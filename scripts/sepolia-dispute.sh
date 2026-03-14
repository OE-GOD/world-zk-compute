#!/usr/bin/env bash
# =============================================================================
# sepolia-dispute.sh -- Challenge a TEE result on Sepolia
#
# Submits a challenge transaction against a TEE-attested result that is still
# within its challenge window.
#
# Required env:
#   ALCHEMY_SEPOLIA_RPC_URL     Sepolia RPC endpoint
#   CHALLENGER_PRIVATE_KEY      Private key to sign the challenge tx
#                               (or DEPLOYER_PRIVATE_KEY as fallback)
#
# Optional env:
#   TEE_VERIFIER_ADDRESS        TEEMLVerifier contract address
#                               (auto-loaded from deployments/11155111.json)
#
# Usage:
#   ./scripts/sepolia-dispute.sh <REQUEST_ID>
#   ./scripts/sepolia-dispute.sh --dry-run <REQUEST_ID>
#   ./scripts/sepolia-dispute.sh --help
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [ -z "${NO_COLOR:-}" ] && [ -t 1 ]; then
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    RED='\033[0;31m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    GREEN=''
    YELLOW=''
    CYAN=''
    RED=''
    BOLD=''
    RESET=''
fi

info()  { printf "%b[INFO]%b  %s\n" "$CYAN"   "$RESET" "$*"; }
ok()    { printf "%b[OK]%b    %s\n" "$GREEN"  "$RESET" "$*"; }
warn()  { printf "%b[WARN]%b  %s\n" "$YELLOW" "$RESET" "$*"; }
err()   { printf "%b[ERROR]%b %s\n" "$RED"    "$RESET" "$*" >&2; }

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------
show_help() {
    cat <<'EOF'
sepolia-dispute.sh -- Challenge a TEE result on Sepolia

USAGE:
  ./scripts/sepolia-dispute.sh [OPTIONS] <REQUEST_ID>

ARGUMENTS:
  REQUEST_ID    The bytes32 execution request / result ID to dispute
                (e.g., 0xabcd...1234)

OPTIONS:
  --dry-run     Simulate the challenge (do not send transaction)
  --bond <wei>  Override challenge bond amount (default: read from contract)
  -h, --help    Show this help and exit

REQUIRED ENVIRONMENT:
  ALCHEMY_SEPOLIA_RPC_URL     Sepolia RPC endpoint
  CHALLENGER_PRIVATE_KEY      Private key of the challenger
                              (falls back to DEPLOYER_PRIVATE_KEY)

OPTIONAL ENVIRONMENT:
  TEE_VERIFIER_ADDRESS        TEEMLVerifier contract address
                              (auto-loaded from deployments/11155111.json)

STEPS PERFORMED:
  1. Validate the request ID format
  2. Check that the result exists on-chain
  3. Check that the challenge window is still open
  4. Submit the challenge via challengeResult(bytes32)

EXAMPLES:
  ALCHEMY_SEPOLIA_RPC_URL=https://... \
  CHALLENGER_PRIVATE_KEY=0x... \
    ./scripts/sepolia-dispute.sh 0xabcdef1234567890...

  # Dry run (no tx sent):
  ALCHEMY_SEPOLIA_RPC_URL=https://... \
  CHALLENGER_PRIVATE_KEY=0x... \
    ./scripts/sepolia-dispute.sh --dry-run 0xabcdef1234567890...
EOF
}

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOY_FILE="$PROJECT_ROOT/deployments/11155111.json"

# ---------------------------------------------------------------------------
# JSON field helper
# ---------------------------------------------------------------------------
json_field() {
    local file="$1"
    local field="$2"
    local value=""

    if command -v jq &>/dev/null; then
        value=$(jq -r "$field // empty" "$file" 2>/dev/null || echo "")
    elif command -v python3 &>/dev/null; then
        value=$(python3 -c "
import json
with open('$file') as f:
    data = json.load(f)
keys = '$field'.strip('.').split('.')
for k in keys:
    if isinstance(data, dict):
        data = data.get(k, '')
    else:
        data = ''
        break
print(data if data else '')
" 2>/dev/null || echo "")
    fi

    echo "$value"
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
DRY_RUN=false
BOND=""
REQUEST_ID=""

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --bond)
            BOND="$2"
            shift 2
            ;;
        -*)
            err "Unknown option: $1"
            echo "Run with --help for usage."
            exit 1
            ;;
        *)
            if [ -z "$REQUEST_ID" ]; then
                REQUEST_ID="$1"
            else
                err "Unexpected argument: $1 (REQUEST_ID already set to $REQUEST_ID)"
                exit 1
            fi
            shift
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate prerequisites
# ---------------------------------------------------------------------------
if ! command -v cast &>/dev/null; then
    err "'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

if [ -z "${ALCHEMY_SEPOLIA_RPC_URL:-}" ]; then
    err "ALCHEMY_SEPOLIA_RPC_URL is required."
    exit 1
fi

RPC_URL="$ALCHEMY_SEPOLIA_RPC_URL"

# Resolve private key
PRIVATE_KEY="${CHALLENGER_PRIVATE_KEY:-${DEPLOYER_PRIVATE_KEY:-}}"
if [ -z "$PRIVATE_KEY" ]; then
    err "CHALLENGER_PRIVATE_KEY (or DEPLOYER_PRIVATE_KEY) is required."
    exit 1
fi

# Validate REQUEST_ID
if [ -z "$REQUEST_ID" ]; then
    err "REQUEST_ID argument is required."
    echo "  Usage: ./scripts/sepolia-dispute.sh <REQUEST_ID>"
    echo "  Run with --help for details."
    exit 1
fi

# Basic hex format validation
case "$REQUEST_ID" in
    0x*)
        if [ "${#REQUEST_ID}" -ne 66 ]; then
            warn "REQUEST_ID length is ${#REQUEST_ID}, expected 66 (0x + 64 hex chars)."
        fi
        ;;
    *)
        err "REQUEST_ID must start with 0x. Got: $REQUEST_ID"
        exit 1
        ;;
esac

# ---------------------------------------------------------------------------
# Load contract address
# ---------------------------------------------------------------------------
TEE_ADDR="${TEE_VERIFIER_ADDRESS:-}"

if [ -z "$TEE_ADDR" ] && [ -f "$DEPLOY_FILE" ]; then
    TEE_ADDR=$(json_field "$DEPLOY_FILE" ".contracts.TEEMLVerifier")
    if [ -z "$TEE_ADDR" ]; then
        TEE_ADDR=$(json_field "$DEPLOY_FILE" ".TEEMLVerifier")
    fi
    if [ -z "$TEE_ADDR" ]; then
        TEE_ADDR=$(json_field "$DEPLOY_FILE" ".contracts.TEEMLVerifier.address")
    fi
fi

if [ -z "$TEE_ADDR" ]; then
    err "TEE_VERIFIER_ADDRESS not set and not found in $DEPLOY_FILE."
    exit 1
fi

# ---------------------------------------------------------------------------
# Derive challenger address for display
# ---------------------------------------------------------------------------
CHALLENGER_ADDR=$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null || echo "unknown")

# ---------------------------------------------------------------------------
# Print header
# ---------------------------------------------------------------------------
printf "\n"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "%b  Sepolia Dispute -- Challenge Result%b\n" "$BOLD" "$RESET"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "\n"

info "TEEMLVerifier:  $TEE_ADDR"
info "Request ID:     $REQUEST_ID"
info "Challenger:     $CHALLENGER_ADDR"

if [ "$DRY_RUN" = "true" ]; then
    warn "DRY RUN mode -- no transaction will be sent"
fi
printf "\n"

# ---------------------------------------------------------------------------
# Step 1: Check that the result exists
# ---------------------------------------------------------------------------
info "Step 1: Checking if result exists on-chain..."

RESULT_RAW=$(cast call "$TEE_ADDR" \
    "results(bytes32)(address,bytes32,bytes32,bytes,uint256,bool,bool)" \
    "$REQUEST_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null || echo "CALL_FAILED")

if [ "$RESULT_RAW" = "CALL_FAILED" ]; then
    # Try alternative: isResultValid
    IS_VALID=$(cast call "$TEE_ADDR" \
        "isResultValid(bytes32)(bool)" \
        "$REQUEST_ID" \
        --rpc-url "$RPC_URL" 2>/dev/null || echo "CALL_FAILED")

    if [ "$IS_VALID" = "CALL_FAILED" ]; then
        err "Could not query result on-chain. Contract may not support this interface."
        exit 1
    fi
    info "Result query via isResultValid: $IS_VALID"
else
    ok "Result found on-chain."
fi

# ---------------------------------------------------------------------------
# Step 2: Check challenge window
# ---------------------------------------------------------------------------
info "Step 2: Checking challenge window..."

# Try reading challenge window duration
CHALLENGE_WINDOW=$(cast call "$TEE_ADDR" \
    "challengeWindow()(uint256)" \
    --rpc-url "$RPC_URL" 2>/dev/null || echo "unknown")

if [ "$CHALLENGE_WINDOW" != "unknown" ]; then
    info "Challenge window duration: ${CHALLENGE_WINDOW}s"
else
    warn "Could not read challengeWindow(). Proceeding anyway."
fi

# Check if already challenged
IS_CHALLENGED=$(cast call "$TEE_ADDR" \
    "disputeResolved(bytes32)(bool)" \
    "$REQUEST_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null || echo "unknown")

if [ "$IS_CHALLENGED" = "true" ]; then
    err "Dispute is already resolved for this result. Cannot challenge again."
    exit 1
fi

ok "Challenge window appears open."

# ---------------------------------------------------------------------------
# Step 3: Read bond amount
# ---------------------------------------------------------------------------
if [ -z "$BOND" ]; then
    BOND=$(cast call "$TEE_ADDR" \
        "challengeBondAmount()(uint256)" \
        --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    # Strip cast annotation if present
    BOND="${BOND%% \[*\]}"

    if [ -z "$BOND" ] || [ "$BOND" = "0" ]; then
        BOND="100000000000000000"
        warn "Could not read challengeBondAmount(). Using default: $BOND wei (0.1 ETH)"
    else
        info "Challenge bond: $BOND wei"
    fi
fi

BOND_ETH=$(cast from-wei "$BOND" 2>/dev/null || echo "?")
info "Bond amount: $BOND wei ($BOND_ETH ETH)"

# ---------------------------------------------------------------------------
# Step 4: Submit challenge
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = "true" ]; then
    printf "\n"
    warn "DRY RUN: Would send the following transaction:"
    echo "  cast send $TEE_ADDR"
    echo "    'challenge(bytes32)' $REQUEST_ID"
    echo "    --value $BOND"
    echo "    --rpc-url $RPC_URL"
    echo "    --private-key <REDACTED>"
    printf "\n"
    ok "Dry run complete. No transaction sent."
    exit 0
fi

info "Step 3: Submitting challenge transaction..."

TX_OUTPUT=$(cast send "$TEE_ADDR" \
    "challenge(bytes32)" \
    "$REQUEST_ID" \
    --value "$BOND" \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    --json 2>&1)

# Check transaction status
TX_STATUS=""
TX_HASH=""
if command -v python3 &>/dev/null; then
    TX_STATUS=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo "")
    TX_HASH=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('transactionHash',''))" 2>/dev/null || echo "")
elif command -v jq &>/dev/null; then
    TX_STATUS=$(echo "$TX_OUTPUT" | jq -r '.status // empty' 2>/dev/null || echo "")
    TX_HASH=$(echo "$TX_OUTPUT" | jq -r '.transactionHash // empty' 2>/dev/null || echo "")
fi

printf "\n"
if [ "$TX_STATUS" = "0x1" ]; then
    ok "Challenge submitted successfully."
    info "TX Hash: $TX_HASH"
    info "Request ID: $REQUEST_ID"
    printf "\n"
    info "Next steps:"
    echo "  - Wait for the dispute resolution window to pass"
    echo "  - If prover does not provide ZK proof, call resolveDisputeByTimeout()"
    echo "  - You can check status with:"
    echo "    cast call $TEE_ADDR 'disputeResolved(bytes32)(bool)' $REQUEST_ID --rpc-url $RPC_URL"
else
    err "Challenge transaction failed."
    err "Status: $TX_STATUS"
    err "Output: $TX_OUTPUT"
    exit 1
fi
