#!/usr/bin/env bash
# validate-config.sh — Check that alertmanager.yml is ready for use.
#
# In CI (before deploy): checks that no raw <REPLACE_*> placeholders remain.
# At deploy time (after envsubst): checks that no un-substituted ${VAR} refs remain.
#
# Usage:
#   ./monitoring/validate-config.sh                    # check default file
#   ./monitoring/validate-config.sh /path/to/config    # check specific file
#   DEPLOY_CHECK=1 ./monitoring/validate-config.sh     # also verify envsubst ran

set -euo pipefail

CONFIG_FILE="${1:-monitoring/alertmanager.yml}"
ERRORS=0

if [ ! -f "$CONFIG_FILE" ]; then
  echo "ERROR: Config file not found: $CONFIG_FILE"
  exit 1
fi

echo "Validating $CONFIG_FILE for unfilled placeholders..."

# Check for legacy <REPLACE_*> placeholders (should never be committed)
while IFS= read -r line; do
  lineno=$(echo "$line" | cut -d: -f1)
  placeholder=$(echo "$line" | grep -oE '<REPLACE_[A-Z_]+>' | head -1)
  echo "  ERROR line $lineno: legacy placeholder $placeholder found (use \${ENV_VAR} pattern instead)"
  ERRORS=$((ERRORS + 1))
done < <(grep -n '<REPLACE_' "$CONFIG_FILE" || true)

# In deploy mode, also check that envsubst has been run (no remaining ${VAR} refs
# outside of comment lines and Go template {{ }} blocks)
if [ "${DEPLOY_CHECK:-0}" = "1" ]; then
  echo "Deploy mode: checking that envsubst has been applied..."
  while IFS= read -r line; do
    lineno=$(echo "$line" | cut -d: -f1)
    content=$(echo "$line" | cut -d: -f2-)
    # Skip comment lines (leading #)
    if echo "$content" | grep -qE '^\s*#'; then
      continue
    fi
    placeholder=$(echo "$content" | grep -oE '\$\{[A-Z_]+\}' | head -1)
    if [ -n "$placeholder" ]; then
      echo "  ERROR line $lineno: $placeholder not substituted (run envsubst before deploying)"
      ERRORS=$((ERRORS + 1))
    fi
  done < <(grep -n '\${[A-Z_]*}' "$CONFIG_FILE" || true)
fi

if [ "$ERRORS" -gt 0 ]; then
  echo ""
  echo "FAILED: $ERRORS issue(s) found."
  echo "See monitoring/README.md for required environment variables and deployment instructions."
  exit 1
fi

echo "OK: All checks passed."
