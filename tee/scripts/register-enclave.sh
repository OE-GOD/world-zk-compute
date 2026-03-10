#!/bin/bash
# Register an enclave on-chain with the TEEMLVerifier contract.
#
# Supports two modes:
#   1. Attestation mode (--use-attestation): Fetch /attestation, verify, register with PCR0
#   2. Legacy mode (default): Fetch /info, register with model_hash
#
# Usage:
#   register-enclave.sh --enclave-url <URL> --verifier <CONTRACT_ADDR> --rpc-url <RPC> --private-key <KEY> [--use-attestation] [--expected-pcr0 <PCR0>]
set -euo pipefail

# Parse arguments
ENCLAVE_URL=""
VERIFIER_ADDR=""
RPC_URL="http://localhost:8545"
PRIVATE_KEY=""
USE_ATTESTATION=false
EXPECTED_PCR0=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --enclave-url)      ENCLAVE_URL="$2"; shift 2 ;;
        --verifier)         VERIFIER_ADDR="$2"; shift 2 ;;
        --rpc-url)          RPC_URL="$2"; shift 2 ;;
        --private-key)      PRIVATE_KEY="$2"; shift 2 ;;
        --use-attestation)  USE_ATTESTATION=true; shift ;;
        --expected-pcr0)    EXPECTED_PCR0="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

if [ -z "$ENCLAVE_URL" ] || [ -z "$VERIFIER_ADDR" ] || [ -z "$PRIVATE_KEY" ]; then
    echo "Usage: register-enclave.sh --enclave-url <URL> --verifier <ADDR> --rpc-url <RPC> --private-key <KEY> [--use-attestation] [--expected-pcr0 <PCR0>]"
    exit 1
fi

if [ "$USE_ATTESTATION" = true ]; then
    echo "Fetching attestation from ${ENCLAVE_URL}/attestation ..."
    ATTESTATION=$(curl -sf "${ENCLAVE_URL}/attestation")

    ENCLAVE_ADDR=$(echo "$ATTESTATION" | python3 -c "import sys,json; print(json.load(sys.stdin)['enclave_address'])")
    IS_NITRO=$(echo "$ATTESTATION" | python3 -c "import sys,json; print(json.load(sys.stdin)['is_nitro'])")
    PCR0=$(echo "$ATTESTATION" | python3 -c "import sys,json; print(json.load(sys.stdin)['pcr0'])")

    echo "Enclave address: ${ENCLAVE_ADDR}"
    echo "Is Nitro:        ${IS_NITRO}"
    echo "PCR0:            ${PCR0}"

    # Validate PCR0 if expected value provided
    if [ -n "$EXPECTED_PCR0" ]; then
        EXPECTED_CLEAN=$(echo "$EXPECTED_PCR0" | sed 's/^0x//')
        if [ "$PCR0" != "$EXPECTED_CLEAN" ]; then
            echo "ERROR: PCR0 mismatch!"
            echo "  Expected: ${EXPECTED_CLEAN}"
            echo "  Got:      ${PCR0}"
            exit 1
        fi
        echo "PCR0 validation passed."
    fi

    # Use first 32 bytes of PCR0 (48 bytes total) as image hash
    IMAGE_HASH="0x${PCR0:0:64}"
else
    echo "Querying enclave at ${ENCLAVE_URL}/info ..."
    INFO=$(curl -sf "${ENCLAVE_URL}/info")
    ENCLAVE_ADDR=$(echo "$INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['enclave_address'])")
    IMAGE_HASH=$(echo "$INFO" | python3 -c "import sys,json; print(json.load(sys.stdin)['model_hash'])")

    echo "Enclave address: ${ENCLAVE_ADDR}"
    echo "Image hash:      ${IMAGE_HASH}"
fi

echo ""
echo "Registering enclave on TEEMLVerifier at ${VERIFIER_ADDR} ..."
cast send \
    --rpc-url "$RPC_URL" \
    --private-key "$PRIVATE_KEY" \
    "$VERIFIER_ADDR" \
    "registerEnclave(address,bytes32)" \
    "$ENCLAVE_ADDR" \
    "$IMAGE_HASH"

echo ""
echo "Enclave registered successfully."
echo "  Address:    ${ENCLAVE_ADDR}"
echo "  Image hash: ${IMAGE_HASH}"
