#!/usr/bin/env bash
# Backup deployment state (contract addresses, config, chain state).
# Usage: ./scripts/backup-deployment.sh [backup-dir]
#
# Arguments:
#   backup-dir   Output directory (default: ./backups/YYYY-MM-DD)
#
# Options:
#   --help, -h    Show this help message

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -10 "$0" | tail -9
    exit 0
fi

DATE=$(date +%Y-%m-%d_%H%M%S)
BACKUP_DIR="${1:-./backups/$DATE}"

mkdir -p "$BACKUP_DIR"
echo "=== Deployment Backup ==="
echo "Output: $BACKUP_DIR"
echo ""

BACKED=0

backup_file() {
    local src="$1"
    local label="$2"
    if [ -f "$src" ]; then
        cp "$src" "$BACKUP_DIR/"
        echo "  [OK] $label: $src"
        BACKED=$((BACKED + 1))
    else
        echo "  [--] $label: not found ($src)"
    fi
}

# Deployment artifacts
backup_file "deployments/registry.json" "Deployment registry"
backup_file "deployments/chains.json" "Chain config"

# Sepolia deployment
for f in deployments/*.json; do
    [ -f "$f" ] && [ "$f" != "deployments/registry.json" ] && [ "$f" != "deployments/chains.json" ] && backup_file "$f" "Deployment"
done

# Environment files (exclude secrets)
for env_file in .env .env.sepolia; do
    if [ -f "$env_file" ]; then
        # Strip private keys from backup
        grep -v 'PRIVATE_KEY' "$env_file" > "$BACKUP_DIR/$(basename "$env_file").sanitized" 2>/dev/null || true
        echo "  [OK] $env_file (sanitized, keys stripped)"
        BACKED=$((BACKED + 1))
    fi
done

# Docker state
if command -v docker &>/dev/null; then
    docker compose ps --format json > "$BACKUP_DIR/docker-compose-state.json" 2>/dev/null || true
    echo "  [OK] Docker Compose state"
    BACKED=$((BACKED + 1))
fi

# Git state
if command -v git &>/dev/null; then
    git rev-parse HEAD > "$BACKUP_DIR/git-commit.txt" 2>/dev/null || true
    git diff --stat > "$BACKUP_DIR/git-diff-stat.txt" 2>/dev/null || true
    echo "  [OK] Git state"
    BACKED=$((BACKED + 1))
fi

echo ""
echo "Backup complete: $BACKED items saved to $BACKUP_DIR"
