#!/usr/bin/env bash
# =============================================================================
# docker-logs.sh -- Aggregate and filter Docker Compose logs
#
# Wraps `docker compose logs` with convenient filtering options.
#
# Usage:
#   ./scripts/docker-logs.sh
#   ./scripts/docker-logs.sh operator prover
#   ./scripts/docker-logs.sh -f --grep "error"
#   ./scripts/docker-logs.sh --file docker-compose.gpu.yml --tail 50
#   ./scripts/docker-logs.sh --help
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

info()  { printf "%b[INFO]%b  %s\n" "$CYAN"   "$RESET" "$*" >&2; }
ok()    { printf "%b[OK]%b    %s\n" "$GREEN"  "$RESET" "$*" >&2; }
warn()  { printf "%b[WARN]%b  %s\n" "$YELLOW" "$RESET" "$*" >&2; }
err()   { printf "%b[ERROR]%b %s\n" "$RED"    "$RESET" "$*" >&2; }

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------
show_help() {
    cat <<'EOF'
docker-logs.sh -- Aggregate and filter Docker Compose logs

USAGE:
  ./scripts/docker-logs.sh [OPTIONS] [SERVICE...]

ARGUMENTS:
  SERVICE...    One or more service names to filter (default: all services)

OPTIONS:
  -f, --follow          Follow log output (live tail)
  --tail <n>            Number of lines to show from end of logs (default: 100)
  --since <duration>    Show logs since duration (e.g., 10m, 1h, 2h30m)
  --grep <pattern>      Filter log lines matching pattern (case-insensitive)
  --file <compose-file> Docker Compose file (default: docker-compose.yml)
                        Can be specified multiple times for multiple files.
  --timestamps          Show timestamps in output
  --no-color            Disable colored output from docker compose
  -h, --help            Show this help and exit

EXAMPLES:
  # Show last 100 lines from all services
  ./scripts/docker-logs.sh

  # Follow logs from operator and prover services
  ./scripts/docker-logs.sh -f operator prover

  # Show errors from last hour
  ./scripts/docker-logs.sh --since 1h --grep "error|panic|fatal"

  # Use GPU compose file, follow enclave logs
  ./scripts/docker-logs.sh --file docker-compose.gpu.yml -f enclave

  # Show last 500 lines with timestamps
  ./scripts/docker-logs.sh --tail 500 --timestamps

  # Multiple compose files
  ./scripts/docker-logs.sh --file docker-compose.yml --file docker-compose.gpu.yml
EOF
}

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
FOLLOW=false
TAIL_LINES="100"
SINCE=""
GREP_PATTERN=""
TIMESTAMPS=false
NO_COLOR_FLAG=false
COMPOSE_FILES=()
SERVICES=()

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        -f|--follow)
            FOLLOW=true
            shift
            ;;
        --tail)
            TAIL_LINES="$2"
            shift 2
            ;;
        --since)
            SINCE="$2"
            shift 2
            ;;
        --grep)
            GREP_PATTERN="$2"
            shift 2
            ;;
        --file)
            COMPOSE_FILES+=("$2")
            shift 2
            ;;
        --timestamps)
            TIMESTAMPS=true
            shift
            ;;
        --no-color)
            NO_COLOR_FLAG=true
            shift
            ;;
        -*)
            err "Unknown option: $1"
            echo "Run with --help for usage." >&2
            exit 1
            ;;
        *)
            SERVICES+=("$1")
            shift
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate prerequisites
# ---------------------------------------------------------------------------
if ! command -v docker &>/dev/null; then
    err "'docker' not found. Please install Docker."
    exit 1
fi

# Check docker compose (v2 plugin or standalone)
COMPOSE_CMD=""
if docker compose version &>/dev/null 2>&1; then
    COMPOSE_CMD="docker compose"
elif command -v docker-compose &>/dev/null; then
    COMPOSE_CMD="docker-compose"
else
    err "'docker compose' not found. Install Docker Compose v2."
    exit 1
fi

# ---------------------------------------------------------------------------
# Resolve compose files
# ---------------------------------------------------------------------------
if [ ${#COMPOSE_FILES[@]} -eq 0 ]; then
    # Default: look for docker-compose.yml in project root
    DEFAULT_COMPOSE="$PROJECT_ROOT/docker-compose.yml"
    if [ -f "$DEFAULT_COMPOSE" ]; then
        COMPOSE_FILES=("$DEFAULT_COMPOSE")
    else
        warn "No docker-compose.yml found in $PROJECT_ROOT"
        warn "Falling back to docker compose default behavior."
    fi
fi

# Validate compose files exist and resolve relative paths
RESOLVED_FILES=()
for cf in "${COMPOSE_FILES[@]}"; do
    if [ ! -f "$cf" ]; then
        if [ -f "$PROJECT_ROOT/$cf" ]; then
            cf="$PROJECT_ROOT/$cf"
        else
            err "Compose file not found: $cf"
            exit 1
        fi
    fi
    RESOLVED_FILES+=("$cf")
done

# ---------------------------------------------------------------------------
# Build the docker compose command
# ---------------------------------------------------------------------------
CMD=()

# Split COMPOSE_CMD into array elements
# shellcheck disable=SC2206
CMD+=($COMPOSE_CMD)

for cf in "${RESOLVED_FILES[@]}"; do
    CMD+=(-f "$cf")
done

CMD+=(logs)
CMD+=(--tail "$TAIL_LINES")

if [ "$FOLLOW" = "true" ]; then
    CMD+=(-f)
fi

if [ -n "$SINCE" ]; then
    CMD+=(--since "$SINCE")
fi

if [ "$TIMESTAMPS" = "true" ]; then
    CMD+=(--timestamps)
fi

if [ "$NO_COLOR_FLAG" = "true" ]; then
    CMD+=(--no-color)
fi

# Append service names
for svc in "${SERVICES[@]}"; do
    CMD+=("$svc")
done

# ---------------------------------------------------------------------------
# Print info header (to stderr so stdout stays clean for piping)
# ---------------------------------------------------------------------------
{
    printf "\n"
    printf "%b========================================%b\n" "$BOLD" "$RESET"
    printf "%b  Docker Compose Logs%b\n" "$BOLD" "$RESET"
    printf "%b========================================%b\n" "$BOLD" "$RESET"
    printf "\n"

    if [ ${#RESOLVED_FILES[@]} -gt 0 ]; then
        for cf in "${RESOLVED_FILES[@]}"; do
            info "Compose file: $cf"
        done
    fi

    if [ ${#SERVICES[@]} -gt 0 ]; then
        svc_list=""
        for svc in "${SERVICES[@]}"; do
            if [ -n "$svc_list" ]; then
                svc_list="$svc_list, $svc"
            else
                svc_list="$svc"
            fi
        done
        info "Services:     $svc_list"
    else
        info "Services:     all"
    fi

    info "Tail:         $TAIL_LINES lines"

    if [ "$FOLLOW" = "true" ]; then
        info "Mode:         follow (Ctrl+C to stop)"
    fi
    if [ -n "$SINCE" ]; then
        info "Since:        $SINCE"
    fi
    if [ -n "$GREP_PATTERN" ]; then
        info "Filter:       $GREP_PATTERN"
    fi

    printf "\n"
} >&2

# ---------------------------------------------------------------------------
# Execute
# ---------------------------------------------------------------------------
if [ -n "$GREP_PATTERN" ]; then
    # Pipe through grep with case-insensitive matching.
    # Use --line-buffered for streaming compatibility with --follow.
    "${CMD[@]}" 2>&1 | grep -i --line-buffered "$GREP_PATTERN" || {
        exit_code=$?
        if [ "$exit_code" -eq 1 ]; then
            warn "No log lines matched pattern: $GREP_PATTERN"
            exit 0
        fi
        exit "$exit_code"
    }
else
    "${CMD[@]}" 2>&1
fi
