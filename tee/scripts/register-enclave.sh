#!/bin/bash
# Register an enclave's ECDSA key on-chain with the TEEMLVerifier contract.
#
# Usage:
#   register-enclave.sh --enclave-url <URL> --verifier <CONTRACT_ADDR> --rpc-url <RPC> --private-key <KEY>
#
# The script:
# 1. Queries the enclave /info endpoint for address and model hash
# 2. Calls TEEMLVerifier.registerEnclave(address, bytes32) on-chain
set -euo pipefail

# Parse arguments
ENCLAVE_URL=""
VERIFIER_ADDR=""
RPC_URL="http://localhost:8545"
PRIVATE_KEY=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --enclave-url)  ENCLAVE_URL="$2"; shift 2 ;;
        --verifier)     VERIFIER_ADDR="$2"; shift 2 ;;
        --rpc-url)      RPC_URL="$2"; shift 2 ;;
        --private-key)  PRIVATE_KEY="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

if [ -z "$ENCLAVE_URL" ] || [ -z "$VERIFIER_ADDR" ] || [ -z "$PRIVATE_KEY" ]; then
    echo "Usage: register-enclave.sh --enclave-url <URL> --verifier <ADDR> --rpc-url <RPC> --private-key <KEY>"
    exit 1
fi

echo "Querying enclave at ${ENCLAVE_URL}/info ..."
INFO=$(curl -sf "${ENCLAVE_URL}/info")
ENCLAVE_ADDR=$(echo "$INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['enclave_address'])")
MODEL_HASH=$(echo "$INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['model_hash'])")

echo "Enclave address: ${ENCLAVE_ADDR}"
echo "Model hash:      ${MODEL_HASH}"

echo ""
echo "Registering enclave on TEEMLVerifier at ${VERIFIER_ADDR} ..."
cast send \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    "$VERIFIER_ADDR" \
    "registerEnclave(address,bytes32)" \
    "$ENCLAVE_ADDR" \
    "$MODEL_HASH"

echo ""
echo "Enclave registered successfully."
