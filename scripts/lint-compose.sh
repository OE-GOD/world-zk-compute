#!/usr/bin/env bash
# Validate all docker-compose files.
# Usage: ./scripts/lint-compose.sh
#
# Options:
#   --help, -h    Show this help message

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -6 "$0" | tail -5
    exit 0
fi

if ! command -v docker &>/dev/null; then
    echo "ERROR: docker not found"
    exit 1
fi

PASS=0
FAIL=0

for f in docker-compose*.yml; do
    [ -f "$f" ] || continue
    printf "  %-45s " "$f"
    if docker compose -f "$f" config --quiet 2>/dev/null; then
        echo "[OK]"
        PASS=$((PASS + 1))
    else
        echo "[FAIL]"
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
