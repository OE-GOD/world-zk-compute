#!/usr/bin/env bash
# =============================================================================
# Sepolia Monitoring — Start Monitoring Stack Alongside Sepolia Services
#
# Launches Prometheus + Grafana alongside the Sepolia Docker Compose stack.
#
# Usage:
#   ./scripts/sepolia-monitoring.sh
#   ./scripts/sepolia-monitoring.sh --down
#   ./scripts/sepolia-monitoring.sh --help
#
# Exit codes:
#   0 -- success
#   1 -- error
# =============================================================================

set -euo pipefail

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
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[FAIL]${RESET}  %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
sepolia-monitoring.sh -- Start monitoring alongside Sepolia services.

USAGE:
  scripts/sepolia-monitoring.sh [OPTIONS]

OPTIONS:
  --down          Stop all services (Sepolia + monitoring)
  --status        Show running container status
  --env-file <f>  Environment file (default: .env.sepolia)
  --help, -h      Show this help message

ACCESS:
  Prometheus:  http://localhost:9091
  Grafana:     http://localhost:3001  (admin/admin)

DASHBOARDS:
  Grafana ships with pre-provisioned dashboards:
    - Operator:  submissions, disputes, errors, gas
    - Enclave:   inference latency, attestation, model stats
    - Indexer:   events indexed, WebSocket connections, REST latency
    - Sepolia:   chain-specific metrics

REQUIREMENTS:
  - docker compose (v2+)
  - .env.sepolia file with required environment variables
USAGE
}

ACTION="up"
ENV_FILE=".env.sepolia"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --down) ACTION="down"; shift ;;
        --status) ACTION="status"; shift ;;
        --env-file) ENV_FILE="$2"; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) err "Unknown option: $1"; usage; exit 1 ;;
    esac
done

cd "$ROOT_DIR" || exit 1

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------
if ! command -v docker &>/dev/null; then
    err "docker is required. Install Docker: https://docs.docker.com/get-docker/"
    exit 1
fi

COMPOSE_FILES="-f docker-compose.sepolia.yml -f docker-compose.monitoring.yml"

# ---------------------------------------------------------------------------
# Actions
# ---------------------------------------------------------------------------
case "$ACTION" in
    up)
        header "Starting Sepolia + Monitoring Stack"

        if [[ ! -f "$ENV_FILE" ]]; then
            err "Environment file not found: $ENV_FILE"
            err "Copy .env.sepolia.example to .env.sepolia and fill in values."
            exit 1
        fi

        if [[ ! -f "docker-compose.sepolia.yml" ]]; then
            err "docker-compose.sepolia.yml not found"
            exit 1
        fi

        if [[ ! -f "docker-compose.monitoring.yml" ]]; then
            err "docker-compose.monitoring.yml not found"
            exit 1
        fi

        info "Using env file: $ENV_FILE"
        info "Compose files: docker-compose.sepolia.yml + docker-compose.monitoring.yml"

        # shellcheck disable=SC2086
        docker compose $COMPOSE_FILES --env-file "$ENV_FILE" up -d

        echo ""
        header "============================================================"
        header "  Monitoring Stack Running"
        header "============================================================"
        echo ""
        printf "  ${BOLD}%-14s${RESET} %s\n" "Prometheus:" "http://localhost:9091"
        printf "  ${BOLD}%-14s${RESET} %s\n" "Grafana:" "http://localhost:3001  (admin/admin)"
        echo ""
        info "Grafana dashboards are auto-provisioned (operator, enclave, indexer, sepolia)."
        info "To stop: ./scripts/sepolia-monitoring.sh --down"
        echo ""
        ok "Monitoring stack started."
        ;;

    down)
        header "Stopping Sepolia + Monitoring Stack"
        # shellcheck disable=SC2086
        docker compose $COMPOSE_FILES --env-file "$ENV_FILE" down 2>/dev/null
        ok "All services stopped."
        ;;

    status)
        header "Container Status"
        # shellcheck disable=SC2086
        docker compose $COMPOSE_FILES --env-file "$ENV_FILE" ps 2>/dev/null || \
            warn "No containers running or compose files not found."
        ;;
esac
