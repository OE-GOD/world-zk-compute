#!/usr/bin/env bash
# validate-config.sh — Check that alertmanager.yml placeholders have been replaced.
# Run before starting the monitoring stack.
#
# Usage: ./monitoring/validate-config.sh

set -euo pipefail

CONFIG_FILE="${1:-monitoring/alertmanager.yml}"
ERRORS=0

if [ ! -f "$CONFIG_FILE" ]; then
  echo "ERROR: Config file not found: $CONFIG_FILE"
  exit 1
fi

echo "Validating $CONFIG_FILE for unfilled placeholders..."

# Find all <REPLACE_*> placeholders
while IFS= read -r line; do
  lineno=$(echo "$line" | cut -d: -f1)
  placeholder=$(echo "$line" | grep -oE '<REPLACE_[A-Z_]+>' | head -1)
  echo "  ERROR line $lineno: $placeholder not replaced"
  ERRORS=$((ERRORS + 1))
done < <(grep -n '<REPLACE_' "$CONFIG_FILE" || true)

if [ "$ERRORS" -gt 0 ]; then
  echo ""
  echo "FAILED: $ERRORS placeholder(s) need to be replaced before use."
  echo "Edit $CONFIG_FILE and replace all <REPLACE_*> values with your credentials."
  exit 1
fi

echo "OK: All placeholders have been replaced."
