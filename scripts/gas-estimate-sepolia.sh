#!/usr/bin/env bash
# Estimate gas costs for key Sepolia operations.
#
# Usage:
#   SEPOLIA_RPC_URL="https://..." ./scripts/gas-estimate-sepolia.sh
#   ./scripts/gas-estimate-sepolia.sh --eth-price 4000
#   ./scripts/gas-estimate-sepolia.sh --help
#
# Operations estimated:
#   submitResult      ~200,000 gas
#   challengeResult   ~150,000 gas
#   resolveDispute    ~300,000 gas
#   registerEnclave   ~100,000 gas
#   finalizeResult    ~80,000 gas
#
# Requires: cast, bc (or python3 as fallback)

set -euo pipefail

# =============================================================================
# Defaults
# =============================================================================

SEPOLIA_RPC_URL="${SEPOLIA_RPC_URL:-}"
ETH_PRICE="${ETH_PRICE:-3500}"

# =============================================================================
# Help
# =============================================================================

show_help() {
    cat <<'EOF'
gas-estimate-sepolia.sh -- Estimate gas costs for Sepolia operations

USAGE:
  SEPOLIA_RPC_URL="https://..." ./scripts/gas-estimate-sepolia.sh [OPTIONS]

OPTIONS:
  --eth-price USD    ETH price in USD (default: 3500, or from ETH_PRICE env)
  --rpc-url URL      Override SEPOLIA_RPC_URL
  -h, --help         Show this help

ENVIRONMENT:
  SEPOLIA_RPC_URL    Required. Sepolia RPC endpoint.
  ETH_PRICE          Optional. ETH price in USD (default: 3500).

OPERATIONS ESTIMATED:
  submitResult       ~200,000 gas   Submit TEE inference result on-chain
  challengeResult    ~150,000 gas   Challenge a submitted result
  resolveDispute     ~300,000 gas   Resolve dispute with ZK proof
  registerEnclave    ~100,000 gas   Register a TEE enclave
  finalizeResult     ~80,000 gas    Finalize an unchallenged result

REQUIRES:
  cast (from Foundry), bc or python3
EOF
}

# =============================================================================
# Parse arguments
# =============================================================================

while [[ $# -gt 0 ]]; do
    case $1 in
        --eth-price) ETH_PRICE="$2"; shift 2 ;;
        --rpc-url)   SEPOLIA_RPC_URL="$2"; shift 2 ;;
        --help|-h)   show_help; exit 0 ;;
        *)           echo "Unknown option: $1"; show_help; exit 1 ;;
    esac
done

# =============================================================================
# Validation
# =============================================================================

if [ -z "$SEPOLIA_RPC_URL" ]; then
    echo "ERROR: SEPOLIA_RPC_URL is required." >&2
    echo "  Set it via environment variable or --rpc-url flag." >&2
    echo "  Example: SEPOLIA_RPC_URL=\"https://ethereum-sepolia-rpc.publicnode.com\" $0" >&2
    exit 1
fi

if ! command -v cast &>/dev/null; then
    echo "ERROR: cast (from Foundry) is required but not found." >&2
    exit 1
fi

# =============================================================================
# Helpers
# =============================================================================

# Multiply two numbers. Uses bc if available, falls back to python3.
calc() {
    local expr="$1"
    if command -v bc &>/dev/null; then
        echo "$expr" | bc -l
    elif command -v python3 &>/dev/null; then
        python3 -c "print($expr)"
    else
        echo "ERROR: neither bc nor python3 available for calculations" >&2
        exit 1
    fi
}

# Format a number to N decimal places.
fmt() {
    local value="$1"
    local decimals="${2:-6}"
    if command -v python3 &>/dev/null; then
        python3 -c "print(f'{float(\"$value\"):.${decimals}f}')"
    elif command -v bc &>/dev/null; then
        printf "%.${decimals}f" "$(echo "$value" | bc -l)"
    else
        echo "$value"
    fi
}

# =============================================================================
# Main
# =============================================================================

echo "========================================"
echo "  Sepolia Gas Cost Estimator"
echo "========================================"
echo ""

# Fetch current gas price
echo "Fetching gas price from Sepolia..."
GAS_PRICE_WEI=$(cast gas-price --rpc-url "$SEPOLIA_RPC_URL" 2>/dev/null)

if [ -z "$GAS_PRICE_WEI" ] || [ "$GAS_PRICE_WEI" = "0" ]; then
    echo "WARNING: Could not fetch gas price. Using 20 gwei as default." >&2
    GAS_PRICE_WEI="20000000000"
fi

GAS_PRICE_GWEI=$(calc "$GAS_PRICE_WEI / 1000000000")
GAS_PRICE_GWEI_FMT=$(fmt "$GAS_PRICE_GWEI" 4)

echo "  RPC:        $SEPOLIA_RPC_URL"
echo "  Gas price:  ${GAS_PRICE_GWEI_FMT} gwei"
echo "  ETH price:  \$${ETH_PRICE}"
echo ""

# Operation gas estimates
declare -a OP_NAMES=(
    "submitResult"
    "challengeResult"
    "resolveDispute"
    "registerEnclave"
    "finalizeResult"
)

declare -a OP_GAS=(
    200000
    150000
    300000
    100000
    80000
)

declare -a OP_DESC=(
    "Submit TEE inference result on-chain"
    "Challenge a submitted result"
    "Resolve dispute with ZK proof"
    "Register a TEE enclave"
    "Finalize an unchallenged result"
)

# Print table header
echo "Operation Estimates"
echo "-------------------------------------------------------------------"
printf "%-20s %10s %14s %10s\n" "Operation" "Gas" "Cost (ETH)" "Cost (USD)"
echo "-------------------------------------------------------------------"

TOTAL_GAS=0
TOTAL_ETH="0"

for i in "${!OP_NAMES[@]}"; do
    name="${OP_NAMES[$i]}"
    gas="${OP_GAS[$i]}"

    # cost_eth = gas * gas_price_wei / 1e18
    cost_eth=$(calc "$gas * $GAS_PRICE_WEI / 1000000000000000000")
    cost_eth_fmt=$(fmt "$cost_eth" 8)

    # cost_usd = cost_eth * eth_price
    cost_usd=$(calc "$cost_eth * $ETH_PRICE")
    cost_usd_fmt=$(fmt "$cost_usd" 4)

    printf "%-20s %10s %14s %10s\n" "$name" "$gas" "$cost_eth_fmt" "\$$cost_usd_fmt"

    TOTAL_GAS=$((TOTAL_GAS + gas))
    TOTAL_ETH=$(calc "$TOTAL_ETH + $cost_eth")
done

echo "-------------------------------------------------------------------"

TOTAL_ETH_FMT=$(fmt "$TOTAL_ETH" 8)
TOTAL_USD=$(calc "$TOTAL_ETH * $ETH_PRICE")
TOTAL_USD_FMT=$(fmt "$TOTAL_USD" 4)

printf "%-20s %10s %14s %10s\n" "TOTAL (all ops)" "$TOTAL_GAS" "$TOTAL_ETH_FMT" "\$$TOTAL_USD_FMT"

echo ""
echo "Descriptions"
echo "-------------------------------------------------------------------"
for i in "${!OP_NAMES[@]}"; do
    printf "  %-20s %s\n" "${OP_NAMES[$i]}" "${OP_DESC[$i]}"
done

echo ""
echo "-------------------------------------------------------------------"
echo "NOTE: Gas estimates are approximate. Actual costs depend on calldata"
echo "size, storage operations, and network congestion at time of execution."
echo ""
echo "Per-inference cost (submit + finalize):"
INFERENCE_GAS=$((200000 + 80000))
INFERENCE_ETH=$(calc "$INFERENCE_GAS * $GAS_PRICE_WEI / 1000000000000000000")
INFERENCE_ETH_FMT=$(fmt "$INFERENCE_ETH" 8)
INFERENCE_USD=$(calc "$INFERENCE_ETH * $ETH_PRICE")
INFERENCE_USD_FMT=$(fmt "$INFERENCE_USD" 4)
echo "  ${INFERENCE_GAS} gas = ${INFERENCE_ETH_FMT} ETH = \$${INFERENCE_USD_FMT}"

echo ""
echo "Per-inference cost with dispute (submit + challenge + resolve):"
DISPUTE_GAS=$((200000 + 150000 + 300000))
DISPUTE_ETH=$(calc "$DISPUTE_GAS * $GAS_PRICE_WEI / 1000000000000000000")
DISPUTE_ETH_FMT=$(fmt "$DISPUTE_ETH" 8)
DISPUTE_USD=$(calc "$DISPUTE_ETH * $ETH_PRICE")
DISPUTE_USD_FMT=$(fmt "$DISPUTE_USD" 4)
echo "  ${DISPUTE_GAS} gas = ${DISPUTE_ETH_FMT} ETH = \$${DISPUTE_USD_FMT}"
