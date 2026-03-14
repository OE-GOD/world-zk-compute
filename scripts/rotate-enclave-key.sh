#!/usr/bin/env bash
# Rotate an enclave signing key on TEEMLVerifier.
# Registers a new enclave key and revokes the old one.
#
# Usage: ./scripts/rotate-enclave-key.sh
#
# Required env vars:
#   RPC_URL                 Ethereum RPC endpoint
#   ADMIN_PRIVATE_KEY       Contract owner private key
#   TEE_VERIFIER_ADDRESS    TEEMLVerifier contract address
#   NEW_ENCLAVE_ADDRESS     New enclave signing address to register
#   NEW_ENCLAVE_IMAGE_HASH  Image hash for the new enclave
#   OLD_ENCLAVE_ADDRESS     Old enclave address to revoke
#
# Options:
#   --help, -h    Show this help message
#   --dry-run     Print commands without executing

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -16 "$0" | tail -15
    exit 0
fi

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
fi

if ! command -v cast &>/dev/null; then
    echo "ERROR: 'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

: "${RPC_URL:?Set RPC_URL}"
: "${ADMIN_PRIVATE_KEY:?Set ADMIN_PRIVATE_KEY}"
: "${TEE_VERIFIER_ADDRESS:?Set TEE_VERIFIER_ADDRESS}"
: "${NEW_ENCLAVE_ADDRESS:?Set NEW_ENCLAVE_ADDRESS}"
: "${NEW_ENCLAVE_IMAGE_HASH:?Set NEW_ENCLAVE_IMAGE_HASH}"
: "${OLD_ENCLAVE_ADDRESS:?Set OLD_ENCLAVE_ADDRESS}"

echo "=== Enclave Key Rotation ==="
echo "Contract:    $TEE_VERIFIER_ADDRESS"
echo "Old enclave: $OLD_ENCLAVE_ADDRESS"
echo "New enclave: $NEW_ENCLAVE_ADDRESS"
echo ""

run_cmd() {
    if [ "$DRY_RUN" = true ]; then
        echo "[DRY-RUN] $*"
    else
        "$@"
    fi
}

echo "Step 1: Register new enclave..."
run_cmd cast send "$TEE_VERIFIER_ADDRESS" \
    "registerEnclave(address,bytes32)" \
    "$NEW_ENCLAVE_ADDRESS" "$NEW_ENCLAVE_IMAGE_HASH" \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_PRIVATE_KEY"

echo ""
echo "Step 2: Verify new enclave is registered..."
if [ "$DRY_RUN" = false ]; then
    REGISTERED=$(cast call "$TEE_VERIFIER_ADDRESS" "enclaves(address)(bool,bool,bytes32)" "$NEW_ENCLAVE_ADDRESS" --rpc-url "$RPC_URL" 2>/dev/null || echo "FAILED")
    echo "  Result: $REGISTERED"
fi

echo ""
echo "Step 3: Revoke old enclave..."
run_cmd cast send "$TEE_VERIFIER_ADDRESS" \
    "revokeEnclave(address)" \
    "$OLD_ENCLAVE_ADDRESS" \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_PRIVATE_KEY"

echo ""
echo "Key rotation complete."
echo "  - New enclave $NEW_ENCLAVE_ADDRESS is active"
echo "  - Old enclave $OLD_ENCLAVE_ADDRESS is revoked"
echo ""
echo "IMPORTANT: Update ENCLAVE_PRIVATE_KEY in your .env file and restart services."
