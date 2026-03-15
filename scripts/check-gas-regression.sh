#!/usr/bin/env bash
# check-gas-regression.sh
#
# Compares a newly generated gas snapshot against the committed baseline
# (.gas-snapshot) and fails if any test's gas usage increased by more than
# the allowed threshold (default 10%).
#
# Must be run from the repository root.
#
# Usage:
#   bash scripts/check-gas-regression.sh [threshold_percent]
#
# Examples:
#   bash scripts/check-gas-regression.sh        # 10% threshold
#   bash scripts/check-gas-regression.sh 5       # 5% threshold

set -euo pipefail

THRESHOLD="${1:-10}"
CONTRACTS_DIR="contracts"
BASELINE="${CONTRACTS_DIR}/.gas-snapshot"
NEW_SNAPSHOT="${CONTRACTS_DIR}/.gas-snapshot-new"

echo "=== Gas Regression Check (threshold: ${THRESHOLD}%) ==="
echo ""

# Step 1: Verify baseline exists
if [ ! -f "$BASELINE" ]; then
    echo "WARNING: No baseline .gas-snapshot found at ${BASELINE}"
    echo "Generating initial baseline..."
    (cd "$CONTRACTS_DIR" && forge snapshot 2>/dev/null) || true
    echo "Baseline created. Commit contracts/.gas-snapshot to establish the baseline."
    exit 0
fi

# Step 2: Generate a new snapshot
echo "Generating new gas snapshot..."
(cd "$CONTRACTS_DIR" && forge snapshot --snap .gas-snapshot-new 2>/dev/null) || true

if [ ! -f "$NEW_SNAPSHOT" ]; then
    echo "ERROR: Failed to generate new gas snapshot."
    exit 1
fi

# Step 3: Compare line by line
regression_found=0
regression_count=0
improvement_count=0
total_compared=0

echo ""
echo "Comparing gas usage..."
echo "-------------------------------------------------------------------"
printf "%-55s %12s %12s %8s\n" "Test" "Baseline" "Current" "Change"
echo "-------------------------------------------------------------------"

while IFS= read -r line; do
    # Each line format: TestContract:test_name() (gas: 12345)
    test_name=$(echo "$line" | sed 's/ (gas:.*//')
    new_gas=$(echo "$line" | grep -o 'gas: [0-9]*' | grep -o '[0-9]*')

    if [ -z "$new_gas" ] || [ -z "$test_name" ]; then
        continue
    fi

    # Find matching line in baseline
    baseline_line=$(grep -F "$test_name" "$BASELINE" 2>/dev/null || true)
    if [ -z "$baseline_line" ]; then
        # New test, no baseline to compare
        continue
    fi

    baseline_gas=$(echo "$baseline_line" | grep -o 'gas: [0-9]*' | grep -o '[0-9]*')
    if [ -z "$baseline_gas" ] || [ "$baseline_gas" -eq 0 ]; then
        continue
    fi

    total_compared=$((total_compared + 1))

    # Calculate percentage change using awk for floating point
    diff=$((new_gas - baseline_gas))
    pct_change=$(awk "BEGIN { printf \"%.1f\", ($diff / $baseline_gas) * 100 }")

    # Check if the increase exceeds threshold
    exceeds=$(awk "BEGIN { print ($pct_change > $THRESHOLD) ? 1 : 0 }")

    if [ "$exceeds" -eq 1 ]; then
        printf "%-55s %12s %12s %7s%% REGRESSION\n" "$test_name" "$baseline_gas" "$new_gas" "+${pct_change}"
        regression_found=1
        regression_count=$((regression_count + 1))
    elif [ "$diff" -gt 0 ]; then
        printf "%-55s %12s %12s %7s%%\n" "$test_name" "$baseline_gas" "$new_gas" "+${pct_change}"
    elif [ "$diff" -lt 0 ]; then
        printf "%-55s %12s %12s %7s%%\n" "$test_name" "$baseline_gas" "$new_gas" "${pct_change}"
        improvement_count=$((improvement_count + 1))
    fi
done < "$NEW_SNAPSHOT"

echo "-------------------------------------------------------------------"
echo ""
echo "Summary:"
echo "  Tests compared:      ${total_compared}"
echo "  Regressions (>${THRESHOLD}%): ${regression_count}"
echo "  Improvements:        ${improvement_count}"
echo ""

# Keep the new snapshot for CI artifact upload (CI cleans up via .gitignore)
# The file stays at contracts/.gas-snapshot-new

if [ "$regression_found" -eq 1 ]; then
    echo "FAILED: ${regression_count} test(s) exceeded the ${THRESHOLD}% gas regression threshold."
    echo ""
    echo "To update the baseline after intentional changes:"
    echo "  cd contracts && forge snapshot"
    echo "  git add contracts/.gas-snapshot"
    echo ""
    exit 1
else
    echo "PASSED: No gas regressions detected above ${THRESHOLD}% threshold."
    exit 0
fi
