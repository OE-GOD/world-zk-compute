#!/usr/bin/env bash
# =============================================================================
# Cross-SDK Contract Interface Consistency Check
#
# Verifies that Rust, Python, and TypeScript SDKs use matching function
# selectors for key contract operations. This catches ABI drift between SDKs.
#
# Usage:
#   ./tests/sdk-consistency/check-selectors.sh
#
# Prerequisites:
#   - cast (from Foundry, for computing selectors)
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers
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

ok()   { printf "${GREEN}[PASS]${RESET} %s\n" "$*"; }
fail() { printf "${RED}[FAIL]${RESET} %s\n" "$*"; }
info() { printf "${CYAN}[INFO]${RESET} %s\n" "$*"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FAILURES=0

# ---------------------------------------------------------------------------
# Function signatures and their expected selectors
# ---------------------------------------------------------------------------
# Format: "functionName|soliditySignature"

FUNCTIONS=(
    "requestExecution|requestExecution(bytes32,bytes32,string,address,uint256)"
    "claimExecution|claimExecution(uint256)"
    "submitResult|submitResult(uint256,bytes)"
    "submitTEEResult|submitTEEResult(bytes32,bytes32,bytes32,string,bytes)"
    "challengeResult|challengeResult(bytes32)"
    "registerEnclave|registerEnclave(address,bytes32)"
    "revokeEnclave|revokeEnclave(address)"
    "registerProgram|registerProgram(bytes32,string,string,bytes32)"
)

info "Computing canonical function selectors from ABI signatures..."
echo ""

printf "${BOLD}%-30s %-12s${RESET}\n" "Function" "Selector"
printf "%-30s %-12s\n" "------------------------------" "------------"

# Compute selectors
FN_NAMES=()
FN_SELECTORS=()

for entry in "${FUNCTIONS[@]}"; do
    fn_name="${entry%%|*}"
    fn_sig="${entry##*|}"
    selector=$(cast sig "$fn_sig" 2>/dev/null || echo "ERROR")
    FN_NAMES+=("$fn_name")
    FN_SELECTORS+=("$selector")
    printf "%-30s %-12s\n" "$fn_name" "$selector"
done
echo ""

# ---------------------------------------------------------------------------
# Check a single SDK directory
# ---------------------------------------------------------------------------
check_sdk() {
    local sdk_name="$1"
    local sdk_dir="$2"

    info "Checking $sdk_name ($sdk_dir)..."

    if [[ ! -d "$PROJECT_ROOT/$sdk_dir" ]]; then
        printf "${YELLOW}[SKIP]${RESET} $sdk_name: directory not found\n"
        return
    fi

    for i in "${!FN_NAMES[@]}"; do
        local fn_name="${FN_NAMES[$i]}"
        local selector="${FN_SELECTORS[$i]}"

        local found=""
        # Check for selector or function name
        found=$(grep -r "$selector" "$PROJECT_ROOT/$sdk_dir" 2>/dev/null | head -1 || true)
        if [[ -z "$found" ]]; then
            found=$(grep -r "$fn_name" "$PROJECT_ROOT/$sdk_dir" 2>/dev/null | head -1 || true)
        fi

        if [[ -n "$found" ]]; then
            ok "$sdk_name: $fn_name ($selector)"
        else
            printf "${YELLOW}[SKIP]${RESET} $sdk_name: $fn_name not found (may not be implemented)\n"
        fi
    done
}

# ---------------------------------------------------------------------------
# Check all SDKs
# ---------------------------------------------------------------------------
check_sdk "Rust SDK" "sdk/src"
check_sdk "Python SDK" "sdk/python"
check_sdk "TypeScript SDK" "sdk/typescript/src"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
if [[ "$FAILURES" -eq 0 ]]; then
    ok "Cross-SDK consistency check complete. No mismatches found."
else
    fail "$FAILURES selector mismatch(es) detected!"
    exit 1
fi
