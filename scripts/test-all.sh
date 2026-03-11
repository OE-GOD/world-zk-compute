#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# World ZK Compute — Unified Test Runner
#
# Runs every test suite in the project and prints a summary.
# Exit code 0 if all suites pass, 1 if any suite fails.
#
# Usage:
#   ./scripts/test-all.sh                     # run all suites
#   ./scripts/test-all.sh --fast              # skip slow suites (xgboost-remainder)
#   ./scripts/test-all.sh --suite solidity    # run only the solidity suite
#   ./scripts/test-all.sh --suite rust-sdk    # run only the rust sdk suite
#   ./scripts/test-all.sh --list              # list available suite names
#   ./scripts/test-all.sh --help              # show this help
# ═══════════════════════════════════════════════════════════════════════════════

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Available suite names (used for --suite filtering and --list) ─────────────
ALL_SUITES=(
    operator
    enclave
    rust-sdk
    python-sdk
    typescript-sdk
    solidity
    xgboost-remainder
    stylus
)

# ── CLI argument parsing ──────────────────────────────────────────────────────
FAST_MODE=false
FILTER_SUITE=""

usage() {
    cat <<'USAGE'
Usage: scripts/test-all.sh [OPTIONS]

Options:
  --help          Show this help message and exit
  --fast          Skip slow suites (xgboost-remainder)
  --suite NAME    Run only the named suite (see --list for names)
  --list          List available suite names and exit

Suite names:
  operator          services/operator (Rust)
  enclave           tee/enclave (Rust)
  rust-sdk          sdk (Rust)
  python-sdk        sdk/python (pytest)
  typescript-sdk    sdk/typescript (vitest)
  solidity          contracts (forge)
  xgboost-remainder examples/xgboost-remainder (Rust, slow)
  stylus            contracts/stylus/gkr-verifier (Rust, native target)

Examples:
  ./scripts/test-all.sh                      Run everything
  ./scripts/test-all.sh --fast               Skip xgboost-remainder
  ./scripts/test-all.sh --suite solidity     Run only Solidity tests
  ./scripts/test-all.sh --suite rust-sdk     Run only the Rust SDK tests
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        --fast)
            FAST_MODE=true
            shift
            ;;
        --suite)
            if [[ -z "${2:-}" ]]; then
                echo "Error: --suite requires a suite name argument."
                echo "Run with --list to see available suites."
                exit 2
            fi
            FILTER_SUITE="$2"
            shift 2
            ;;
        --list)
            echo "Available test suites:"
            for s in "${ALL_SUITES[@]}"; do
                echo "  $s"
            done
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            usage
            exit 2
            ;;
    esac
done

# Validate --suite name if provided
if [[ -n "$FILTER_SUITE" ]]; then
    valid=false
    for s in "${ALL_SUITES[@]}"; do
        if [[ "$s" == "$FILTER_SUITE" ]]; then
            valid=true
            break
        fi
    done
    if [[ "$valid" == "false" ]]; then
        echo "Error: unknown suite '$FILTER_SUITE'."
        echo "Run with --list to see available suites."
        exit 2
    fi
fi

# ── Counters ──────────────────────────────────────────────────────────────────
TOTAL=0
PASSED=0
FAILED=0
SKIPPED=0
RESULTS=()
START_TIME=$(date +%s)

# ── Helpers ───────────────────────────────────────────────────────────────────

should_run() {
    local suite_key="$1"
    # If no filter, run everything
    if [[ -z "$FILTER_SUITE" ]]; then
        return 0
    fi
    # Otherwise only run the matching suite
    [[ "$FILTER_SUITE" == "$suite_key" ]]
}

run_suite() {
    local name="$1"
    local dir="$2"
    shift 2
    local cmd=("$@")

    TOTAL=$((TOTAL + 1))

    # Check directory exists
    if [[ ! -d "$dir" ]]; then
        SKIPPED=$((SKIPPED + 1))
        RESULTS+=("SKIP  ${name}  (directory not found: $dir)")
        echo ""
        echo "  Skipped: $name (directory not found)"
        return 0
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Running: $name"
    echo "  Command: ${cmd[*]}"
    echo "  Dir:     $dir"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local suite_start
    suite_start=$(date +%s)

    # Use set +e so a test failure does not abort the whole script
    set +e
    (cd "$dir" && "${cmd[@]}" 2>&1)
    local exit_code=$?
    set -e

    local suite_end
    suite_end=$(date +%s)
    local duration=$((suite_end - suite_start))

    if [[ $exit_code -eq 0 ]]; then
        PASSED=$((PASSED + 1))
        RESULTS+=("PASS  ${name}  (${duration}s)")
        echo "  -> PASSED (${duration}s)"
    else
        FAILED=$((FAILED + 1))
        RESULTS+=("FAIL  ${name}  (${duration}s, exit code $exit_code)")
        echo "  -> FAILED (${duration}s, exit code $exit_code)"
    fi
}

skip_suite() {
    local name="$1"
    local reason="${2:-}"
    TOTAL=$((TOTAL + 1))
    SKIPPED=$((SKIPPED + 1))
    if [[ -n "$reason" ]]; then
        RESULTS+=("SKIP  ${name}  ($reason)")
    else
        RESULTS+=("SKIP  ${name}")
    fi
    echo ""
    echo "  Skipped: $name${reason:+ ($reason)}"
}

# ── Detect native Rust target for Stylus tests ───────────────────────────────
NATIVE_TARGET="$(rustc -vV 2>/dev/null | awk '/^host:/{print $2}')"

# ═══════════════════════════════════════════════════════════════════════════════
# Test Suites
# ═══════════════════════════════════════════════════════════════════════════════

# ── 1. Operator (Rust) ────────────────────────────────────────────────────────
if should_run "operator"; then
    run_suite "Operator (Rust)" \
        "$ROOT_DIR/services/operator" \
        cargo test
fi

# ── 2. Enclave (Rust) ─────────────────────────────────────────────────────────
if should_run "enclave"; then
    run_suite "Enclave (Rust)" \
        "$ROOT_DIR/tee/enclave" \
        cargo test
fi

# ── 3. Rust SDK ───────────────────────────────────────────────────────────────
if should_run "rust-sdk"; then
    run_suite "Rust SDK" \
        "$ROOT_DIR/sdk" \
        cargo test
fi

# ── 4. Python SDK ─────────────────────────────────────────────────────────────
if should_run "python-sdk"; then
    if command -v python3 &>/dev/null; then
        run_suite "Python SDK" \
            "$ROOT_DIR/sdk/python" \
            python3 -m pytest tests/ -v
    else
        skip_suite "Python SDK" "python3 not found"
    fi
fi

# ── 5. TypeScript SDK ─────────────────────────────────────────────────────────
if should_run "typescript-sdk"; then
    if command -v npx &>/dev/null; then
        run_suite "TypeScript SDK" \
            "$ROOT_DIR/sdk/typescript" \
            npx vitest run --reporter=verbose
    else
        skip_suite "TypeScript SDK" "npx not found"
    fi
fi

# ── 6. Solidity Contracts ─────────────────────────────────────────────────────
if should_run "solidity"; then
    if command -v forge &>/dev/null; then
        run_suite "Solidity Contracts" \
            "$ROOT_DIR/contracts" \
            forge test -vv
    else
        skip_suite "Solidity Contracts" "forge not found"
    fi
fi

# ── 7. XGBoost Remainder (slow -- circuit building + proving) ─────────────────
if should_run "xgboost-remainder"; then
    if [[ "$FAST_MODE" == "true" && -z "$FILTER_SUITE" ]]; then
        skip_suite "XGBoost Remainder" "--fast mode"
    else
        run_suite "XGBoost Remainder" \
            "$ROOT_DIR" \
            cargo test --manifest-path examples/xgboost-remainder/Cargo.toml
    fi
fi

# ── 8. Stylus GKR Verifier (needs native target, not wasm32) ─────────────────
if should_run "stylus"; then
    if [[ -n "$NATIVE_TARGET" ]]; then
        run_suite "Stylus GKR Verifier" \
            "$ROOT_DIR/contracts/stylus/gkr-verifier" \
            cargo test --target "$NATIVE_TARGET"
    else
        skip_suite "Stylus GKR Verifier" "could not detect native Rust target"
    fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════════════════════════

END_TIME=$(date +%s)
TOTAL_TIME=$((END_TIME - START_TIME))

echo ""
echo "==============================================================="
echo "  TEST SUMMARY"
echo "==============================================================="
for result in "${RESULTS[@]:-}"; do
    if [[ -n "$result" ]]; then
        echo "  $result"
    fi
done
echo "---------------------------------------------------------------"
echo "  Total: $TOTAL | Passed: $PASSED | Failed: $FAILED | Skipped: $SKIPPED"
echo "  Total time: ${TOTAL_TIME}s"
echo "==============================================================="

if [[ "$FAILED" -gt 0 ]]; then
    exit 1
fi
