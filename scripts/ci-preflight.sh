#!/usr/bin/env bash
# =============================================================================
# CI Preflight — Fast Local Checks Before Pushing
#
# Catches common CI failures locally before wasting CI minutes:
#   - Shell script syntax (bash -n)
#   - Cargo.toml validity (cargo verify-project)
#   - Docker Compose file validity (docker compose config)
#   - JSON file validity (python3 json.tool)
#   - Rust formatting (cargo fmt --check)
#   - Solidity formatting (forge fmt --check)
#
# Usage:
#   ./scripts/ci-preflight.sh
#   ./scripts/ci-preflight.sh --help
#
# Exit codes:
#   0 -- all checks passed
#   1 -- one or more checks failed
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

FAILURES=0
CHECKS=0
SKIPPED=0

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
ci-preflight.sh -- Fast local checks before pushing to CI.

USAGE:
  scripts/ci-preflight.sh [OPTIONS]

OPTIONS:
  --help, -h    Show this help message

CHECKS:
  1. Shell script syntax (bash -n on all .sh files)
  2. Cargo.toml parse validity
  3. Docker Compose file validity
  4. JSON file validity (deployments/, contracts/test/fixtures/)
  5. Rust formatting (cargo fmt --check)
  6. Solidity formatting (forge fmt --check)
USAGE
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

# ---------------------------------------------------------------------------
# Helper: run a check
# ---------------------------------------------------------------------------
run_check() {
    local name="$1"
    shift
    CHECKS=$((CHECKS + 1))
    if "$@" 2>/dev/null; then
        ok "$name"
    else
        err "$name"
        FAILURES=$((FAILURES + 1))
    fi
}

run_check_or_skip() {
    local name="$1"
    local tool="$2"
    shift 2
    if ! command -v "$tool" &>/dev/null; then
        warn "SKIP: $name ($tool not found)"
        SKIPPED=$((SKIPPED + 1))
        return
    fi
    CHECKS=$((CHECKS + 1))
    if "$@" 2>/dev/null; then
        ok "$name"
    else
        err "$name"
        FAILURES=$((FAILURES + 1))
    fi
}

header "============================================================"
header "  CI Preflight Checks"
header "============================================================"

cd "$ROOT_DIR" || exit 1

# ---------------------------------------------------------------------------
# 1. Shell script syntax
# ---------------------------------------------------------------------------
header "1/6  Shell Script Syntax (bash -n)"

SHELL_FAIL=0
while IFS= read -r -d '' script; do
    if ! bash -n "$script" 2>/dev/null; then
        err "  Syntax error: $script"
        SHELL_FAIL=1
    fi
done < <(find scripts/ -name '*.sh' -print0 2>/dev/null)

CHECKS=$((CHECKS + 1))
if [[ "$SHELL_FAIL" -eq 0 ]]; then
    ok "All shell scripts pass syntax check"
else
    FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------
# 2. Cargo.toml validity
# ---------------------------------------------------------------------------
header "2/6  Cargo.toml Validity"

CARGO_FAIL=0
while IFS= read -r -d '' toml; do
    if ! python3 -c "
import sys
try:
    import tomllib
except ImportError:
    import tomli as tomllib
with open('$toml', 'rb') as f:
    tomllib.load(f)
" 2>/dev/null; then
        err "  Invalid TOML: $toml"
        CARGO_FAIL=1
    fi
done < <(find . -name 'Cargo.toml' -not -path '*/target/*' -print0 2>/dev/null)

CHECKS=$((CHECKS + 1))
if [[ "$CARGO_FAIL" -eq 0 ]]; then
    ok "All Cargo.toml files parse successfully"
else
    FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------
# 3. Docker Compose file validity
# ---------------------------------------------------------------------------
header "3/6  Docker Compose Files"

COMPOSE_FAIL=0
for f in docker-compose*.yml; do
    [[ -f "$f" ]] || continue
    if ! python3 -c "
import yaml, sys
with open('$f') as fh:
    yaml.safe_load(fh)
" 2>/dev/null; then
        err "  Invalid YAML: $f"
        COMPOSE_FAIL=1
    fi
done

CHECKS=$((CHECKS + 1))
if [[ "$COMPOSE_FAIL" -eq 0 ]]; then
    ok "All docker-compose files parse successfully"
else
    FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------
# 4. JSON file validity
# ---------------------------------------------------------------------------
header "4/6  JSON File Validity"

JSON_FAIL=0
for dir in deployments; do
    [[ -d "$dir" ]] || continue
    while IFS= read -r -d '' jf; do
        if ! python3 -m json.tool "$jf" > /dev/null 2>&1; then
            err "  Invalid JSON: $jf"
            JSON_FAIL=1
        fi
    done < <(find "$dir" -name '*.json' -print0 2>/dev/null)
done

CHECKS=$((CHECKS + 1))
if [[ "$JSON_FAIL" -eq 0 ]]; then
    ok "All JSON files parse successfully"
else
    FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------
# 5. Rust formatting
# ---------------------------------------------------------------------------
header "5/6  Rust Formatting"

FMT_FAIL=0
for manifest in sdk/Cargo.toml services/operator/Cargo.toml tee/enclave/Cargo.toml \
    services/admin-cli/Cargo.toml services/indexer/Cargo.toml \
    crates/watcher/Cargo.toml crates/events/Cargo.toml; do
    [[ -f "$manifest" ]] || continue
    if ! cargo fmt --manifest-path "$manifest" --check 2>/dev/null; then
        err "  Formatting issue: $manifest"
        FMT_FAIL=1
    fi
done

CHECKS=$((CHECKS + 1))
if [[ "$FMT_FAIL" -eq 0 ]]; then
    ok "All Rust code is properly formatted"
else
    FAILURES=$((FAILURES + 1))
fi

# ---------------------------------------------------------------------------
# 6. Solidity formatting
# ---------------------------------------------------------------------------
header "6/6  Solidity Formatting"

if command -v forge &>/dev/null && [[ -d contracts ]]; then
    CHECKS=$((CHECKS + 1))
    if (cd contracts && forge fmt --check 2>/dev/null); then
        ok "All Solidity code is properly formatted"
    else
        err "Solidity formatting issues found (run: cd contracts && forge fmt)"
        FAILURES=$((FAILURES + 1))
    fi
else
    warn "SKIP: Solidity formatting (forge not found or no contracts/)"
    SKIPPED=$((SKIPPED + 1))
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  Preflight Summary"
header "============================================================"
echo ""
printf "  ${BOLD}%-12s${RESET} %d\n" "Checks:" "$CHECKS"
printf "  ${GREEN}%-12s${RESET} %d\n" "Passed:" "$((CHECKS - FAILURES))"
if [[ "$FAILURES" -gt 0 ]]; then
    printf "  ${RED}%-12s${RESET} %d\n" "Failed:" "$FAILURES"
fi
if [[ "$SKIPPED" -gt 0 ]]; then
    printf "  ${YELLOW}%-12s${RESET} %d\n" "Skipped:" "$SKIPPED"
fi
echo ""

if [[ "$FAILURES" -gt 0 ]]; then
    err "Preflight failed. Fix the above issues before pushing."
    exit 1
fi

ok "All preflight checks passed. Safe to push."
exit 0
