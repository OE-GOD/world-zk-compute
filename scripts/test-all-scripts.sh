#!/usr/bin/env bash
# Validate all shell scripts in scripts/ directory.
# Usage: ./scripts/test-all-scripts.sh
#
# Runs bash -n (syntax check), shellcheck (if installed), and --help (if supported).

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -5 "$0" | tail -4
    exit 0
fi

PASS=0
FAIL=0
SKIP=0
RESULTS=()

record_pass() { PASS=$((PASS + 1)); RESULTS+=("PASS  $1"); }
record_fail() { FAIL=$((FAIL + 1)); RESULTS+=("FAIL  $1: $2"); }
record_skip() { SKIP=$((SKIP + 1)); RESULTS+=("SKIP  $1"); }

HAS_SHELLCHECK=false
if command -v shellcheck &>/dev/null; then
    HAS_SHELLCHECK=true
fi

echo "========================================"
echo "  Shell Script Validation"
echo "========================================"
echo ""

for script in scripts/*.sh; do
    [ -f "$script" ] || continue
    name=$(basename "$script")

    # 1. Syntax check
    if bash -n "$script" 2>/dev/null; then
        record_pass "$name (syntax)"
    else
        record_fail "$name (syntax)" "bash -n failed"
        continue
    fi

    # 2. Shellcheck
    if [ "$HAS_SHELLCHECK" = true ]; then
        if shellcheck "$script" 2>/dev/null; then
            record_pass "$name (shellcheck)"
        else
            record_fail "$name (shellcheck)" "shellcheck warnings"
        fi
    else
        record_skip "$name (shellcheck)"
    fi

    # 3. --help flag (only if script supports it)
    if grep -q '\-\-help' "$script" 2>/dev/null; then
        if chmod +x "$script" && "$script" --help >/dev/null 2>&1; then
            record_pass "$name (--help)"
        else
            record_fail "$name (--help)" "exited non-zero"
        fi
    fi
done

echo ""
echo "========================================"
echo "  Results"
echo "========================================"
for r in "${RESULTS[@]}"; do
    echo "  $r"
done
echo ""
echo "Total: $((PASS + FAIL + SKIP))  Pass: $PASS  Fail: $FAIL  Skip: $SKIP"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
