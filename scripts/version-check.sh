#!/usr/bin/env bash
# Check that required tool versions meet minimum requirements.
#
# Usage:
#   ./scripts/version-check.sh
#   ./scripts/version-check.sh --help
#
# Shows PASS/FAIL/SKIP for each tool. Exit code 0 if all required tools
# pass, 1 if any required tool fails.
#
# Requires: bash 4+

set -uo pipefail

# =============================================================================
# Help
# =============================================================================

show_help() {
    cat <<'EOF'
version-check.sh -- Verify tool versions for World ZK Compute

USAGE:
  ./scripts/version-check.sh

CHECKS:
  Tool               Minimum Version    Required
  ----               ---------------    --------
  rustc              1.75.0             yes
  cargo              (any)              yes
  forge (Foundry)    (any)              yes
  cast (Foundry)     (any)              yes
  docker             20.0               yes
  docker compose     2.0                no
  node               18.0               no
  python3            3.9                no
  jq                 (any)              no
  curl               (any)              no

EXIT:
  0 if all required tools pass
  1 if any required tool is missing or too old

OPTIONS:
  -h, --help    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case $1 in
        --help|-h) show_help; exit 0 ;;
        *) echo "Unknown option: $1"; show_help; exit 1 ;;
    esac
done

# =============================================================================
# Helpers
# =============================================================================

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
REQUIRED_FAIL=0

# Compare two version strings (major.minor.patch).
# Returns 0 if $1 >= $2, 1 otherwise.
version_gte() {
    local v1="$1"
    local v2="$2"

    # Split into components
    local IFS='.'
    # shellcheck disable=SC2206
    local v1_parts=($v1)
    # shellcheck disable=SC2206
    local v2_parts=($v2)

    local max=${#v2_parts[@]}
    for (( i=0; i<max; i++ )); do
        local a=${v1_parts[$i]:-0}
        local b=${v2_parts[$i]:-0}

        # Strip non-numeric suffixes (e.g., "18.19.0" vs "18")
        a=${a%%[!0-9]*}
        b=${b%%[!0-9]*}
        a=${a:-0}
        b=${b:-0}

        if (( a > b )); then
            return 0
        elif (( a < b )); then
            return 1
        fi
    done
    return 0
}

# Extract version number from a string.
# Matches the first occurrence of digits.digits (optionally .digits).
extract_version() {
    echo "$1" | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -1
}

record_pass() {
    local tool="$1"
    local version="$2"
    local min="${3:-any}"
    printf "  [PASS] %-20s %-14s (min: %s)\n" "$tool" "$version" "$min"
    PASS_COUNT=$((PASS_COUNT + 1))
}

record_fail() {
    local tool="$1"
    local detail="$2"
    local required="${3:-true}"
    printf "  [FAIL] %-20s %s\n" "$tool" "$detail" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
    if [ "$required" = "true" ]; then
        REQUIRED_FAIL=$((REQUIRED_FAIL + 1))
    fi
}

record_skip() {
    local tool="$1"
    local reason="$2"
    printf "  [SKIP] %-20s %s\n" "$tool" "$reason"
    SKIP_COUNT=$((SKIP_COUNT + 1))
}

# Check a tool.
#   check_tool <name> <command> <version-flag> <min-version|"any"> <required: true|false>
check_tool() {
    local name="$1"
    local cmd="$2"
    local version_flag="$3"
    local min_version="$4"
    local required="$5"

    if ! command -v "$cmd" &>/dev/null; then
        if [ "$required" = "true" ]; then
            record_fail "$name" "not installed" "true"
        else
            record_skip "$name" "not installed (optional)"
        fi
        return
    fi

    # Get version output
    local version_output
    version_output=$($cmd $version_flag 2>&1) || true

    local version
    version=$(extract_version "$version_output")

    if [ -z "$version" ]; then
        # Could not parse version, but tool exists
        if [ "$min_version" = "any" ]; then
            record_pass "$name" "(found)" "any"
        else
            record_fail "$name" "could not parse version" "$required"
        fi
        return
    fi

    if [ "$min_version" = "any" ]; then
        record_pass "$name" "$version" "any"
        return
    fi

    if version_gte "$version" "$min_version"; then
        record_pass "$name" "$version" "$min_version"
    else
        record_fail "$name" "found $version, need >= $min_version" "$required"
    fi
}

# =============================================================================
# Main
# =============================================================================

echo "========================================"
echo "  World ZK Compute -- Version Check"
echo "========================================"
echo ""

# Required tools
check_tool "rustc"           "rustc"    "--version"  "1.75.0"  "true"
check_tool "cargo"           "cargo"    "--version"  "any"     "true"
check_tool "forge (Foundry)" "forge"    "--version"  "any"     "true"
check_tool "cast (Foundry)"  "cast"     "--version"  "any"     "true"
check_tool "docker"          "docker"   "--version"  "20.0"    "true"

echo ""

# Optional tools
check_tool "docker compose"  "docker"   "compose version"  "2.0"   "false"
check_tool "node"            "node"     "--version"        "18.0"  "false"
check_tool "python3"         "python3"  "--version"        "3.9"   "false"
check_tool "jq"              "jq"       "--version"        "any"   "false"
check_tool "curl"            "curl"     "--version"        "any"   "false"

# =============================================================================
# Summary
# =============================================================================

echo ""
echo "========================================"
echo "  Summary"
echo "========================================"
echo "  PASS: $PASS_COUNT"
echo "  FAIL: $FAIL_COUNT"
echo "  SKIP: $SKIP_COUNT"
echo ""

if [ "$REQUIRED_FAIL" -gt 0 ]; then
    echo "  ERROR: $REQUIRED_FAIL required tool(s) missing or too old." >&2
    exit 1
else
    echo "  All required tools present and meet minimum versions."
    exit 0
fi
