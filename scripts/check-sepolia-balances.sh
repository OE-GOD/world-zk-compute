#!/usr/bin/env bash
# Check Sepolia wallet balances for all configured roles.
# Usage: ./scripts/check-sepolia-balances.sh
#
# Required env vars:
#   ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC endpoint
#
# Optional env vars (derive addresses from private keys):
#   DEPLOYER_PRIVATE_KEY
#   ENCLAVE_PRIVATE_KEY
#   OPERATOR_PRIVATE_KEY
#   REQUESTER_PRIVATE_KEY
#
# Or provide addresses directly:
#   DEPLOYER_ADDRESS, ENCLAVE_ADDRESS, OPERATOR_ADDRESS, REQUESTER_ADDRESS
#
# Options:
#   --help, -h    Show this help message

set -euo pipefail

# Color helpers (respects NO_COLOR)
if [ -z "${NO_COLOR:-}" ] && [ -t 1 ]; then
    GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; BOLD='\033[1m'; RESET='\033[0m'
else
    GREEN=''; YELLOW=''; RED=''; BOLD=''; RESET=''
fi

ok()   { printf "  ${GREEN}[OK]${RESET}   %s\n" "$1"; }
warn() { printf "  ${YELLOW}[LOW]${RESET}  %s\n" "$1"; }
err()  { printf "  ${RED}[EMPTY]${RESET} %s\n" "$1"; }

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -17 "$0" | tail -16
    exit 0
fi

# Validate prerequisites
if ! command -v cast &>/dev/null; then
    echo "ERROR: 'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

RPC_URL="${ALCHEMY_SEPOLIA_RPC_URL:-}"
if [ -z "$RPC_URL" ]; then
    echo "ERROR: ALCHEMY_SEPOLIA_RPC_URL is not set"
    exit 1
fi

# Derive addresses from private keys or use provided addresses
derive_address() {
    local key_var="$1" addr_var="$2" role="$3"
    local key="${!key_var:-}" addr="${!addr_var:-}"
    if [ -n "$addr" ]; then
        echo "$addr"
    elif [ -n "$key" ]; then
        cast wallet address "$key" 2>/dev/null || echo ""
    else
        echo ""
    fi
}

DEPLOYER=$(derive_address DEPLOYER_PRIVATE_KEY DEPLOYER_ADDRESS "Deployer")
ENCLAVE=$(derive_address ENCLAVE_PRIVATE_KEY ENCLAVE_ADDRESS "Enclave")
OPERATOR=$(derive_address OPERATOR_PRIVATE_KEY OPERATOR_ADDRESS "Operator")
REQUESTER=$(derive_address REQUESTER_PRIVATE_KEY REQUESTER_ADDRESS "Requester")

echo "========================================"
echo "  Sepolia Wallet Balance Check"
echo "========================================"
echo "RPC: $RPC_URL"
echo ""

MIN_BALANCE="0.01"  # ETH threshold for LOW warning

printf "  ${BOLD}%-12s %-44s %12s  %s${RESET}\n" "Role" "Address" "Balance" "Status"
printf "  %-12s %-44s %12s  %s\n" "────────────" "────────────────────────────────────────────" "────────────" "──────"

check_balance() {
    local role="$1" addr="$2"
    if [ -z "$addr" ]; then
        printf "  %-12s %-44s %12s  %s\n" "$role" "(not configured)" "-" "SKIP"
        return
    fi

    local wei
    wei=$(cast balance "$addr" --rpc-url "$RPC_URL" 2>/dev/null || echo "0")
    local eth
    eth=$(cast from-wei "$wei" 2>/dev/null || echo "0")

    local status
    if [ "$wei" = "0" ]; then
        status="${RED}EMPTY${RESET}"
    elif python3 -c "exit(0 if float('$eth') < float('$MIN_BALANCE') else 1)" 2>/dev/null; then
        status="${YELLOW}LOW${RESET}"
    else
        status="${GREEN}OK${RESET}"
    fi

    printf "  %-12s %-44s %12s  %b\n" "$role" "$addr" "${eth} ETH" "$status"
}

check_balance "Deployer" "$DEPLOYER"
check_balance "Enclave" "$ENCLAVE"
check_balance "Operator" "$OPERATOR"
check_balance "Requester" "$REQUESTER"

echo ""
echo "Minimum recommended balance: ${MIN_BALANCE} ETH per wallet"
echo "Get Sepolia ETH: https://sepolia-faucet.pk910.de/"
