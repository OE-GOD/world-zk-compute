#!/usr/bin/env bash
set -euo pipefail

# Restore World ZK Compute data from backup
#
# Usage: ./scripts/restore.sh <backup-dir>

BACKUP_DIR="${1:?Usage: ./scripts/restore.sh <backup-dir>}"

if [ ! -d "$BACKUP_DIR" ]; then
    echo "Error: backup directory not found: $BACKUP_DIR"
    exit 1
fi

echo "=== World ZK Compute Restore ==="
echo "From: $BACKUP_DIR"

if [ -f "$BACKUP_DIR/manifest.json" ]; then
    echo "Backup timestamp: $(python3 -c "import json; print(json.load(open('$BACKUP_DIR/manifest.json'))['timestamp'])")"
fi
echo ""

# Stop services first
echo "Stopping services..."
docker compose -f docker-compose.offchain.yml down 2>/dev/null || true

# Restore databases
for db in proofs.db transparency.db; do
    if [ -f "$BACKUP_DIR/$db" ]; then
        cp "$BACKUP_DIR/$db" "./$db"
        echo "  Restored: $db"
    fi
done

# Restore proof archive
if [ -f "$BACKUP_DIR/proof-store.tar.gz" ]; then
    tar xzf "$BACKUP_DIR/proof-store.tar.gz" -C .
    echo "  Restored: proof-store/"
fi

# Restore configs
for f in CONTRACT_ADDRESSES.md .env .env.bank-demo .env.production; do
    if [ -f "$BACKUP_DIR/$f" ]; then
        target=$([ "$f" = "CONTRACT_ADDRESSES.md" ] && echo "docs/$f" || echo "$f")
        cp "$BACKUP_DIR/$f" "$target"
        echo "  Restored: $f"
    fi
done

echo ""
echo "Restore complete. Restart services with:"
echo "  docker compose -f docker-compose.offchain.yml up -d"
