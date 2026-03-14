#!/usr/bin/env bash
# Build all Docker images for the World ZK Compute stack.
#
# Usage:
#   ./scripts/docker-build-all.sh
#   ./scripts/docker-build-all.sh --no-cache
#   ./scripts/docker-build-all.sh --parallel
#   ./scripts/docker-build-all.sh --parallel --no-cache
#
# Images built:
#   world-zk-enclave:latest
#   world-zk-warm-prover:latest
#   world-zk-operator:latest
#   world-zk-private-input-server:latest
#
# Requires: docker

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Options
NO_CACHE=false
PARALLEL=false

# =============================================================================
# Helpers
# =============================================================================

show_help() {
    cat <<'EOF'
docker-build-all.sh -- Build all Docker images for World ZK Compute

USAGE:
  ./scripts/docker-build-all.sh [OPTIONS]

OPTIONS:
  --no-cache     Build without Docker cache
  --parallel     Build images in parallel (background)
  -h, --help     Show this help

IMAGES:
  world-zk-enclave:latest              TEE enclave service
  world-zk-warm-prover:latest          Warm prover (risc0 ZK)
  world-zk-operator:latest             Operator service
  world-zk-private-input-server:latest Private input server

REQUIRES:
  docker
EOF
}

log()  { echo "==> $*"; }
ok()   { echo "  [PASS] $*"; }
err()  { echo "  [FAIL] $*" >&2; }
info() { echo "  [INFO] $*"; }

# Build a single Docker image and report timing.
#   build_image <name> <dockerfile-path> <context-path>
build_image() {
    local name="$1"
    local dockerfile="$2"
    local context="$3"

    local cache_flag=""
    if [ "$NO_CACHE" = true ]; then
        cache_flag="--no-cache"
    fi

    log "Building ${name}..."
    local start_time
    start_time=$(date +%s)

    if docker build $cache_flag -t "${name}:latest" -f "$dockerfile" "$context"; then
        local end_time
        end_time=$(date +%s)
        local elapsed=$(( end_time - start_time ))
        ok "${name} (${elapsed}s)"
        return 0
    else
        local end_time
        end_time=$(date +%s)
        local elapsed=$(( end_time - start_time ))
        err "${name} (${elapsed}s)"
        return 1
    fi
}

# =============================================================================
# Parse arguments
# =============================================================================

while [[ $# -gt 0 ]]; do
    case $1 in
        --no-cache) NO_CACHE=true; shift ;;
        --parallel) PARALLEL=true; shift ;;
        --help|-h)  show_help; exit 0 ;;
        *)          echo "Unknown option: $1"; show_help; exit 1 ;;
    esac
done

# =============================================================================
# Pre-flight checks
# =============================================================================

if ! command -v docker &>/dev/null; then
    err "docker is not installed or not in PATH"
    exit 1
fi

echo "========================================"
echo "  World ZK Compute -- Docker Build"
echo "========================================"
echo "Root:      $ROOT_DIR"
echo "No-cache:  $NO_CACHE"
echo "Parallel:  $PARALLEL"
echo ""

OVERALL_START=$(date +%s)

# =============================================================================
# Image definitions: (tag, Dockerfile path, build context)
# =============================================================================

IMAGES=(
    "world-zk-enclave|${ROOT_DIR}/tee/Dockerfile|${ROOT_DIR}/tee"
    "world-zk-warm-prover|${ROOT_DIR}/prover/Dockerfile|${ROOT_DIR}/prover"
    "world-zk-operator|${ROOT_DIR}/services/operator/Dockerfile|${ROOT_DIR}/services/operator"
    "world-zk-private-input-server|${ROOT_DIR}/private-input-server/Dockerfile|${ROOT_DIR}/private-input-server"
)

FAILED=0

if [ "$PARALLEL" = true ]; then
    log "Building all images in parallel..."
    echo ""

    PIDS=()
    NAMES=()

    for entry in "${IMAGES[@]}"; do
        IFS='|' read -r name dockerfile context <<< "$entry"

        # Check Dockerfile exists
        if [ ! -f "$dockerfile" ]; then
            info "Skipping ${name} (Dockerfile not found: ${dockerfile})"
            continue
        fi

        build_image "$name" "$dockerfile" "$context" &
        PIDS+=($!)
        NAMES+=("$name")
    done

    # Wait for all background builds
    for i in "${!PIDS[@]}"; do
        if ! wait "${PIDS[$i]}"; then
            FAILED=$((FAILED + 1))
        fi
    done
else
    for entry in "${IMAGES[@]}"; do
        IFS='|' read -r name dockerfile context <<< "$entry"

        # Check Dockerfile exists
        if [ ! -f "$dockerfile" ]; then
            info "Skipping ${name} (Dockerfile not found: ${dockerfile})"
            continue
        fi

        if ! build_image "$name" "$dockerfile" "$context"; then
            FAILED=$((FAILED + 1))
        fi
        echo ""
    done
fi

# =============================================================================
# Summary
# =============================================================================

OVERALL_END=$(date +%s)
OVERALL_ELAPSED=$(( OVERALL_END - OVERALL_START ))

echo ""
echo "========================================"
echo "  Build Summary"
echo "========================================"
echo "Total time: ${OVERALL_ELAPSED}s"
echo ""

log "Built images:"
docker images --format "  {{.Repository}}:{{.Tag}}\t{{.Size}}" | grep "world-zk-" | sort

echo ""

if [ "$FAILED" -gt 0 ]; then
    err "${FAILED} image(s) failed to build"
    exit 1
else
    ok "All images built successfully"
    exit 0
fi
