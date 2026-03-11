#!/bin/bash
# Post-deployment health check for TEEMLVerifier contracts.
#
# Runs a series of on-chain read calls to confirm the contract is
# deployed, configured, and ready to accept submissions.
#
# Positional args (or env vars):
#   $1 / RPC_URL           -- RPC endpoint
#   $2 / CONTRACT_ADDRESS  -- TEEMLVerifier proxy/contract address
#
# Usage:
#   bash scripts/verify-deployment.sh https://sepolia-rollup.arbitrum.io/rpc 0xAbC...123
#   RPC_URL=https://... CONTRACT_ADDRESS=0x... bash scripts/verify-deployment.sh
#   bash scripts/verify-deployment.sh --help

set -euo pipefail

# ── Help ──────────────────────────────────────────────────────────────────────
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    cat <<'USAGE'
verify-deployment.sh -- Post-deployment health check for TEEMLVerifier

USAGE:
  scripts/verify-deployment.sh <RPC_URL> <CONTRACT_ADDRESS>
  RPC_URL=... CONTRACT_ADDRESS=... scripts/verify-deployment.sh

CHECKS:
  1. Contract deployed (code size > 0)
  2. Owner is set and non-zero
  3. Contract is not paused
  4. proverStake is non-zero
  5. challengeBondAmount is non-zero
  6. remainderVerifier is set (non-zero address)

EXIT:
  0 on all checks passing, 1 on any failure.

REQUIRES:
  cast (from Foundry)
USAGE
    exit 0
fi

# ── Resolve args / env ────────────────────────────────────────────────────────
RPC_URL="${1:-${RPC_URL:-}}"
CONTRACT_ADDRESS="${2:-${CONTRACT_ADDRESS:-}}"

if [[ -z "$RPC_URL" ]]; then
    echo "ERROR: RPC_URL not provided. Pass as first argument or set RPC_URL env var."
    echo "Run with --help for usage."
    exit 1
fi

if [[ -z "$CONTRACT_ADDRESS" ]]; then
    echo "ERROR: CONTRACT_ADDRESS not provided. Pass as second argument or set CONTRACT_ADDRESS env var."
    echo "Run with --help for usage."
    exit 1
fi

# Verify cast is available
if ! command -v cast &>/dev/null; then
    echo "ERROR: 'cast' not found. Install Foundry: https://getfoundry.sh"
    exit 1
fi

# ── State ─────────────────────────────────────────────────────────────────────
PASS_COUNT=0
FAIL_COUNT=0
TOTAL_CHECKS=8

ZERO_ADDRESS="0x0000000000000000000000000000000000000000"

echo "=== TEEMLVerifier Deployment Health Check ==="
echo "  RPC:      $RPC_URL"
echo "  Contract: $CONTRACT_ADDRESS"
echo ""

# ── Helper ────────────────────────────────────────────────────────────────────
record_pass() {
    local label="$1"
    local detail="$2"
    echo "  [PASS] $label: $detail"
    PASS_COUNT=$((PASS_COUNT + 1))
}

record_fail() {
    local label="$1"
    local detail="$2"
    echo "  [FAIL] $label: $detail"
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# Strip cast formatting annotations like " [1e17]" from numeric outputs
strip_cast_annotation() {
    echo "$1" | sed 's/ \[.*\]$//'
}

# ── Check 1: Contract deployed (code size > 0) ───────────────────────────────
CODE=$(cast code "$CONTRACT_ADDRESS" --rpc-url "$RPC_URL" 2>/dev/null || echo "0x")
if [[ "$CODE" == "0x" || -z "$CODE" ]]; then
    record_fail "Contract deployed" "no bytecode at address"
else
    CODE_BYTES=$(( ${#CODE} / 2 - 1 ))
    record_pass "Contract deployed" "code size = ${CODE_BYTES} bytes"
fi

# ── Check 2: Owner is set and non-zero ────────────────────────────────────────
OWNER=$(cast call "$CONTRACT_ADDRESS" "owner()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")
if [[ -z "$OWNER" ]]; then
    record_fail "Owner set" "call reverted or returned empty"
elif [[ "$OWNER" == "$ZERO_ADDRESS" ]]; then
    record_fail "Owner set" "owner is zero address"
else
    record_pass "Owner set" "$OWNER"
fi

# ── Check 3: Contract is not paused ──────────────────────────────────────────
PAUSED=$(cast call "$CONTRACT_ADDRESS" "paused()(bool)" --rpc-url "$RPC_URL" 2>/dev/null || echo "error")
if [[ "$PAUSED" == "error" ]]; then
    record_fail "Not paused" "call reverted or returned error"
elif [[ "$PAUSED" == "true" ]]; then
    record_fail "Not paused" "contract IS paused"
else
    record_pass "Not paused" "paused = false"
fi

# ── Check 4: proverStake is non-zero ─────────────────────────────────────────
PROVER_STAKE_RAW=$(cast call "$CONTRACT_ADDRESS" "proverStake()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "error")
PROVER_STAKE_RAW=$(strip_cast_annotation "$PROVER_STAKE_RAW")
if [[ "$PROVER_STAKE_RAW" == "error" ]]; then
    record_fail "Prover stake" "call reverted or returned error"
elif [[ "$PROVER_STAKE_RAW" == "0" ]]; then
    record_fail "Prover stake" "proverStake is 0"
else
    PROVER_STAKE_ETH=$(cast from-wei "$PROVER_STAKE_RAW" 2>/dev/null || echo "${PROVER_STAKE_RAW} wei")
    record_pass "Prover stake" "${PROVER_STAKE_ETH} ETH (${PROVER_STAKE_RAW} wei)"
fi

# ── Check 5: challengeBondAmount is non-zero ──────────────────────────────────
BOND_RAW=$(cast call "$CONTRACT_ADDRESS" "challengeBondAmount()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "error")
BOND_RAW=$(strip_cast_annotation "$BOND_RAW")
if [[ "$BOND_RAW" == "error" ]]; then
    record_fail "Challenge bond" "call reverted or returned error"
elif [[ "$BOND_RAW" == "0" ]]; then
    record_fail "Challenge bond" "challengeBondAmount is 0"
else
    BOND_ETH=$(cast from-wei "$BOND_RAW" 2>/dev/null || echo "${BOND_RAW} wei")
    record_pass "Challenge bond" "${BOND_ETH} ETH (${BOND_RAW} wei)"
fi

# ── Check 6: remainderVerifier is set ─────────────────────────────────────────
REMAINDER_VERIFIER=$(cast call "$CONTRACT_ADDRESS" "remainderVerifier()(address)" --rpc-url "$RPC_URL" 2>/dev/null || echo "")
if [[ -z "$REMAINDER_VERIFIER" ]]; then
    record_fail "Remainder verifier" "call reverted or returned empty"
elif [[ "$REMAINDER_VERIFIER" == "$ZERO_ADDRESS" ]]; then
    record_fail "Remainder verifier" "remainderVerifier is zero address"
else
    record_pass "Remainder verifier" "$REMAINDER_VERIFIER"
fi

# ── Check 7: CHALLENGE_WINDOW constant is set ─────────────────────────────────
CHALLENGE_WINDOW=$(cast call "$CONTRACT_ADDRESS" "CHALLENGE_WINDOW()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "error")
CHALLENGE_WINDOW=$(strip_cast_annotation "$CHALLENGE_WINDOW")
if [[ "$CHALLENGE_WINDOW" == "error" ]]; then
    record_fail "Challenge window" "call reverted or returned error"
elif [[ "$CHALLENGE_WINDOW" == "0" ]]; then
    record_fail "Challenge window" "CHALLENGE_WINDOW is 0"
else
    record_pass "Challenge window" "${CHALLENGE_WINDOW}s ($(( CHALLENGE_WINDOW / 60 ))m)"
fi

# ── Check 8: DISPUTE_WINDOW constant is set ──────────────────────────────────
DISPUTE_WINDOW=$(cast call "$CONTRACT_ADDRESS" "DISPUTE_WINDOW()(uint256)" --rpc-url "$RPC_URL" 2>/dev/null || echo "error")
DISPUTE_WINDOW=$(strip_cast_annotation "$DISPUTE_WINDOW")
if [[ "$DISPUTE_WINDOW" == "error" ]]; then
    record_fail "Dispute window" "call reverted or returned error"
elif [[ "$DISPUTE_WINDOW" == "0" ]]; then
    record_fail "Dispute window" "DISPUTE_WINDOW is 0"
else
    record_pass "Dispute window" "${DISPUTE_WINDOW}s ($(( DISPUTE_WINDOW / 3600 ))h)"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "=== Summary: ${PASS_COUNT}/${TOTAL_CHECKS} passed, ${FAIL_COUNT}/${TOTAL_CHECKS} failed ==="

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    echo "=== OVERALL: FAIL ==="
    exit 1
else
    echo "=== OVERALL: PASS ==="
    exit 0
fi
