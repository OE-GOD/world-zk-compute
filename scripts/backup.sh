#!/usr/bin/env bash
set -euo pipefail

# Backup World ZK Compute data
#
# Usage: ./scripts/backup.sh [--output-dir /path/to/backups]
#
# Backs up:
#   - Proof registry database
#   - Proof archive files
#   - Service configuration
#   - Contract addresses

OUTPUT_DIR="${1:-./backups}"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
BACKUP_DIR="$OUTPUT_DIR/backup-$TIMESTAMP"

mkdir -p "$BACKUP_DIR"

echo "=== World ZK Compute Backup ==="
echo "Output: $BACKUP_DIR"
echo ""

# Proof registry DB
if [ -f "./proofs.db" ]; then
    cp ./proofs.db "$BACKUP_DIR/proofs.db"
    echo "  Backed up: proofs.db"
fi

# Transparency log DB
if [ -f "./transparency.db" ]; then
    cp ./transparency.db "$BACKUP_DIR/transparency.db"
    echo "  Backed up: transparency.db"
fi

# Proof archive
if [ -d "./proof-store" ]; then
    tar czf "$BACKUP_DIR/proof-store.tar.gz" -C . proof-store/
    echo "  Backed up: proof-store/ ($(du -sh proof-store/ | cut -f1))"
fi

# Contract addresses
if [ -f "docs/CONTRACT_ADDRESSES.md" ]; then
    cp docs/CONTRACT_ADDRESSES.md "$BACKUP_DIR/"
    echo "  Backed up: CONTRACT_ADDRESSES.md"
fi

# Environment configs
for env_file in .env .env.bank-demo .env.production; do
    if [ -f "$env_file" ]; then
        cp "$env_file" "$BACKUP_DIR/"
        echo "  Backed up: $env_file"
    fi
done

# Create manifest
cat > "$BACKUP_DIR/manifest.json" << MANIFEST
{
  "timestamp": "$TIMESTAMP",
  "hostname": "$(hostname)",
  "version": "0.1.0",
  "files": $(ls -1 "$BACKUP_DIR" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read().strip().split('\n')))")
}
MANIFEST

echo ""
echo "Backup complete: $BACKUP_DIR"
echo "Total size: $(du -sh "$BACKUP_DIR" | cut -f1)"
