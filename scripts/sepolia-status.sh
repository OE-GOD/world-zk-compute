#!/usr/bin/env bash
# Check deployment health on Sepolia testnet.
# Usage: ./scripts/sepolia-status.sh
#
# Required env vars:
#   ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC endpoint
#
# Optional env vars:
#   TEE_VERIFIER_ADDRESS      TEEMLVerifier contract address
#   EXECUTION_ENGINE_ADDRESS  ExecutionEngine contract address
#
# Options:
#   --help, -h    Show this help message

set -euo pipefail

if [ -z "${NO_COLOR:-}" ] && [ -t 1 ]; then
    GREEN='\033[0;32m'; RED='\033[0;31m'; BOLD='\033[1m'; RESET='\033[0m'
else
    GREEN=''; RED=''; BOLD=''; RESET=''
fi

ok()   { printf "  %b[OK]%b   %s\n" "$GREEN" "$RESET" "$1"; }
fail() { printf "  %b[FAIL]%b %s\n" "$RED" "$RESET" "$1"; }

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    head -14 "$0" | tail -13
    exit 0
fi

if ! command -v cast &>/dev/null; then
    echo "ERROR: 'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

RPC_URL="${ALCHEMY_SEPOLIA_RPC_URL:-}"
if [ -z "$RPC_URL" ]; then
    echo "ERROR: ALCHEMY_SEPOLIA_RPC_URL is not set"
    exit 1
fi

echo ""
printf "%bWorld ZK Compute — Sepolia Status%b\n" "$BOLD" "$RESET"
echo "========================================"
echo ""

# 1. RPC connectivity
printf "%bRPC Connectivity%b\n" "$BOLD" "$RESET"
if BLOCK=$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null); then
    ok "Connected — block $BLOCK"
else
    fail "Cannot reach RPC"
fi
if CHAIN_ID=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null); then
    ok "Chain ID: $CHAIN_ID"
else
    fail "Cannot get chain ID"
fi
echo ""

# 2. Contract status
if [ -n "${TEE_VERIFIER_ADDRESS:-}" ]; then
    printf "%bTEEMLVerifier (%s)%b\n" "$BOLD" "$TEE_VERIFIER_ADDRESS" "$RESET"
    if OWNER=$(cast call "$TEE_VERIFIER_ADDRESS" "owner()(address)" --rpc-url "$RPC_URL" 2>/dev/null); then
        ok "Owner: $OWNER"
    else
        fail "Cannot read owner"
    fi
    if PAUSED=$(cast call "$TEE_VERIFIER_ADDRESS" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null); then
        ok "Paused: $PAUSED"
    else
        fail "Cannot read paused"
    fi
    if STAKE=$(cast call "$TEE_VERIFIER_ADDRESS" "proverStake()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null); then
        ok "Prover stake: $STAKE wei"
    else
        fail "Cannot read stake"
    fi
    if BOND=$(cast call "$TEE_VERIFIER_ADDRESS" "challengeBondAmount()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null); then
        ok "Challenge bond: $BOND wei"
    else
        fail "Cannot read bond"
    fi
    echo ""
else
    echo "TEE_VERIFIER_ADDRESS not set — skipping contract checks"
    echo ""
fi

if [ -n "${EXECUTION_ENGINE_ADDRESS:-}" ]; then
    printf "%bExecutionEngine (%s)%b\n" "$BOLD" "$EXECUTION_ENGINE_ADDRESS" "$RESET"
    if NEXT_ID=$(cast call "$EXECUTION_ENGINE_ADDRESS" "nextRequestId()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null); then
        ok "Next request ID: $NEXT_ID"
    else
        fail "Cannot read nextRequestId"
    fi
    echo ""
fi

# 3. Service health (if running locally via docker-compose)
printf "%bLocal Service Health (optional)%b\n" "$BOLD" "$RESET"
for svc in "Enclave:8080" "Prover:3000" "Operator:9090" "Indexer:8081"; do
    NAME="${svc%%:*}"
    PORT="${svc##*:}"
    STATUS=$(curl -so /dev/null -w '%{http_code}' --connect-timeout 2 "http://127.0.0.1:${PORT}/health" 2>/dev/null || echo "000")
    if [ "$STATUS" = "200" ]; then
        ok "$NAME (port $PORT) — healthy"
    else
        fail "$NAME (port $PORT) — unreachable or unhealthy (HTTP $STATUS)"
    fi
done
echo ""

echo "Done."
