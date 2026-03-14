#!/usr/bin/env bash
# =============================================================================
# operator-health.sh -- Check operator service health
#
# Queries the operator service /health and /metrics endpoints and reports
# overall status, uptime, version, pending jobs, and last processed block.
#
# Optional env:
#   OPERATOR_URL   Operator base URL (default: http://localhost:8080)
#
# Usage:
#   ./scripts/operator-health.sh
#   ./scripts/operator-health.sh --url http://remote:8080
#   ./scripts/operator-health.sh --help
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [ -z "${NO_COLOR:-}" ] && [ -t 1 ]; then
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    RED='\033[0;31m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    GREEN=''
    YELLOW=''
    CYAN=''
    RED=''
    BOLD=''
    RESET=''
fi

info()  { printf "%b[INFO]%b  %s\n" "$CYAN"   "$RESET" "$*"; }
ok()    { printf "%b[OK]%b    %s\n" "$GREEN"  "$RESET" "$*"; }
warn()  { printf "%b[WARN]%b  %s\n" "$YELLOW" "$RESET" "$*"; }
err()   { printf "%b[ERROR]%b %s\n" "$RED"    "$RESET" "$*" >&2; }

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------
show_help() {
    cat <<'EOF'
operator-health.sh -- Check operator service health

USAGE:
  ./scripts/operator-health.sh [OPTIONS]

OPTIONS:
  --url <url>       Override operator URL (default: OPERATOR_URL env or
                    http://localhost:8080)
  --json            Output health check results as JSON
  --quiet, -q       Only print status line (exit 0=healthy, 1=unhealthy)
  -h, --help        Show this help and exit

OPTIONAL ENVIRONMENT:
  OPERATOR_URL      Operator base URL (default: http://localhost:8080)

CHECKS:
  - /health endpoint (HTTP 200 = healthy)
  - /metrics endpoint (Prometheus metrics)
  - Extracts: status, uptime, version, pending jobs, last processed block

EXAMPLES:
  ./scripts/operator-health.sh
  ./scripts/operator-health.sh --url http://operator.example.com:8080
  OPERATOR_URL=http://10.0.0.5:8080 ./scripts/operator-health.sh --json
EOF
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
URL_OVERRIDE=""
JSON_OUTPUT=false
QUIET=false

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        --url)
            URL_OVERRIDE="$2"
            shift 2
            ;;
        --json)
            JSON_OUTPUT=true
            shift
            ;;
        -q|--quiet)
            QUIET=true
            shift
            ;;
        *)
            err "Unknown option: $1"
            echo "Run with --help for usage."
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
if ! command -v curl &>/dev/null; then
    err "'curl' not found. Please install curl."
    exit 1
fi

BASE_URL="${URL_OVERRIDE:-${OPERATOR_URL:-http://localhost:8080}}"
BASE_URL="${BASE_URL%/}"

# ---------------------------------------------------------------------------
# JSON field extractor
# ---------------------------------------------------------------------------
json_extract() {
    local json_str="$1"
    local field="$2"

    if command -v jq &>/dev/null; then
        echo "$json_str" | jq -r "$field // empty" 2>/dev/null || echo ""
    elif command -v python3 &>/dev/null; then
        echo "$json_str" | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    keys = '$field'.strip('.').split('.')
    for k in keys:
        if isinstance(d, dict):
            d = d.get(k, '')
        else:
            d = ''
            break
    print(d if d else '')
except Exception:
    print('')
" 2>/dev/null || echo ""
    else
        echo ""
    fi
}

# ---------------------------------------------------------------------------
# Prometheus metric extractor
# ---------------------------------------------------------------------------
metric_value() {
    local metrics_text="$1"
    local metric_name="$2"

    echo "$metrics_text" | grep "^${metric_name}" | head -1 | awk '{print $2}' || echo ""
}

# ---------------------------------------------------------------------------
# Health check
# ---------------------------------------------------------------------------
HEALTH_STATUS="unhealthy"
HEALTH_BODY=""
HEALTH_CODE="000"
HEALTH_MSG=""

HEALTH_RESPONSE=$(curl -s -w "\n%{http_code}" --connect-timeout 5 --max-time 10 \
    "${BASE_URL}/health" 2>/dev/null || echo -e "\n000")

HEALTH_CODE=$(echo "$HEALTH_RESPONSE" | tail -1)
HEALTH_BODY=$(echo "$HEALTH_RESPONSE" | sed '$d')

if [ "$HEALTH_CODE" = "200" ]; then
    HEALTH_STATUS="healthy"
    HEALTH_MSG="$HEALTH_BODY"
elif [ "$HEALTH_CODE" = "000" ]; then
    HEALTH_STATUS="unreachable"
    HEALTH_MSG="Cannot connect to $BASE_URL"
else
    HEALTH_STATUS="unhealthy"
    HEALTH_MSG="HTTP $HEALTH_CODE"
fi

# ---------------------------------------------------------------------------
# Extract fields from health response
# ---------------------------------------------------------------------------
VERSION=""
UPTIME=""
PENDING_JOBS=""
LAST_BLOCK=""

if [ -n "$HEALTH_BODY" ]; then
    VERSION=$(json_extract "$HEALTH_BODY" ".version")
    UPTIME=$(json_extract "$HEALTH_BODY" ".uptime")
    PENDING_JOBS=$(json_extract "$HEALTH_BODY" ".pending_jobs")
    LAST_BLOCK=$(json_extract "$HEALTH_BODY" ".last_processed_block")

    # Try alternate field names
    if [ -z "$UPTIME" ]; then
        UPTIME=$(json_extract "$HEALTH_BODY" ".uptimeSeconds")
    fi
    if [ -z "$PENDING_JOBS" ]; then
        PENDING_JOBS=$(json_extract "$HEALTH_BODY" ".pendingJobs")
    fi
    if [ -z "$LAST_BLOCK" ]; then
        LAST_BLOCK=$(json_extract "$HEALTH_BODY" ".lastProcessedBlock")
    fi
fi

# ---------------------------------------------------------------------------
# Metrics check
# ---------------------------------------------------------------------------
METRICS_STATUS="unavailable"
METRICS_BODY=""
METRICS_CODE="000"

METRICS_RESPONSE=$(curl -s -w "\n%{http_code}" --connect-timeout 5 --max-time 10 \
    "${BASE_URL}/metrics" 2>/dev/null || echo -e "\n000")

METRICS_CODE=$(echo "$METRICS_RESPONSE" | tail -1)
METRICS_BODY=$(echo "$METRICS_RESPONSE" | sed '$d')

if [ "$METRICS_CODE" = "200" ]; then
    METRICS_STATUS="available"

    # Try to extract useful metrics from Prometheus format
    if [ -z "$PENDING_JOBS" ]; then
        PENDING_JOBS=$(metric_value "$METRICS_BODY" "operator_pending_jobs")
    fi
    if [ -z "$LAST_BLOCK" ]; then
        LAST_BLOCK=$(metric_value "$METRICS_BODY" "operator_last_processed_block")
    fi
fi

# ---------------------------------------------------------------------------
# Process check (local only)
# ---------------------------------------------------------------------------
PROCESS_STATUS="unknown"
PROCESS_PID=""

if [ "$BASE_URL" = "http://localhost:8080" ] || [ "$BASE_URL" = "http://127.0.0.1:8080" ]; then
    PROCESS_PID=$(pgrep -f "operator" 2>/dev/null | head -1 || echo "")
    if [ -n "$PROCESS_PID" ]; then
        PROCESS_STATUS="running (PID: $PROCESS_PID)"
    else
        PROCESS_STATUS="not found"
    fi
fi

# ---------------------------------------------------------------------------
# JSON output
# ---------------------------------------------------------------------------
if [ "$JSON_OUTPUT" = "true" ]; then
    cat <<EOF
{
  "url": "$BASE_URL",
  "health": "$HEALTH_STATUS",
  "healthCode": "$HEALTH_CODE",
  "metrics": "$METRICS_STATUS",
  "version": "${VERSION:-unknown}",
  "uptime": "${UPTIME:-unknown}",
  "pendingJobs": "${PENDING_JOBS:-unknown}",
  "lastProcessedBlock": "${LAST_BLOCK:-unknown}",
  "process": "$PROCESS_STATUS"
}
EOF
    if [ "$HEALTH_STATUS" = "healthy" ]; then
        exit 0
    else
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Quiet output
# ---------------------------------------------------------------------------
if [ "$QUIET" = "true" ]; then
    if [ "$HEALTH_STATUS" = "healthy" ]; then
        echo "healthy"
        exit 0
    else
        echo "$HEALTH_STATUS"
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Human-readable output
# ---------------------------------------------------------------------------
printf "\n"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "%b  Operator Health Check%b\n" "$BOLD" "$RESET"
printf "%b========================================%b\n" "$BOLD" "$RESET"
printf "\n"

info "URL: $BASE_URL"
printf "\n"

# Health status
printf "  %-22s " "Health:"
if [ "$HEALTH_STATUS" = "healthy" ]; then
    printf "%b%s%b\n" "$GREEN" "$HEALTH_STATUS" "$RESET"
elif [ "$HEALTH_STATUS" = "unreachable" ]; then
    printf "%b%s%b\n" "$RED" "$HEALTH_STATUS" "$RESET"
else
    printf "%b%s%b (HTTP %s)\n" "$RED" "$HEALTH_STATUS" "$RESET" "$HEALTH_CODE"
fi

# Metrics endpoint
printf "  %-22s " "Metrics:"
if [ "$METRICS_STATUS" = "available" ]; then
    printf "%b%s%b\n" "$GREEN" "$METRICS_STATUS" "$RESET"
else
    printf "%b%s%b\n" "$YELLOW" "$METRICS_STATUS" "$RESET"
fi

# Process status (local only)
if [ "$PROCESS_STATUS" != "unknown" ]; then
    printf "  %-22s " "Process:"
    if [ -n "$PROCESS_PID" ]; then
        printf "%b%s%b\n" "$GREEN" "$PROCESS_STATUS" "$RESET"
    else
        printf "%b%s%b\n" "$YELLOW" "$PROCESS_STATUS" "$RESET"
    fi
fi

printf "\n"

# Details
printf "  %-22s %s\n" "Version:" "${VERSION:-unknown}"
printf "  %-22s %s\n" "Uptime:" "${UPTIME:-unknown}"
printf "  %-22s %s\n" "Pending jobs:" "${PENDING_JOBS:-unknown}"
printf "  %-22s %s\n" "Last processed block:" "${LAST_BLOCK:-unknown}"

printf "\n"

# Overall result
if [ "$HEALTH_STATUS" = "healthy" ]; then
    ok "Operator is healthy."
    exit 0
elif [ "$HEALTH_STATUS" = "unreachable" ]; then
    err "Operator is unreachable at $BASE_URL."
    echo "  Ensure the operator service is running and accessible."
    exit 1
else
    err "Operator is unhealthy ($HEALTH_MSG)."
    exit 1
fi
