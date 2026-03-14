#!/usr/bin/env bash
# =============================================================================
# Query a TEE Result on Sepolia (TEEMLVerifier)
#
# Queries the on-chain status of a TEE result: validity, challenge status,
# finalization, and time remaining in challenge/dispute windows.
#
# Usage:
#   ./scripts/sepolia-query-result.sh \
#     --result-id 0xabcd...ef
#
#   ./scripts/sepolia-query-result.sh --help
#
# Exit codes:
#   0 -- query succeeded
#   1 -- error (result not found or RPC failure)
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
RESULT_ID=""
RPC_URL="${ALCHEMY_SEPOLIA_RPC_URL:-https://ethereum-sepolia-rpc.publicnode.com}"
TEE_ADDR=""

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
sepolia-query-result.sh -- Query TEE result status on Sepolia.

USAGE:
  scripts/sepolia-query-result.sh --result-id <hex> [OPTIONS]

REQUIRED:
  --result-id <hex>         Result ID (0x + 64 hex chars)

OPTIONS:
  --rpc-url <url>           RPC endpoint (default: ALCHEMY_SEPOLIA_RPC_URL or public node)
  --tee-address <addr>      TEEMLVerifier address (default: from deployments/11155111.json)
  --help, -h                Show this help message

ENVIRONMENT:
  ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC URL (fallback if --rpc-url not given)

EXAMPLES:
  # Query with result ID
  ./scripts/sepolia-query-result.sh \
    --result-id 0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890

  # Query with custom RPC and contract address
  ./scripts/sepolia-query-result.sh \
    --result-id 0xabcd...ef \
    --rpc-url https://eth-sepolia.g.alchemy.com/v2/KEY \
    --tee-address 0x1234...
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --result-id)
            RESULT_ID="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        --tee-address)
            TEE_ADDR="$2"
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
if [[ -z "$RESULT_ID" ]]; then
    err "--result-id is required"
    echo ""
    err "Run with --help for usage details."
    exit 1
fi

# ---------------------------------------------------------------------------
# Resolve TEE address from deployment file if not provided
# ---------------------------------------------------------------------------
if [[ -z "$TEE_ADDR" ]]; then
    if [[ -f "$DEPLOY_FILE" ]]; then
        TEE_ADDR=$(jq -r '.contracts.TEEMLVerifier.address // empty' "$DEPLOY_FILE" 2>/dev/null)
        if [[ -z "$TEE_ADDR" || "$TEE_ADDR" == "null" ]]; then
            err "TEEMLVerifier address not found in $DEPLOY_FILE"
            err "Use --tee-address to specify it manually."
            exit 1
        fi
        info "TEEMLVerifier address from deployment file: $TEE_ADDR"
    else
        err "No --tee-address given and deployment file not found: $DEPLOY_FILE"
        err "Deploy contracts first or specify --tee-address."
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Banner
# ---------------------------------------------------------------------------
header "============================================================"
header "  Query TEE Result -- Sepolia"
header "============================================================"
echo ""
info "RPC:              $RPC_URL"
info "TEEMLVerifier:    $TEE_ADDR"
info "Result ID:        $RESULT_ID"
echo ""

# ---------------------------------------------------------------------------
# Helper: format duration as human-readable string
# ---------------------------------------------------------------------------
format_duration() {
    local secs="$1"
    if [[ "$secs" -le 0 ]]; then
        echo "0s (expired)"
        return
    fi
    local hours=$((secs / 3600))
    local mins=$(((secs % 3600) / 60))
    local remainder_secs=$((secs % 60))
    if [[ "$hours" -gt 0 ]]; then
        echo "${hours}h ${mins}m ${remainder_secs}s"
    elif [[ "$mins" -gt 0 ]]; then
        echo "${mins}m ${remainder_secs}s"
    else
        echo "${remainder_secs}s"
    fi
}

# ---------------------------------------------------------------------------
# Step 1: Check isResultValid
# ---------------------------------------------------------------------------
header "Step 1/4: Check validity"

IS_VALID=$(cast call "$TEE_ADDR" \
    'isResultValid(bytes32)(bool)' \
    "$RESULT_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null) || IS_VALID="error"

if [[ "$IS_VALID" == "true" ]]; then
    ok "Result is VALID"
elif [[ "$IS_VALID" == "false" ]]; then
    info "Result is NOT valid (may be pending, challenged, or non-existent)"
else
    warn "Could not query isResultValid (RPC error or contract issue)"
fi

# ---------------------------------------------------------------------------
# Step 2: Get full result details via getResult
# ---------------------------------------------------------------------------
header "Step 2/4: Get result details"

# getResult returns an MLResult struct as ABI-encoded tuple:
#   (address enclave, address submitter, bytes32 modelHash, bytes32 inputHash,
#    bytes32 resultHash, bytes result, uint256 submittedAt,
#    uint256 challengeDeadline, uint256 disputeDeadline, uint256 challengeBond,
#    uint256 proverStakeAmount, bool finalized, bool challenged, address challenger)
RESULT_RAW=$(cast call "$TEE_ADDR" \
    'getResult(bytes32)((address,address,bytes32,bytes32,bytes32,bytes,uint256,uint256,uint256,uint256,uint256,bool,bool,address))' \
    "$RESULT_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null) || RESULT_RAW=""

if [[ -z "$RESULT_RAW" ]]; then
    err "Could not fetch result details. The result may not exist."
    exit 1
fi

# Parse the tuple fields using python3 for robustness
# cast returns the struct as a parenthesized tuple like:
# (0xAddr, 0xAddr, 0xHash, 0xHash, 0xHash, 0xBytes, 12345, 12345, 0, 0, 0, false, false, 0xAddr)
PARSED=$(echo "$RESULT_RAW" | python3 -c "
import sys, re

raw = sys.stdin.read().strip()
# Remove outer parens if present
if raw.startswith('(') and raw.endswith(')'):
    raw = raw[1:-1]

# Split on commas, but respect nested parens and hex strings
parts = []
depth = 0
current = ''
for ch in raw:
    if ch == '(':
        depth += 1
        current += ch
    elif ch == ')':
        depth -= 1
        current += ch
    elif ch == ',' and depth == 0:
        parts.append(current.strip())
        current = ''
    else:
        current += ch
if current.strip():
    parts.append(current.strip())

# Expected fields (14 total):
#  0: enclave, 1: submitter, 2: modelHash, 3: inputHash, 4: resultHash,
#  5: result (bytes), 6: submittedAt, 7: challengeDeadline, 8: disputeDeadline,
#  9: challengeBond, 10: proverStakeAmount, 11: finalized, 12: challenged, 13: challenger
if len(parts) >= 14:
    for p in parts[:14]:
        print(p)
elif len(parts) >= 11:
    # Some versions may not return all fields via cast; print what we have
    for p in parts:
        print(p)
else:
    # Fallback: just print raw
    print('RAW:' + raw)
" 2>/dev/null)

# Read parsed fields into variables
ENCLAVE=""
SUBMITTER=""
MODEL_HASH=""
INPUT_HASH=""
RESULT_HASH=""
RESULT_DATA=""
SUBMITTED_AT=""
CHALLENGE_DEADLINE=""
DISPUTE_DEADLINE=""
CHALLENGE_BOND=""
PROVER_STAKE=""
FINALIZED=""
CHALLENGED=""
CHALLENGER=""

if [[ -n "$PARSED" ]]; then
    IDX=0
    while IFS= read -r line; do
        case $IDX in
            0)  ENCLAVE="$line" ;;
            1)  SUBMITTER="$line" ;;
            2)  MODEL_HASH="$line" ;;
            3)  INPUT_HASH="$line" ;;
            4)  RESULT_HASH="$line" ;;
            5)  RESULT_DATA="$line" ;;
            6)  SUBMITTED_AT="$line" ;;
            7)  CHALLENGE_DEADLINE="$line" ;;
            8)  DISPUTE_DEADLINE="$line" ;;
            9)  CHALLENGE_BOND="$line" ;;
            10) PROVER_STAKE="$line" ;;
            11) FINALIZED="$line" ;;
            12) CHALLENGED="$line" ;;
            13) CHALLENGER="$line" ;;
        esac
        IDX=$((IDX + 1))
    done <<< "$PARSED"
fi

# Check if the result exists (submittedAt == 0 means no result)
SUBMITTED_AT_NUM=0
if [[ -n "$SUBMITTED_AT" ]]; then
    SUBMITTED_AT_NUM=$(echo "$SUBMITTED_AT" | python3 -c "
import sys
v = sys.stdin.read().strip()
# Handle both decimal and hex
try:
    if v.startswith('0x'):
        print(int(v, 16))
    else:
        print(int(v))
except:
    print(0)
" 2>/dev/null || echo "0")
fi

if [[ "$SUBMITTED_AT_NUM" -eq 0 ]]; then
    err "No result found for this result ID."
    err "The result may not have been submitted, or the ID is incorrect."
    exit 1
fi

ok "Result found on-chain"

# Print basic details
echo ""
printf "  ${BOLD}%-22s${RESET} %s\n" "Enclave:" "$ENCLAVE"
printf "  ${BOLD}%-22s${RESET} %s\n" "Submitter:" "$SUBMITTER"
printf "  ${BOLD}%-22s${RESET} %s\n" "Model Hash:" "$MODEL_HASH"
printf "  ${BOLD}%-22s${RESET} %s\n" "Input Hash:" "$INPUT_HASH"
printf "  ${BOLD}%-22s${RESET} %s\n" "Result Hash:" "$RESULT_HASH"
printf "  ${BOLD}%-22s${RESET} %s\n" "Result Data:" "${RESULT_DATA:0:80}${RESULT_DATA:+...}"
printf "  ${BOLD}%-22s${RESET} %s\n" "Submitted At:" "$SUBMITTED_AT"
printf "  ${BOLD}%-22s${RESET} %s\n" "Finalized:" "$FINALIZED"
printf "  ${BOLD}%-22s${RESET} %s\n" "Challenged:" "$CHALLENGED"

if [[ "$CHALLENGED" == "true" ]]; then
    printf "  ${BOLD}%-22s${RESET} %s\n" "Challenger:" "$CHALLENGER"
fi

# ---------------------------------------------------------------------------
# Step 3: Check challenge/dispute status
# ---------------------------------------------------------------------------
header "Step 3/4: Check challenge and dispute status"

# Get current block timestamp
CURRENT_TIMESTAMP=$(cast block latest --rpc-url "$RPC_URL" -j 2>/dev/null | python3 -c "
import sys, json
data = json.load(sys.stdin)
ts = data.get('timestamp', '0x0')
if isinstance(ts, str) and ts.startswith('0x'):
    print(int(ts, 16))
else:
    print(int(ts))
" 2>/dev/null || echo "0")

info "Current block timestamp: $CURRENT_TIMESTAMP"

# Parse deadline timestamps
CHALLENGE_DL_NUM=0
DISPUTE_DL_NUM=0

if [[ -n "$CHALLENGE_DEADLINE" ]]; then
    CHALLENGE_DL_NUM=$(echo "$CHALLENGE_DEADLINE" | python3 -c "
import sys
v = sys.stdin.read().strip()
try:
    print(int(v, 16) if v.startswith('0x') else int(v))
except:
    print(0)
" 2>/dev/null || echo "0")
fi

if [[ -n "$DISPUTE_DEADLINE" ]]; then
    DISPUTE_DL_NUM=$(echo "$DISPUTE_DEADLINE" | python3 -c "
import sys
v = sys.stdin.read().strip()
try:
    print(int(v, 16) if v.startswith('0x') else int(v))
except:
    print(0)
" 2>/dev/null || echo "0")
fi

# Determine status
if [[ "$FINALIZED" == "true" && "$CHALLENGED" != "true" ]]; then
    printf "  ${GREEN}${BOLD}%-22s${RESET} %s\n" "Status:" "FINALIZED (unchallenged)"
    ok "Result was finalized without any challenge"

elif [[ "$CHALLENGED" == "true" ]]; then
    # Check dispute resolution
    DISPUTE_RESOLVED=$(cast call "$TEE_ADDR" \
        'disputeResolved(bytes32)(bool)' \
        "$RESULT_ID" \
        --rpc-url "$RPC_URL" 2>/dev/null) || DISPUTE_RESOLVED="false"

    PROVER_WON=$(cast call "$TEE_ADDR" \
        'disputeProverWon(bytes32)(bool)' \
        "$RESULT_ID" \
        --rpc-url "$RPC_URL" 2>/dev/null) || PROVER_WON="false"

    printf "  ${BOLD}%-22s${RESET} %s\n" "Dispute Resolved:" "$DISPUTE_RESOLVED"
    printf "  ${BOLD}%-22s${RESET} %s\n" "Prover Won:" "$PROVER_WON"

    if [[ "$DISPUTE_RESOLVED" == "true" ]]; then
        if [[ "$PROVER_WON" == "true" ]]; then
            printf "  ${GREEN}${BOLD}%-22s${RESET} %s\n" "Status:" "DISPUTE RESOLVED -- Prover won"
            ok "Dispute resolved in prover's favor. Result is valid."
        else
            printf "  ${RED}${BOLD}%-22s${RESET} %s\n" "Status:" "DISPUTE RESOLVED -- Challenger won"
            warn "Dispute resolved in challenger's favor. Result is invalid."
        fi
    else
        # Dispute is active
        if [[ "$CURRENT_TIMESTAMP" -gt 0 && "$DISPUTE_DL_NUM" -gt 0 ]]; then
            REMAINING=$((DISPUTE_DL_NUM - CURRENT_TIMESTAMP))
            if [[ "$REMAINING" -gt 0 ]]; then
                REMAINING_STR=$(format_duration "$REMAINING")
                printf "  ${YELLOW}${BOLD}%-22s${RESET} %s\n" "Status:" "CHALLENGED -- Dispute active"
                printf "  ${BOLD}%-22s${RESET} %s\n" "Dispute Deadline:" "$DISPUTE_DL_NUM"
                printf "  ${YELLOW}${BOLD}%-22s${RESET} %s\n" "Time Remaining:" "$REMAINING_STR"
                warn "Dispute window is open. Prover must submit ZK proof or lose."
            else
                OVERDUE=$(( -REMAINING ))
                OVERDUE_STR=$(format_duration "$OVERDUE")
                printf "  ${RED}${BOLD}%-22s${RESET} %s\n" "Status:" "CHALLENGED -- Dispute window EXPIRED"
                printf "  ${BOLD}%-22s${RESET} %s\n" "Dispute Deadline:" "$DISPUTE_DL_NUM"
                printf "  ${RED}${BOLD}%-22s${RESET} %s\n" "Overdue By:" "$OVERDUE_STR"
                warn "Dispute deadline has passed. Anyone can call resolveDisputeByTimeout()."
            fi
        else
            printf "  ${YELLOW}${BOLD}%-22s${RESET} %s\n" "Status:" "CHALLENGED -- Dispute pending"
        fi
    fi

elif [[ "$FINALIZED" != "true" ]]; then
    # Not finalized, not challenged -- in challenge window
    if [[ "$CURRENT_TIMESTAMP" -gt 0 && "$CHALLENGE_DL_NUM" -gt 0 ]]; then
        REMAINING=$((CHALLENGE_DL_NUM - CURRENT_TIMESTAMP))
        if [[ "$REMAINING" -gt 0 ]]; then
            REMAINING_STR=$(format_duration "$REMAINING")
            printf "  ${YELLOW}${BOLD}%-22s${RESET} %s\n" "Status:" "PENDING -- In challenge window"
            printf "  ${BOLD}%-22s${RESET} %s\n" "Challenge Deadline:" "$CHALLENGE_DL_NUM"
            printf "  ${YELLOW}${BOLD}%-22s${RESET} %s\n" "Time Remaining:" "$REMAINING_STR"
            info "Result can be challenged during this window."
            info "After window expires, call finalize() to mark it valid."
        else
            OVERDUE=$(( -REMAINING ))
            OVERDUE_STR=$(format_duration "$OVERDUE")
            printf "  ${CYAN}${BOLD}%-22s${RESET} %s\n" "Status:" "PENDING -- Challenge window EXPIRED"
            printf "  ${BOLD}%-22s${RESET} %s\n" "Challenge Deadline:" "$CHALLENGE_DL_NUM"
            printf "  ${CYAN}${BOLD}%-22s${RESET} %s\n" "Overdue By:" "$OVERDUE_STR"
            info "Challenge window has passed. Call finalize() to complete."
        fi
    else
        printf "  ${YELLOW}${BOLD}%-22s${RESET} %s\n" "Status:" "PENDING"
    fi
fi

# ---------------------------------------------------------------------------
# Step 4: Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  RESULT SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}%-22s${RESET} %s\n" "Result ID:" "$RESULT_ID"
printf "  ${BOLD}%-22s${RESET} %s\n" "Valid:" "$IS_VALID"
printf "  ${BOLD}%-22s${RESET} %s\n" "Finalized:" "$FINALIZED"
printf "  ${BOLD}%-22s${RESET} %s\n" "Challenged:" "$CHALLENGED"
printf "  ${BOLD}%-22s${RESET} %s\n" "Submitter:" "$SUBMITTER"
printf "  ${BOLD}%-22s${RESET} %s\n" "Enclave:" "$ENCLAVE"
printf "  ${BOLD}%-22s${RESET} %s\n" "TEEMLVerifier:" "$TEE_ADDR"
echo ""

ok "Query complete."
