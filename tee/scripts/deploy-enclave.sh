#!/usr/bin/env bash
set -euo pipefail

# Deploy the enclave to AWS Nitro (requires nitro-cli on an enclave-enabled EC2 instance)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENCLAVE_DIR="$(cd "$SCRIPT_DIR/../enclave" && pwd)"
EIF_PATH="${1:-$ENCLAVE_DIR/enclave.eif}"

CPU_COUNT="${CPU_COUNT:-2}"
MEMORY_MB="${MEMORY_MB:-512}"

if ! command -v nitro-cli &>/dev/null; then
    echo "Error: nitro-cli not found. Run on an enclave-enabled EC2 instance."
    exit 1
fi

if [ ! -f "$EIF_PATH" ]; then
    echo "Error: Enclave image not found at $EIF_PATH"
    echo "Run build-enclave.sh first."
    exit 1
fi

echo "==> Starting Nitro Enclave..."
echo "    EIF:    $EIF_PATH"
echo "    CPUs:   $CPU_COUNT"
echo "    Memory: ${MEMORY_MB}MB"

ENCLAVE_ID=$(nitro-cli run-enclave \
    --eif-path "$EIF_PATH" \
    --cpu-count "$CPU_COUNT" \
    --memory "$MEMORY_MB" \
    | jq -r '.EnclaveID')

echo "  ✓ Enclave running: $ENCLAVE_ID"
echo ""
echo "To check status:  nitro-cli describe-enclaves"
echo "To terminate:     nitro-cli terminate-enclave --enclave-id $ENCLAVE_ID"
