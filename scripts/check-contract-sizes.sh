#!/usr/bin/env bash
# Check contract bytecode sizes against EIP-170 limit (24,576 bytes).
# Usage: ./scripts/check-contract-sizes.sh
# Exit code 1 if any production contract exceeds the limit.

set -uo pipefail

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
    "PausableExecutionEngine"
)

cd "$CONTRACTS_DIR" || exit 1

echo "Building contracts..."
SIZES_OUTPUT=$(forge build --sizes 2>&1)

echo ""
echo "┌──────────────────────────────────┬─────────────┬─────────┬──────────┐"
echo "│ Contract                         │ Size (bytes)│ % Limit │ Status   │"
echo "├──────────────────────────────────┼─────────────┼─────────┼──────────┤"

FAILED=0

for contract in "${PRODUCTION_CONTRACTS[@]}"; do
    # Extract runtime size — field 3 after splitting on |, strip commas/spaces
    SIZE=$(echo "$SIZES_OUTPUT" | grep "| ${contract} " | head -1 | awk -F'|' '{gsub(/[, ]/,"",$3); print $3}')

    if [ -z "$SIZE" ]; then
        printf "│ %-32s │ %11s │ %7s │ %-8s │\n" "$contract" "N/A" "N/A" "SKIP"
        continue
    fi

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

if [ "$FAILED" -eq 1 ]; then
    echo ""
    echo "WARNING: One or more contracts exceed the EIP-170 size limit!"
    echo "Note: RemainderVerifier is expected to exceed (deployed via Arbitrum Stylus)."
fi

echo ""
echo "Tip: Run 'forge snapshot --check' to detect gas regressions."

exit "$FAILED"
