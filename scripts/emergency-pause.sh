#!/usr/bin/env bash
# =============================================================================
# World ZK Compute -- Emergency Contract Pause / Unpause
#
# Pauses (or unpauses) deployed contracts in an emergency. For each contract
# address supplied, calls pause() and then verifies isPaused() returns true.
# When --unpause is given, calls unpause() and verifies isPaused() returns
# false instead.
#
# Usage:
#   scripts/emergency-pause.sh --rpc-url <url> --private-key <key> \
#       --contracts 0xABC,0xDEF
#   scripts/emergency-pause.sh --rpc-url <url> --private-key <key> \
#       --contracts all --chain-id 421614
#   scripts/emergency-pause.sh --rpc-url <url> --private-key <key> \
#       --contracts all --chain-id 421614 --unpause
#
# Flags:
#   --rpc-url <url>       RPC endpoint (required)
#   --private-key <key>   Admin private key (required)
#   --contracts <addrs>   Comma-separated addresses, or 'all' to read from
#                         deployment file
#   --chain-id <id>       Chain ID (used with --contracts all to locate
#                         deployments/<chainId>.json)
#   --unpause             Unpause instead of pause
#   --yes                 Skip confirmation prompt
#   --help                Show this help message
#
# Exit codes:
#   0 -- all contracts paused/unpaused successfully
#   1 -- one or more operations failed, or user aborted
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

info()    { printf "${CYAN}[INFO]${RESET}  %s  %s\n" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"; }
ok()      { printf "${GREEN}[OK]${RESET}    %s  %s\n" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s  %s\n" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s  %s\n" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"
CHAINS_FILE="$DEPLOYMENTS_DIR/chains.json"

# ---------------------------------------------------------------------------
# Default flags
# ---------------------------------------------------------------------------
RPC_URL=""
PRIVATE_KEY=""
CONTRACTS_ARG=""
CHAIN_ID=""
UNPAUSE=false
SKIP_CONFIRM=false

# ---------------------------------------------------------------------------
# Tracking
# ---------------------------------------------------------------------------
PASS_COUNT=0
FAIL_COUNT=0

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
emergency-pause.sh -- Emergency pause/unpause for World ZK Compute contracts.

USAGE:
  scripts/emergency-pause.sh --rpc-url <url> --private-key <key> --contracts <addrs>

REQUIRED:
  --rpc-url <url>       RPC endpoint
  --private-key <key>   Admin private key for the contracts
  --contracts <addrs>   Comma-separated contract addresses, or 'all' (requires --chain-id)

OPTIONS:
  --chain-id <id>       Chain ID (required when --contracts all)
  --unpause             Unpause contracts instead of pausing
  --yes                 Skip confirmation prompt
  --help                Show this help message

EXAMPLES:
  # Pause specific contracts
  scripts/emergency-pause.sh --rpc-url http://127.0.0.1:8545 \
      --private-key 0xac09... --contracts 0xABC...,0xDEF...

  # Pause all contracts from deployment file
  scripts/emergency-pause.sh --rpc-url http://127.0.0.1:8545 \
      --private-key 0xac09... --contracts all --chain-id 421614

  # Unpause all contracts
  scripts/emergency-pause.sh --rpc-url http://127.0.0.1:8545 \
      --private-key 0xac09... --contracts all --chain-id 421614 --unpause
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        --private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        --contracts)
            CONTRACTS_ARG="$2"
            shift 2
            ;;
        --chain-id)
            CHAIN_ID="$2"
            shift 2
            ;;
        --unpause)
            UNPAUSE=true
            shift
            ;;
        --yes)
            SKIP_CONFIRM=true
            shift
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
if [[ -z "$RPC_URL" ]]; then
    err "--rpc-url is required."
    echo ""
    usage
    exit 1
fi

if [[ -z "$PRIVATE_KEY" ]]; then
    err "--private-key is required."
    echo ""
    usage
    exit 1
fi

if [[ -z "$CONTRACTS_ARG" ]]; then
    err "--contracts is required. Provide comma-separated addresses or 'all'."
    echo ""
    usage
    exit 1
fi

if ! command -v cast &>/dev/null; then
    err "cast is required. Install Foundry: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

# ---------------------------------------------------------------------------
# Derive admin address
# ---------------------------------------------------------------------------
ADMIN_ADDR=$(cast wallet address --private-key "$PRIVATE_KEY" 2>/dev/null) || {
    err "Invalid private key. Could not derive address."
    exit 1
}

# ---------------------------------------------------------------------------
# Resolve contract addresses
# ---------------------------------------------------------------------------
CONTRACT_ADDRS=()

if [[ "$CONTRACTS_ARG" == "all" ]]; then
    # Read from deployment file
    if [[ -z "$CHAIN_ID" ]]; then
        err "--chain-id is required when --contracts is 'all'."
        exit 1
    fi

    DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_ID}.json"

    # Also try name-based lookup if numeric file not found
    if [[ ! -f "$DEPLOY_FILE" && -f "$CHAINS_FILE" ]]; then
        CHAIN_NAME=""
        if command -v jq &>/dev/null; then
            CHAIN_NAME=$(jq -r ".chains[] | select(.chainId == $CHAIN_ID) | .name // empty" "$CHAINS_FILE" 2>/dev/null || echo "")
        elif command -v python3 &>/dev/null; then
            CHAIN_NAME=$(python3 -c "
import json
with open('$CHAINS_FILE') as f:
    data = json.load(f)
for c in data.get('chains', []):
    if c.get('chainId') == $CHAIN_ID:
        print(c.get('name', ''))
        break
" 2>/dev/null || echo "")
        fi

        if [[ -n "$CHAIN_NAME" && -f "$DEPLOYMENTS_DIR/${CHAIN_NAME}.json" ]]; then
            DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_NAME}.json"
        fi
    fi

    if [[ ! -f "$DEPLOY_FILE" ]]; then
        err "Deployment file not found: $DEPLOY_FILE"
        err "Deploy first or provide explicit addresses with --contracts."
        exit 1
    fi

    info "Loading contract addresses from $DEPLOY_FILE"

    # Extract all contract addresses from the deployment file
    # Supports both flat format (contracts.Name.address) and direct format (contracts.Name = "0x...")
    if command -v jq &>/dev/null; then
        while IFS= read -r addr; do
            if [[ -n "$addr" && "$addr" != "null" && "$addr" != "" && ${#addr} -ge 42 ]]; then
                CONTRACT_ADDRS+=("$addr")
            fi
        done < <(jq -r '
            .contracts // {} | to_entries[] |
            (if .value | type == "object" then .value.address else .value end) // empty
        ' "$DEPLOY_FILE" 2>/dev/null)
    elif command -v python3 &>/dev/null; then
        while IFS= read -r addr; do
            if [[ -n "$addr" && ${#addr} -ge 42 ]]; then
                CONTRACT_ADDRS+=("$addr")
            fi
        done < <(python3 -c "
import json
with open('$DEPLOY_FILE') as f:
    data = json.load(f)
contracts = data.get('contracts', {})
for name, val in contracts.items():
    if isinstance(val, dict):
        addr = val.get('address', '')
    else:
        addr = str(val)
    if addr and len(addr) >= 42 and addr != '0x0000000000000000000000000000000000000000':
        print(addr)
" 2>/dev/null)
    else
        err "jq or python3 is required to parse the deployment file."
        exit 1
    fi
else
    # Parse comma-separated addresses
    IFS=',' read -ra CONTRACT_ADDRS <<< "$CONTRACTS_ARG"
fi

# Filter out empty/zero addresses
VALID_ADDRS=()
ZERO_ADDRESS="0x0000000000000000000000000000000000000000"
for addr in "${CONTRACT_ADDRS[@]}"; do
    addr=$(echo "$addr" | tr -d '[:space:]')
    if [[ -n "$addr" && "$addr" != "$ZERO_ADDRESS" && ${#addr} -ge 42 ]]; then
        VALID_ADDRS+=("$addr")
    fi
done

if [[ ${#VALID_ADDRS[@]} -eq 0 ]]; then
    err "No valid contract addresses found."
    exit 1
fi

# ---------------------------------------------------------------------------
# Determine action
# ---------------------------------------------------------------------------
if [[ "$UNPAUSE" == "true" ]]; then
    ACTION="unpause"
    ACTION_UPPER="UNPAUSE"
    EXPECTED_PAUSED="false"
else
    ACTION="pause"
    ACTION_UPPER="PAUSE"
    EXPECTED_PAUSED="true"
fi

# ---------------------------------------------------------------------------
# Print plan
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- Emergency $ACTION_UPPER"
header "============================================================"
echo ""
info "RPC URL:    $RPC_URL"
info "Admin:      $ADMIN_ADDR"
info "Action:     $ACTION_UPPER"
info "Contracts:  ${#VALID_ADDRS[@]}"
echo ""

for addr in "${VALID_ADDRS[@]}"; do
    info "  -> $addr"
done
echo ""

# ---------------------------------------------------------------------------
# Confirmation prompt
# ---------------------------------------------------------------------------
if [[ "$SKIP_CONFIRM" != "true" ]]; then
    echo ""
    printf "${RED}${BOLD}This will %s all contracts. Type YES to confirm: ${RESET}" "$ACTION"
    read -r CONFIRM
    if [[ "$CONFIRM" != "YES" ]]; then
        warn "Aborted by user."
        exit 1
    fi
    echo ""
fi

# ---------------------------------------------------------------------------
# Execute pause/unpause for each contract
# ---------------------------------------------------------------------------
header "Executing $ACTION_UPPER..."
echo ""

for addr in "${VALID_ADDRS[@]}"; do
    info "Processing $addr ..."

    # Check if the contract has code
    CODE=$(cast code "$addr" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")
    if [[ "$CODE" == "0x" || -z "$CODE" ]]; then
        err "  No bytecode at $addr -- skipping"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        continue
    fi

    # Check current pause state before acting
    CURRENT_PAUSED=$(cast call "$addr" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null || echo "UNSUPPORTED")

    if [[ "$CURRENT_PAUSED" == "UNSUPPORTED" ]]; then
        warn "  $addr does not support paused() -- skipping"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        continue
    fi

    if [[ "$CURRENT_PAUSED" == "$EXPECTED_PAUSED" ]]; then
        ok "  $addr is already ${ACTION}d -- skipping"
        PASS_COUNT=$((PASS_COUNT + 1))
        continue
    fi

    # Send the pause/unpause transaction
    TX_OUTPUT=$(cast send "$addr" "${ACTION}()" \
        --rpc-url "$RPC_URL" \
        --private-key "$PRIVATE_KEY" \
        --json 2>&1) || {
        err "  Failed to $ACTION $addr"
        err "  Output: $TX_OUTPUT"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        continue
    }

    # Check transaction status
    TX_STATUS=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo "")
    TX_HASH=$(echo "$TX_OUTPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('transactionHash',''))" 2>/dev/null || echo "unknown")

    if [[ "$TX_STATUS" != "0x1" ]]; then
        err "  Transaction reverted for $addr (tx: $TX_HASH)"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        continue
    fi

    # Verify the state changed
    VERIFY_PAUSED=$(cast call "$addr" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null || echo "unknown")

    if [[ "$VERIFY_PAUSED" == "$EXPECTED_PAUSED" ]]; then
        ok "  $addr ${ACTION}d successfully (tx: $TX_HASH)"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        err "  $addr state mismatch after ${ACTION}: paused()=$VERIFY_PAUSED, expected=$EXPECTED_PAUSED"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
TOTAL=$((PASS_COUNT + FAIL_COUNT))

echo ""
header "============================================================"
header "  $ACTION_UPPER SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}Action:${RESET}     %s\n" "$ACTION_UPPER"
printf "  ${BOLD}Admin:${RESET}      %s\n" "$ADMIN_ADDR"
printf "  ${BOLD}Succeeded:${RESET}  %d\n" "$PASS_COUNT"
printf "  ${BOLD}Failed:${RESET}     %d\n" "$FAIL_COUNT"
printf "  ${BOLD}Total:${RESET}      %d\n" "$TOTAL"
echo ""

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    err "Some contracts could not be ${ACTION}d. Review output above."
    echo ""
    exit 1
else
    ok "All $PASS_COUNT contracts ${ACTION}d successfully."
    echo ""
    exit 0
fi
