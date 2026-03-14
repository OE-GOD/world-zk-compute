#!/usr/bin/env bash
# =============================================================================
# TEE Deployment + Registration Script for Sepolia Testnet
#
# Wraps ./scripts/deploy.sh and adds TEE-specific setup: program registration,
# enclave registration, and optional wallet funding.
#
# Usage:
#   DEPLOYER_PRIVATE_KEY=0x... ENCLAVE_PRIVATE_KEY=0x... \
#     ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/... \
#     ./scripts/deploy-sepolia-tee.sh
#
#   ./scripts/deploy-sepolia-tee.sh --help
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
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENTS_DIR="$ROOT_DIR/deployments"
DEPLOY_FILE="$DEPLOYMENTS_DIR/11155111.json"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
CHAIN_ID=11155111
CHAIN_NAME="sepolia"
PROGRAM_IMAGE_ID="${PROGRAM_IMAGE_ID:-0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33}"
ENCLAVE_IMAGE_HASH="${ENCLAVE_IMAGE_HASH:-0x0000000000000000000000000000000000000000000000000000000000000001}"
FUND_AMOUNT="0.01ether"

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
deploy-sepolia-tee.sh -- Deploy World ZK Compute to Sepolia with TEE setup.

Wraps ./scripts/deploy.sh for Sepolia and adds:
  - XGBoost program registration in ProgramRegistry
  - TEE enclave registration in TEEMLVerifier
  - Optional wallet funding for operator/requester

USAGE:
  scripts/deploy-sepolia-tee.sh [OPTIONS]

REQUIRED ENVIRONMENT:
  DEPLOYER_PRIVATE_KEY       Private key for deployer (admin)
  ENCLAVE_PRIVATE_KEY        Private key for the TEE enclave signer
  ALCHEMY_SEPOLIA_RPC_URL    Alchemy (or other provider) RPC URL for Sepolia

OPTIONAL ENVIRONMENT:
  ETHERSCAN_API_KEY          Etherscan API key for contract verification
  PROGRAM_IMAGE_ID           Image ID for the program to register
                             (default: rule-engine v3.0 image ID)
  ENCLAVE_IMAGE_HASH         PCR0 / image hash for enclave attestation
                             (default: placeholder 0x...0001)
  OPERATOR_PRIVATE_KEY       If set, fund this wallet with 0.01 ETH from deployer
  REQUESTER_PRIVATE_KEY      If set, fund this wallet with 0.01 ETH from deployer

OPTIONS:
  --help, -h                 Show this help message

EXAMPLES:
  # Full deployment with verification
  DEPLOYER_PRIVATE_KEY=0xabc... \
  ENCLAVE_PRIVATE_KEY=0xdef... \
  ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
  ETHERSCAN_API_KEY=YOUR_KEY \
    ./scripts/deploy-sepolia-tee.sh

  # Deployment without Etherscan verification
  DEPLOYER_PRIVATE_KEY=0xabc... \
  ENCLAVE_PRIVATE_KEY=0xdef... \
  ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
    ./scripts/deploy-sepolia-tee.sh
USAGE
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        *)
            err "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Validate prerequisites
# ---------------------------------------------------------------------------
header "Checking prerequisites..."

MISSING_TOOLS=false
for tool in cast forge jq; do
    if ! command -v "$tool" &>/dev/null; then
        err "$tool is required. Install: curl -L https://foundry.paradigm.xyz | bash"
        MISSING_TOOLS=true
    fi
done

if [[ "$MISSING_TOOLS" == "true" ]]; then
    exit 1
fi
ok "All required tools found (cast, forge, jq)"

# ---------------------------------------------------------------------------
# Validate environment variables
# ---------------------------------------------------------------------------
MISSING_VARS=false

if [[ -z "${DEPLOYER_PRIVATE_KEY:-}" ]]; then
    err "DEPLOYER_PRIVATE_KEY is required"
    MISSING_VARS=true
fi

if [[ -z "${ENCLAVE_PRIVATE_KEY:-}" ]]; then
    err "ENCLAVE_PRIVATE_KEY is required"
    MISSING_VARS=true
fi

if [[ -z "${ALCHEMY_SEPOLIA_RPC_URL:-}" ]]; then
    err "ALCHEMY_SEPOLIA_RPC_URL is required (public Sepolia RPCs rate-limit heavily)"
    MISSING_VARS=true
fi

if [[ "$MISSING_VARS" == "true" ]]; then
    echo ""
    err "Missing required environment variables. Run with --help for details."
    exit 1
fi
ok "All required environment variables set"

# ---------------------------------------------------------------------------
# Derive addresses
# ---------------------------------------------------------------------------
DEPLOYER_ADDR=$(cast wallet address --private-key "$DEPLOYER_PRIVATE_KEY" 2>/dev/null) || {
    err "Invalid DEPLOYER_PRIVATE_KEY. Could not derive address."
    exit 1
}

ENCLAVE_ADDR=$(cast wallet address --private-key "$ENCLAVE_PRIVATE_KEY" 2>/dev/null) || {
    err "Invalid ENCLAVE_PRIVATE_KEY. Could not derive address."
    exit 1
}

# Set RPC_URL for deploy.sh and all cast calls
RPC_URL="$ALCHEMY_SEPOLIA_RPC_URL"
export RPC_URL

# ---------------------------------------------------------------------------
# Banner
# ---------------------------------------------------------------------------
header "============================================================"
header "  World ZK Compute -- Sepolia TEE Deployment"
header "============================================================"
echo ""
info "Chain:              $CHAIN_NAME (id: $CHAIN_ID)"
info "RPC:                $RPC_URL"
info "Deployer:           $DEPLOYER_ADDR"
info "Enclave Signer:     $ENCLAVE_ADDR"
info "Program Image ID:   $PROGRAM_IMAGE_ID"
info "Enclave Image Hash: $ENCLAVE_IMAGE_HASH"

if [[ -n "${ETHERSCAN_API_KEY:-}" ]]; then
    info "Etherscan Verify:   enabled"
else
    info "Etherscan Verify:   disabled (set ETHERSCAN_API_KEY to enable)"
fi

if [[ -n "${OPERATOR_PRIVATE_KEY:-}" ]]; then
    OPERATOR_ADDR=$(cast wallet address --private-key "$OPERATOR_PRIVATE_KEY" 2>/dev/null) || true
    info "Operator Wallet:    ${OPERATOR_ADDR:-invalid key}"
fi

if [[ -n "${REQUESTER_PRIVATE_KEY:-}" ]]; then
    REQUESTER_ADDR=$(cast wallet address --private-key "$REQUESTER_PRIVATE_KEY" 2>/dev/null) || true
    info "Requester Wallet:   ${REQUESTER_ADDR:-invalid key}"
fi

echo ""

# ---------------------------------------------------------------------------
# Step 1: Check deployer balance
# ---------------------------------------------------------------------------
header "Step 1/6: Check deployer balance"

BALANCE_WEI=$(cast balance "$DEPLOYER_ADDR" --rpc-url "$RPC_URL" 2>/dev/null) || {
    warn "Could not check deployer balance (RPC may be unreachable)"
    BALANCE_WEI="0"
}

BALANCE_ETH=$(cast from-wei "$BALANCE_WEI" 2>/dev/null || echo "unknown")
info "Deployer balance: $BALANCE_ETH ETH"

# Warn if balance is below 0.06 ETH (deployment + registration + funding)
if [[ "$BALANCE_WEI" != "unknown" ]] && [[ "$BALANCE_WEI" -lt 60000000000000000 ]] 2>/dev/null; then
    warn "Balance is below 0.06 ETH. Deployment may fail due to insufficient funds."
    warn "Fund the deployer at: $DEPLOYER_ADDR"
fi

# ---------------------------------------------------------------------------
# Step 2: Deploy contracts via deploy.sh
# ---------------------------------------------------------------------------
header "Step 2/6: Deploy contracts"

DEPLOY_ARGS=("--chain" "sepolia")

if [[ -n "${ETHERSCAN_API_KEY:-}" ]]; then
    DEPLOY_ARGS+=("--verify")
    info "Etherscan verification enabled"
else
    info "Skipping Etherscan verification (no ETHERSCAN_API_KEY)"
fi

info "Running: ./scripts/deploy.sh ${DEPLOY_ARGS[*]}"
echo ""

export DEPLOYER_PRIVATE_KEY
"$SCRIPT_DIR/deploy.sh" "${DEPLOY_ARGS[@]}" || {
    err "deploy.sh failed. Check output above for details."
    exit 1
}

ok "Base deployment complete"

# ---------------------------------------------------------------------------
# Step 3: Read deployed addresses
# ---------------------------------------------------------------------------
header "Step 3/6: Read deployed addresses"

if [[ ! -f "$DEPLOY_FILE" ]]; then
    err "Deployment file not found: $DEPLOY_FILE"
    err "Expected deploy.sh to create this file for chain ID $CHAIN_ID."
    exit 1
fi

# Extract addresses from the deployment JSON
# deploy.sh stores contracts as: { "ContractName": { "address": "0x...", "txHash": "0x..." } }
REGISTRY_ADDR=$(jq -r '.contracts.ProgramRegistry.address // empty' "$DEPLOY_FILE" 2>/dev/null)
TEE_ADDR=$(jq -r '.contracts.TEEMLVerifier.address // empty' "$DEPLOY_FILE" 2>/dev/null)
ENGINE_ADDR=$(jq -r '.contracts.ExecutionEngine.address // empty' "$DEPLOY_FILE" 2>/dev/null)

if [[ -z "$REGISTRY_ADDR" || "$REGISTRY_ADDR" == "null" ]]; then
    err "ProgramRegistry address not found in $DEPLOY_FILE"
    exit 1
fi

if [[ -z "$TEE_ADDR" || "$TEE_ADDR" == "null" ]]; then
    err "TEEMLVerifier address not found in $DEPLOY_FILE"
    exit 1
fi

if [[ -z "$ENGINE_ADDR" || "$ENGINE_ADDR" == "null" ]]; then
    err "ExecutionEngine address not found in $DEPLOY_FILE"
    exit 1
fi

ok "ProgramRegistry:  $REGISTRY_ADDR"
ok "TEEMLVerifier:    $TEE_ADDR"
ok "ExecutionEngine:  $ENGINE_ADDR"

# ---------------------------------------------------------------------------
# Step 4: Register XGBoost program
# ---------------------------------------------------------------------------
header "Step 4/6: Register XGBoost program"

info "Registering program with image ID: $PROGRAM_IMAGE_ID"
info "  Registry: $REGISTRY_ADDR"

REGISTER_OUTPUT=$(cast send "$REGISTRY_ADDR" \
    'registerProgram(bytes32,string,string,bytes32)' \
    "$PROGRAM_IMAGE_ID" \
    "xgboost-rule-engine" \
    "https://github.com/worldcoin/world-zk-compute" \
    "0x0000000000000000000000000000000000000000000000000000000000000000" \
    --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" 2>&1) || {
    warn "Program registration failed (program may already be registered)"
    warn "Output: $REGISTER_OUTPUT"
}

# Verify registration
PROGRAM_EXISTS=$(cast call "$REGISTRY_ADDR" \
    'programs(bytes32)(string,string,bytes32,bool)' \
    "$PROGRAM_IMAGE_ID" \
    --rpc-url "$RPC_URL" 2>/dev/null) || true

if [[ -n "$PROGRAM_EXISTS" ]]; then
    ok "Program registered successfully"
else
    warn "Could not verify program registration"
fi

# ---------------------------------------------------------------------------
# Step 5: Register TEE enclave
# ---------------------------------------------------------------------------
header "Step 5/6: Register TEE enclave"

info "Registering enclave signer: $ENCLAVE_ADDR"
info "  Image hash: $ENCLAVE_IMAGE_HASH"
info "  TEEMLVerifier: $TEE_ADDR"

ENCLAVE_OUTPUT=$(cast send "$TEE_ADDR" \
    'registerEnclave(address,bytes32)' \
    "$ENCLAVE_ADDR" \
    "$ENCLAVE_IMAGE_HASH" \
    --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" 2>&1) || {
    warn "Enclave registration failed (enclave may already be registered)"
    warn "Output: $ENCLAVE_OUTPUT"
}

# Verify registration
ENCLAVE_REGISTERED=$(cast call "$TEE_ADDR" \
    'registeredEnclaves(address)(bytes32)' \
    "$ENCLAVE_ADDR" \
    --rpc-url "$RPC_URL" 2>/dev/null) || true

if [[ -n "$ENCLAVE_REGISTERED" && "$ENCLAVE_REGISTERED" != "0x0000000000000000000000000000000000000000000000000000000000000000" ]]; then
    ok "Enclave registered successfully"
else
    warn "Could not verify enclave registration"
fi

# ---------------------------------------------------------------------------
# Step 6: Fund operator/requester wallets (optional)
# ---------------------------------------------------------------------------
header "Step 6/6: Fund wallets (optional)"

FUNDED_COUNT=0

if [[ -n "${OPERATOR_PRIVATE_KEY:-}" ]]; then
    OPERATOR_ADDR=$(cast wallet address --private-key "$OPERATOR_PRIVATE_KEY" 2>/dev/null) || {
        warn "Invalid OPERATOR_PRIVATE_KEY, skipping funding"
        OPERATOR_ADDR=""
    }
    if [[ -n "$OPERATOR_ADDR" ]]; then
        info "Funding operator wallet: $OPERATOR_ADDR ($FUND_AMOUNT)"
        if cast send "$OPERATOR_ADDR" --value "$FUND_AMOUNT" \
            --rpc-url "$RPC_URL" --private-key "$DEPLOYER_PRIVATE_KEY" &>/dev/null; then
            ok "Operator funded with $FUND_AMOUNT"
            FUNDED_COUNT=$((FUNDED_COUNT + 1))
        else
            warn "Failed to fund operator wallet"
        fi
    fi
fi

if [[ -n "${REQUESTER_PRIVATE_KEY:-}" ]]; then
    REQUESTER_ADDR=$(cast wallet address --private-key "$REQUESTER_PRIVATE_KEY" 2>/dev/null) || {
        warn "Invalid REQUESTER_PRIVATE_KEY, skipping funding"
        REQUESTER_ADDR=""
    }
    if [[ -n "$REQUESTER_ADDR" ]]; then
        info "Funding requester wallet: $REQUESTER_ADDR ($FUND_AMOUNT)"
        if cast send "$REQUESTER_ADDR" --value "$FUND_AMOUNT" \
            --rpc-url "$RPC_URL" --private-key "$DEPLOYER_PRIVATE_KEY" &>/dev/null; then
            ok "Requester funded with $FUND_AMOUNT"
            FUNDED_COUNT=$((FUNDED_COUNT + 1))
        else
            warn "Failed to fund requester wallet"
        fi
    fi
fi

if [[ "$FUNDED_COUNT" -eq 0 ]]; then
    info "No wallets to fund (set OPERATOR_PRIVATE_KEY or REQUESTER_PRIVATE_KEY to enable)"
fi

# ---------------------------------------------------------------------------
# Deployment summary
# ---------------------------------------------------------------------------
header "============================================================"
header "  DEPLOYMENT SUMMARY"
header "============================================================"
echo ""
printf "  ${BOLD}%-25s %-44s %s${RESET}\n" "COMPONENT" "ADDRESS / VALUE" "STATUS"
printf "  %-25s %-44s %s\n" "-------------------------" "--------------------------------------------" "----------"
printf "  ${GREEN}%-25s${RESET} %-44s %s\n" "ProgramRegistry" "$REGISTRY_ADDR" "deployed"
printf "  ${GREEN}%-25s${RESET} %-44s %s\n" "TEEMLVerifier" "$TEE_ADDR" "deployed"
printf "  ${GREEN}%-25s${RESET} %-44s %s\n" "ExecutionEngine" "$ENGINE_ADDR" "deployed"
printf "  ${CYAN}%-25s${RESET} %-44s %s\n" "XGBoost Program" "${PROGRAM_IMAGE_ID:0:18}..." "registered"
printf "  ${CYAN}%-25s${RESET} %-44s %s\n" "TEE Enclave" "$ENCLAVE_ADDR" "registered"

if [[ -n "${OPERATOR_ADDR:-}" ]]; then
    printf "  ${CYAN}%-25s${RESET} %-44s %s\n" "Operator Wallet" "$OPERATOR_ADDR" "funded"
fi

if [[ -n "${REQUESTER_ADDR:-}" ]]; then
    printf "  ${CYAN}%-25s${RESET} %-44s %s\n" "Requester Wallet" "$REQUESTER_ADDR" "funded"
fi

echo ""
printf "  ${BOLD}Chain:${RESET}              %s (id: %s)\n" "$CHAIN_NAME" "$CHAIN_ID"
printf "  ${BOLD}RPC:${RESET}                %s\n" "$RPC_URL"
printf "  ${BOLD}Deployer:${RESET}           %s\n" "$DEPLOYER_ADDR"
printf "  ${BOLD}Deployment File:${RESET}    %s\n" "$DEPLOY_FILE"
echo ""

# Verification links
echo "  ${BOLD}Etherscan Verification:${RESET}"
if [[ -n "${TEE_ADDR:-}" ]]; then
    echo "    TEEMLVerifier:     https://sepolia.etherscan.io/address/${TEE_ADDR}"
fi
if [[ -n "${ENGINE_ADDR:-}" ]]; then
    echo "    ExecutionEngine:   https://sepolia.etherscan.io/address/${ENGINE_ADDR}"
fi
if [[ -n "${REGISTRY_ADDR:-}" ]]; then
    echo "    ProgramRegistry:   https://sepolia.etherscan.io/address/${REGISTRY_ADDR}"
fi
echo ""

# Next steps
echo "  ${BOLD}Next Steps:${RESET}"
echo "    1. Run E2E validation:  source .env.sepolia && ./scripts/sepolia-e2e.sh"
echo "    2. Start services:      docker compose -f docker-compose.sepolia.yml --env-file .env.sepolia up -d"
echo "    3. Check balances:      source .env.sepolia && ./scripts/check-sepolia-balances.sh"
echo ""

ok "Sepolia TEE deployment complete."
