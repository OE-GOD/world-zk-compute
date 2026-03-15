#!/usr/bin/env bash
#
# Health check for all World ZK Compute services.
# Checks each service's health endpoint and reports PASS/FAIL.
#
# Usage:
#   ./scripts/health-check-all.sh
#   ./scripts/health-check-all.sh --host 192.168.1.100
#   ./scripts/health-check-all.sh --help

set -euo pipefail

# Defaults
HOST="localhost"
TIMEOUT=5
FAILED=0
CHECKED=0

# Service definitions: name:port:path
SERVICES=(
  "enclave:8080:/health"
  "operator:9090:/health"
  "prover:3000:/health"
  "private-input:3001:/health"
  "indexer:8081:/health"
)

usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Check health of all World ZK Compute services.

Options:
  --host HOST    Base hostname or IP (default: localhost)
  --timeout SEC  HTTP request timeout in seconds (default: 5)
  --help         Show this help message

Services checked:
  enclave        port 8080
  operator       port 9090
  prover         port 3000
  private-input  port 3001
  indexer        port 8081

Exit codes:
  0  All services healthy
  1  One or more services unreachable or unhealthy
EOF
  exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --host)
      HOST="$2"
      shift 2
      ;;
    --timeout)
      TIMEOUT="$2"
      shift 2
      ;;
    --help|-h)
      usage
      ;;
    *)
      echo "Unknown option: $1"
      echo "Run with --help for usage."
      exit 1
      ;;
  esac
done

echo "Health check: host=${HOST}, timeout=${TIMEOUT}s"
echo "-------------------------------------------"

for entry in "${SERVICES[@]}"; do
  IFS=':' read -r name port path <<< "$entry"
  url="http://${HOST}:${port}${path}"
  CHECKED=$((CHECKED + 1))

  if http_code=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "$TIMEOUT" --max-time "$TIMEOUT" "$url" 2>/dev/null); then
    if [[ "$http_code" -ge 200 && "$http_code" -lt 300 ]]; then
      echo "  PASS  ${name} (${url}) [HTTP ${http_code}]"
    else
      echo "  FAIL  ${name} (${url}) [HTTP ${http_code}]"
      FAILED=$((FAILED + 1))
    fi
  else
    echo "  FAIL  ${name} (${url}) [connection refused or timeout]"
    FAILED=$((FAILED + 1))
  fi
done

echo "-------------------------------------------"
echo "Results: $((CHECKED - FAILED))/${CHECKED} services healthy"

if [[ "$FAILED" -gt 0 ]]; then
  echo "WARNING: ${FAILED} service(s) unhealthy"
  exit 1
fi

echo "All services healthy."
exit 0
