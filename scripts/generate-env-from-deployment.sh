#!/usr/bin/env bash
# =============================================================================
# generate-env-from-deployment.sh -- Generate .env file from deployment JSON
#
# Reads a deployment JSON file and outputs contract addresses and chain
# configuration in .env format. Optionally merges with an existing .env file.
#
# Args:
#   CHAIN_ID or path to deployment JSON file
#
# Usage:
#   ./scripts/generate-env-from-deployment.sh 11155111
#   ./scripts/generate-env-from-deployment.sh deployments/11155111.json
#   ./scripts/generate-env-from-deployment.sh 421614 --output .env.arb
#   ./scripts/generate-env-from-deployment.sh 31337 --merge --output .env
#   ./scripts/generate-env-from-deployment.sh --help
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
generate-env-from-deployment.sh -- Generate .env from deployment JSON

USAGE:
  ./scripts/generate-env-from-deployment.sh [OPTIONS] <CHAIN_ID | FILE_PATH>

ARGUMENTS:
  CHAIN_ID      Numeric chain ID (e.g., 11155111, 421614, 31337)
                Reads from deployments/<chainId>.json
  FILE_PATH     Direct path to a deployment JSON file

OPTIONS:
  --output, -o <path>   Write .env to file (default: stdout)
  --merge               Merge with existing .env file (only with --output)
                        Existing values are preserved; new keys are appended.
  -h, --help            Show this help and exit

SUPPORTED DEPLOYMENT JSON FORMATS:
  Flat:     { "TEEMLVerifier": "0x...", "ExecutionEngine": "0x...", ... }
  Nested:   { "contracts": { "TEEMLVerifier": { "address": "0x..." }, ... } }
  Mixed:    { "contracts": { "TEEMLVerifier": "0x...", ... } }

OUTPUT FORMAT:
  # Chain: sepolia (11155111)
  CHAIN_ID=11155111
  TEE_VERIFIER_ADDRESS=0x...
  EXECUTION_ENGINE_ADDRESS=0x...
  PROGRAM_REGISTRY_ADDRESS=0x...
  ...

EXAMPLES:
  ./scripts/generate-env-from-deployment.sh 11155111
  ./scripts/generate-env-from-deployment.sh 421614 -o .env.arb-sepolia
  ./scripts/generate-env-from-deployment.sh 31337 --merge -o .env
  ./scripts/generate-env-from-deployment.sh deployments/11155111.json
EOF
}

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENTS_DIR="$PROJECT_ROOT/deployments"
CHAINS_FILE="$DEPLOYMENTS_DIR/chains.json"

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
INPUT=""
OUTPUT=""
MERGE=false

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        --output|-o)
            OUTPUT="$2"
            shift 2
            ;;
        --merge)
            MERGE=true
            shift
            ;;
        -*)
            err "Unknown option: $1"
            echo "Run with --help for usage."
            exit 1
            ;;
        *)
            if [ -z "$INPUT" ]; then
                INPUT="$1"
            else
                err "Unexpected argument: $1"
                exit 1
            fi
            shift
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
if [ -z "$INPUT" ]; then
    err "CHAIN_ID or deployment file path is required."
    echo "  Usage: ./scripts/generate-env-from-deployment.sh <CHAIN_ID | FILE_PATH>"
    echo "  Run with --help for details."
    exit 1
fi

if [ "$MERGE" = "true" ] && [ -z "$OUTPUT" ]; then
    err "--merge requires --output <file>."
    exit 1
fi

# ---------------------------------------------------------------------------
# Resolve deployment file
# ---------------------------------------------------------------------------
DEPLOY_FILE=""
CHAIN_ID=""

# Check if INPUT is a file path
if [ -f "$INPUT" ]; then
    DEPLOY_FILE="$INPUT"
    info "Using deployment file: $DEPLOY_FILE"
elif echo "$INPUT" | grep -qE '^[0-9]+$'; then
    CHAIN_ID="$INPUT"
    DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_ID}.json"
    if [ ! -f "$DEPLOY_FILE" ]; then
        err "Deployment file not found: $DEPLOY_FILE"
        echo "  Available deployment files:"
        for f in "$DEPLOYMENTS_DIR"/*.json; do
            if [ -f "$f" ]; then
                BASENAME=$(basename "$f")
                if [ "$BASENAME" != "chains.json" ] && [ "$BASENAME" != "registry.json" ]; then
                    echo "    $BASENAME"
                fi
            fi
        done
        exit 1
    fi
else
    err "Invalid argument: $INPUT"
    echo "  Provide a numeric chain ID or a path to a deployment JSON file."
    exit 1
fi

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
# Extract chain ID from deployment file if not already known
# ---------------------------------------------------------------------------
if [ -z "$CHAIN_ID" ]; then
    CHAIN_ID=$(json_field "$DEPLOY_FILE" ".chainId")
    if [ -z "$CHAIN_ID" ]; then
        CHAIN_ID=$(json_field "$DEPLOY_FILE" ".chain_id")
    fi
    if [ -z "$CHAIN_ID" ]; then
        # Try to infer from filename
        BASENAME=$(basename "$DEPLOY_FILE" .json)
        if echo "$BASENAME" | grep -qE '^[0-9]+$'; then
            CHAIN_ID="$BASENAME"
        fi
    fi
fi

# ---------------------------------------------------------------------------
# Resolve chain name
# ---------------------------------------------------------------------------
CHAIN_NAME=""
RPC_URL=""

case "${CHAIN_ID:-}" in
    11155111) CHAIN_NAME="sepolia" ;;
    421614)   CHAIN_NAME="arbitrum-sepolia" ;;
    31337)    CHAIN_NAME="localhost" ;;
    4801)     CHAIN_NAME="world-chain-sepolia" ;;
    1)        CHAIN_NAME="mainnet" ;;
    42161)    CHAIN_NAME="arbitrum" ;;
esac

# Try chains.json for more info
if [ -f "$CHAINS_FILE" ] && [ -n "$CHAIN_ID" ]; then
    if command -v jq &>/dev/null; then
        FOUND_NAME=$(jq -r --argjson id "$CHAIN_ID" \
            '.chains[] | select(.chainId == $id) | .name // empty' \
            "$CHAINS_FILE" 2>/dev/null || echo "")
        FOUND_RPC=$(jq -r --argjson id "$CHAIN_ID" \
            '.chains[] | select(.chainId == $id) | .rpcUrl // empty' \
            "$CHAINS_FILE" 2>/dev/null || echo "")
        if [ -n "$FOUND_NAME" ]; then
            CHAIN_NAME="$FOUND_NAME"
        fi
        if [ -n "$FOUND_RPC" ]; then
            RPC_URL="$FOUND_RPC"
        fi
    fi
fi

if [ -z "$CHAIN_NAME" ]; then
    CHAIN_NAME="chain-${CHAIN_ID:-unknown}"
fi

# ---------------------------------------------------------------------------
# Extract contract addresses
# ---------------------------------------------------------------------------
extract_address() {
    local contract_name="$1"
    local addr=""

    # Try: .contracts.<name>.address (nested object)
    addr=$(json_field "$DEPLOY_FILE" ".contracts.${contract_name}.address")
    if [ -n "$addr" ] && [ "$addr" != "null" ]; then
        echo "$addr"
        return
    fi

    # Try: .contracts.<name> (flat string)
    addr=$(json_field "$DEPLOY_FILE" ".contracts.${contract_name}")
    if [ -n "$addr" ] && [ "$addr" != "null" ]; then
        echo "$addr"
        return
    fi

    # Try: .<name> (top-level)
    addr=$(json_field "$DEPLOY_FILE" ".${contract_name}")
    if [ -n "$addr" ] && [ "$addr" != "null" ]; then
        echo "$addr"
        return
    fi

    echo ""
}

TEE_VERIFIER=$(extract_address "TEEMLVerifier")
EXECUTION_ENGINE=$(extract_address "ExecutionEngine")
PROGRAM_REGISTRY=$(extract_address "ProgramRegistry")
PROVER_REGISTRY=$(extract_address "ProverRegistry")
PROVER_REPUTATION=$(extract_address "ProverReputation")
REMAINDER_VERIFIER=$(extract_address "RemainderVerifier")
MOCK_VERIFIER=$(extract_address "MockRiscZeroVerifier")

# Also try common alternate names
if [ -z "$TEE_VERIFIER" ]; then
    TEE_VERIFIER=$(extract_address "teeVerifier")
fi
if [ -z "$EXECUTION_ENGINE" ]; then
    EXECUTION_ENGINE=$(extract_address "executionEngine")
fi
if [ -z "$PROGRAM_REGISTRY" ]; then
    PROGRAM_REGISTRY=$(extract_address "programRegistry")
fi

# Extract deployer from deployment
DEPLOYER=$(json_field "$DEPLOY_FILE" ".deployer")

# ---------------------------------------------------------------------------
# Generate .env content
# ---------------------------------------------------------------------------
generate_env() {
    echo "# ==================================================="
    echo "# World ZK Compute -- Environment Configuration"
    echo "# Generated from: $(basename "$DEPLOY_FILE")"
    echo "# Date: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    echo "# ==================================================="
    echo ""
    echo "# Chain"
    echo "CHAIN_ID=${CHAIN_ID:-}"
    echo "CHAIN_NAME=${CHAIN_NAME}"
    if [ -n "$RPC_URL" ]; then
        echo "RPC_URL=${RPC_URL}"
    fi
    echo ""

    echo "# Contract Addresses"
    if [ -n "$TEE_VERIFIER" ]; then
        echo "TEE_VERIFIER_ADDRESS=${TEE_VERIFIER}"
    else
        echo "# TEE_VERIFIER_ADDRESS="
    fi

    if [ -n "$EXECUTION_ENGINE" ]; then
        echo "EXECUTION_ENGINE_ADDRESS=${EXECUTION_ENGINE}"
    else
        echo "# EXECUTION_ENGINE_ADDRESS="
    fi

    if [ -n "$PROGRAM_REGISTRY" ]; then
        echo "PROGRAM_REGISTRY_ADDRESS=${PROGRAM_REGISTRY}"
    else
        echo "# PROGRAM_REGISTRY_ADDRESS="
    fi

    if [ -n "$PROVER_REGISTRY" ]; then
        echo "PROVER_REGISTRY_ADDRESS=${PROVER_REGISTRY}"
    fi

    if [ -n "$PROVER_REPUTATION" ]; then
        echo "PROVER_REPUTATION_ADDRESS=${PROVER_REPUTATION}"
    fi

    if [ -n "$REMAINDER_VERIFIER" ]; then
        echo "REMAINDER_VERIFIER_ADDRESS=${REMAINDER_VERIFIER}"
    fi

    if [ -n "$MOCK_VERIFIER" ]; then
        echo "MOCK_VERIFIER_ADDRESS=${MOCK_VERIFIER}"
    fi

    echo ""
    echo "# Deployer"
    if [ -n "$DEPLOYER" ]; then
        echo "DEPLOYER_ADDRESS=${DEPLOYER}"
    fi
    echo "# DEPLOYER_PRIVATE_KEY="

    echo ""
    echo "# Private Keys (fill in manually -- DO NOT commit)"
    echo "# CHALLENGER_PRIVATE_KEY="
    echo "# ENCLAVE_PRIVATE_KEY="
    echo "# OPERATOR_PRIVATE_KEY="

    echo ""
    echo "# Services"
    echo "# OPERATOR_URL=http://localhost:8080"
    echo "# PRIVATE_INPUT_SERVER_URL=http://localhost:8090"

    # Chain-specific additions
    if [ "${CHAIN_ID:-}" = "11155111" ]; then
        echo ""
        echo "# Sepolia RPC (replace with your API key)"
        echo "# ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY"
    fi
}

# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------
if [ -z "$OUTPUT" ]; then
    # Stdout mode
    info "Chain: $CHAIN_NAME (${CHAIN_ID:-unknown})"
    info "Source: $DEPLOY_FILE"
    printf "\n"
    generate_env
else
    # File output mode
    if [ "$MERGE" = "true" ] && [ -f "$OUTPUT" ]; then
        info "Merging with existing: $OUTPUT"

        # Generate new content to a temp file
        TEMP_FILE=$(mktemp)
        generate_env > "$TEMP_FILE"

        # Read existing .env keys
        MERGED_FILE=$(mktemp)
        cp "$OUTPUT" "$MERGED_FILE"

        # Append new keys that do not already exist
        ADDED=0
        while IFS= read -r line; do
            # Skip comments and empty lines
            case "$line" in
                '#'*|'')
                    continue
                    ;;
            esac

            # Extract key name
            KEY=$(echo "$line" | cut -d= -f1)

            # Check if key exists in current file
            if ! grep -qE "^${KEY}=" "$OUTPUT" 2>/dev/null; then
                echo "$line" >> "$MERGED_FILE"
                ADDED=$((ADDED + 1))
            fi
        done < "$TEMP_FILE"

        mv "$MERGED_FILE" "$OUTPUT"
        rm -f "$TEMP_FILE"

        ok "Merged into $OUTPUT ($ADDED new keys added)"
    else
        generate_env > "$OUTPUT"
        ok "Generated: $OUTPUT"
    fi

    info "Chain: $CHAIN_NAME (${CHAIN_ID:-unknown})"

    # Count non-empty addresses
    ADDR_COUNT=0
    for addr in "$TEE_VERIFIER" "$EXECUTION_ENGINE" "$PROGRAM_REGISTRY" \
                "$PROVER_REGISTRY" "$PROVER_REPUTATION" "$REMAINDER_VERIFIER"; do
        if [ -n "$addr" ]; then
            ADDR_COUNT=$((ADDR_COUNT + 1))
        fi
    done

    info "Contract addresses found: $ADDR_COUNT"
fi
