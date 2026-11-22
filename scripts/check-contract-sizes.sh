#!/usr/bin/env bash
# Check contract bytecode sizes against EIP-170 limit (24,576 bytes).
# Usage: ./scripts/check-contract-sizes.sh
# Exit code 1 if any production contract exceeds the limit.

set -euo pipefail

LIMIT=24576
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONTRACTS_DIR="$(dirname "$SCRIPT_DIR")/contracts"

# Only check production contracts (exclude test/mock contracts)
PRODUCTION_CONTRACTS=(
    "TEEMLVerifier"
    "ExecutionEngine"
    "ProgramRegistry"
    "RemainderVerifier"
    "RemainderVerifierAdapter"
    "RiscZeroVerifierAdapter"
    "RiscZeroVerifierRouter"
    "UpgradeableExecutionEngine"
)

cd "$CONTRACTS_DIR" || exit 1

echo "Building contracts..."
# forge exits non-zero when any contract is over the limit, so its status is
# not used: the table below decides. A real build failure shows up as "no
# sizes found" further down.
SIZES_OUTPUT=$(forge build --sizes 2>&1) || true

echo ""
echo "┌──────────────────────────────────┬─────────────┬─────────┬──────────┐"
echo "│ Contract                         │ Size (bytes)│ % Limit │ Status   │"
echo "├──────────────────────────────────┼─────────────┼─────────┼──────────┤"

FAILED=0
FOUND=0

for contract in "${PRODUCTION_CONTRACTS[@]}"; do
    # Extract runtime size — field 3 after splitting on |, strip commas/spaces
    SIZE=$(echo "$SIZES_OUTPUT" | grep "| ${contract} " | head -1 | awk -F'|' '{gsub(/[, ]/,"",$3); print $3}')

    if [ -z "$SIZE" ]; then
        printf "│ %-32s │ %11s │ %7s │ %-8s │\n" "$contract" "N/A" "N/A" "SKIP"
        continue
    fi

    FOUND=$((FOUND + 1))
    PCT=$(( SIZE * 100 / LIMIT ))

    if [ "$SIZE" -gt "$LIMIT" ]; then
        STATUS="FAIL"
        FAILED=1
    elif [ "$PCT" -gt 90 ]; then
        STATUS="WARN"
    else
        STATUS="OK"
    fi

    printf "│ %-32s │ %11s │ %5d%% │ %-8s │\n" "$contract" "$SIZE" "$PCT" "$STATUS"
done

echo "└──────────────────────────────────┴─────────────┴─────────┴──────────┘"
echo ""
echo "EIP-170 limit: ${LIMIT} bytes (24 KB)"

if [ "$FOUND" -eq 0 ]; then
    echo ""
    echo "ERROR: no contract sizes found. forge build output:"
    echo "$SIZES_OUTPUT"
    exit 1
fi

if [ "$FAILED" -eq 1 ]; then
    echo ""
    echo "WARNING: One or more contracts exceed the EIP-170 size limit!"
    echo "A contract over the limit cannot be deployed on a chain that enforces it."
fi

echo ""
echo "Tip: Run 'forge snapshot --check' to detect gas regressions."

exit "$FAILED"
