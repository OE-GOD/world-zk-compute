#!/usr/bin/env bash
# =============================================================================
# Extract contract ABIs from forge build output.
#
# Runs forge build, extracts ABIs for key contracts, and copies them to the
# TypeScript and Python SDK directories.
#
# Usage:
#   ./scripts/generate-abis.sh
#   ./scripts/generate-abis.sh --help
#
# Requires: forge, jq (or python3)
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
CONTRACTS_OUT="$CONTRACTS_DIR/out"
TS_ABI_DIR="$PROJECT_ROOT/sdk/typescript/src/abi"
PY_ABI_DIR="$PROJECT_ROOT/sdk/python/worldzk/abi"

# ---------------------------------------------------------------------------
# Contracts to extract
# ---------------------------------------------------------------------------
CONTRACTS=(
    "ExecutionEngine"
    "ProgramRegistry"
    "TEEMLVerifier"
    "ITEEMLVerifier"
)

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------
show_help() {
    cat <<'EOF'
generate-abis.sh -- Extract contract ABIs from forge build output.

USAGE:
  scripts/generate-abis.sh [OPTIONS]

OPTIONS:
  -h, --help    Show this help message

DESCRIPTION:
  Runs forge build in the contracts/ directory, then extracts the ABI for
  each key contract from the forge output artifacts in contracts/out/.
  Copies the resulting JSON files to:
    - sdk/typescript/src/abi/
    - sdk/python/worldzk/abi/

CONTRACTS:
  ExecutionEngine, ProgramRegistry, TEEMLVerifier, ITEEMLVerifier

REQUIRES:
  forge   (from Foundry)
  jq      (preferred) or python3 (fallback for ABI extraction)
EOF
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            echo "Run with --help for usage."
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

HAS_JQ=false
if command -v jq &>/dev/null; then
    HAS_JQ=true
elif ! command -v python3 &>/dev/null; then
    err "Either jq or python3 is required to extract ABIs."
    exit 1
fi

# ---------------------------------------------------------------------------
# Build contracts
# ---------------------------------------------------------------------------
header "Building contracts..."
cd "$CONTRACTS_DIR" || { err "Cannot cd to $CONTRACTS_DIR"; exit 1; }

if ! forge build 2>&1; then
    err "forge build failed. Fix compilation errors first."
    exit 1
fi
ok "Contracts compiled successfully"

# ---------------------------------------------------------------------------
# Create output directories
# ---------------------------------------------------------------------------
mkdir -p "$TS_ABI_DIR"
mkdir -p "$PY_ABI_DIR"

# ---------------------------------------------------------------------------
# Extract ABIs
# ---------------------------------------------------------------------------
header "Extracting ABIs..."

EXTRACTED=0
FAILED=0

extract_abi() {
    local name="$1"
    local artifact_file=""

    # Find the artifact JSON file under contracts/out/<name>.sol/<name>.json
    artifact_file="$CONTRACTS_OUT/${name}.sol/${name}.json"

    if [[ ! -f "$artifact_file" ]]; then
        # Try interface naming: I<Name>.sol/<Name>.json
        artifact_file="$CONTRACTS_OUT/I${name}.sol/I${name}.json"
    fi

    if [[ ! -f "$artifact_file" ]]; then
        # Broad search under out/
        artifact_file=$(find "$CONTRACTS_OUT" -name "${name}.json" -path "*/out/*" 2>/dev/null | head -1)
    fi

    if [[ -z "$artifact_file" || ! -f "$artifact_file" ]]; then
        warn "Artifact not found for $name -- skipping"
        FAILED=$((FAILED + 1))
        return 1
    fi

    local abi_json=""

    if [[ "$HAS_JQ" == "true" ]]; then
        abi_json=$(jq '.abi' "$artifact_file" 2>/dev/null)
    else
        abi_json=$(python3 -c "
import json, sys
with open('$artifact_file') as f:
    data = json.load(f)
print(json.dumps(data.get('abi', []), indent=2))
" 2>/dev/null)
    fi

    if [[ -z "$abi_json" || "$abi_json" == "null" || "$abi_json" == "[]" ]]; then
        warn "Empty ABI for $name -- skipping"
        FAILED=$((FAILED + 1))
        return 1
    fi

    # Write to TypeScript SDK directory
    echo "$abi_json" > "$TS_ABI_DIR/${name}.json"

    # Write to Python SDK directory
    echo "$abi_json" > "$PY_ABI_DIR/${name}.json"

    local entry_count=""
    if [[ "$HAS_JQ" == "true" ]]; then
        entry_count=$(echo "$abi_json" | jq 'length' 2>/dev/null)
    else
        entry_count=$(python3 -c "import json; print(len(json.loads('''$abi_json''')))" 2>/dev/null || echo "?")
    fi

    ok "$name  ($entry_count entries)"
    EXTRACTED=$((EXTRACTED + 1))
    return 0
}

for contract in "${CONTRACTS[@]}"; do
    extract_abi "$contract" || true
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  ABI Extraction Summary"
header "============================================================"
echo ""
info "Extracted: $EXTRACTED"
if [[ "$FAILED" -gt 0 ]]; then
    warn "Skipped:   $FAILED"
fi
echo ""
info "TypeScript: $TS_ABI_DIR/"
info "Python:     $PY_ABI_DIR/"
echo ""

printf "  ${BOLD}%-25s %-40s %-40s${RESET}\n" "CONTRACT" "TYPESCRIPT" "PYTHON"
printf "  %-25s %-40s %-40s\n" "-------------------------" "----------------------------------------" "----------------------------------------"

for contract in "${CONTRACTS[@]}"; do
    ts_file="$TS_ABI_DIR/${contract}.json"
    py_file="$PY_ABI_DIR/${contract}.json"

    ts_status="--"
    py_status="--"

    if [[ -f "$ts_file" ]]; then
        ts_size=$(wc -c < "$ts_file" | tr -d ' ')
        ts_status="${ts_size} bytes"
    fi

    if [[ -f "$py_file" ]]; then
        py_size=$(wc -c < "$py_file" | tr -d ' ')
        py_status="${py_size} bytes"
    fi

    printf "  %-25s %-40s %-40s\n" "$contract" "$ts_status" "$py_status"
done

echo ""

if [[ "$FAILED" -gt 0 && "$EXTRACTED" -eq 0 ]]; then
    err "No ABIs were extracted."
    exit 1
fi

ok "Done."
