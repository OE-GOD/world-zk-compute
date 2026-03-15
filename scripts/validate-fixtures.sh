#!/usr/bin/env bash
# validate-fixtures.sh -- Validate JSON test fixtures in contracts/test/fixtures/
#
# Checks:
#   1. Each file is valid JSON (parseable by Python's json module)
#   2. No file exceeds the size limit (default 50MB)
#   3. Prints a summary table of all fixtures
#
# Usage:
#   bash scripts/validate-fixtures.sh
#
# Exit codes:
#   0 - all fixtures are valid
#   1 - one or more fixtures failed validation

set -euo pipefail

FIXTURES_DIR="${FIXTURES_DIR:-contracts/test/fixtures}"
MAX_SIZE_BYTES="${MAX_SIZE_BYTES:-52428800}"  # 50MB

# Move to repo root if running from scripts/
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

if [ ! -d "$FIXTURES_DIR" ]; then
  echo "ERROR: Fixtures directory not found: $FIXTURES_DIR"
  exit 1
fi

# Collect fixture files
shopt -s nullglob
fixture_files=("$FIXTURES_DIR"/*.json)
shopt -u nullglob

if [ ${#fixture_files[@]} -eq 0 ]; then
  echo "No JSON fixtures found in $FIXTURES_DIR"
  exit 0
fi

total=0
passed=0
failed=0
errors=()

echo "============================================"
echo "  Test Fixture Validation"
echo "============================================"
echo ""
printf "%-45s %12s %10s\n" "File" "Size" "Status"
printf "%-45s %12s %10s\n" "----" "----" "------"

for filepath in "${fixture_files[@]}"; do
  filename="$(basename "$filepath")"
  total=$((total + 1))

  # Check file size
  file_size=$(wc -c < "$filepath" | tr -d ' ')
  if [ "$file_size" -gt 1048576 ]; then
    size_display="$(echo "scale=1; $file_size / 1048576" | bc)MB"
  elif [ "$file_size" -gt 1024 ]; then
    size_display="$(echo "scale=1; $file_size / 1024" | bc)KB"
  else
    size_display="${file_size}B"
  fi

  if [ "$file_size" -gt "$MAX_SIZE_BYTES" ]; then
    printf "%-45s %12s %10s\n" "$filename" "$size_display" "FAIL"
    errors+=("$filename: exceeds size limit (${size_display} > 50MB)")
    failed=$((failed + 1))
    continue
  fi

  # Validate JSON
  if python3 -c "import json; json.load(open('$filepath'))" 2>/dev/null; then
    printf "%-45s %12s %10s\n" "$filename" "$size_display" "OK"
    passed=$((passed + 1))
  else
    printf "%-45s %12s %10s\n" "$filename" "$size_display" "FAIL"
    errors+=("$filename: invalid JSON")
    failed=$((failed + 1))
  fi
done

echo ""
echo "============================================"
echo "  Summary: $passed/$total passed, $failed failed"
echo "============================================"

if [ ${#errors[@]} -gt 0 ]; then
  echo ""
  echo "Errors:"
  for err in "${errors[@]}"; do
    echo "  - $err"
  done
  exit 1
fi

echo ""
echo "All fixtures valid."
exit 0
