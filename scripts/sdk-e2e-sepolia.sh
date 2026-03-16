#!/usr/bin/env bash
# =============================================================================
# SDK E2E Tests — Run All SDK Integration Tests Against Sepolia
#
# Runs TypeScript, Python, and Rust SDK tests that connect to Sepolia.
#
# Usage:
#   SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
#     ./scripts/sdk-e2e-sepolia.sh
#
#   ./scripts/sdk-e2e-sepolia.sh --help
#
# Exit codes:
#   0 -- all tests passed
#   1 -- one or more tests failed
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
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[FAIL]${RESET}  %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
sdk-e2e-sepolia.sh -- Run all SDK E2E tests against Sepolia.

USAGE:
  scripts/sdk-e2e-sepolia.sh [OPTIONS]

OPTIONS:
  --skip-ts       Skip TypeScript SDK tests
  --skip-python   Skip Python SDK tests
  --skip-rust     Skip Rust SDK tests
  --help, -h      Show this help message

ENVIRONMENT:
  SEPOLIA_RPC_URL         Sepolia RPC URL (REQUIRED)
  TEE_VERIFIER_ADDRESS    TEEMLVerifier address (optional, reads from deployments/)
USAGE
}

SKIP_TS=false
SKIP_PYTHON=false
SKIP_RUST=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-ts) SKIP_TS=true; shift ;;
        --skip-python) SKIP_PYTHON=true; shift ;;
        --skip-rust) SKIP_RUST=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) err "Unknown option: $1"; usage; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
if [[ -z "${SEPOLIA_RPC_URL:-}" ]]; then
    err "SEPOLIA_RPC_URL is required."
    err "Example: SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY"
    exit 1
fi

header "============================================================"
header "  SDK E2E Tests — Sepolia"
header "============================================================"
echo ""
info "RPC URL: ${SEPOLIA_RPC_URL:0:40}..."

FAILURES=0
PASSED=0
SKIPPED=0

# ---------------------------------------------------------------------------
# TypeScript SDK
# ---------------------------------------------------------------------------
if [[ "$SKIP_TS" == "true" ]]; then
    warn "TypeScript SDK tests: SKIPPED"
    SKIPPED=$((SKIPPED + 1))
else
    header "TypeScript SDK Tests"
    if [[ -d "$ROOT_DIR/sdk/typescript" ]] && command -v npx &>/dev/null; then
        if (cd "$ROOT_DIR/sdk/typescript" && SEPOLIA_RPC_URL="$SEPOLIA_RPC_URL" npx vitest run src/__tests__/sepolia-e2e.test.ts 2>&1); then
            ok "TypeScript SDK: PASSED"
            PASSED=$((PASSED + 1))
        else
            err "TypeScript SDK: FAILED"
            FAILURES=$((FAILURES + 1))
        fi
    else
        warn "TypeScript SDK: SKIPPED (npx not found or sdk/typescript missing)"
        SKIPPED=$((SKIPPED + 1))
    fi
fi

# ---------------------------------------------------------------------------
# Python SDK
# ---------------------------------------------------------------------------
if [[ "$SKIP_PYTHON" == "true" ]]; then
    warn "Python SDK tests: SKIPPED"
    SKIPPED=$((SKIPPED + 1))
else
    header "Python SDK Tests"
    if [[ -d "$ROOT_DIR/sdk/python" ]] && command -v python3 &>/dev/null; then
        if (cd "$ROOT_DIR/sdk/python" && SEPOLIA_RPC_URL="$SEPOLIA_RPC_URL" python3 -m pytest tests/test_sepolia.py -v 2>&1); then
            ok "Python SDK: PASSED"
            PASSED=$((PASSED + 1))
        else
            err "Python SDK: FAILED"
            FAILURES=$((FAILURES + 1))
        fi
    else
        warn "Python SDK: SKIPPED (python3 not found or sdk/python missing)"
        SKIPPED=$((SKIPPED + 1))
    fi
fi

# ---------------------------------------------------------------------------
# Rust SDK
# ---------------------------------------------------------------------------
if [[ "$SKIP_RUST" == "true" ]]; then
    warn "Rust SDK tests: SKIPPED"
    SKIPPED=$((SKIPPED + 1))
else
    header "Rust SDK Tests"
    if [[ -f "$ROOT_DIR/sdk/Cargo.toml" ]] && command -v cargo &>/dev/null; then
        if (cd "$ROOT_DIR" && SEPOLIA_RPC_URL="$SEPOLIA_RPC_URL" cargo test --manifest-path sdk/Cargo.toml -- --ignored sepolia 2>&1); then
            ok "Rust SDK: PASSED"
            PASSED=$((PASSED + 1))
        else
            err "Rust SDK: FAILED"
            FAILURES=$((FAILURES + 1))
        fi
    else
        warn "Rust SDK: SKIPPED (cargo not found or sdk/Cargo.toml missing)"
        SKIPPED=$((SKIPPED + 1))
    fi
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  Summary"
header "============================================================"
echo ""
printf "  ${BOLD}%-12s${RESET} %d\n" "Passed:" "$PASSED"
if [[ "$FAILURES" -gt 0 ]]; then
    printf "  ${RED}%-12s${RESET} %d\n" "Failed:" "$FAILURES"
fi
if [[ "$SKIPPED" -gt 0 ]]; then
    printf "  ${YELLOW}%-12s${RESET} %d\n" "Skipped:" "$SKIPPED"
fi
echo ""

if [[ "$FAILURES" -gt 0 ]]; then
    err "Some SDK tests failed."
    exit 1
fi

ok "All SDK E2E tests passed."
exit 0
