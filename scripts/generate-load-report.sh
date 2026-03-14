#!/usr/bin/env bash
# Generate a markdown report from load test results.
# Usage: ./scripts/generate-load-report.sh [results-dir]
#
# Arguments:
#   results-dir   Directory containing load test output files (default: ./load-test-results)
#
# Options:
#   --help, -h    Show this help message

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -10 "$0" | tail -9
    exit 0
fi

RESULTS_DIR="${1:-./load-test-results}"

if [ ! -d "$RESULTS_DIR" ]; then
    echo "ERROR: Results directory not found: $RESULTS_DIR"
    echo "Run load tests first: docker compose -f docker-compose.loadtest.yml up"
    exit 1
fi

REPORT_FILE="${RESULTS_DIR}/report.md"
TIMESTAMP=$(date -u +"%Y-%m-%d %H:%M:%S UTC")

cat > "$REPORT_FILE" << EOF
# Load Test Report

**Generated:** $TIMESTAMP
**Results directory:** $RESULTS_DIR

---

## Test Results

EOF

# Process each result file
for result_file in "$RESULTS_DIR"/*.txt "$RESULTS_DIR"/*.log "$RESULTS_DIR"/*.json; do
    [ -f "$result_file" ] || continue
    NAME=$(basename "$result_file")

    cat >> "$REPORT_FILE" << EOF
### $NAME

\`\`\`
$(cat "$result_file" 2>/dev/null | head -50)
\`\`\`

EOF
done

# Summary
TOTAL_FILES=$(find "$RESULTS_DIR" -type f \( -name "*.txt" -o -name "*.log" -o -name "*.json" \) 2>/dev/null | wc -l | tr -d ' ')

cat >> "$REPORT_FILE" << EOF
---

## Summary

- **Total result files:** $TOTAL_FILES
- **Results directory:** \`$RESULTS_DIR\`
- **Report generated:** $TIMESTAMP
EOF

echo "Report generated: $REPORT_FILE"
