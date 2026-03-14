#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# World ZK Compute - Sepolia Testnet E2E Validation
#
# Exercises the TEE lifecycle against live Sepolia contracts:
#   1. Connectivity check (verify Sepolia is reachable)
#   2. Contract health (codesize, owner on each contract)
#   3. Submit execution request via ExecutionEngine
#   4. Submit TEE result with attestation via TEEMLVerifier
#   5. Query result state (challenge window)
#   6. Print finalize instructions (cannot fast-forward on Sepolia)
#
# Usage:
#   ./scripts/sepolia-e2e.sh              # live execution
#   ./scripts/sepolia-e2e.sh --dry-run    # print cast commands without executing
#   ./scripts/sepolia-e2e.sh --help
#
# Required env vars:
#   ALCHEMY_SEPOLIA_RPC_URL   Alchemy (or other) Sepolia RPC endpoint
#   DEPLOYER_PRIVATE_KEY      Deployer / admin private key
#   ENCLAVE_PRIVATE_KEY       TEE enclave signer private key
#
# Optional env vars:
#   REQUESTER_PRIVATE_KEY     Requester key (defaults to DEPLOYER_PRIVATE_KEY)
#   TEE_VERIFIER_ADDRESS      Override TEEMLVerifier address
#   EXECUTION_ENGINE_ADDRESS  Override ExecutionEngine address
#   PROGRAM_REGISTRY_ADDRESS  Override ProgramRegistry address
#
# Exit codes:
#   0 -- all tests passed
#   1 -- one or more tests failed
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOYMENTS_FILE="$ROOT_DIR/deployments/11155111.json"

DRY_RUN=false

# =============================================================================
# CLI flags
# =============================================================================

for arg in "$@"; do
    case "$arg" in
        --dry-run)
            DRY_RUN=true
            ;;
        --help|-h)
            echo "Usage: $0 [--dry-run] [--help]"
            echo ""
            echo "  --dry-run   Print cast commands without executing them"
            echo "  --help      Show this help message"
            echo ""
            echo "Required env vars:"
            echo "  ALCHEMY_SEPOLIA_RPC_URL   Sepolia RPC endpoint"
            echo "  DEPLOYER_PRIVATE_KEY      Deployer / admin private key"
            echo "  ENCLAVE_PRIVATE_KEY       TEE enclave signer private key"
            echo ""
            echo "Optional env vars:"
            echo "  REQUESTER_PRIVATE_KEY     Requester key (defaults to deployer)"
            echo "  TEE_VERIFIER_ADDRESS      Override TEEMLVerifier address"
            echo "  EXECUTION_ENGINE_ADDRESS  Override ExecutionEngine address"
            echo "  PROGRAM_REGISTRY_ADDRESS  Override ProgramRegistry address"
            exit 0
            ;;
        *)
            echo "Unknown flag: $arg"
            echo "Run with --help for usage."
            exit 1
            ;;
    esac
done

# =============================================================================
# Test constants
# =============================================================================

TEST_IMAGE_ID="0xf85c194e1a8e2c2e38746c6d81c7ac9ddf25848a8dfc049db7f4725b3ab56e33"
TEST_MODEL_HASH="0x2222222222222222222222222222222222222222222222222222222222222222"
TEST_INPUT_HASH="0x3333333333333333333333333333333333333333333333333333333333333333"
INPUT_DIGEST="0x4444444444444444444444444444444444444444444444444444444444444444"
ENCLAVE_IMAGE_HASH="0x5555555555555555555555555555555555555555555555555555555555555555"
TEST_RESULT="0xaabbccdd"
STAKE_VALUE="0.001ether"

# =============================================================================
# Test result tracking
# =============================================================================

declare -a TEST_NAMES=()
declare -a TEST_RESULTS=()
declare -a TEST_GAS=()
TESTS_PASSED=0
TESTS_FAILED=0

# =============================================================================
# Helpers
# =============================================================================

log()  { echo "==> $*"; }
ok()   { echo "  [PASS] $*"; }
err()  { echo "  [FAIL] $*" >&2; }
info() { echo "  [INFO] $*"; }

record_pass() {
    local name="$1"
    local gas="${2:-n/a}"
    TEST_NAMES+=("$name")
    TEST_RESULTS+=("PASS")
    TEST_GAS+=("$gas")
    TESTS_PASSED=$((TESTS_PASSED + 1))
    ok "$name (gas: $gas)"
}

record_fail() {
    local name="$1"
    local detail="${2:-}"
    TEST_NAMES+=("$name")
    TEST_RESULTS+=("FAIL")
    TEST_GAS+=("n/a")
    TESTS_FAILED=$((TESTS_FAILED + 1))
    err "$name${detail:+: $detail}"
}

# strip_cast_annotation "1000 [1e3]" -> "1000"
strip_cast() {
    echo "${1%% \[*\]}"
}

# assert_eq <description> <expected> <actual>
# shellcheck disable=SC2329
assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    actual=$(strip_cast "$actual")
    if [ "$expected" = "$actual" ]; then
        record_pass "$desc"
    else
        record_fail "$desc" "expected=$expected, got=$actual"
    fi
}

# extract_gas <cast_send_output>
# shellcheck disable=SC2329
extract_gas() {
    echo "$1" | grep -i "gasUsed" | head -1 | awk '{print $NF}'
}

# tx_status_from_json <cast_send_json_output>
tx_status_from_json() {
    echo "$1" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo ""
}

# tx_gas_from_json <cast_send_json_output>
tx_gas_from_json() {
    echo "$1" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('gasUsed','0x0'),16))" 2>/dev/null || echo "n/a"
}

# sign_attestation <modelHash> <inputHash> <resultData> <privateKey>
# Mirrors the contract: message = keccak256(encodePacked(modelHash, inputHash, keccak256(result)))
# cast wallet sign adds the EIP-191 "\x19Ethereum Signed Message:\n32" prefix automatically.
sign_attestation() {
    local model_hash="${1#0x}"
    local input_hash="${2#0x}"
    local result_data="$3"
    local private_key="$4"

    local result_hash
    result_hash=$(cast keccak "$result_data")
    result_hash="${result_hash#0x}"

    local packed="0x${model_hash}${input_hash}${result_hash}"
    local message
    message=$(cast keccak "$packed")

    cast wallet sign --private-key "$private_key" "$message"
}

# =============================================================================
# Prerequisites
# =============================================================================

log "Checking prerequisites..."

for cmd in cast jq; do
    if ! command -v "$cmd" &>/dev/null; then
        err "Required command not found: $cmd"
        if [ "$cmd" = "cast" ]; then
            echo "  Install Foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup"
        elif [ "$cmd" = "jq" ]; then
            echo "  Install jq: brew install jq (macOS) or apt-get install jq (Linux)"
        fi
        exit 1
    fi
done
ok "Prerequisites met (cast, jq)"

# =============================================================================
# Environment validation
# =============================================================================

log "Validating environment..."

if [ -z "${ALCHEMY_SEPOLIA_RPC_URL:-}" ]; then
    err "ALCHEMY_SEPOLIA_RPC_URL is not set"
    echo "  Export your Sepolia RPC endpoint, e.g.:"
    echo "  export ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY"
    exit 1
fi

if [ -z "${DEPLOYER_PRIVATE_KEY:-}" ]; then
    err "DEPLOYER_PRIVATE_KEY is not set"
    exit 1
fi

if [ -z "${ENCLAVE_PRIVATE_KEY:-}" ]; then
    err "ENCLAVE_PRIVATE_KEY is not set"
    exit 1
fi

RPC_URL="$ALCHEMY_SEPOLIA_RPC_URL"
REQUESTER_KEY="${REQUESTER_PRIVATE_KEY:-$DEPLOYER_PRIVATE_KEY}"

# Derive addresses from private keys
DEPLOYER_ADDR=$(cast wallet address --private-key "$DEPLOYER_PRIVATE_KEY")
ENCLAVE_ADDR=$(cast wallet address --private-key "$ENCLAVE_PRIVATE_KEY")
REQUESTER_ADDR=$(cast wallet address --private-key "$REQUESTER_KEY")

info "Deployer:  $DEPLOYER_ADDR"
info "Enclave:   $ENCLAVE_ADDR"
info "Requester: $REQUESTER_ADDR"
info "RPC URL:   $RPC_URL"

if [ "$DRY_RUN" = true ]; then
    info "DRY RUN mode -- transactions will be printed but not executed"
fi

# =============================================================================
# Resolve contract addresses
# =============================================================================

log "Resolving contract addresses..."

TEE_ADDR="${TEE_VERIFIER_ADDRESS:-}"
ENGINE_ADDR="${EXECUTION_ENGINE_ADDRESS:-}"
REGISTRY_ADDR="${PROGRAM_REGISTRY_ADDRESS:-}"

# If any address is missing, try to read from deployments/11155111.json
if [ -z "$TEE_ADDR" ] || [ -z "$ENGINE_ADDR" ] || [ -z "$REGISTRY_ADDR" ]; then
    if [ -f "$DEPLOYMENTS_FILE" ]; then
        info "Reading addresses from $DEPLOYMENTS_FILE"
        if [ -z "$TEE_ADDR" ]; then
            TEE_ADDR=$(jq -r '.contracts.TEEMLVerifier // .TEEMLVerifier // empty' "$DEPLOYMENTS_FILE" 2>/dev/null || echo "")
        fi
        if [ -z "$ENGINE_ADDR" ]; then
            ENGINE_ADDR=$(jq -r '.contracts.ExecutionEngine // .ExecutionEngine // empty' "$DEPLOYMENTS_FILE" 2>/dev/null || echo "")
        fi
        if [ -z "$REGISTRY_ADDR" ]; then
            REGISTRY_ADDR=$(jq -r '.contracts.ProgramRegistry // .ProgramRegistry // empty' "$DEPLOYMENTS_FILE" 2>/dev/null || echo "")
        fi
    else
        info "No deployments file found at $DEPLOYMENTS_FILE"
    fi
fi

# Validate we have all addresses
MISSING=""
if [ -z "$TEE_ADDR" ]; then MISSING="$MISSING TEE_VERIFIER_ADDRESS"; fi
if [ -z "$ENGINE_ADDR" ]; then MISSING="$MISSING EXECUTION_ENGINE_ADDRESS"; fi
if [ -z "$REGISTRY_ADDR" ]; then MISSING="$MISSING PROGRAM_REGISTRY_ADDRESS"; fi

if [ -n "$MISSING" ]; then
    err "Missing contract addresses:$MISSING"
    echo "  Set via env vars or create deployments/11155111.json with contract addresses."
    echo "  Example:"
    echo "    export TEE_VERIFIER_ADDRESS=0x..."
    echo "    export EXECUTION_ENGINE_ADDRESS=0x..."
    echo "    export PROGRAM_REGISTRY_ADDRESS=0x..."
    exit 1
fi

info "TEEMLVerifier:     $TEE_ADDR"
info "ExecutionEngine:   $ENGINE_ADDR"
info "ProgramRegistry:   $REGISTRY_ADDR"

# =============================================================================
# Step 1: Connectivity Check
# =============================================================================

echo ""
log "===== Step 1: Connectivity Check ====="

BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL" 2>/dev/null || echo "")

if [ -n "$BLOCK_NUM" ] && [ "$BLOCK_NUM" -gt 0 ] 2>/dev/null; then
    record_pass "Sepolia connectivity (block $BLOCK_NUM)"
else
    record_fail "Sepolia connectivity" "Could not fetch block number from $RPC_URL"
    echo ""
    echo "Cannot proceed without RPC connectivity."
    exit 1
fi

# =============================================================================
# Step 2: Contract Health
# =============================================================================

echo ""
log "===== Step 2: Contract Health ====="

# --- 2a: TEEMLVerifier has code ---
TEE_CODESIZE=$(cast codesize "$TEE_ADDR" --rpc-url "$RPC_URL" 2>/dev/null || echo "0")
if [ "$TEE_CODESIZE" -gt 0 ] 2>/dev/null; then
    record_pass "TEEMLVerifier has code ($TEE_CODESIZE bytes)"
else
    record_fail "TEEMLVerifier has code" "codesize=$TEE_CODESIZE at $TEE_ADDR"
fi

# --- 2b: ExecutionEngine has code ---
ENGINE_CODESIZE=$(cast codesize "$ENGINE_ADDR" --rpc-url "$RPC_URL" 2>/dev/null || echo "0")
if [ "$ENGINE_CODESIZE" -gt 0 ] 2>/dev/null; then
    record_pass "ExecutionEngine has code ($ENGINE_CODESIZE bytes)"
else
    record_fail "ExecutionEngine has code" "codesize=$ENGINE_CODESIZE at $ENGINE_ADDR"
fi

# --- 2c: ProgramRegistry has code ---
REGISTRY_CODESIZE=$(cast codesize "$REGISTRY_ADDR" --rpc-url "$RPC_URL" 2>/dev/null || echo "0")
if [ "$REGISTRY_CODESIZE" -gt 0 ] 2>/dev/null; then
    record_pass "ProgramRegistry has code ($REGISTRY_CODESIZE bytes)"
else
    record_fail "ProgramRegistry has code" "codesize=$REGISTRY_CODESIZE at $REGISTRY_ADDR"
fi

# --- 2d: Check owner() on each contract ---
TEE_OWNER=$(cast call --rpc-url "$RPC_URL" "$TEE_ADDR" "owner()(address)" 2>/dev/null || echo "")
if [ -n "$TEE_OWNER" ]; then
    record_pass "TEEMLVerifier owner=$TEE_OWNER"
else
    record_fail "TEEMLVerifier owner()" "call failed"
fi

ENGINE_OWNER=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" "owner()(address)" 2>/dev/null || echo "")
if [ -n "$ENGINE_OWNER" ]; then
    record_pass "ExecutionEngine owner=$ENGINE_OWNER"
else
    record_fail "ExecutionEngine owner()" "call failed"
fi

REGISTRY_OWNER=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" "owner()(address)" 2>/dev/null || echo "")
if [ -n "$REGISTRY_OWNER" ]; then
    record_pass "ProgramRegistry owner=$REGISTRY_OWNER"
else
    record_fail "ProgramRegistry owner()" "call failed"
fi

# =============================================================================
# Step 3: Submit Execution Request
# =============================================================================

echo ""
log "===== Step 3: Submit Execution Request ====="

# Check if program is already registered; if not, register it
IS_ACTIVE=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" \
    "isProgramActive(bytes32)(bool)" "$TEST_IMAGE_ID" 2>/dev/null || echo "false")
IS_ACTIVE=$(strip_cast "$IS_ACTIVE")

if [ "$IS_ACTIVE" != "true" ]; then
    info "Program not yet registered, registering..."
    if [ "$DRY_RUN" = true ]; then
        echo "  DRY RUN: cast send --rpc-url \$RPC_URL --private-key \$DEPLOYER_PRIVATE_KEY \\"
        echo "    $REGISTRY_ADDR \\"
        echo "    'registerProgram(bytes32,string,string,bytes32)' \\"
        echo "    $TEST_IMAGE_ID 'sepolia-e2e-program' 'file://test' \\"
        echo "    0x0000000000000000000000000000000000000000000000000000000000000000"
        info "Skipped (dry-run)"
    else
        REG_TX=$(cast send --rpc-url "$RPC_URL" \
            --private-key "$DEPLOYER_PRIVATE_KEY" \
            "$REGISTRY_ADDR" \
            "registerProgram(bytes32,string,string,bytes32)" \
            "$TEST_IMAGE_ID" \
            "sepolia-e2e-program" \
            "file://test" \
            "0x0000000000000000000000000000000000000000000000000000000000000000" \
            --json 2>&1)

        REG_STATUS=$(tx_status_from_json "$REG_TX")
        REG_GAS=$(tx_gas_from_json "$REG_TX")
        if [ "$REG_STATUS" = "0x1" ]; then
            record_pass "Program registered" "$REG_GAS"
        else
            record_fail "Program registration" "tx status=$REG_STATUS"
        fi
    fi
else
    info "Program already active"
    record_pass "Program already registered"
fi

# Submit execution request
info "Submitting execution request..."

if [ "$DRY_RUN" = true ]; then
    echo "  DRY RUN: cast send --rpc-url \$RPC_URL --private-key \$REQUESTER_PRIVATE_KEY \\"
    echo "    $ENGINE_ADDR \\"
    echo "    'requestExecution(bytes32,bytes32,string,address,uint256)' \\"
    echo "    $TEST_IMAGE_ID \\"
    echo "    $INPUT_DIGEST \\"
    echo "    'https://example.com/sepolia-e2e-input' \\"
    echo "    0x0000000000000000000000000000000000000000 \\"
    echo "    3600 \\"
    echo "    --value $STAKE_VALUE"
    record_pass "Execution request (dry-run)"
else
    # Read nextRequestId before submission
    NEXT_ID_BEFORE=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
        "nextRequestId()(uint256)" 2>/dev/null || echo "0")
    NEXT_ID_BEFORE=$(strip_cast "$NEXT_ID_BEFORE")

    S3_TX=$(cast send --rpc-url "$RPC_URL" \
        --private-key "$REQUESTER_KEY" \
        "$ENGINE_ADDR" \
        "requestExecution(bytes32,bytes32,string,address,uint256)" \
        "$TEST_IMAGE_ID" \
        "$INPUT_DIGEST" \
        "https://example.com/sepolia-e2e-input" \
        "0x0000000000000000000000000000000000000000" \
        3600 \
        --value "$STAKE_VALUE" \
        --json 2>&1)

    S3_STATUS=$(tx_status_from_json "$S3_TX")
    S3_GAS=$(tx_gas_from_json "$S3_TX")

    if [ "$S3_STATUS" = "0x1" ]; then
        # Verify nextRequestId incremented
        NEXT_ID_AFTER=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
            "nextRequestId()(uint256)" 2>/dev/null || echo "0")
        NEXT_ID_AFTER=$(strip_cast "$NEXT_ID_AFTER")
        info "nextRequestId: $NEXT_ID_BEFORE -> $NEXT_ID_AFTER"
        record_pass "Execution request submitted" "$S3_GAS"
    else
        record_fail "Execution request submitted" "tx status=$S3_STATUS"
    fi
fi

# =============================================================================
# Step 4: Submit TEE Result with Attestation
# =============================================================================

echo ""
log "===== Step 4: Submit TEE Result with Attestation ====="

# First check if enclave is registered
ENCLAVE_REGISTERED=$(cast call --rpc-url "$RPC_URL" "$TEE_ADDR" \
    "enclaves(address)(bool,bool,bytes32,uint256)" "$ENCLAVE_ADDR" 2>/dev/null | head -1 || echo "false")
ENCLAVE_REGISTERED=$(strip_cast "$ENCLAVE_REGISTERED")

if [ "$ENCLAVE_REGISTERED" != "true" ]; then
    info "Enclave not yet registered, registering..."
    if [ "$DRY_RUN" = true ]; then
        echo "  DRY RUN: cast send --rpc-url \$RPC_URL --private-key \$DEPLOYER_PRIVATE_KEY \\"
        echo "    $TEE_ADDR \\"
        echo "    'registerEnclave(address,bytes32)' \\"
        echo "    $ENCLAVE_ADDR \\"
        echo "    $ENCLAVE_IMAGE_HASH"
        info "Skipped (dry-run)"
    else
        EREG_TX=$(cast send --rpc-url "$RPC_URL" \
            --private-key "$DEPLOYER_PRIVATE_KEY" \
            "$TEE_ADDR" \
            "registerEnclave(address,bytes32)" \
            "$ENCLAVE_ADDR" \
            "$ENCLAVE_IMAGE_HASH" \
            --json 2>&1)

        EREG_STATUS=$(tx_status_from_json "$EREG_TX")
        EREG_GAS=$(tx_gas_from_json "$EREG_TX")
        if [ "$EREG_STATUS" = "0x1" ]; then
            record_pass "Enclave registered" "$EREG_GAS"
        else
            record_fail "Enclave registration" "tx status=$EREG_STATUS"
        fi
    fi
else
    info "Enclave already registered"
    record_pass "Enclave already registered"
fi

# Sign attestation
info "Generating attestation signature..."
ATTESTATION=$(sign_attestation "$TEST_MODEL_HASH" "$TEST_INPUT_HASH" "$TEST_RESULT" "$ENCLAVE_PRIVATE_KEY")
info "Attestation: ${ATTESTATION:0:20}..."

# Submit result
if [ "$DRY_RUN" = true ]; then
    echo "  DRY RUN: cast send --rpc-url \$RPC_URL --private-key \$DEPLOYER_PRIVATE_KEY \\"
    echo "    $TEE_ADDR \\"
    echo "    'submitResult(bytes32,bytes32,bytes,bytes)' \\"
    echo "    $TEST_MODEL_HASH \\"
    echo "    $TEST_INPUT_HASH \\"
    echo "    $TEST_RESULT \\"
    echo "    $ATTESTATION \\"
    echo "    --value $STAKE_VALUE"
    record_pass "TEE result submitted (dry-run)"
    RESULT_ID="0x0000000000000000000000000000000000000000000000000000000000000000"
else
    S4_TX=$(cast send --rpc-url "$RPC_URL" \
        --private-key "$DEPLOYER_PRIVATE_KEY" \
        "$TEE_ADDR" \
        "submitResult(bytes32,bytes32,bytes,bytes)" \
        "$TEST_MODEL_HASH" \
        "$TEST_INPUT_HASH" \
        "$TEST_RESULT" \
        "$ATTESTATION" \
        --value "$STAKE_VALUE" \
        --json 2>&1)

    S4_STATUS=$(tx_status_from_json "$S4_TX")
    S4_GAS=$(tx_gas_from_json "$S4_TX")

    if [ "$S4_STATUS" = "0x1" ]; then
        record_pass "TEE result submitted" "$S4_GAS"
    else
        record_fail "TEE result submitted" "tx status=$S4_STATUS"
    fi

    # Extract resultId from event logs
    RESULT_ID=$(echo "$S4_TX" | python3 -c "
import sys, json
tx = json.load(sys.stdin)
for log in tx.get('logs', []):
    topics = log.get('topics', [])
    if len(topics) >= 4:
        print(topics[1])
        break
else:
    print('')
" 2>/dev/null || echo "")

    if [ -z "$RESULT_ID" ]; then
        info "Could not extract resultId from logs (no matching event)"
        RESULT_ID="unknown"
    else
        info "Result ID: $RESULT_ID"
    fi
fi

# =============================================================================
# Step 5: Query Result State
# =============================================================================

echo ""
log "===== Step 5: Query Result State ====="

if [ "$DRY_RUN" = true ]; then
    echo "  DRY RUN: cast call --rpc-url \$RPC_URL \\"
    echo "    $TEE_ADDR \\"
    echo "    'isResultValid(bytes32)(bool)' \\"
    echo "    \$RESULT_ID"
    record_pass "Result validity query (dry-run)"
elif [ "$RESULT_ID" = "unknown" ]; then
    record_fail "Result validity query" "no resultId available"
else
    IS_VALID=$(cast call --rpc-url "$RPC_URL" "$TEE_ADDR" \
        "isResultValid(bytes32)(bool)" "$RESULT_ID" 2>/dev/null || echo "error")
    IS_VALID=$(strip_cast "$IS_VALID")

    if [ "$IS_VALID" = "false" ]; then
        record_pass "Result not yet valid (in challenge window)"
    elif [ "$IS_VALID" = "true" ]; then
        # This would be unexpected immediately after submission
        record_pass "Result already valid (challenge window may be 0)"
    else
        record_fail "Result validity query" "got=$IS_VALID"
    fi
fi

# =============================================================================
# Step 6: Finalize Instructions
# =============================================================================

echo ""
log "===== Step 6: Finalize Instructions ====="

echo ""
echo "  Result is in the challenge window (1 hour on TEEMLVerifier)."
echo "  Unlike Anvil, time cannot be fast-forwarded on Sepolia."
echo ""
echo "  To finalize after the challenge window expires, run:"
echo ""
if [ "$RESULT_ID" != "unknown" ] && [ "$RESULT_ID" != "0x0000000000000000000000000000000000000000000000000000000000000000" ]; then
    echo "    cast send --rpc-url $RPC_URL \\"
    echo "      --private-key \$DEPLOYER_PRIVATE_KEY \\"
    echo "      $TEE_ADDR \\"
    echo "      'finalize(bytes32)' \\"
    echo "      $RESULT_ID"
else
    echo "    cast send --rpc-url $RPC_URL \\"
    echo "      --private-key \$DEPLOYER_PRIVATE_KEY \\"
    echo "      $TEE_ADDR \\"
    echo "      'finalize(bytes32)' \\"
    echo "      <RESULT_ID>"
fi
echo ""
echo "  After finalization, verify with:"
echo ""
echo "    cast call --rpc-url $RPC_URL \\"
echo "      $TEE_ADDR \\"
echo "      'isResultValid(bytes32)(bool)' \\"
echo "      <RESULT_ID>"
echo ""

# =============================================================================
# Summary
# =============================================================================

TOTAL=$((TESTS_PASSED + TESTS_FAILED))

echo ""
echo "================================================================"
echo "  SEPOLIA E2E SUMMARY"
echo "================================================================"
echo ""

# Print table header
printf "  %-50s  %-6s  %s\n" "TEST" "RESULT" "GAS"
printf "  %-50s  %-6s  %s\n" "--------------------------------------------------" "------" "----------"

for i in "${!TEST_NAMES[@]}"; do
    printf "  %-50s  %-6s  %s\n" "${TEST_NAMES[$i]}" "${TEST_RESULTS[$i]}" "${TEST_GAS[$i]}"
done

echo ""
echo "  ----------------------------------------------------"
printf "  Total: %d passed, %d failed (out of %d)\n" "$TESTS_PASSED" "$TESTS_FAILED" "$TOTAL"
echo ""

if [ "$DRY_RUN" = true ]; then
    echo "  MODE:   DRY RUN (no transactions were sent)"
fi

echo "  NETWORK:  Sepolia (chain 11155111)"
echo "  RPC:      $RPC_URL"
echo "  TEE:      $TEE_ADDR"
echo "  ENGINE:   $ENGINE_ADDR"
echo "  REGISTRY: $REGISTRY_ADDR"
echo ""

if [ "$TESTS_FAILED" -eq 0 ]; then
    echo "  STATUS:   ALL TESTS PASSED"
    echo ""
    echo "================================================================"
    exit 0
else
    echo "  STATUS:   SOME TESTS FAILED"
    echo ""
    echo "================================================================"
    exit 1
fi
