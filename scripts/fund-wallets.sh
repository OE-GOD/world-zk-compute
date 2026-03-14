#!/usr/bin/env bash
# Batch fund multiple wallets from a deployer account.
# Useful for setting up Sepolia test wallets.
#
# Usage: ./scripts/fund-wallets.sh [amount_eth]
#
# Arguments:
#   amount_eth   Amount of ETH to send to each wallet (default: 0.01)
#
# Required env vars:
#   RPC_URL                 Ethereum RPC endpoint
#   DEPLOYER_PRIVATE_KEY    Funder private key
#
# Optional env vars (provide addresses or private keys to derive them):
#   ENCLAVE_PRIVATE_KEY / ENCLAVE_ADDRESS
#   OPERATOR_PRIVATE_KEY / OPERATOR_ADDRESS
#   REQUESTER_PRIVATE_KEY / REQUESTER_ADDRESS
#
# Options:
#   --help, -h    Show this help message

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -20 "$0" | tail -19
    exit 0
fi

if ! command -v cast &>/dev/null; then
    echo "ERROR: 'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

: "${RPC_URL:?Set RPC_URL}"
: "${DEPLOYER_PRIVATE_KEY:?Set DEPLOYER_PRIVATE_KEY}"

AMOUNT="${1:-0.01}"
AMOUNT_WEI=$(cast to-wei "$AMOUNT" 2>/dev/null || echo "")
if [ -z "$AMOUNT_WEI" ]; then
    echo "ERROR: Invalid amount: $AMOUNT"
    exit 1
fi

derive_address() {
    cast wallet address --private-key "$1" 2>/dev/null || echo ""
}

DEPLOYER_ADDR=$(derive_address "$DEPLOYER_PRIVATE_KEY")
echo "=== Batch Wallet Funding ==="
echo "Funder:  $DEPLOYER_ADDR"
echo "Amount:  $AMOUNT ETH per wallet"
echo ""

TARGETS=()

if [ -n "${ENCLAVE_ADDRESS:-}" ]; then
    TARGETS+=("Enclave:$ENCLAVE_ADDRESS")
elif [ -n "${ENCLAVE_PRIVATE_KEY:-}" ]; then
    ADDR=$(derive_address "$ENCLAVE_PRIVATE_KEY")
    [ -n "$ADDR" ] && TARGETS+=("Enclave:$ADDR")
fi

if [ -n "${OPERATOR_ADDRESS:-}" ]; then
    TARGETS+=("Operator:$OPERATOR_ADDRESS")
elif [ -n "${OPERATOR_PRIVATE_KEY:-}" ]; then
    ADDR=$(derive_address "$OPERATOR_PRIVATE_KEY")
    [ -n "$ADDR" ] && TARGETS+=("Operator:$ADDR")
fi

if [ -n "${REQUESTER_ADDRESS:-}" ]; then
    TARGETS+=("Requester:$REQUESTER_ADDRESS")
elif [ -n "${REQUESTER_PRIVATE_KEY:-}" ]; then
    ADDR=$(derive_address "$REQUESTER_PRIVATE_KEY")
    [ -n "$ADDR" ] && TARGETS+=("Requester:$ADDR")
fi

if [ ${#TARGETS[@]} -eq 0 ]; then
    echo "No target wallets configured. Set at least one of:"
    echo "  ENCLAVE_ADDRESS, OPERATOR_ADDRESS, REQUESTER_ADDRESS"
    echo "  or their corresponding _PRIVATE_KEY env vars."
    exit 1
fi

FUNDED=0
FAILED=0

for entry in "${TARGETS[@]}"; do
    ROLE="${entry%%:*}"
    ADDR="${entry##*:}"
    printf "  Funding %-12s (%s)... " "$ROLE" "$ADDR"
    if cast send "$ADDR" --value "${AMOUNT_WEI}" \
        --rpc-url "$RPC_URL" \
        --private-key "$DEPLOYER_PRIVATE_KEY" \
        >/dev/null 2>&1; then
        BALANCE=$(cast balance "$ADDR" --ether --rpc-url "$RPC_URL" 2>/dev/null || echo "?")
        echo "OK (balance: ${BALANCE} ETH)"
        FUNDED=$((FUNDED + 1))
    else
        echo "FAILED"
        FAILED=$((FAILED + 1))
    fi
done

echo ""
echo "Done: $FUNDED funded, $FAILED failed."
