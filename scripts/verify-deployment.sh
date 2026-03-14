#!/usr/bin/env bash
# Full-stack deployment verification for World ZK Compute contracts.
#
# Verifies that all deployed contracts have bytecode, correct ownership,
# are not paused, and are properly wired together.
#
# Options:
#   --chain-id <id>     Chain ID to look up deployment file and RPC
#   --rpc-url <url>     RPC endpoint (overrides chains.json lookup)
#
# Address sources (in priority order):
#   1. deployments/<chain-id>.json  (if it exists and has addresses)
#   2. Environment variables:
#        TEE_VERIFIER_ADDR
#        EXECUTION_ENGINE_ADDR
#        PROGRAM_REGISTRY_ADDR
#        PROVER_REGISTRY_ADDR
#        PROVER_REPUTATION_ADDR
#
# Usage:
#   scripts/verify-deployment.sh --chain-id 31337
#   scripts/verify-deployment.sh --chain-id 421614 --rpc-url https://sepolia-rollup.arbitrum.io/rpc
#   TEE_VERIFIER_ADDR=0x... EXECUTION_ENGINE_ADDR=0x... scripts/verify-deployment.sh --rpc-url http://localhost:8545
#
# Requires: cast (from Foundry)

set -euo pipefail

# ==============================================================================
# Constants
# ==============================================================================

ZERO_ADDRESS="0x0000000000000000000000000000000000000000"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENTS_DIR="$PROJECT_DIR/deployments"
CHAINS_FILE="$DEPLOYMENTS_DIR/chains.json"

DEFAULT_RPC_URL="http://127.0.0.1:8545"

# Counters
PASS_COUNT=0
FAIL_COUNT=0
WARN_COUNT=0
TOTAL_CHECKS=0

# ==============================================================================
# Help
# ==============================================================================

show_help() {
    cat <<'EOF'
verify-deployment.sh -- Full-stack deployment health check

USAGE:
  scripts/verify-deployment.sh --chain-id <id> [--rpc-url <url>]
  scripts/verify-deployment.sh --rpc-url <url>

OPTIONS:
  --chain-id <id>    Chain ID (loads addresses from deployments/<id>.json)
  --rpc-url <url>    RPC endpoint (default: http://127.0.0.1:8545)
  -h, --help         Show this help

ADDRESS SOURCES (priority order):
  1. deployments/<chain-id>.json
  2. Environment variables:
       TEE_VERIFIER_ADDR, EXECUTION_ENGINE_ADDR, PROGRAM_REGISTRY_ADDR,
       PROVER_REGISTRY_ADDR, PROVER_REPUTATION_ADDR

CHECKS PER CONTRACT:
  - Bytecode exists (not 0x)
  - owner() returns non-zero address
  - paused() returns false (where applicable)

WIRING CHECKS:
  - ExecutionEngine.verifier() matches expected verifier
  - ExecutionEngine.registry() matches ProgramRegistry
  - ExecutionEngine.reputation() matches ProverReputation
  - ProverReputation.authorizedReporters(engine) is true

EXIT:
  0 if all checks pass, 1 if any check fails.

REQUIRES:
  cast (from Foundry)
EOF
}

# ==============================================================================
# Helpers
# ==============================================================================

record_pass() {
    local label="$1"
    local detail="$2"
    printf "  [PASS] %-30s %s\n" "$label" "$detail"
    PASS_COUNT=$((PASS_COUNT + 1))
    TOTAL_CHECKS=$((TOTAL_CHECKS + 1))
}

record_fail() {
    local label="$1"
    local detail="$2"
    printf "  [FAIL] %-30s %s\n" "$label" "$detail"
    FAIL_COUNT=$((FAIL_COUNT + 1))
    TOTAL_CHECKS=$((TOTAL_CHECKS + 1))
}

record_warn() {
    local label="$1"
    local detail="$2"
    printf "  [WARN] %-30s %s\n" "$label" "$detail"
    WARN_COUNT=$((WARN_COUNT + 1))
    TOTAL_CHECKS=$((TOTAL_CHECKS + 1))
}

# Strip cast formatting annotations like " [1e17]"
# shellcheck disable=SC2329
strip_annotation() {
    local val="$1"
    echo "${val%% \[*\]}"
}

# Shorten an address for display: 0x1234...abcd
short_addr() {
    local addr="$1"
    if [[ ${#addr} -ge 42 ]]; then
        echo "${addr:0:6}...${addr: -4}"
    else
        echo "$addr"
    fi
}

# Read a JSON field using cast (avoids jq dependency). Falls back to python/jq.
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

# Check if a value looks like a valid nonzero address
is_valid_addr() {
    local addr="$1"
    if [[ -z "$addr" || "$addr" == "$ZERO_ADDRESS" || "$addr" == "0x" || ${#addr} -lt 42 ]]; then
        return 1
    fi
    return 0
}

# ==============================================================================
# Contract verification functions
# ==============================================================================

# Check that bytecode exists at an address.
# Returns 0 if code exists, 1 otherwise.
check_has_code() {
    local name="$1"
    local addr="$2"

    local code
    code=$(cast code "$addr" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")

    if [[ "$code" == "0x" || -z "$code" ]]; then
        record_fail "$name has_code" "no bytecode at $addr"
        return 1
    else
        local code_bytes=$(( ${#code} / 2 - 1 ))
        record_pass "$name has_code" "${code_bytes} bytes"
        return 0
    fi
}

# Query owner() and record pass/fail. Sets LAST_OWNER as a side effect.
# Does NOT use subshell so counters are preserved.
LAST_OWNER=""
check_owner() {
    local name="$1"
    local addr="$2"

    LAST_OWNER=$(cast call "$addr" "owner()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    if [[ -z "$LAST_OWNER" ]]; then
        record_fail "$name owner" "call reverted or empty"
        LAST_OWNER=""
        return 1
    elif [[ "$LAST_OWNER" == "$ZERO_ADDRESS" ]]; then
        record_fail "$name owner" "owner is zero address"
        return 1
    else
        record_pass "$name owner" "$(short_addr "$LAST_OWNER")"
        return 0
    fi
}

# Check paused() returns false. Not all contracts have this; pass gracefully if missing.
check_not_paused() {
    local name="$1"
    local addr="$2"

    local paused
    paused=$(cast call "$addr" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null || echo "UNSUPPORTED")

    if [[ "$paused" == "UNSUPPORTED" ]]; then
        record_warn "$name paused" "no paused() function"
        return 0
    elif [[ "$paused" == "true" ]]; then
        record_fail "$name paused" "contract IS paused"
        return 1
    else
        record_pass "$name paused" "false"
        return 0
    fi
}

# Owner storage (bash 3.x compatible -- no associative arrays)
OWNER_TEEMLVerifier=""
OWNER_ExecutionEngine=""
OWNER_ProgramRegistry=""
OWNER_ProverRegistry=""
OWNER_ProverReputation=""

set_owner() {
    local name="$1"
    local value="$2"
    case "$name" in
        TEEMLVerifier)     OWNER_TEEMLVerifier="$value" ;;
        ExecutionEngine)   OWNER_ExecutionEngine="$value" ;;
        ProgramRegistry)   OWNER_ProgramRegistry="$value" ;;
        ProverRegistry)    OWNER_ProverRegistry="$value" ;;
        ProverReputation)  OWNER_ProverReputation="$value" ;;
    esac
}

get_owner() {
    local name="$1"
    case "$name" in
        TEEMLVerifier)     echo "$OWNER_TEEMLVerifier" ;;
        ExecutionEngine)   echo "$OWNER_ExecutionEngine" ;;
        ProgramRegistry)   echo "$OWNER_ProgramRegistry" ;;
        ProverRegistry)    echo "$OWNER_ProverRegistry" ;;
        ProverReputation)  echo "$OWNER_ProverReputation" ;;
        *)                 echo "" ;;
    esac
}

# Full per-contract health check: code + owner + paused.
verify_contract() {
    local name="$1"
    local addr="$2"

    if ! is_valid_addr "$addr"; then
        record_warn "$name" "address not provided, skipping"
        return 0
    fi

    echo ""
    echo "--- $name ($addr) ---"

    local has_code_ok=true
    check_has_code "$name" "$addr" || has_code_ok=false

    if [[ "$has_code_ok" == "false" ]]; then
        record_fail "$name owner" "skipped (no bytecode)"
        record_fail "$name paused" "skipped (no bytecode)"
        return 1
    fi

    check_owner "$name" "$addr" || true
    set_owner "$name" "$LAST_OWNER"

    check_not_paused "$name" "$addr" || true
}

# ==============================================================================
# Wiring verification functions
# ==============================================================================

check_engine_verifier() {
    local engine_addr="$1"
    local expected_verifier="$2"

    if ! is_valid_addr "$engine_addr"; then
        return 0
    fi

    local actual
    actual=$(cast call "$engine_addr" "verifier()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    if [[ -z "$actual" ]]; then
        record_fail "Engine->verifier" "call reverted"
        return 1
    fi

    # Normalize to lowercase for comparison
    local actual_lower expected_lower
    actual_lower=$(echo "$actual" | tr '[:upper:]' '[:lower:]')

    if is_valid_addr "$expected_verifier"; then
        expected_lower=$(echo "$expected_verifier" | tr '[:upper:]' '[:lower:]')
        if [[ "$actual_lower" == "$expected_lower" ]]; then
            record_pass "Engine->verifier" "$(short_addr "$actual")"
        else
            record_fail "Engine->verifier" "got $(short_addr "$actual"), expected $(short_addr "$expected_verifier")"
        fi
    else
        # No expected value, just confirm it is set
        if [[ "$actual" == "$ZERO_ADDRESS" ]]; then
            record_fail "Engine->verifier" "verifier is zero address"
        else
            record_pass "Engine->verifier" "$(short_addr "$actual")"
        fi
    fi
}

check_engine_registry() {
    local engine_addr="$1"
    local expected_registry="$2"

    if ! is_valid_addr "$engine_addr"; then
        return 0
    fi

    local actual
    actual=$(cast call "$engine_addr" "registry()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    if [[ -z "$actual" ]]; then
        record_fail "Engine->registry" "call reverted"
        return 1
    fi

    local actual_lower
    actual_lower=$(echo "$actual" | tr '[:upper:]' '[:lower:]')

    if is_valid_addr "$expected_registry"; then
        local expected_lower
        expected_lower=$(echo "$expected_registry" | tr '[:upper:]' '[:lower:]')
        if [[ "$actual_lower" == "$expected_lower" ]]; then
            record_pass "Engine->registry" "$(short_addr "$actual")"
        else
            record_fail "Engine->registry" "got $(short_addr "$actual"), expected $(short_addr "$expected_registry")"
        fi
    else
        if [[ "$actual" == "$ZERO_ADDRESS" ]]; then
            record_fail "Engine->registry" "registry is zero address"
        else
            record_pass "Engine->registry" "$(short_addr "$actual")"
        fi
    fi
}

check_engine_reputation() {
    local engine_addr="$1"
    local expected_reputation="$2"

    if ! is_valid_addr "$engine_addr"; then
        return 0
    fi

    local actual
    actual=$(cast call "$engine_addr" "reputation()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    if [[ -z "$actual" ]]; then
        # reputation() may not exist on all ExecutionEngine versions
        record_warn "Engine->reputation" "call reverted (may not be configured)"
        return 0
    fi

    local actual_lower
    actual_lower=$(echo "$actual" | tr '[:upper:]' '[:lower:]')

    if is_valid_addr "$expected_reputation"; then
        local expected_lower
        expected_lower=$(echo "$expected_reputation" | tr '[:upper:]' '[:lower:]')
        if [[ "$actual_lower" == "$expected_lower" ]]; then
            record_pass "Engine->reputation" "$(short_addr "$actual")"
        else
            record_fail "Engine->reputation" "got $(short_addr "$actual"), expected $(short_addr "$expected_reputation")"
        fi
    else
        if [[ "$actual" == "$ZERO_ADDRESS" ]]; then
            record_warn "Engine->reputation" "not configured (zero address)"
        else
            record_pass "Engine->reputation" "$(short_addr "$actual")"
        fi
    fi
}

check_reputation_reporter() {
    local reputation_addr="$1"
    local engine_addr="$2"

    if ! is_valid_addr "$reputation_addr" || ! is_valid_addr "$engine_addr"; then
        return 0
    fi

    local authorized
    authorized=$(cast call "$reputation_addr" "authorizedReporters(address)(bool)" "$engine_addr" --rpc-url "$RPC_URL" 2>/dev/null || echo "UNSUPPORTED")

    if [[ "$authorized" == "UNSUPPORTED" ]]; then
        record_warn "Reputation reporters" "call reverted"
        return 0
    elif [[ "$authorized" == "true" ]]; then
        record_pass "Reputation reporters" "Engine $(short_addr "$engine_addr") authorized"
    else
        record_fail "Reputation reporters" "Engine $(short_addr "$engine_addr") NOT authorized"
    fi
}

# ==============================================================================
# Parse arguments
# ==============================================================================

CHAIN_ID=""
RPC_URL=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --chain-id)
            CHAIN_ID="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo "ERROR: Unknown argument: $1"
            echo "Run with --help for usage."
            exit 1
            ;;
    esac
done

# ==============================================================================
# Resolve RPC URL from chains.json if not explicitly provided
# ==============================================================================

if [[ -z "$RPC_URL" && -n "$CHAIN_ID" && -f "$CHAINS_FILE" ]]; then
    if command -v jq &>/dev/null; then
        RPC_URL=$(jq -r ".chains[] | select(.chainId == $CHAIN_ID) | .rpcUrl // empty" "$CHAINS_FILE" 2>/dev/null || echo "")
    elif command -v python3 &>/dev/null; then
        RPC_URL=$(python3 -c "
import json
with open('$CHAINS_FILE') as f:
    data = json.load(f)
for c in data.get('chains', []):
    if c.get('chainId') == $CHAIN_ID:
        print(c.get('rpcUrl', ''))
        break
" 2>/dev/null || echo "")
    fi
fi

RPC_URL="${RPC_URL:-$DEFAULT_RPC_URL}"

# ==============================================================================
# Verify cast is available
# ==============================================================================

if ! command -v cast &>/dev/null; then
    echo "ERROR: 'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

# ==============================================================================
# Load addresses from deployment file or env vars
# ==============================================================================

ADDR_TEE_VERIFIER="${TEE_VERIFIER_ADDR:-}"
ADDR_EXECUTION_ENGINE="${EXECUTION_ENGINE_ADDR:-}"
ADDR_PROGRAM_REGISTRY="${PROGRAM_REGISTRY_ADDR:-}"
ADDR_PROVER_REGISTRY="${PROVER_REGISTRY_ADDR:-}"
ADDR_PROVER_REPUTATION="${PROVER_REPUTATION_ADDR:-}"

# Try loading from deployments/<chain-id>.json
if [[ -n "$CHAIN_ID" ]]; then
    DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_ID}.json"

    if [[ ! -f "$DEPLOY_FILE" ]]; then
        # Also try name-based files (e.g., arbitrum-sepolia.json)
        # Look up chain name from chains.json
        CHAIN_NAME=""
        if [[ -f "$CHAINS_FILE" ]]; then
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
        fi

        if [[ -n "$CHAIN_NAME" && -f "$DEPLOYMENTS_DIR/${CHAIN_NAME}.json" ]]; then
            DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_NAME}.json"
        fi
    fi

    if [[ -f "$DEPLOY_FILE" ]]; then
        echo "Loading addresses from $DEPLOY_FILE"

        # Try both flat format and nested "contracts" format
        _try_field() {
            local field="$1"
            local val=""
            val=$(json_field "$DEPLOY_FILE" ".${field}")
            if [[ -z "$val" ]]; then
                val=$(json_field "$DEPLOY_FILE" ".contracts.${field}")
            fi
            echo "$val"
        }

        [[ -z "$ADDR_TEE_VERIFIER" ]]       && ADDR_TEE_VERIFIER=$(_try_field "TEEMLVerifier")
        [[ -z "$ADDR_EXECUTION_ENGINE" ]]    && ADDR_EXECUTION_ENGINE=$(_try_field "ExecutionEngine")
        [[ -z "$ADDR_PROGRAM_REGISTRY" ]]    && ADDR_PROGRAM_REGISTRY=$(_try_field "ProgramRegistry")
        [[ -z "$ADDR_PROVER_REGISTRY" ]]     && ADDR_PROVER_REGISTRY=$(_try_field "ProverRegistry")
        [[ -z "$ADDR_PROVER_REPUTATION" ]]   && ADDR_PROVER_REPUTATION=$(_try_field "ProverReputation")
    else
        echo "No deployment file found for chain $CHAIN_ID"
    fi
fi

# ==============================================================================
# Validate we have at least one address to check
# ==============================================================================

HAS_ANY_ADDR=false
for addr in "$ADDR_TEE_VERIFIER" "$ADDR_EXECUTION_ENGINE" "$ADDR_PROGRAM_REGISTRY" \
            "$ADDR_PROVER_REGISTRY" "$ADDR_PROVER_REPUTATION"; do
    if is_valid_addr "$addr"; then
        HAS_ANY_ADDR=true
        break
    fi
done

if [[ "$HAS_ANY_ADDR" == "false" ]]; then
    echo ""
    echo "ERROR: No contract addresses provided."
    echo ""
    echo "Provide addresses via:"
    echo "  1. deployments/<chain-id>.json with --chain-id"
    echo "  2. Environment variables: TEE_VERIFIER_ADDR, EXECUTION_ENGINE_ADDR, etc."
    echo ""
    echo "Run with --help for details."
    exit 1
fi

# ==============================================================================
# Print header
# ==============================================================================

echo ""
echo "========================================================"
echo " World ZK Compute -- Deployment Health Check"
echo "========================================================"
echo "  RPC URL:           $RPC_URL"
[[ -n "$CHAIN_ID" ]] && echo "  Chain ID:          $CHAIN_ID"
echo ""
echo "  Addresses:"
is_valid_addr "$ADDR_TEE_VERIFIER"       && echo "    TEEMLVerifier:     $ADDR_TEE_VERIFIER"
is_valid_addr "$ADDR_EXECUTION_ENGINE"    && echo "    ExecutionEngine:   $ADDR_EXECUTION_ENGINE"
is_valid_addr "$ADDR_PROGRAM_REGISTRY"    && echo "    ProgramRegistry:   $ADDR_PROGRAM_REGISTRY"
is_valid_addr "$ADDR_PROVER_REGISTRY"     && echo "    ProverRegistry:    $ADDR_PROVER_REGISTRY"
is_valid_addr "$ADDR_PROVER_REPUTATION"   && echo "    ProverReputation:  $ADDR_PROVER_REPUTATION"

# ==============================================================================
# Per-contract verification
# ==============================================================================

echo ""
echo "========================================================"
echo " Contract Health"
echo "========================================================"

verify_contract "TEEMLVerifier"     "$ADDR_TEE_VERIFIER"     || true
verify_contract "ExecutionEngine"   "$ADDR_EXECUTION_ENGINE"  || true
verify_contract "ProgramRegistry"   "$ADDR_PROGRAM_REGISTRY"  || true
verify_contract "ProverRegistry"    "$ADDR_PROVER_REGISTRY"   || true
verify_contract "ProverReputation"  "$ADDR_PROVER_REPUTATION" || true

# ==============================================================================
# Wiring verification
# ==============================================================================

echo ""
echo "========================================================"
echo " Wiring Checks"
echo "========================================================"
echo ""

if is_valid_addr "$ADDR_EXECUTION_ENGINE"; then
    check_engine_verifier   "$ADDR_EXECUTION_ENGINE" ""                         || true
    check_engine_registry   "$ADDR_EXECUTION_ENGINE" "$ADDR_PROGRAM_REGISTRY"   || true
    check_engine_reputation "$ADDR_EXECUTION_ENGINE" "$ADDR_PROVER_REPUTATION"  || true
else
    echo "  [SKIP] ExecutionEngine not provided -- wiring checks skipped"
fi

if is_valid_addr "$ADDR_PROVER_REPUTATION" && is_valid_addr "$ADDR_EXECUTION_ENGINE"; then
    check_reputation_reporter "$ADDR_PROVER_REPUTATION" "$ADDR_EXECUTION_ENGINE" || true
fi

# ==============================================================================
# Health report table
# ==============================================================================

echo ""
echo "========================================================"
echo " Health Report"
echo "========================================================"
echo ""

# Table header
printf "  %-20s %-44s %-44s %-8s %-10s\n" "CONTRACT" "ADDRESS" "OWNER" "PAUSED" "HAS_CODE"
printf "  %-20s %-44s %-44s %-8s %-10s\n" "--------------------" "--------------------------------------------" "--------------------------------------------" "--------" "----------"

# Helper to print a row
print_row() {
    local name="$1"
    local addr="$2"

    if ! is_valid_addr "$addr"; then
        printf "  %-20s %-44s %-44s %-8s %-10s\n" "$name" "(not provided)" "-" "-" "-"
        return
    fi

    # owner
    local owner
    owner=$(get_owner "$name")
    local owner_display
    if [[ -z "$owner" ]]; then
        owner_display="(unknown)"
    elif [[ "$owner" == "$ZERO_ADDRESS" ]]; then
        owner_display="ZERO"
    else
        owner_display="$(short_addr "$owner")"
    fi

    # paused
    local paused_val
    paused_val=$(cast call "$addr" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null || echo "N/A")
    local paused_display="$paused_val"

    # code
    local code
    code=$(cast code "$addr" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")
    local code_display
    if [[ "$code" == "0x" || -z "$code" ]]; then
        code_display="NO"
    else
        code_display="YES"
    fi

    printf "  %-20s %-44s %-44s %-8s %-10s\n" "$name" "$addr" "$owner_display" "$paused_display" "$code_display"
}

print_row "TEEMLVerifier"     "$ADDR_TEE_VERIFIER"
print_row "ExecutionEngine"   "$ADDR_EXECUTION_ENGINE"
print_row "ProgramRegistry"   "$ADDR_PROGRAM_REGISTRY"
print_row "ProverRegistry"    "$ADDR_PROVER_REGISTRY"
print_row "ProverReputation"  "$ADDR_PROVER_REPUTATION"

# ==============================================================================
# Summary
# ==============================================================================

echo ""
echo "========================================================"
echo " Summary"
echo "========================================================"
echo ""
echo "  Passed:   $PASS_COUNT"
echo "  Failed:   $FAIL_COUNT"
echo "  Warnings: $WARN_COUNT"
echo "  Total:    $TOTAL_CHECKS"
echo ""

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    echo "  RESULT: FAIL"
    echo ""
    exit 1
else
    echo "  RESULT: PASS"
    echo ""
    exit 0
fi
