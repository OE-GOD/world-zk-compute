#!/bin/sh
set -e

# Read contract addresses from deployer's shared volume (if available)
ADDRESSES_FILE="/deployment/addresses.json"
if [ -f "$ADDRESSES_FILE" ]; then
    echo "Reading deployment addresses from $ADDRESSES_FILE"
    TEE_ADDR=$(jq -r '.TEEMLVerifier // empty' "$ADDRESSES_FILE")
    if [ -n "$TEE_ADDR" ]; then
        export TEE_VERIFIER_ADDRESS="$TEE_ADDR"
        echo "  TEE_VERIFIER_ADDRESS=$TEE_ADDR"
    fi
fi

# Validate required env vars
: "${OPERATOR_PRIVATE_KEY:?Set OPERATOR_PRIVATE_KEY}"
: "${TEE_VERIFIER_ADDRESS:?Set TEE_VERIFIER_ADDRESS or mount /deployment/addresses.json}"

# Default RPC URL
export OPERATOR_RPC_URL="${OPERATOR_RPC_URL:-http://127.0.0.1:8545}"

echo "Starting tee-operator $*"
echo "  RPC:      $OPERATOR_RPC_URL"
echo "  Contract: $TEE_VERIFIER_ADDRESS"
echo "  Enclave:  ${ENCLAVE_URL:-http://127.0.0.1:8080}"

exec tee-operator "$@"
