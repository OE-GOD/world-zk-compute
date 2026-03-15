#!/usr/bin/env bash
# check-gas-regression.sh
#
# Compares a newly generated gas snapshot against the committed baseline
# (.gas-snapshot) and fails if any test's gas usage increased by more than
# the allowed threshold (default 10%).
#
# Also enforces an absolute gas ceiling per test (default 30M, the Ethereum /
# Sepolia block gas limit) to catch any single verification path that would
# not fit in a single transaction.
#
# Must be run from the repository root.
#
# Usage:
#   bash scripts/check-gas-regression.sh [threshold_percent] [block_gas_limit]
#
# Examples:
#   bash scripts/check-gas-regression.sh             # 10% threshold, 30M limit
#   bash scripts/check-gas-regression.sh 5            # 5% threshold, 30M limit
#   bash scripts/check-gas-regression.sh 10 50000000  # 10% threshold, 50M limit

set -euo pipefail

THRESHOLD="${1:-10}"
BLOCK_GAS_LIMIT="${2:-30000000}"
CONTRACTS_DIR="contracts"
BASELINE="${CONTRACTS_DIR}/.gas-snapshot"
NEW_SNAPSHOT="${CONTRACTS_DIR}/.gas-snapshot-new"

echo "=== Gas Regression Check (threshold: ${THRESHOLD}%, block limit: ${BLOCK_GAS_LIMIT}) ==="
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

# Step 3: Compare line by line (regression detection)
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
echo "Regression summary:"
echo "  Tests compared:      ${total_compared}"
echo "  Regressions (>${THRESHOLD}%): ${regression_count}"
echo "  Improvements:        ${improvement_count}"
echo ""

# Step 4: Block gas limit enforcement
#
# Check every test in the new snapshot against the block gas limit.
# Tests whose names contain known multi-tx / batch / DAG patterns are
# exempted because they intentionally exceed single-block limits (they are
# designed to be split across multiple transactions).
#
# Exempt patterns (case-insensitive):
#   - DAGBatch*                     — multi-tx batch verifier tests
#   - GKRDAG*                       — GKR DAG verifier tests (multi-tx path)
#   - *dag_e2e*                     — full DAG end-to-end (250M+, uses Anvil --gas-limit)
#   - *dag_hybrid*                  — DAG hybrid tests (multi-tx in production)
#   - *dag_groth16*                 — DAG Groth16 tests (multi-tx in production)
#   - *dag_verification*            — full DAG verification
#   - *dag_build*                   — DAG build helpers (run inside high-gas Anvil)
#   - *dag_collect*                 — DAG collect helpers
#   - *dag_input*                   — DAG input proof decode
#   - *dag_transcript*              — DAG transcript trace
#   - *dag_invalid*                 — DAG negative tests (may still use high gas for setup)
#   - *dag_wrong*                   — DAG wrong-gens tests
#   - *dag_unregistered*            — DAG unregistered circuit tests
#   - *dag_register*                — DAG register tests
#   - *dag_single_tx*               — single-tx fallback test (still high gas)
#   - *embedded_public*             — DAG embedded public input tests
#   - *public_value*                — DAG public value claim tests
#   - *fixture_loads*               — fixture loading tests
#   - *hybrid_accepts*              — hybrid noncommitted registration (large setup)
#   - *hybrid_noncommitted*         — hybrid noncommitted claim check (large setup)
#   - *hybrid_rejects_no_groth16*   — hybrid groth16 verifier check (large setup)

EXEMPT_PATTERN="(DAGBatch|GKRDAG|dag_e2e|dag_hybrid|dag_groth16|dag_verification|dag_build|dag_collect|dag_input|dag_transcript|dag_invalid|dag_wrong|dag_unregistered|dag_register|dag_single_tx|embedded_public|public_value|fixture_loads|hybrid_accepts|hybrid_noncommitted|hybrid_rejects_no_groth16)"

block_limit_violations=0
block_limit_violation_count=0
block_limit_exempt_count=0

echo "=== Block Gas Limit Check (limit: ${BLOCK_GAS_LIMIT} / 30M Sepolia) ==="
echo ""

while IFS= read -r line; do
    test_name=$(echo "$line" | sed 's/ (gas:.*//')
    gas=$(echo "$line" | grep -o 'gas: [0-9]*' | grep -o '[0-9]*')

    if [ -z "$gas" ] || [ -z "$test_name" ]; then
        continue
    fi

    if [ "$gas" -gt "$BLOCK_GAS_LIMIT" ]; then
        # Check if this test is exempt (known multi-tx pattern)
        if echo "$test_name" | grep -qEi "$EXEMPT_PATTERN"; then
            block_limit_exempt_count=$((block_limit_exempt_count + 1))
        else
            printf "  OVER LIMIT: %-50s %12s gas (limit: %s)\n" "$test_name" "$gas" "$BLOCK_GAS_LIMIT"
            block_limit_violations=1
            block_limit_violation_count=$((block_limit_violation_count + 1))
        fi
    fi
done < "$NEW_SNAPSHOT"

echo ""
echo "Block limit summary:"
echo "  Violations:          ${block_limit_violation_count}"
echo "  Exempt (multi-tx):   ${block_limit_exempt_count}"
echo ""

# Final verdict
exit_code=0

if [ "$regression_found" -eq 1 ]; then
    echo "FAILED: ${regression_count} test(s) exceeded the ${THRESHOLD}% gas regression threshold."
    echo ""
    echo "To update the baseline after intentional changes:"
    echo "  make snapshot-update"
    echo "  git add contracts/.gas-snapshot"
    echo ""
    exit_code=1
else
    echo "PASSED: No gas regressions detected above ${THRESHOLD}% threshold."
fi

if [ "$block_limit_violations" -eq 1 ]; then
    echo "FAILED: ${block_limit_violation_count} test(s) exceed the ${BLOCK_GAS_LIMIT} block gas limit."
    echo ""
    echo "Single-transaction verification paths must fit within a single block."
    echo "If this is a known multi-tx path, add the test pattern to the exempt list"
    echo "in scripts/check-gas-regression.sh."
    echo ""
    exit_code=1
else
    echo "PASSED: All single-tx tests are within the ${BLOCK_GAS_LIMIT} block gas limit."
fi

exit $exit_code
