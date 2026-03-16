#!/usr/bin/env bash
# =============================================================================
# World ZK Compute -- Post-Deploy Security Audit
#
# Automated security audit for deployed contracts. Reads contract addresses
# from a deployment file (or env vars), runs a battery of checks, and prints
# PASS/FAIL for each with an overall risk score.
#
# Usage:
#   scripts/audit-deployment.sh --rpc-url http://127.0.0.1:8545 --chain-id 31337
#   scripts/audit-deployment.sh --rpc-url https://sepolia-rollup.arbitrum.io/rpc \
#       --chain-id 421614 --expected-owner 0xABC...
#   scripts/audit-deployment.sh --help
#
# Flags:
#   --rpc-url <url>            RPC endpoint (required)
#   --chain-id <id>            Chain ID (reads from deployments/<id>.json)
#   --expected-owner <addr>    Expected owner address for all contracts
#   --help                     Show this help message
#
# Exit codes:
#   0 -- all checks passed (risk score = 0)
#   1 -- one or more checks failed
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
fail_msg(){ printf "${RED}[FAIL]${RESET}  %s\n" "$*"; }
warn_msg(){ printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
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
CHAIN_ID=""
EXPECTED_OWNER=""

# ---------------------------------------------------------------------------
# Tracking
# ---------------------------------------------------------------------------
ZERO_ADDRESS="0x0000000000000000000000000000000000000000"

declare -a CHECK_NAMES=()
declare -a CHECK_RESULTS=()
declare -a CHECK_DETAILS=()
PASS_COUNT=0
FAIL_COUNT=0
WARN_COUNT=0

record_pass() {
    local name="$1"
    local detail="${2:-}"
    CHECK_NAMES+=("$name")
    CHECK_RESULTS+=("PASS")
    CHECK_DETAILS+=("$detail")
    PASS_COUNT=$((PASS_COUNT + 1))
    ok "$name${detail:+ -- $detail}"
}

record_fail() {
    local name="$1"
    local detail="${2:-}"
    CHECK_NAMES+=("$name")
    CHECK_RESULTS+=("FAIL")
    CHECK_DETAILS+=("$detail")
    FAIL_COUNT=$((FAIL_COUNT + 1))
    fail_msg "$name${detail:+ -- $detail}"
}

record_warn() {
    local name="$1"
    local detail="${2:-}"
    CHECK_NAMES+=("$name")
    CHECK_RESULTS+=("WARN")
    CHECK_DETAILS+=("$detail")
    WARN_COUNT=$((WARN_COUNT + 1))
    warn_msg "$name${detail:+ -- $detail}"
}

# Strip cast formatting annotations like " [1e17]"
strip_annotation() {
    local val="$1"
    echo "${val%% \[*\]}"
}

# Short address for display
short_addr() {
    local addr="$1"
    if [[ ${#addr} -ge 42 ]]; then
        echo "${addr:0:6}...${addr: -4}"
    else
        echo "$addr"
    fi
}

is_valid_addr() {
    local addr="$1"
    if [[ -z "$addr" || "$addr" == "$ZERO_ADDRESS" || "$addr" == "0x" || ${#addr} -lt 42 ]]; then
        return 1
    fi
    return 0
}

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
audit-deployment.sh -- Post-deploy security audit for World ZK Compute.

USAGE:
  scripts/audit-deployment.sh --rpc-url <url> --chain-id <id> [OPTIONS]

REQUIRED:
  --rpc-url <url>              RPC endpoint
  --chain-id <id>              Chain ID (loads addresses from deployments/<id>.json)

OPTIONS:
  --expected-owner <addr>      Expected owner address for all contracts
  --help                       Show this help message

CHECKS:
  - Contract bytecode exists
  - Contract code size vs EIP-170 limit (24,576 bytes)
  - Owner is expected address (if --expected-owner provided)
  - Contracts are not paused
  - TEEMLVerifier prover stake > 0
  - TEEMLVerifier challenge bond > 0
  - Registered program count
  - Registered enclave count
  - Admin wallet balance

OUTPUT:
  PASS/FAIL for each check, risk score summary.

EXAMPLES:
  scripts/audit-deployment.sh --rpc-url http://127.0.0.1:8545 --chain-id 31337
  scripts/audit-deployment.sh --rpc-url https://sepolia-rollup.arbitrum.io/rpc \
      --chain-id 421614 --expected-owner 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
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
        --chain-id)
            CHAIN_ID="$2"
            shift 2
            ;;
        --expected-owner)
            EXPECTED_OWNER="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            fail_msg "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate prerequisites
# ---------------------------------------------------------------------------
if [[ -z "$RPC_URL" ]]; then
    fail_msg "--rpc-url is required."
    echo ""
    usage
    exit 1
fi

if [[ -z "$CHAIN_ID" ]]; then
    fail_msg "--chain-id is required."
    echo ""
    usage
    exit 1
fi

if ! command -v cast &>/dev/null; then
    fail_msg "cast is required. Install Foundry: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

# ---------------------------------------------------------------------------
# Load chain config for code size limit
# ---------------------------------------------------------------------------
CODE_SIZE_LIMIT=24576
if [[ -f "$CHAINS_FILE" ]]; then
    if command -v jq &>/dev/null; then
        CHAIN_LIMIT=$(jq -r ".chains[] | select(.chainId == $CHAIN_ID) | .codeSizeLimit // 24576" "$CHAINS_FILE" 2>/dev/null || echo "24576")
        CODE_SIZE_LIMIT="$CHAIN_LIMIT"
    fi
fi

# ---------------------------------------------------------------------------
# Load contract addresses from deployment file
# ---------------------------------------------------------------------------
ADDR_TEE_VERIFIER=""
ADDR_EXECUTION_ENGINE=""
ADDR_PROGRAM_REGISTRY=""
ADDR_REMAINDER_VERIFIER=""
DEPLOYER_ADDR=""

DEPLOY_FILE="$DEPLOYMENTS_DIR/${CHAIN_ID}.json"

# Try name-based lookup if numeric file not found
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
    fail_msg "Deployment file not found: $DEPLOY_FILE"
    exit 1
fi

info "Loading addresses from $DEPLOY_FILE"

# Extract addresses -- supports both { "contracts": { "Name": { "address": "0x..." } } }
# and { "contracts": { "Name": "0x..." } } formats
_extract_addr() {
    local key="$1"
    local val=""
    if command -v jq &>/dev/null; then
        val=$(jq -r ".contracts.\"$key\".address // .contracts.\"$key\" // empty" "$DEPLOY_FILE" 2>/dev/null || echo "")
    elif command -v python3 &>/dev/null; then
        val=$(python3 -c "
import json
with open('$DEPLOY_FILE') as f:
    data = json.load(f)
c = data.get('contracts', {}).get('$key', '')
if isinstance(c, dict):
    print(c.get('address', ''))
else:
    print(str(c) if c else '')
" 2>/dev/null || echo "")
    fi
    echo "$val"
}

ADDR_TEE_VERIFIER=$(_extract_addr "TEEMLVerifier")
ADDR_EXECUTION_ENGINE=$(_extract_addr "ExecutionEngine")
ADDR_PROGRAM_REGISTRY=$(_extract_addr "ProgramRegistry")
ADDR_REMAINDER_VERIFIER=$(_extract_addr "RemainderVerifier")

# Extract deployer
if command -v jq &>/dev/null; then
    DEPLOYER_ADDR=$(jq -r '.deployer // empty' "$DEPLOY_FILE" 2>/dev/null || echo "")
elif command -v python3 &>/dev/null; then
    DEPLOYER_ADDR=$(python3 -c "
import json
with open('$DEPLOY_FILE') as f:
    data = json.load(f)
print(data.get('deployer', ''))
" 2>/dev/null || echo "")
fi

# ---------------------------------------------------------------------------
# Print audit plan
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- Post-Deploy Security Audit"
header "============================================================"
echo ""
info "RPC URL:       $RPC_URL"
info "Chain ID:      $CHAIN_ID"
info "Code Limit:    $CODE_SIZE_LIMIT bytes"
info "Deploy File:   $DEPLOY_FILE"
if is_valid_addr "$DEPLOYER_ADDR"; then
    info "Deployer:      $DEPLOYER_ADDR"
fi
if is_valid_addr "$EXPECTED_OWNER"; then
    info "Expected Owner: $EXPECTED_OWNER"
fi
echo ""
info "Contracts found:"
is_valid_addr "$ADDR_TEE_VERIFIER"       && info "  TEEMLVerifier:       $ADDR_TEE_VERIFIER"
is_valid_addr "$ADDR_EXECUTION_ENGINE"    && info "  ExecutionEngine:     $ADDR_EXECUTION_ENGINE"
is_valid_addr "$ADDR_PROGRAM_REGISTRY"    && info "  ProgramRegistry:     $ADDR_PROGRAM_REGISTRY"
is_valid_addr "$ADDR_REMAINDER_VERIFIER"  && info "  RemainderVerifier:   $ADDR_REMAINDER_VERIFIER"

# =============================================================================
# AUDIT CHECKS
# =============================================================================

# ---------------------------------------------------------------------------
# Check 1: Contract bytecode existence and code size
# ---------------------------------------------------------------------------
header "--- Check: Contract Bytecode & Code Size ---"

check_bytecode_and_size() {
    local name="$1"
    local addr="$2"

    if ! is_valid_addr "$addr"; then
        record_warn "$name bytecode" "address not provided, skipping"
        return
    fi

    local code
    code=$(cast code "$addr" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")

    if [[ "$code" == "0x" || -z "$code" ]]; then
        record_fail "$name bytecode" "no bytecode at $addr"
        record_fail "$name code size" "skipped (no bytecode)"
        return
    fi

    # Code length in bytes: (hex chars - 2 for 0x prefix) / 2
    local code_bytes=$(( (${#code} - 2) / 2 ))
    record_pass "$name bytecode" "${code_bytes} bytes at $(short_addr "$addr")"

    # Check against code size limit
    if [[ "$code_bytes" -gt "$CODE_SIZE_LIMIT" ]]; then
        local pct=$(( code_bytes * 100 / CODE_SIZE_LIMIT ))
        record_fail "$name code size" "${code_bytes}/${CODE_SIZE_LIMIT} bytes (${pct}% -- EXCEEDS LIMIT)"
    else
        local pct=$(( code_bytes * 100 / CODE_SIZE_LIMIT ))
        record_pass "$name code size" "${code_bytes}/${CODE_SIZE_LIMIT} bytes (${pct}%)"
    fi
}

check_bytecode_and_size "TEEMLVerifier"     "$ADDR_TEE_VERIFIER"
check_bytecode_and_size "ExecutionEngine"   "$ADDR_EXECUTION_ENGINE"
check_bytecode_and_size "ProgramRegistry"   "$ADDR_PROGRAM_REGISTRY"
check_bytecode_and_size "RemainderVerifier" "$ADDR_REMAINDER_VERIFIER"

# ---------------------------------------------------------------------------
# Check 2: Ownership
# ---------------------------------------------------------------------------
header "--- Check: Contract Ownership ---"

check_owner() {
    local name="$1"
    local addr="$2"

    if ! is_valid_addr "$addr"; then
        record_warn "$name owner" "address not provided, skipping"
        return
    fi

    local owner
    owner=$(cast call "$addr" "owner()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")

    if [[ -z "$owner" ]]; then
        record_fail "$name owner" "owner() call reverted"
        return
    fi

    if [[ "$owner" == "$ZERO_ADDRESS" ]]; then
        record_fail "$name owner" "owner is zero address (renounced?)"
        return
    fi

    if is_valid_addr "$EXPECTED_OWNER"; then
        local owner_lower expected_lower
        owner_lower=$(echo "$owner" | tr '[:upper:]' '[:lower:]')
        expected_lower=$(echo "$EXPECTED_OWNER" | tr '[:upper:]' '[:lower:]')
        if [[ "$owner_lower" == "$expected_lower" ]]; then
            record_pass "$name owner" "$(short_addr "$owner") (matches expected)"
        else
            record_fail "$name owner" "$(short_addr "$owner") (expected $(short_addr "$EXPECTED_OWNER"))"
        fi
    else
        record_pass "$name owner" "$(short_addr "$owner")"
    fi
}

check_owner "TEEMLVerifier"     "$ADDR_TEE_VERIFIER"
check_owner "ExecutionEngine"   "$ADDR_EXECUTION_ENGINE"
check_owner "ProgramRegistry"   "$ADDR_PROGRAM_REGISTRY"
check_owner "RemainderVerifier" "$ADDR_REMAINDER_VERIFIER"

# ---------------------------------------------------------------------------
# Check 3: Pause state
# ---------------------------------------------------------------------------
header "--- Check: Pause State ---"

check_not_paused() {
    local name="$1"
    local addr="$2"

    if ! is_valid_addr "$addr"; then
        record_warn "$name paused" "address not provided, skipping"
        return
    fi

    local paused
    paused=$(cast call "$addr" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null || echo "UNSUPPORTED")

    if [[ "$paused" == "UNSUPPORTED" ]]; then
        record_warn "$name paused" "no paused() function"
        return
    fi

    if [[ "$paused" == "true" ]]; then
        record_fail "$name paused" "contract IS paused"
    else
        record_pass "$name paused" "false"
    fi
}

check_not_paused "TEEMLVerifier"     "$ADDR_TEE_VERIFIER"
check_not_paused "ExecutionEngine"   "$ADDR_EXECUTION_ENGINE"
check_not_paused "ProgramRegistry"   "$ADDR_PROGRAM_REGISTRY"

# ---------------------------------------------------------------------------
# Check 4: TEEMLVerifier prover stake
# ---------------------------------------------------------------------------
header "--- Check: TEEMLVerifier Configuration ---"

if is_valid_addr "$ADDR_TEE_VERIFIER"; then
    # Prover stake
    PROVER_STAKE=$(cast call "$ADDR_TEE_VERIFIER" "proverStake()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")
    PROVER_STAKE=$(strip_annotation "$PROVER_STAKE")

    if [[ -z "$PROVER_STAKE" ]]; then
        record_fail "TEE prover stake" "proverStake() call failed"
    elif [[ "$PROVER_STAKE" == "0" ]]; then
        record_fail "TEE prover stake" "stake is 0 (anyone can submit without collateral)"
    else
        STAKE_ETH=$(cast from-wei "$PROVER_STAKE" 2>/dev/null || echo "?")
        record_pass "TEE prover stake" "${STAKE_ETH} ETH ($PROVER_STAKE wei)"
    fi

    # Challenge bond
    CHALLENGE_BOND=$(cast call "$ADDR_TEE_VERIFIER" "challengeBondAmount()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")
    CHALLENGE_BOND=$(strip_annotation "$CHALLENGE_BOND")

    if [[ -z "$CHALLENGE_BOND" ]]; then
        record_fail "TEE challenge bond" "challengeBondAmount() call failed"
    elif [[ "$CHALLENGE_BOND" == "0" ]]; then
        record_fail "TEE challenge bond" "bond is 0 (free challenges -- griefing risk)"
    else
        BOND_ETH=$(cast from-wei "$CHALLENGE_BOND" 2>/dev/null || echo "?")
        record_pass "TEE challenge bond" "${BOND_ETH} ETH ($CHALLENGE_BOND wei)"
    fi
else
    record_warn "TEE prover stake" "TEEMLVerifier not deployed, skipping"
    record_warn "TEE challenge bond" "TEEMLVerifier not deployed, skipping"
fi

# ---------------------------------------------------------------------------
# Check 5: Registered programs
# ---------------------------------------------------------------------------
header "--- Check: Registered Programs ---"

if is_valid_addr "$ADDR_PROGRAM_REGISTRY"; then
    PROGRAM_COUNT=$(cast call "$ADDR_PROGRAM_REGISTRY" "programCount()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "UNSUPPORTED")
    PROGRAM_COUNT=$(strip_annotation "$PROGRAM_COUNT")

    if [[ "$PROGRAM_COUNT" == "UNSUPPORTED" || -z "$PROGRAM_COUNT" ]]; then
        # Try nextProgramId as alternative
        PROGRAM_COUNT=$(cast call "$ADDR_PROGRAM_REGISTRY" "nextProgramId()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")
        PROGRAM_COUNT=$(strip_annotation "$PROGRAM_COUNT")
        if [[ -n "$PROGRAM_COUNT" && "$PROGRAM_COUNT" != "0" ]]; then
            PROGRAM_COUNT=$((PROGRAM_COUNT - 1))
        fi
    fi

    if [[ -z "$PROGRAM_COUNT" ]]; then
        record_warn "Registered programs" "could not query program count"
    elif [[ "$PROGRAM_COUNT" == "0" ]]; then
        record_warn "Registered programs" "0 programs registered (deploy may be incomplete)"
    else
        record_pass "Registered programs" "$PROGRAM_COUNT program(s) registered"
    fi
else
    record_warn "Registered programs" "ProgramRegistry not deployed, skipping"
fi

# ---------------------------------------------------------------------------
# Check 6: Registered enclaves
# ---------------------------------------------------------------------------
header "--- Check: Registered Enclaves ---"

if is_valid_addr "$ADDR_TEE_VERIFIER"; then
    ENCLAVE_COUNT=$(cast call "$ADDR_TEE_VERIFIER" "enclaveCount()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "UNSUPPORTED")
    ENCLAVE_COUNT=$(strip_annotation "$ENCLAVE_COUNT")

    if [[ "$ENCLAVE_COUNT" == "UNSUPPORTED" || -z "$ENCLAVE_COUNT" ]]; then
        record_warn "Registered enclaves" "enclaveCount() not available (check manually)"
    elif [[ "$ENCLAVE_COUNT" == "0" ]]; then
        record_warn "Registered enclaves" "0 enclaves registered (no TEE attestation possible)"
    else
        record_pass "Registered enclaves" "$ENCLAVE_COUNT enclave(s) registered"
    fi
else
    record_warn "Registered enclaves" "TEEMLVerifier not deployed, skipping"
fi

# ---------------------------------------------------------------------------
# Check 7: Admin wallet balance
# ---------------------------------------------------------------------------
header "--- Check: Admin Wallet Balance ---"

ADMIN_TO_CHECK=""
if is_valid_addr "$EXPECTED_OWNER"; then
    ADMIN_TO_CHECK="$EXPECTED_OWNER"
elif is_valid_addr "$DEPLOYER_ADDR"; then
    ADMIN_TO_CHECK="$DEPLOYER_ADDR"
fi

if is_valid_addr "$ADMIN_TO_CHECK"; then
    BALANCE_WEI=$(cast balance "$ADMIN_TO_CHECK" --rpc-url "$RPC_URL" 2>/dev/null || echo "0")
    BALANCE_WEI=$(strip_annotation "$BALANCE_WEI")
    BALANCE_ETH=$(cast from-wei "$BALANCE_WEI" 2>/dev/null || echo "?")

    # Warn if balance is below 0.01 ETH
    MIN_BALANCE=10000000000000000  # 0.01 ETH
    if [[ "$BALANCE_WEI" -lt "$MIN_BALANCE" ]] 2>/dev/null; then
        record_fail "Admin wallet balance" "${BALANCE_ETH} ETH for $(short_addr "$ADMIN_TO_CHECK") (below 0.01 ETH -- may not afford admin txs)"
    else
        record_pass "Admin wallet balance" "${BALANCE_ETH} ETH for $(short_addr "$ADMIN_TO_CHECK")"
    fi
else
    record_warn "Admin wallet balance" "no admin address available to check"
fi

# =============================================================================
# SUMMARY
# =============================================================================

TOTAL=$((PASS_COUNT + FAIL_COUNT + WARN_COUNT))

echo ""
header "============================================================"
header "  AUDIT SUMMARY"
header "============================================================"
echo ""

# Print results table
printf "  ${BOLD}%-45s  %-6s  %s${RESET}\n" "CHECK" "RESULT" "DETAILS"
printf "  %-45s  %-6s  %s\n" "---------------------------------------------" "------" "--------------------------------------------"

for i in "${!CHECK_NAMES[@]}"; do
    RESULT="${CHECK_RESULTS[$i]}"
    case "$RESULT" in
        PASS) COLOR="$GREEN" ;;
        FAIL) COLOR="$RED" ;;
        WARN) COLOR="$YELLOW" ;;
        *)    COLOR="$RESET" ;;
    esac
    printf "  %-45s  ${COLOR}%-6s${RESET}  %s\n" "${CHECK_NAMES[$i]}" "$RESULT" "${CHECK_DETAILS[$i]}"
done

echo ""
printf "  %-45s\n" "---------------------------------------------"
printf "  ${GREEN}Passed:${RESET}   %d\n" "$PASS_COUNT"
printf "  ${RED}Failed:${RESET}   %d\n" "$FAIL_COUNT"
printf "  ${YELLOW}Warnings:${RESET} %d\n" "$WARN_COUNT"
printf "  Total:    %d\n" "$TOTAL"
echo ""

# ---------------------------------------------------------------------------
# Risk score
#   Each FAIL adds 10 points, each WARN adds 2 points.
#   0     = excellent
#   1-10  = low risk
#   11-30 = medium risk
#   31+   = high risk
# ---------------------------------------------------------------------------
RISK_SCORE=$(( FAIL_COUNT * 10 + WARN_COUNT * 2 ))

if [[ "$RISK_SCORE" -eq 0 ]]; then
    RISK_LABEL="EXCELLENT"
    RISK_COLOR="$GREEN"
elif [[ "$RISK_SCORE" -le 10 ]]; then
    RISK_LABEL="LOW"
    RISK_COLOR="$GREEN"
elif [[ "$RISK_SCORE" -le 30 ]]; then
    RISK_LABEL="MEDIUM"
    RISK_COLOR="$YELLOW"
else
    RISK_LABEL="HIGH"
    RISK_COLOR="$RED"
fi

printf "  ${BOLD}Risk Score:${RESET} ${RISK_COLOR}%d (%s)${RESET}\n" "$RISK_SCORE" "$RISK_LABEL"
echo ""

if [[ "$RISK_LABEL" == "HIGH" ]]; then
    printf "  %b%bACTION REQUIRED:%b Multiple critical issues found.\n" "$RED" "$BOLD" "$RESET"
    printf "  Review failed checks above and remediate before going to production.\n"
elif [[ "$RISK_LABEL" == "MEDIUM" ]]; then
    printf "  %b%bATTENTION:%b Some issues found. Review and address before production.\n" "$YELLOW" "$BOLD" "$RESET"
fi

echo ""

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
else
    exit 0
fi
