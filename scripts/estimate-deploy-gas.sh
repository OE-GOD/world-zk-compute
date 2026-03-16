#!/usr/bin/env bash
# Estimate gas costs for deploying all contracts.
# Usage: ./scripts/estimate-deploy-gas.sh [OPTIONS]
#
# Options:
#   --rpc-url URL     RPC endpoint (default: http://127.0.0.1:8545)
#   --eth-price USD   ETH price in USD (default: 3000)
#   --gas-price GWEI  Override gas price in gwei (default: auto-detect)
#
# Requires: forge, cast, bc

set -euo pipefail

RPC_URL="http://127.0.0.1:8545"
ETH_PRICE=3000
GAS_PRICE_OVERRIDE=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --rpc-url) RPC_URL="$2"; shift 2 ;;
        --eth-price) ETH_PRICE="$2"; shift 2 ;;
        --gas-price) GAS_PRICE_OVERRIDE="$2"; shift 2 ;;
        --help|-h)
            head -10 "$0" | tail -9
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONTRACTS_DIR="$(dirname "$SCRIPT_DIR")/contracts"

cd "$CONTRACTS_DIR" || exit 1

# Get gas price
if [ -n "$GAS_PRICE_OVERRIDE" ]; then
    GAS_PRICE_GWEI="$GAS_PRICE_OVERRIDE"
    echo "Using override gas price: ${GAS_PRICE_GWEI} gwei"
else
    GAS_PRICE_WEI=$(cast gas-price --rpc-url "$RPC_URL" 2>/dev/null || echo "0")
    if [ "$GAS_PRICE_WEI" = "0" ]; then
        GAS_PRICE_GWEI="0.1"
        echo "Could not fetch gas price from $RPC_URL, using default: ${GAS_PRICE_GWEI} gwei"
    else
        GAS_PRICE_GWEI=$(python3 -c "print(f'{int(\"$GAS_PRICE_WEI\") / 1e9:.4f}')" 2>/dev/null || echo "0.1")
        echo "Current gas price: ${GAS_PRICE_GWEI} gwei (from $RPC_URL)"
    fi
fi

echo "ETH price: \$${ETH_PRICE}"
echo ""

# Contracts to estimate (name, constructor args for size estimation)
# We use the initcode size from forge build --sizes as a proxy for deployment gas
echo "Building contracts..."
SIZES_OUTPUT=$(forge build --sizes 2>&1)

CONTRACTS=(
    "TEEMLVerifier"
    "ExecutionEngine"
    "ProgramRegistry"
    "RemainderVerifier"
)

echo ""
echo "┌──────────────────────────┬──────────────┬──────────────┬──────────────┐"
echo "│ Contract                 │ Deploy Gas   │ ETH Cost     │ USD Cost     │"
echo "├──────────────────────────┼──────────────┼──────────────┼──────────────┤"

TOTAL_GAS=0

for contract in "${CONTRACTS[@]}"; do
    # Extract initcode size as gas estimate (roughly 200 gas per byte + 32K base)
    INITCODE_SIZE=$(echo "$SIZES_OUTPUT" | grep "| ${contract} " | head -1 | awk -F'|' '{gsub(/[, ]/,"",$4); print $4}')

    if [ -z "$INITCODE_SIZE" ]; then
        printf "│ %-24s │ %12s │ %12s │ %12s │\n" "$contract" "N/A" "N/A" "N/A"
        continue
    fi

    # Estimate: ~200 gas/byte for contract creation + 21000 base + 32000 create
    DEPLOY_GAS=$((INITCODE_SIZE * 200 + 53000))
    TOTAL_GAS=$((TOTAL_GAS + DEPLOY_GAS))

    ETH_COST=$(python3 -c "print(f'{$DEPLOY_GAS * $GAS_PRICE_GWEI * 1e-9:.6f}')" 2>/dev/null || echo "?")
    USD_COST=$(python3 -c "print(f'{$DEPLOY_GAS * $GAS_PRICE_GWEI * 1e-9 * $ETH_PRICE:.2f}')" 2>/dev/null || echo "?")

    printf "│ %-24s │ %12s │ %10s │ $%10s │\n" "$contract" "$DEPLOY_GAS" "${ETH_COST} ETH" "$USD_COST"
done

echo "├──────────────────────────┼──────────────┼──────────────┼──────────────┤"

TOTAL_ETH=$(python3 -c "print(f'{$TOTAL_GAS * $GAS_PRICE_GWEI * 1e-9:.6f}')" 2>/dev/null || echo "?")
TOTAL_USD=$(python3 -c "print(f'{$TOTAL_GAS * $GAS_PRICE_GWEI * 1e-9 * $ETH_PRICE:.2f}')" 2>/dev/null || echo "?")
printf "│ %-24s │ %12s │ %10s │ $%10s │\n" "TOTAL" "$TOTAL_GAS" "${TOTAL_ETH} ETH" "$TOTAL_USD"

echo "└──────────────────────────┴──────────────┴──────────────┴──────────────┘"
echo ""
echo "Note: Gas estimates are approximate (based on initcode size * 200 + 53K base)."
echo "Actual deployment gas may vary. Use 'forge script --estimate' for precise values."
echo ""
echo "Tip: On Arbitrum L2, gas prices are typically 0.01-0.1 gwei."
echo "     On Ethereum L1, gas prices are typically 10-50 gwei."
