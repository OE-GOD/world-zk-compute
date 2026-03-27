#!/usr/bin/env bash
set -euo pipefail

# Deploy the World ZK Compute off-chain stack to fly.io
# Prerequisites: flyctl installed and authenticated (flyctl auth login)
# Usage: ./scripts/deploy-fly.sh [--prefix worldzk] [--dry-run]

PREFIX="worldzk"
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) PREFIX="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

echo "=== Deploying World ZK Compute to fly.io ==="
echo "Prefix: $PREFIX"
echo ""

# Check flyctl
if ! command -v flyctl &>/dev/null; then
    echo "ERROR: flyctl not installed. Install: curl -L https://fly.io/install.sh | sh"
    exit 1
fi

if ! flyctl auth whoami &>/dev/null; then
    echo "ERROR: Not authenticated. Run: flyctl auth login"
    exit 1
fi

SERVICES=("verifier" "proof-registry" "proof-generator" "gateway")
DIRS=("services/verifier" "services/proof-registry" "services/proof-generator" "services/gateway")

for i in "${!SERVICES[@]}"; do
    SVC="${SERVICES[$i]}"
    DIR="${DIRS[$i]}"
    APP="${PREFIX}-${SVC}"

    echo "--- Deploying $SVC ($APP) ---"

    if [ "$DRY_RUN" = true ]; then
        echo "  [DRY RUN] Would deploy $DIR to $APP"
        continue
    fi

    # Create app if it doesn't exist
    if ! flyctl apps list | grep -q "$APP"; then
        echo "  Creating app $APP..."
        flyctl apps create "$APP" --org personal 2>/dev/null || true
    fi

    # Deploy
    (cd "$DIR" && flyctl deploy --app "$APP" --remote-only) || {
        echo "  WARNING: Failed to deploy $SVC. Continuing..."
    }

    echo "  Deployed: https://${APP}.fly.dev"
    echo ""
done

echo "=== Deployment Complete ==="
echo ""
echo "Endpoints:"
echo "  Gateway:   https://${PREFIX}-gateway.fly.dev"
echo "  Verifier:  https://${PREFIX}-verifier.fly.dev"
echo "  Registry:  https://${PREFIX}-proof-registry.fly.dev"
echo "  Generator: https://${PREFIX}-proof-generator.fly.dev"
echo ""
echo "Test: curl https://${PREFIX}-gateway.fly.dev/health"
