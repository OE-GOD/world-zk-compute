#!/usr/bin/env bash
# =============================================================================
# watch-sepolia-events.sh -- Live event monitor for Sepolia
#
# Polls contract events using `cast logs` for TEEMLVerifier and
# ExecutionEngine on Sepolia.
#
# Required env:
#   ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC endpoint
#
# Optional env:
#   TEE_VERIFIER_ADDRESS       TEEMLVerifier contract (auto-loaded from deployment)
#   EXECUTION_ENGINE_ADDRESS   ExecutionEngine contract (auto-loaded from deployment)
#   POLL_INTERVAL              Seconds between polls (default: 10)
#
# Usage:
#   ./scripts/watch-sepolia-events.sh
#   ./scripts/watch-sepolia-events.sh --once
#   ./scripts/watch-sepolia-events.sh --from-block 12345
#
# Options:
#   --once              Poll once and exit
#   --from-block <n>    Start from a specific block (default: latest - 100)
#   --poll <secs>       Override poll interval
#   -h, --help          Show this help
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
watch-sepolia-events.sh -- Live event monitor for Sepolia

USAGE:
  ./scripts/watch-sepolia-events.sh [OPTIONS]

OPTIONS:
  --once              Poll once and exit (do not loop)
  --from-block <n>    Start from a specific block number (default: latest - 100)
  --poll <secs>       Override poll interval (default: 10, or POLL_INTERVAL env)
  -h, --help          Show this help and exit

REQUIRED ENVIRONMENT:
  ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC endpoint (e.g., Alchemy or Infura URL)

OPTIONAL ENVIRONMENT:
  TEE_VERIFIER_ADDRESS       TEEMLVerifier contract address
  EXECUTION_ENGINE_ADDRESS   ExecutionEngine contract address
  POLL_INTERVAL              Seconds between polls (default: 10)

  If TEE_VERIFIER_ADDRESS or EXECUTION_ENGINE_ADDRESS are not set, the script
  attempts to load them from deployments/11155111.json.

EVENT TOPICS:
  ResultSubmitted:  0xc5188349eace8be84e9c2fc4a49c039e04e0d1b0dff89443503e4dcc0ad8fe8a
  ResultChallenged: 0x7e26d1a76cf43de9e898e8231e7788c3a93e76f3cacf1eacc3cc8d29e3da4cc2
  ResultFinalized:  0x9b1e4e33c28d1ee88a2ea395a3ed952e6e1e0e7c97fc4e0e9b8a1a6b1a1e5c4a

EXAMPLES:
  ALCHEMY_SEPOLIA_RPC_URL=https://... ./scripts/watch-sepolia-events.sh
  ALCHEMY_SEPOLIA_RPC_URL=https://... ./scripts/watch-sepolia-events.sh --once
  ALCHEMY_SEPOLIA_RPC_URL=https://... ./scripts/watch-sepolia-events.sh --from-block 5000000

EXIT:
  Ctrl+C to stop (SIGINT trapped for clean exit).
EOF
}

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOY_FILE="$PROJECT_ROOT/deployments/11155111.json"

# ---------------------------------------------------------------------------
# Event topics
# ---------------------------------------------------------------------------
TOPIC_RESULT_SUBMITTED="0xc5188349eace8be84e9c2fc4a49c039e04e0d1b0dff89443503e4dcc0ad8fe8a"
TOPIC_RESULT_CHALLENGED="0x7e26d1a76cf43de9e898e8231e7788c3a93e76f3cacf1eacc3cc8d29e3da4cc2"
TOPIC_RESULT_FINALIZED="0x9b1e4e33c28d1ee88a2ea395a3ed952e6e1e0e7c97fc4e0e9b8a1a6b1a1e5c4a"

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
ONCE=false
FROM_BLOCK=""
POLL="${POLL_INTERVAL:-10}"

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        --once)
            ONCE=true
            shift
            ;;
        --from-block)
            FROM_BLOCK="$2"
            shift 2
            ;;
        --poll)
            POLL="$2"
            shift 2
            ;;
        *)
            err "Unknown option: $1"
            echo "Run with --help for usage."
            exit 1
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
    echo "  Export it before running: export ALCHEMY_SEPOLIA_RPC_URL=https://..."
    exit 1
fi

RPC_URL="$ALCHEMY_SEPOLIA_RPC_URL"

# ---------------------------------------------------------------------------
# JSON field helper (no jq dependency required; falls back to python3)
# ---------------------------------------------------------------------------
json_field() {
    local file="$1"
    local field="$2"
    local value=""

    if command -v jq &>/dev/null; then
        value=$(jq -r "$field // empty" "$file" 2>/dev/null || echo "")
    elif command -v python3 &>/dev/null; then
        value=$(python3 -c "
import json, sys
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
# Load contract addresses from deployment file if not already set
# ---------------------------------------------------------------------------
TEE_ADDR="${TEE_VERIFIER_ADDRESS:-}"
ENGINE_ADDR="${EXECUTION_ENGINE_ADDRESS:-}"

if [ -f "$DEPLOY_FILE" ]; then
    if [ -z "$TEE_ADDR" ]; then
        TEE_ADDR=$(json_field "$DEPLOY_FILE" ".contracts.TEEMLVerifier")
        if [ -z "$TEE_ADDR" ]; then
            TEE_ADDR=$(json_field "$DEPLOY_FILE" ".TEEMLVerifier")
        fi
        # Handle nested object with .address
        if [ -z "$TEE_ADDR" ]; then
            TEE_ADDR=$(json_field "$DEPLOY_FILE" ".contracts.TEEMLVerifier.address")
        fi
    fi
    if [ -z "$ENGINE_ADDR" ]; then
        ENGINE_ADDR=$(json_field "$DEPLOY_FILE" ".contracts.ExecutionEngine")
        if [ -z "$ENGINE_ADDR" ]; then
            ENGINE_ADDR=$(json_field "$DEPLOY_FILE" ".ExecutionEngine")
        fi
        if [ -z "$ENGINE_ADDR" ]; then
            ENGINE_ADDR=$(json_field "$DEPLOY_FILE" ".contracts.ExecutionEngine.address")
        fi
    fi
fi

if [ -z "$TEE_ADDR" ] && [ -z "$ENGINE_ADDR" ]; then
    err "No contract addresses found."
    echo "  Set TEE_VERIFIER_ADDRESS and/or EXECUTION_ENGINE_ADDRESS,"
    echo "  or provide a deployment file at deployments/11155111.json."
    exit 1
fi

# ---------------------------------------------------------------------------
# Resolve starting block
# ---------------------------------------------------------------------------
LATEST=$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null || echo "0")

if [ -z "$FROM_BLOCK" ]; then
    if [ "$LATEST" -gt 100 ]; then
        FROM_BLOCK=$((LATEST - 100))
    else
        FROM_BLOCK=0
    fi
fi

# ---------------------------------------------------------------------------
# Decode event type from topic
# ---------------------------------------------------------------------------
decode_event() {
    local topic="$1"
    case "$topic" in
        "$TOPIC_RESULT_SUBMITTED")  echo "ResultSubmitted"  ;;
        "$TOPIC_RESULT_CHALLENGED") echo "ResultChallenged" ;;
        "$TOPIC_RESULT_FINALIZED")  echo "ResultFinalized"  ;;
        *)                          echo "Unknown($topic)"  ;;
    esac
}

# ---------------------------------------------------------------------------
# Trap SIGINT for clean exit
# ---------------------------------------------------------------------------
cleanup() {
    printf "\n"
    info "Stopping event watcher."
    exit 0
}
trap cleanup SIGINT

# ---------------------------------------------------------------------------
# Print header
# ---------------------------------------------------------------------------
printf "\n"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "%b  Sepolia Event Watcher%b\n" "$BOLD" "$RESET"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "\n"

if [ -n "$TEE_ADDR" ]; then
    info "TEEMLVerifier:    $TEE_ADDR"
fi
if [ -n "$ENGINE_ADDR" ]; then
    info "ExecutionEngine:  $ENGINE_ADDR"
fi
info "From block:       $FROM_BLOCK"
info "Latest block:     $LATEST"
info "Poll interval:    ${POLL}s"

if [ "$ONCE" = "true" ]; then
    info "Mode:             single poll"
else
    info "Mode:             continuous (Ctrl+C to stop)"
fi

printf "\n"

# ---------------------------------------------------------------------------
# Poll function: fetch and display logs for a contract
# ---------------------------------------------------------------------------
poll_contract() {
    local label="$1"
    local address="$2"
    local from="$3"
    local to="$4"

    if [ -z "$address" ]; then
        return
    fi

    local logs
    logs=$(cast logs \
        --from-block "$from" \
        --to-block "$to" \
        --address "$address" \
        --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    if [ -z "$logs" ]; then
        return
    fi

    local timestamp
    timestamp=$(date -u +"%Y-%m-%d %H:%M:%S UTC")

    printf "%b[%s]%b %b%s%b blocks %s..%s\n" \
        "$CYAN" "$timestamp" "$RESET" \
        "$BOLD" "$label" "$RESET" \
        "$from" "$to"

    # Parse log entries line by line looking for topic[0]
    local line_topic=""
    while IFS= read -r line; do
        # cast logs outputs topics as "topics: [0x...]"
        case "$line" in
            *"topic"*|*"Topic"*)
                # Extract the first hex value after colon
                line_topic=$(echo "$line" | grep -oE '0x[0-9a-fA-F]{64}' | head -1 || echo "")
                if [ -n "$line_topic" ]; then
                    local event_name
                    event_name=$(decode_event "$line_topic")
                    printf "  %b->%b %s\n" "$GREEN" "$RESET" "$event_name"
                fi
                ;;
            *"blockNumber"*|*"block_number"*)
                local block_num
                block_num=$(echo "$line" | grep -oE '[0-9]+' | head -1 || echo "?")
                printf "     block: %s\n" "$block_num"
                ;;
            *"transactionHash"*|*"transaction_hash"*)
                local tx_hash
                tx_hash=$(echo "$line" | grep -oE '0x[0-9a-fA-F]+' | head -1 || echo "?")
                printf "     tx:    %s\n" "$tx_hash"
                ;;
        esac
    done <<< "$logs"

    printf "\n"
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
CURRENT_BLOCK=$FROM_BLOCK

while true; do
    LATEST=$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null || echo "$CURRENT_BLOCK")

    if [ "$LATEST" -gt "$CURRENT_BLOCK" ]; then
        poll_contract "TEEMLVerifier" "$TEE_ADDR" "$CURRENT_BLOCK" "$LATEST"
        poll_contract "ExecutionEngine" "$ENGINE_ADDR" "$CURRENT_BLOCK" "$LATEST"
        CURRENT_BLOCK=$((LATEST + 1))
    else
        if [ "$ONCE" = "false" ]; then
            printf "%b.%b" "$CYAN" "$RESET"
        fi
    fi

    if [ "$ONCE" = "true" ]; then
        ok "Single poll complete (blocks $FROM_BLOCK..$LATEST)."
        exit 0
    fi

    sleep "$POLL"
done
