#!/usr/bin/env bash
# =============================================================================
# Docker Cleanup Script for World ZK Compute
#
# Removes Docker images, volumes, and networks associated with the
# world-zk-compute project. Stops running compose stacks first.
#
# Usage:
#   ./scripts/cleanup-docker.sh              # clean everything (default --all)
#   ./scripts/cleanup-docker.sh --images     # only images
#   ./scripts/cleanup-docker.sh --volumes    # only volumes
#   ./scripts/cleanup-docker.sh --networks   # only networks
#   ./scripts/cleanup-docker.sh --dry-run    # preview what would be removed
#   ./scripts/cleanup-docker.sh --yes        # skip confirmation prompt
#   ./scripts/cleanup-docker.sh --help
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
ok()      { printf "${GREEN}[OK]${RESET}    %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Default flags
# ---------------------------------------------------------------------------
CLEAN_IMAGES=false
CLEAN_VOLUMES=false
CLEAN_NETWORKS=false
DRY_RUN=false
YES=false
EXPLICIT_SCOPE=false

# ---------------------------------------------------------------------------
# Name patterns for filtering project resources
# ---------------------------------------------------------------------------
IMAGE_PATTERNS=("world-zk" "worldzk")
VOLUME_PATTERNS=("world-zk" "worldzk")
NETWORK_PATTERNS=("world-zk" "worldzk")

# ---------------------------------------------------------------------------
# Compose files (relative to PROJECT_ROOT)
# ---------------------------------------------------------------------------
COMPOSE_FILES=(
    "docker-compose.yml"
    "docker-compose.test.yml"
    "docker-compose.sepolia.yml"
    "docker-compose.loadtest.yml"
    "docker-compose.monitoring.yml"
    "docker-compose.gpu.yml"
    "prover/docker-compose.yml"
    "tee/docker-compose.yml"
)

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
cleanup-docker.sh -- Remove Docker resources for World ZK Compute.

USAGE:
  scripts/cleanup-docker.sh [OPTIONS]

OPTIONS:
  --all          Remove images, volumes, and networks (default if no scope given)
  --images       Remove only project Docker images
  --volumes      Remove only project Docker volumes
  --networks     Remove only project Docker networks
  --dry-run      Show what would be removed without deleting anything
  --yes, -y      Skip the confirmation prompt
  --help, -h     Show this help message

FILTERS:
  Resources are matched by name containing "world-zk" or "worldzk".

EXAMPLES:
  ./scripts/cleanup-docker.sh                    # remove everything (with confirmation)
  ./scripts/cleanup-docker.sh --dry-run          # preview only
  ./scripts/cleanup-docker.sh --images --yes     # remove images without asking
  ./scripts/cleanup-docker.sh --volumes --networks  # remove volumes and networks
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --all)
            CLEAN_IMAGES=true
            CLEAN_VOLUMES=true
            CLEAN_NETWORKS=true
            EXPLICIT_SCOPE=true
            shift
            ;;
        --images)
            CLEAN_IMAGES=true
            EXPLICIT_SCOPE=true
            shift
            ;;
        --volumes)
            CLEAN_VOLUMES=true
            EXPLICIT_SCOPE=true
            shift
            ;;
        --networks)
            CLEAN_NETWORKS=true
            EXPLICIT_SCOPE=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --yes|-y)
            YES=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# Default to --all when no scope flags were given
if [[ "$EXPLICIT_SCOPE" == "false" ]]; then
    CLEAN_IMAGES=true
    CLEAN_VOLUMES=true
    CLEAN_NETWORKS=true
fi

# ---------------------------------------------------------------------------
# Prerequisite check
# ---------------------------------------------------------------------------
if ! command -v docker &>/dev/null; then
    err "docker is not installed or not in PATH."
    exit 1
fi

# ---------------------------------------------------------------------------
# Helper: match a name against project patterns
# ---------------------------------------------------------------------------
matches_project() {
    local name="$1"
    shift
    for pattern in "$@"; do
        if [[ "$name" == *"$pattern"* ]]; then
            return 0
        fi
    done
    return 1
}

# ---------------------------------------------------------------------------
# Helper: format bytes to human-readable size
# ---------------------------------------------------------------------------
human_size() {
    local bytes="$1"
    if [[ "$bytes" -ge 1073741824 ]]; then
        awk "BEGIN {printf \"%.1f GB\", $bytes / 1073741824}"
    elif [[ "$bytes" -ge 1048576 ]]; then
        awk "BEGIN {printf \"%.1f MB\", $bytes / 1048576}"
    elif [[ "$bytes" -ge 1024 ]]; then
        awk "BEGIN {printf \"%.1f KB\", $bytes / 1024}"
    else
        printf "%d B" "$bytes"
    fi
}

# ---------------------------------------------------------------------------
# Helper: parse image size string to bytes (approximate)
# ---------------------------------------------------------------------------
parse_size_bytes() {
    local size_str="$1"
    local number unit
    number=$(echo "$size_str" | sed 's/[^0-9.]//g')
    unit=$(echo "$size_str" | sed 's/[0-9.]//g')

    case "$unit" in
        GB|gb) awk "BEGIN {printf \"%.0f\", $number * 1073741824}" ;;
        MB|mb) awk "BEGIN {printf \"%.0f\", $number * 1048576}" ;;
        KB|kb|kB) awk "BEGIN {printf \"%.0f\", $number * 1024}" ;;
        B|b)   awk "BEGIN {printf \"%.0f\", $number}" ;;
        *)     echo "0" ;;
    esac
}

# ---------------------------------------------------------------------------
# Discover resources
# ---------------------------------------------------------------------------
discover_images() {
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        local repo
        repo=$(echo "$line" | awk -F'\t' '{print $1}')
        if matches_project "$repo" "${IMAGE_PATTERNS[@]}"; then
            echo "$line"
        fi
    done < <(docker images --format "{{.Repository}}\t{{.Tag}}\t{{.ID}}\t{{.Size}}" 2>/dev/null || true)
}

discover_volumes() {
    while IFS= read -r name; do
        [[ -z "$name" ]] && continue
        if matches_project "$name" "${VOLUME_PATTERNS[@]}"; then
            echo "$name"
        fi
    done < <(docker volume ls --format "{{.Name}}" 2>/dev/null || true)
}

discover_networks() {
    while IFS= read -r name; do
        [[ -z "$name" ]] && continue
        # Skip default Docker networks
        case "$name" in
            bridge|host|none) continue ;;
        esac
        if matches_project "$name" "${NETWORK_PATTERNS[@]}"; then
            echo "$name"
        fi
    done < <(docker network ls --format "{{.Name}}" 2>/dev/null || true)
}

# ---------------------------------------------------------------------------
# Print header
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- Docker Cleanup"
header "============================================================"
echo ""

scope_parts=()
[[ "$CLEAN_IMAGES"   == "true" ]] && scope_parts+=("images")
[[ "$CLEAN_VOLUMES"  == "true" ]] && scope_parts+=("volumes")
[[ "$CLEAN_NETWORKS" == "true" ]] && scope_parts+=("networks")
scope_display=$(IFS=", "; echo "${scope_parts[*]}")

info "Scope:        $scope_display"
info "Project root: $PROJECT_ROOT"
if [[ "$DRY_RUN" == "true" ]]; then
    warn "DRY RUN mode -- nothing will be removed"
fi
echo ""

# ---------------------------------------------------------------------------
# Step 1: Discover what will be cleaned
# ---------------------------------------------------------------------------
header "Scanning for project Docker resources..."

IMAGE_LIST=""
VOLUME_LIST=""
NETWORK_LIST=""
IMAGE_COUNT=0
VOLUME_COUNT=0
NETWORK_COUNT=0
TOTAL_IMAGE_BYTES=0

if [[ "$CLEAN_IMAGES" == "true" ]]; then
    IMAGE_LIST=$(discover_images)
    if [[ -n "$IMAGE_LIST" ]]; then
        IMAGE_COUNT=$(echo "$IMAGE_LIST" | wc -l | tr -d ' ')
        while IFS= read -r line; do
            size_str=$(echo "$line" | awk -F'\t' '{print $4}')
            bytes=$(parse_size_bytes "$size_str")
            TOTAL_IMAGE_BYTES=$((TOTAL_IMAGE_BYTES + bytes))
        done <<< "$IMAGE_LIST"
    fi
fi

if [[ "$CLEAN_VOLUMES" == "true" ]]; then
    VOLUME_LIST=$(discover_volumes)
    if [[ -n "$VOLUME_LIST" ]]; then
        VOLUME_COUNT=$(echo "$VOLUME_LIST" | wc -l | tr -d ' ')
    fi
fi

if [[ "$CLEAN_NETWORKS" == "true" ]]; then
    NETWORK_LIST=$(discover_networks)
    if [[ -n "$NETWORK_LIST" ]]; then
        NETWORK_COUNT=$(echo "$NETWORK_LIST" | wc -l | tr -d ' ')
    fi
fi

# ---------------------------------------------------------------------------
# Step 2: Display what will be removed
# ---------------------------------------------------------------------------
if [[ "$CLEAN_IMAGES" == "true" ]]; then
    header "Images ($IMAGE_COUNT found)"
    if [[ -n "$IMAGE_LIST" ]]; then
        printf "  ${BOLD}%-40s %-10s %-14s %s${RESET}\n" "REPOSITORY" "TAG" "IMAGE ID" "SIZE"
        while IFS= read -r line; do
            repo=$(echo "$line" | awk -F'\t' '{print $1}')
            tag=$(echo "$line" | awk -F'\t' '{print $2}')
            id=$(echo "$line" | awk -F'\t' '{print $3}')
            size=$(echo "$line" | awk -F'\t' '{print $4}')
            printf "  %-40s %-10s %-14s %s\n" "$repo" "$tag" "$id" "$size"
        done <<< "$IMAGE_LIST"
        info "Approximate image space: $(human_size "$TOTAL_IMAGE_BYTES")"
    else
        info "No matching images found."
    fi
fi

if [[ "$CLEAN_VOLUMES" == "true" ]]; then
    header "Volumes ($VOLUME_COUNT found)"
    if [[ -n "$VOLUME_LIST" ]]; then
        while IFS= read -r name; do
            printf "  %s\n" "$name"
        done <<< "$VOLUME_LIST"
    else
        info "No matching volumes found."
    fi
fi

if [[ "$CLEAN_NETWORKS" == "true" ]]; then
    header "Networks ($NETWORK_COUNT found)"
    if [[ -n "$NETWORK_LIST" ]]; then
        while IFS= read -r name; do
            printf "  %s\n" "$name"
        done <<< "$NETWORK_LIST"
    else
        info "No matching networks found."
    fi
fi

# ---------------------------------------------------------------------------
# Early exit if nothing to do
# ---------------------------------------------------------------------------
TOTAL_RESOURCES=$((IMAGE_COUNT + VOLUME_COUNT + NETWORK_COUNT))

if [[ "$TOTAL_RESOURCES" -eq 0 ]]; then
    echo ""
    ok "Nothing to clean up. No matching Docker resources found."
    exit 0
fi

# ---------------------------------------------------------------------------
# Step 3: Confirmation prompt
# ---------------------------------------------------------------------------
echo ""
if [[ "$DRY_RUN" == "true" ]]; then
    ok "Dry run complete. $TOTAL_RESOURCES resource(s) would be removed."
    exit 0
fi

if [[ "$YES" == "false" ]]; then
    printf "${YELLOW}Remove %d resource(s)? [y/N]${RESET} " "$TOTAL_RESOURCES"
    read -r answer
    case "$answer" in
        [yY]|[yY][eE][sS])
            ;;
        *)
            info "Aborted."
            exit 0
            ;;
    esac
fi

# ---------------------------------------------------------------------------
# Step 4: Stop running compose stacks
# ---------------------------------------------------------------------------
header "Stopping compose stacks..."

COMPOSE_STOPPED=0
for compose_file in "${COMPOSE_FILES[@]}"; do
    full_path="$PROJECT_ROOT/$compose_file"
    if [[ -f "$full_path" ]]; then
        info "Stopping stack: $compose_file"
        if docker compose -f "$full_path" down --remove-orphans 2>/dev/null; then
            ok "Stopped: $compose_file"
            COMPOSE_STOPPED=$((COMPOSE_STOPPED + 1))
        else
            # Compose stack may not be running; that is fine
            info "Stack not running or already stopped: $compose_file"
        fi
    fi
done

if [[ "$COMPOSE_STOPPED" -eq 0 ]]; then
    info "No compose stacks were running."
fi

# ---------------------------------------------------------------------------
# Step 5: Remove images
# ---------------------------------------------------------------------------
IMAGES_REMOVED=0
IMAGES_FAILED=0

if [[ "$CLEAN_IMAGES" == "true" && -n "$IMAGE_LIST" ]]; then
    header "Removing images..."
    while IFS= read -r line; do
        id=$(echo "$line" | awk -F'\t' '{print $3}')
        repo=$(echo "$line" | awk -F'\t' '{print $1}')
        tag=$(echo "$line" | awk -F'\t' '{print $2}')
        display="${repo}:${tag}"

        if docker rmi -f "$id" &>/dev/null; then
            ok "Removed image: $display ($id)"
            IMAGES_REMOVED=$((IMAGES_REMOVED + 1))
        else
            warn "Failed to remove image: $display ($id)"
            IMAGES_FAILED=$((IMAGES_FAILED + 1))
        fi
    done <<< "$IMAGE_LIST"
fi

# ---------------------------------------------------------------------------
# Step 6: Remove volumes
# ---------------------------------------------------------------------------
VOLUMES_REMOVED=0
VOLUMES_FAILED=0

if [[ "$CLEAN_VOLUMES" == "true" && -n "$VOLUME_LIST" ]]; then
    header "Removing volumes..."
    while IFS= read -r name; do
        if docker volume rm "$name" &>/dev/null; then
            ok "Removed volume: $name"
            VOLUMES_REMOVED=$((VOLUMES_REMOVED + 1))
        else
            warn "Failed to remove volume: $name (may be in use)"
            VOLUMES_FAILED=$((VOLUMES_FAILED + 1))
        fi
    done <<< "$VOLUME_LIST"
fi

# ---------------------------------------------------------------------------
# Step 7: Remove networks
# ---------------------------------------------------------------------------
NETWORKS_REMOVED=0
NETWORKS_FAILED=0

if [[ "$CLEAN_NETWORKS" == "true" && -n "$NETWORK_LIST" ]]; then
    header "Removing networks..."
    while IFS= read -r name; do
        if docker network rm "$name" &>/dev/null; then
            ok "Removed network: $name"
            NETWORKS_REMOVED=$((NETWORKS_REMOVED + 1))
        else
            warn "Failed to remove network: $name (may be in use)"
            NETWORKS_FAILED=$((NETWORKS_FAILED + 1))
        fi
    done <<< "$NETWORK_LIST"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  CLEANUP SUMMARY"
header "============================================================"
echo ""

TOTAL_REMOVED=$((IMAGES_REMOVED + VOLUMES_REMOVED + NETWORKS_REMOVED))
TOTAL_FAILED=$((IMAGES_FAILED + VOLUMES_FAILED + NETWORKS_FAILED))

if [[ "$CLEAN_IMAGES" == "true" ]]; then
    if [[ "$IMAGES_REMOVED" -gt 0 ]]; then
        ok "Images removed:   $IMAGES_REMOVED (approx. $(human_size "$TOTAL_IMAGE_BYTES") reclaimed)"
    else
        info "Images removed:   0"
    fi
    if [[ "$IMAGES_FAILED" -gt 0 ]]; then
        warn "Images failed:    $IMAGES_FAILED"
    fi
fi

if [[ "$CLEAN_VOLUMES" == "true" ]]; then
    if [[ "$VOLUMES_REMOVED" -gt 0 ]]; then
        ok "Volumes removed:  $VOLUMES_REMOVED"
    else
        info "Volumes removed:  0"
    fi
    if [[ "$VOLUMES_FAILED" -gt 0 ]]; then
        warn "Volumes failed:   $VOLUMES_FAILED"
    fi
fi

if [[ "$CLEAN_NETWORKS" == "true" ]]; then
    if [[ "$NETWORKS_REMOVED" -gt 0 ]]; then
        ok "Networks removed: $NETWORKS_REMOVED"
    else
        info "Networks removed: 0"
    fi
    if [[ "$NETWORKS_FAILED" -gt 0 ]]; then
        warn "Networks failed:  $NETWORKS_FAILED"
    fi
fi

echo ""

if [[ "$TOTAL_FAILED" -gt 0 ]]; then
    warn "Cleanup completed with $TOTAL_FAILED failure(s). Some resources may still be in use."
    warn "Stop containers using them first, then re-run this script."
    exit 1
else
    ok "Cleanup complete. $TOTAL_REMOVED resource(s) removed."
fi
