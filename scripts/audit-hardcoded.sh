#!/usr/bin/env bash
# =============================================================================
# World ZK Compute -- Hardcoded Address Audit
#
# Scans source files for hardcoded localhost/127.0.0.1 references that should
# not appear in production code. Warns on all matches, fails if any are found
# in production service code (services/*/src/*).
#
# Usage:
#   scripts/audit-hardcoded.sh
#
# Exit codes:
#   0 -- no hardcoded addresses found in production code
#   1 -- hardcoded addresses found in production service code
# =============================================================================

set -uo pipefail

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
warn_msg(){ printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
fail_msg(){ printf "${RED}[FAIL]${RESET}  %s\n" "$*"; }

# ---------------------------------------------------------------------------
# Project root
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
# Patterns to search for
PATTERNS=("127\.0\.0\.1" "localhost")

# File extensions to scan
EXTENSIONS=("rs" "sol" "ts" "py")

# Paths and patterns to exclude (test files, examples, config templates)
EXCLUDE_PATTERNS=(
    "*_test.rs"
    "*.t.sol"
    "test_*.py"
    "*.test.ts"
    "*.spec.ts"
    "*/examples/*"
    "*/docker-compose*"
    "*.env.example"
    "*/.env.example"
)

# Production paths -- matches here cause a non-zero exit
PRODUCTION_PATHS=("services/*/src/*")

# ---------------------------------------------------------------------------
# Build grep arguments
# ---------------------------------------------------------------------------

# Build include args for file extensions
INCLUDE_ARGS=()
for ext in "${EXTENSIONS[@]}"; do
    INCLUDE_ARGS+=("--include=*.${ext}")
done

# Build exclude args
EXCLUDE_ARGS=()
for pat in "${EXCLUDE_PATTERNS[@]}"; do
    EXCLUDE_ARGS+=("--exclude=${pat}")
done
# Also exclude this script itself
EXCLUDE_ARGS+=("--exclude=audit-hardcoded.sh")

# ---------------------------------------------------------------------------
# Run scan
# ---------------------------------------------------------------------------
header() { printf "\n${BOLD}%s${RESET}\n" "$*"; }

header "============================================================"
header "  World ZK Compute -- Hardcoded Address Audit"
header "============================================================"
echo ""
info "Scanning for hardcoded localhost/127.0.0.1 references..."
info "File types: ${EXTENSIONS[*]}"
info "Excluding: test files, examples/, docker-compose*, .env.example, vendor dirs"
echo ""

WARN_COUNT=0
PROD_COUNT=0
ALL_MATCHES=""

for pattern in "${PATTERNS[@]}"; do
    # Use grep -rn with includes/excludes
    MATCHES=$(grep -rn "${INCLUDE_ARGS[@]}" "${EXCLUDE_ARGS[@]}" \
        --exclude-dir=examples \
        --exclude-dir=target \
        --exclude-dir=node_modules \
        --exclude-dir=.git \
        --exclude-dir=out \
        --exclude-dir=cache \
        --exclude-dir=lib \
        --exclude-dir=.venv \
        --exclude-dir=venv \
        --exclude-dir=dist \
        --exclude-dir=build \
        --exclude-dir=artifacts \
        -E "$pattern" "$PROJECT_ROOT" 2>/dev/null || true)

    if [[ -z "$MATCHES" ]]; then
        continue
    fi

    while IFS= read -r line; do
        # Strip project root prefix for cleaner output
        REL_LINE="${line#${PROJECT_ROOT}/}"
        FILE_PATH="${REL_LINE%%:*}"

        # Check if this is a production service file
        IS_PROD=0
        for prod_pat in "${PRODUCTION_PATHS[@]}"; do
            # Use bash pattern matching
            # shellcheck disable=SC2254
            case "$FILE_PATH" in
                services/*/src/*)
                    IS_PROD=1
                    break
                    ;;
            esac
        done

        if [[ "$IS_PROD" -eq 1 ]]; then
            fail_msg "$REL_LINE"
            PROD_COUNT=$((PROD_COUNT + 1))
        else
            warn_msg "$REL_LINE"
        fi
        WARN_COUNT=$((WARN_COUNT + 1))
    done <<< "$MATCHES"
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
header "------------------------------------------------------------"
header "  SUMMARY"
header "------------------------------------------------------------"
echo ""

TOTAL_WARNINGS=$((WARN_COUNT))
TOTAL_PROD=$((PROD_COUNT))

if [[ "$TOTAL_WARNINGS" -eq 0 ]]; then
    ok "No hardcoded localhost/127.0.0.1 references found."
    echo ""
    exit 0
fi

printf "  Total matches:      %d\n" "$TOTAL_WARNINGS"
printf "  Production matches: %d\n" "$TOTAL_PROD"
echo ""

if [[ "$TOTAL_PROD" -gt 0 ]]; then
    fail_msg "Found $TOTAL_PROD hardcoded address(es) in production service code."
    printf "  Fix these before deploying to production.\n"
    echo ""
    exit 1
else
    warn_msg "Found $TOTAL_WARNINGS match(es) in non-production code (tests, scripts, config)."
    printf "  These are acceptable but worth reviewing.\n"
    echo ""
    exit 0
fi
