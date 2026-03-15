#!/usr/bin/env bash
# =============================================================================
# Contract Bytecode Size Report for World ZK Compute
#
# Builds contracts with Forge, extracts deployedBytecode from each compiled
# JSON artifact, and prints a detailed table sorted by size descending.
# Highlights contracts approaching or exceeding the EIP-170 limit.
#
# Usage:
#   ./scripts/contract-size-report.sh
#   ./scripts/contract-size-report.sh --limit 49152   # custom limit (e.g., L2)
#   ./scripts/contract-size-report.sh --no-build       # skip forge build
#   ./scripts/contract-size-report.sh --help
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
ok()      { printf "${GREEN}[OK]${RESET}    %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$PROJECT_ROOT/contracts"
OUT_DIR="$CONTRACTS_DIR/out"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
EIP170_LIMIT=24576   # 24 KB — EIP-170 contract size limit
SKIP_BUILD=false
WARN_THRESHOLD=80    # percent of limit to trigger WARN

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
contract-size-report.sh -- Detailed contract bytecode size report.

USAGE:
  ./scripts/contract-size-report.sh [OPTIONS]

OPTIONS:
  --limit <bytes>   Override the EIP-170 limit (default: 24576)
  --no-build        Skip `forge build`, use existing artifacts
  --help, -h        Show this help message

DESCRIPTION:
  Compiles contracts with `forge build`, then walks every JSON artifact
  in contracts/out/ to extract the deployedBytecode hex string. Computes
  byte sizes, sorts by size descending, and prints a table with:

    Contract | Size (bytes) | % of Limit | Status

  Status values:
    OK    -- Under 80% of the limit (green)
    WARN  -- Between 80-100% of the limit (yellow)
    OVER  -- Exceeds the limit (red)

PREREQUISITES:
  - forge (Foundry)
  - jq
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --limit)
            if [[ -z "${2:-}" ]] || ! [[ "$2" =~ ^[0-9]+$ ]]; then
                err "--limit requires a numeric argument"
                exit 1
            fi
            EIP170_LIMIT="$2"
            shift 2
            ;;
        --no-build)
            SKIP_BUILD=true
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
# Prerequisites
# ---------------------------------------------------------------------------
if ! command -v forge &>/dev/null; then
    err "forge is required. Install: curl -L https://foundry.paradigm.xyz | bash"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    err "jq is required. Install: brew install jq (macOS) or apt-get install jq (Linux)"
    exit 1
fi

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
if [[ "$SKIP_BUILD" == false ]]; then
    info "Building contracts..."
    (cd "$CONTRACTS_DIR" && forge build --silent)
    ok "Build complete."
fi

if [[ ! -d "$OUT_DIR" ]]; then
    err "Contracts output directory not found: $OUT_DIR"
    err "Run 'forge build' in $CONTRACTS_DIR first."
    exit 1
fi

# ---------------------------------------------------------------------------
# Collect sizes from compiled artifacts
# ---------------------------------------------------------------------------
info "Scanning artifacts in $OUT_DIR ..."

# Temporary file for sorting
TMPFILE=$(mktemp)
trap 'rm -f "$TMPFILE"' EXIT

CONTRACT_COUNT=0

for sol_dir in "$OUT_DIR"/*/; do
    sol_name="$(basename "$sol_dir")"
    for json_file in "$sol_dir"*.json; do
        [[ -f "$json_file" ]] || continue

        contract_name="$(basename "$json_file" .json)"

        # Extract the deployedBytecode.object hex string
        deployed_hex=$(jq -r '.deployedBytecode.object // empty' "$json_file" 2>/dev/null)

        # Skip contracts with no deployed bytecode (interfaces, abstract contracts, libraries)
        if [[ -z "$deployed_hex" ]] || [[ "$deployed_hex" == "0x" ]] || [[ "$deployed_hex" == "null" ]]; then
            continue
        fi

        # Strip the 0x prefix and compute byte count: each pair of hex chars = 1 byte
        hex_chars="${deployed_hex#0x}"
        byte_size=$(( ${#hex_chars} / 2 ))

        # Skip zero-size artifacts
        if [[ "$byte_size" -eq 0 ]]; then
            continue
        fi

        # Write to temp file: size<TAB>contract_name<TAB>sol_name
        printf "%d\t%s\t%s\n" "$byte_size" "$contract_name" "$sol_name" >> "$TMPFILE"
        CONTRACT_COUNT=$((CONTRACT_COUNT + 1))
    done
done

if [[ "$CONTRACT_COUNT" -eq 0 ]]; then
    warn "No contracts with deployed bytecode found."
    exit 0
fi

# Sort by size descending
SORTED=$(sort -t$'\t' -k1 -n -r "$TMPFILE")

# ---------------------------------------------------------------------------
# Print table
# ---------------------------------------------------------------------------
header "Contract Bytecode Size Report"
info "EIP-170 limit: $EIP170_LIMIT bytes ($(( EIP170_LIMIT / 1024 )) KB)"
info "Warning threshold: ${WARN_THRESHOLD}% of limit"
echo ""

printf "  ${BOLD}%-45s  %12s  %8s  %s${RESET}\n" "CONTRACT" "SIZE (bytes)" "% LIMIT" "STATUS"
printf "  %-45s  %12s  %8s  %s\n" \
    "---------------------------------------------" "------------" "--------" "------"

DEPLOYABLE=0
WARN_COUNT=0
OVER_COUNT=0

while IFS=$'\t' read -r byte_size contract_name sol_name; do
    # Compute percentage of limit
    if [[ "$EIP170_LIMIT" -gt 0 ]]; then
        PCT=$(( byte_size * 100 / EIP170_LIMIT ))
    else
        PCT=0
    fi

    # Determine status and color
    if [[ "$byte_size" -gt "$EIP170_LIMIT" ]]; then
        STATUS="OVER"
        COLOR="$RED"
        OVER_COUNT=$((OVER_COUNT + 1))
    elif [[ "$PCT" -ge "$WARN_THRESHOLD" ]]; then
        STATUS="WARN"
        COLOR="$YELLOW"
        WARN_COUNT=$((WARN_COUNT + 1))
        DEPLOYABLE=$((DEPLOYABLE + 1))
    else
        STATUS="OK"
        COLOR="$GREEN"
        DEPLOYABLE=$((DEPLOYABLE + 1))
    fi

    # Format with comma-separated thousands for readability
    SIZE_FMT=$(printf "%'d" "$byte_size" 2>/dev/null || printf "%d" "$byte_size")

    printf "  ${COLOR}%-45s${RESET}  %12s  %7d%%  ${COLOR}%s${RESET}\n" \
        "$contract_name" "$SIZE_FMT" "$PCT" "$STATUS"
done <<< "$SORTED"

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "Summary"
info "Total contracts scanned:   $CONTRACT_COUNT"
ok   "Deployable (under limit):  $DEPLOYABLE"

if [[ "$WARN_COUNT" -gt 0 ]]; then
    warn "Warnings (>=${WARN_THRESHOLD}% of limit): $WARN_COUNT"
fi

if [[ "$OVER_COUNT" -gt 0 ]]; then
    err  "Over limit:                $OVER_COUNT"
    echo ""
    warn "Contracts exceeding EIP-170 may still be deployable on L2 chains"
    warn "with higher limits (e.g., Arbitrum) or via proxy patterns."
    exit 1
fi

echo ""
ok "All contracts are within the ${EIP170_LIMIT}-byte limit."
