#!/usr/bin/env bash
# =============================================================================
# Sepolia Faucet Helper
#
# Checks wallet balance and provides links to Sepolia testnet faucets.
# Faucets require CAPTCHA so this script cannot fully automate funding.
#
# Usage:
#   ./scripts/sepolia-faucet.sh --address 0x1234...
#   ./scripts/sepolia-faucet.sh --address 0x1234... --open
#   ./scripts/sepolia-faucet.sh --help
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Color helpers (respects NO_COLOR)
# ---------------------------------------------------------------------------
if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' CYAN='' BOLD='' RESET=''
fi

info()    { printf "${CYAN}[INFO]${RESET}  %s\n" "$*"; }
ok()      { printf "${GREEN}[OK]${RESET}    %s\n" "$*"; }
warn()    { printf "${YELLOW}[WARN]${RESET}  %s\n" "$*"; }
err()     { printf "${RED}[ERROR]${RESET} %s\n" "$*" >&2; }
header()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }

# ---------------------------------------------------------------------------
# Project paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ---------------------------------------------------------------------------
# Faucet list
# ---------------------------------------------------------------------------
FAUCET_URLS=(
    "https://sepoliafaucet.com"
    "https://www.infura.io/faucet/sepolia"
    "https://cloud.google.com/application/web3/faucet/ethereum/sepolia"
    "https://sepolia-faucet.pk910.de"
)

FAUCET_NAMES=(
    "Alchemy"
    "Infura"
    "Google Cloud"
    "PoW Faucet"
)

FAUCET_NOTES=(
    "Requires Alchemy account; 0.5 ETH/day"
    "Requires Infura account; 0.5 ETH/day"
    "Requires Google Cloud account; 0.05 ETH/day"
    "No account needed; mine ETH via proof-of-work in browser"
)

# ---------------------------------------------------------------------------
# Default flags
# ---------------------------------------------------------------------------
ADDRESS=""
OPEN_BROWSER=false
RPC_URL="${SEPOLIA_RPC_URL:-https://rpc.sepolia.org}"

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<EOF

${BOLD}Sepolia Faucet Helper${RESET}

Checks wallet balance on Sepolia and provides faucet links for funding.
Faucets require CAPTCHA, so manual interaction is needed.

${BOLD}USAGE${RESET}
    $(basename "$0") --address <addr> [--open] [--rpc-url <url>]
    $(basename "$0") --help

${BOLD}FLAGS${RESET}
    --address <addr>    Ethereum address to check balance for
    --open              Open the first faucet URL in default browser
    --rpc-url <url>     Sepolia RPC endpoint (default: \$SEPOLIA_RPC_URL or https://rpc.sepolia.org)
    --help, -h          Show this help message

${BOLD}EXAMPLES${RESET}
    $(basename "$0") --address 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
    $(basename "$0") --address 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 --open

${BOLD}ENVIRONMENT${RESET}
    SEPOLIA_RPC_URL     Override default Sepolia RPC endpoint
EOF
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --address)
            ADDRESS="$2"
            shift 2
            ;;
        --open)
            OPEN_BROWSER=true
            shift
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            err "Unknown flag: $1"
            usage
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Prerequisites
# ---------------------------------------------------------------------------
if ! command -v cast &>/dev/null; then
    err "'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
header "Sepolia Testnet Faucet Helper"

# Check balance if address provided
if [[ -n "$ADDRESS" ]]; then
    header "Wallet Balance"
    info "Address: $ADDRESS"
    info "RPC:     $RPC_URL"

    BALANCE_WEI=$(cast balance "$ADDRESS" --rpc-url "$RPC_URL" 2>/dev/null || echo "error")
    if [[ "$BALANCE_WEI" == "error" ]]; then
        err "Failed to query balance. Check address and RPC URL."
    else
        BALANCE_ETH=$(cast from-wei "$BALANCE_WEI" 2>/dev/null || echo "0")
        if [[ "$BALANCE_WEI" == "0" ]]; then
            warn "Balance: 0 ETH (wallet is empty)"
        else
            ok "Balance: ${BALANCE_ETH} ETH"
        fi
    fi
else
    warn "No --address provided. Skipping balance check."
fi

# Recommended amounts
header "Recommended ETH Amounts"
printf "  %-20s %s\n" "Role" "Minimum ETH"
printf "  %-20s %s\n" "--------------------" "-----------"
printf "  %-20s %s\n" "Deployer"            "0.1 ETH"
printf "  %-20s %s\n" "Operator"            "0.05 ETH"
printf "  %-20s %s\n" "Enclave"             "0.05 ETH"

# Faucet list
header "Sepolia Faucets"
info "All faucets require CAPTCHA or account sign-in."
echo ""

for i in "${!FAUCET_URLS[@]}"; do
    idx=$((i + 1))
    printf "  ${BOLD}%d. %s${RESET}\n" "$idx" "${FAUCET_NAMES[$i]}"
    printf "     URL:  %s\n" "${FAUCET_URLS[$i]}"
    printf "     Note: %s\n" "${FAUCET_NOTES[$i]}"
    echo ""
done

# Open browser if requested
if [[ "$OPEN_BROWSER" == true ]]; then
    FAUCET_TO_OPEN="${FAUCET_URLS[0]}"
    info "Opening ${FAUCET_NAMES[0]} faucet in browser..."

    if [[ "$(uname)" == "Darwin" ]]; then
        open "$FAUCET_TO_OPEN"
    elif command -v xdg-open &>/dev/null; then
        xdg-open "$FAUCET_TO_OPEN"
    else
        warn "Cannot detect browser opener. Visit manually: $FAUCET_TO_OPEN"
    fi
fi

# Tip
header "Tips"
info "After receiving ETH, verify your balance with:"
echo "  cast balance <address> --rpc-url $RPC_URL --ether"
echo ""
info "Or use the balance checker script:"
echo "  ./scripts/check-sepolia-balances.sh"
echo ""
