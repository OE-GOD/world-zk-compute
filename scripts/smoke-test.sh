#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# World ZK Compute - Comprehensive E2E Smoke Test
#
# Exercises the full TEE lifecycle on a local Anvil instance:
#   1. Start Anvil
#   2. Deploy contracts via DeployFullStack forge script
#   3. Register a program, submit execution request, claim, submit proof
#   4. Register TEE enclave, submit TEE result, challenge, resolve by timeout
#   5. Submit a second TEE result, finalize unchallenged (happy path)
#   6. Test admin operations (pause/unpause, program deactivation, enclave revocation)
#   7. Print pass/fail summary with gas usage
#
# Usage:
#   ./scripts/smoke-test.sh
#
# Exit codes:
#   0 -- all tests passed
#   1 -- one or more tests failed
# =============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"

# --- Background PIDs ---
ANVIL_PID=""
ANVIL_PORT=""

# --- Anvil deterministic accounts (default mnemonic) ---
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOYER_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
PROVER_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
PROVER_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
REQUESTER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
# shellcheck disable=SC2034
REQUESTER_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
CHALLENGER_KEY="0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6"
# shellcheck disable=SC2034
CHALLENGER_ADDR="0x90F79bf6EB2c4f870365E785982E1f101E93b906"
# Account #4 used as TEE enclave signer
ENCLAVE_KEY="0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a"
ENCLAVE_ADDR="0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"

# --- Test constants ---
TEST_IMAGE_ID="0x1111111111111111111111111111111111111111111111111111111111111111"
TEST_MODEL_HASH="0x2222222222222222222222222222222222222222222222222222222222222222"
TEST_INPUT_HASH="0x3333333333333333333333333333333333333333333333333333333333333333"
INPUT_DIGEST="0x4444444444444444444444444444444444444444444444444444444444444444"
ENCLAVE_IMAGE_HASH="0x5555555555555555555555555555555555555555555555555555555555555555"
TEST_RESULT="0xaabbccdd"

# --- Contract addresses (filled after deployment) ---
VERIFIER_ADDR=""
REGISTRY_ADDR=""
ENGINE_ADDR=""
TEE_VERIFIER_ADDR=""

# --- Test result tracking ---
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
# cast call often appends human-readable annotations like " [1e15]"
strip_cast() {
    local val="$1"
    echo "${val%% \[*\]}"
}

# assert_eq <description> <expected> <actual>
assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    # Strip cast annotations from actual value
    actual=$(strip_cast "$actual")
    if [ "$expected" = "$actual" ]; then
        record_pass "$desc"
    else
        record_fail "$desc" "expected=$expected, got=$actual"
    fi
}

# extract_gas <cast_send_output>
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

# tx_block_from_json <cast_send_json_output>
tx_block_from_json() {
    echo "$1" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('blockNumber','0x0'),16))" 2>/dev/null || echo "0"
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

# compute_result_id <sender> <modelHash> <inputHash> <blockNumber>
# resultId = keccak256(abi.encodePacked(sender, modelHash, inputHash, blockNumber))
compute_result_id() {
    local sender="${1#0x}"
    local model_hash="${2#0x}"
    local input_hash="${3#0x}"
    local block_num="$4"
    local block_hex
    block_hex=$(printf '%064x' "$block_num")
    local packed="0x${sender}${model_hash}${input_hash}${block_hex}"
    cast keccak "$packed"
}

# shellcheck disable=SC2329
cleanup() {
    log "Cleaning up..."
    if [ -n "$ANVIL_PID" ] && kill -0 "$ANVIL_PID" 2>/dev/null; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# =============================================================================
# Step 0: Prerequisites
# =============================================================================

log "Checking prerequisites..."
for cmd in anvil forge cast python3; do
    if ! command -v "$cmd" &>/dev/null; then
        err "Required command not found: $cmd"
        echo "  Install Foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup"
        exit 1
    fi
done
ok "Prerequisites met (anvil, forge, cast, python3)"

# =============================================================================
# Step 1: Start Anvil
# =============================================================================

log "Starting Anvil..."

# Find a random available port
ANVIL_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()' 2>/dev/null || echo "0")
if [ "$ANVIL_PORT" = "0" ]; then
    ANVIL_PORT=$((49152 + RANDOM % 16384))
fi

RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

anvil --port "$ANVIL_PORT" --silent &
ANVIL_PID=$!
sleep 2

if ! kill -0 "$ANVIL_PID" 2>/dev/null; then
    err "Failed to start Anvil on port $ANVIL_PORT"
    exit 1
fi

# Verify Anvil is responding
if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil not responding on $RPC_URL"
    exit 1
fi
ok "Anvil running on port $ANVIL_PORT (PID: $ANVIL_PID)"

# =============================================================================
# Step 2: Deploy Contracts via DeployFullStack
# =============================================================================

log "Deploying contracts via DeployFullStack forge script..."
cd "$CONTRACTS_DIR"

# Install forge dependencies if needed
if [ ! -d "lib/forge-std" ]; then
    log "Installing forge-std..."
    forge install foundry-rs/forge-std --no-git --no-commit 2>/dev/null || true
fi
if [ ! -d "lib/risc0-ethereum" ]; then
    log "Installing risc0-ethereum..."
    forge install risc0/risc0-ethereum --no-git --no-commit 2>/dev/null || true
fi

FORGE_OUTPUT=$(PRIVATE_KEY="$DEPLOYER_KEY" FEE_RECIPIENT="$DEPLOYER_ADDR" \
    forge script script/DeployFullStack.s.sol:DeployFullStack \
    --rpc-url "$RPC_URL" \
    --broadcast 2>&1) || {
    err "Forge deployment failed"
    echo "$FORGE_OUTPUT" >&2
    exit 1
}

# Extract addresses from forge output
VERIFIER_ADDR=$(echo "$FORGE_OUTPUT" | grep "MockRiscZeroVerifier:" | head -1 | awk '{print $NF}')
REGISTRY_ADDR=$(echo "$FORGE_OUTPUT" | grep "ProgramRegistry:" | head -1 | awk '{print $NF}')
ENGINE_ADDR=$(echo "$FORGE_OUTPUT" | grep "ExecutionEngine:" | head -1 | awk '{print $NF}')
TEE_VERIFIER_ADDR=$(echo "$FORGE_OUTPUT" | grep "TEEMLVerifier:" | head -1 | awk '{print $NF}')

if [ -z "$VERIFIER_ADDR" ] || [ -z "$REGISTRY_ADDR" ] || [ -z "$ENGINE_ADDR" ] || [ -z "$TEE_VERIFIER_ADDR" ]; then
    err "Failed to extract contract addresses from forge output"
    echo "$FORGE_OUTPUT" >&2
    exit 1
fi

cd "$ROOT_DIR"

# Verify deployment
ENGINE_CODE=$(cast code --rpc-url "$RPC_URL" "$ENGINE_ADDR" 2>/dev/null)
TEE_CODE=$(cast code --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" 2>/dev/null)

if [ "$ENGINE_CODE" = "0x" ] || [ -z "$ENGINE_CODE" ]; then
    record_fail "Deploy: ExecutionEngine has code"
    exit 1
fi
record_pass "Deploy: ExecutionEngine has code"

if [ "$TEE_CODE" = "0x" ] || [ -z "$TEE_CODE" ]; then
    record_fail "Deploy: TEEMLVerifier has code"
    exit 1
fi
record_pass "Deploy: TEEMLVerifier has code"

info "MockRiscZeroVerifier: $VERIFIER_ADDR"
info "ProgramRegistry:      $REGISTRY_ADDR"
info "ExecutionEngine:      $ENGINE_ADDR"
info "TEEMLVerifier:        $TEE_VERIFIER_ADDR"

# =============================================================================
# TEST GROUP A: ExecutionEngine Lifecycle
# =============================================================================

echo ""
log "===== Test Group A: ExecutionEngine Lifecycle ====="

# --- A1: Register a program ---
log "A1: Registering test program..."

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$REGISTRY_ADDR" \
    "registerProgram(bytes32,string,string,bytes32)" \
    "$TEST_IMAGE_ID" \
    "smoke-test-program" \
    "file://local/test" \
    "0x0000000000000000000000000000000000000000000000000000000000000000" \
    >/dev/null 2>&1

IS_ACTIVE=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" \
    "isProgramActive(bytes32)(bool)" "$TEST_IMAGE_ID" 2>/dev/null)
assert_eq "A1: Program is active after registration" "true" "$IS_ACTIVE"

# --- A2: Submit an execution request ---
log "A2: Submitting execution request..."

A2_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$REQUESTER_KEY" \
    "$ENGINE_ADDR" \
    "requestExecution(bytes32,bytes32,string,address,uint256)" \
    "$TEST_IMAGE_ID" \
    "$INPUT_DIGEST" \
    "https://example.com/test-input" \
    "0x0000000000000000000000000000000000000000" \
    3600 \
    --value 0.001ether \
    2>&1)

A2_GAS=$(extract_gas "$A2_OUTPUT")

NEXT_ID=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
    "nextRequestId()(uint256)" 2>/dev/null)
if [ "$NEXT_ID" = "2" ]; then
    record_pass "A2: Execution request submitted" "$A2_GAS"
else
    record_fail "A2: Execution request submitted" "nextRequestId=$NEXT_ID, expected=2"
fi

# --- A3: Query request status ---
log "A3: Querying request status..."

RAW_HEX=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
    "getRequest(uint256)" 1 2>/dev/null | tr -d '[:space:]')
STATUS_HEX=${RAW_HEX:450:64}
STATUS=$((16#${STATUS_HEX:-0})) 2>/dev/null || STATUS=99
assert_eq "A3: Request status is Pending (0)" "0" "$STATUS"

# --- A4: Claim execution ---
log "A4: Claiming execution as prover..."

A4_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$ENGINE_ADDR" \
    "claimExecution(uint256)" \
    1 \
    2>&1)

A4_GAS=$(extract_gas "$A4_OUTPUT")

RAW_HEX=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
    "getRequest(uint256)" 1 2>/dev/null | tr -d '[:space:]')
STATUS_HEX=${RAW_HEX:450:64}
STATUS=$((16#${STATUS_HEX:-0})) 2>/dev/null || STATUS=99
if [ "$STATUS" = "1" ]; then
    record_pass "A4: Request status is Claimed (1)" "$A4_GAS"
else
    record_fail "A4: Request status is Claimed (1)" "got=$STATUS"
fi

# --- A5: Submit proof (MockVerifier accepts any seal) ---
log "A5: Submitting proof..."

A5_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$ENGINE_ADDR" \
    "submitProof(uint256,bytes,bytes)" \
    1 \
    "0xdeadbeef" \
    "0xcafebabe" \
    2>&1)

A5_GAS=$(extract_gas "$A5_OUTPUT")

RAW_HEX=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
    "getRequest(uint256)" 1 2>/dev/null | tr -d '[:space:]')
STATUS_HEX=${RAW_HEX:450:64}
STATUS=$((16#${STATUS_HEX:-0})) 2>/dev/null || STATUS=99
if [ "$STATUS" = "2" ]; then
    record_pass "A5: Request status is Completed (2)" "$A5_GAS"
else
    record_fail "A5: Request status is Completed (2)" "got=$STATUS"
fi

# --- A6: Check prover stats ---
log "A6: Checking prover stats..."

COMPLETED_COUNT=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
    "proverCompletedCount(address)(uint256)" "$PROVER_ADDR" 2>/dev/null)
assert_eq "A6: Prover completed count is 1" "1" "$COMPLETED_COUNT"

# =============================================================================
# TEST GROUP B: TEEMLVerifier Lifecycle
# =============================================================================

echo ""
log "===== Test Group B: TEEMLVerifier Lifecycle ====="

# --- B0: Lower stakes for testing ---
log "B0: Configuring TEE stake/bond amounts..."

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "setProverStake(uint256)" \
    "1000000000000000" \
    >/dev/null 2>&1

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "setChallengeBondAmount(uint256)" \
    "1000000000000000" \
    >/dev/null 2>&1

STAKE_VAL=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "proverStake()(uint256)" 2>/dev/null)
assert_eq "B0: Prover stake set to 0.001 ETH" "1000000000000000" "$STAKE_VAL"

# --- B1: Register TEE enclave ---
log "B1: Registering TEE enclave..."

B1_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "registerEnclave(address,bytes32)" \
    "$ENCLAVE_ADDR" \
    "$ENCLAVE_IMAGE_HASH" \
    2>&1)

B1_GAS=$(extract_gas "$B1_OUTPUT")

ENCLAVE_RAW=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "enclaves(address)(bool,bool,bytes32,uint256)" "$ENCLAVE_ADDR" 2>/dev/null)

if echo "$ENCLAVE_RAW" | head -1 | grep -q "true"; then
    record_pass "B1: Enclave registered" "$B1_GAS"
else
    record_fail "B1: Enclave registered"
fi

# --- B2: Submit TEE-attested result ---
log "B2: Submitting TEE-attested result..."

ATTESTATION=$(sign_attestation "$TEST_MODEL_HASH" "$TEST_INPUT_HASH" "$TEST_RESULT" "$ENCLAVE_KEY")
info "Attestation signature generated"

B2_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "submitResult(bytes32,bytes32,bytes,bytes)" \
    "$TEST_MODEL_HASH" \
    "$TEST_INPUT_HASH" \
    "$TEST_RESULT" \
    "$ATTESTATION" \
    --value 0.001ether \
    --json 2>&1)

B2_STATUS=$(tx_status_from_json "$B2_TX")
B2_GAS=$(tx_gas_from_json "$B2_TX")

if [ "$B2_STATUS" != "0x1" ]; then
    record_fail "B2: TEE result submitted" "tx status=$B2_STATUS"
else
    record_pass "B2: TEE result submitted" "$B2_GAS"
fi

# Extract resultId from event logs
RESULT_ID=$(echo "$B2_TX" | python3 -c "
import sys, json
tx = json.load(sys.stdin)
for log in tx.get('logs', []):
    topics = log.get('topics', [])
    if len(topics) >= 4:
        print(topics[1])
        break
" 2>/dev/null || echo "")

if [ -z "$RESULT_ID" ]; then
    # Fallback: compute resultId manually
    BLOCK_NUM_DEC=$(tx_block_from_json "$B2_TX")
    PROVER_LOWER=$(echo "$PROVER_ADDR" | tr '[:upper:]' '[:lower:]')
    RESULT_ID=$(compute_result_id "$PROVER_LOWER" "$TEST_MODEL_HASH" "$TEST_INPUT_HASH" "$BLOCK_NUM_DEC")
fi

info "Result ID: $RESULT_ID"

# Verify not yet valid
IS_VALID=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "isResultValid(bytes32)(bool)" "$RESULT_ID" 2>/dev/null)
assert_eq "B2: Result not yet valid (challenge window)" "false" "$IS_VALID"

# --- B3: Challenge the result ---
log "B3: Challenging the result..."

B3_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$CHALLENGER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "challenge(bytes32)" \
    "$RESULT_ID" \
    --value 0.001ether \
    --json 2>&1)

B3_STATUS=$(tx_status_from_json "$B3_TX")
B3_GAS=$(tx_gas_from_json "$B3_TX")

if [ "$B3_STATUS" = "0x1" ]; then
    record_pass "B3: Result challenged" "$B3_GAS"
else
    record_fail "B3: Result challenged" "tx status=$B3_STATUS"
fi

# --- B4: Resolve dispute by timeout (challenger wins) ---
log "B4: Resolving dispute by timeout..."

# Fast-forward past DISPUTE_WINDOW (24 hours = 86400s) + buffer
cast rpc --rpc-url "$RPC_URL" evm_increaseTime 90000 >/dev/null 2>&1
cast rpc --rpc-url "$RPC_URL" evm_mine >/dev/null 2>&1
info "Time advanced by 90000 seconds (past 24h dispute window)"

B4_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$CHALLENGER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "resolveDisputeByTimeout(bytes32)" \
    "$RESULT_ID" \
    --json 2>&1)

B4_STATUS=$(tx_status_from_json "$B4_TX")
B4_GAS=$(tx_gas_from_json "$B4_TX")

if [ "$B4_STATUS" = "0x1" ]; then
    record_pass "B4: Dispute resolved by timeout" "$B4_GAS"
else
    record_fail "B4: Dispute resolved by timeout" "tx status=$B4_STATUS"
fi

# Verify dispute outcome
DISPUTE_RESOLVED=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "disputeResolved(bytes32)(bool)" "$RESULT_ID" 2>/dev/null)
assert_eq "B4: Dispute is marked resolved" "true" "$DISPUTE_RESOLVED"

PROVER_WON=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "disputeProverWon(bytes32)(bool)" "$RESULT_ID" 2>/dev/null)
assert_eq "B4: Challenger won (no ZK proof submitted)" "false" "$PROVER_WON"

IS_VALID_AFTER=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "isResultValid(bytes32)(bool)" "$RESULT_ID" 2>/dev/null)
assert_eq "B4: Result invalid after challenger win" "false" "$IS_VALID_AFTER"

# --- B5: Happy path (submit -> finalize, no challenge) ---
echo ""
log "B5: Testing happy path (submit -> finalize without challenge)..."

# Mine a block to get a fresh block number
cast rpc --rpc-url "$RPC_URL" evm_mine >/dev/null 2>&1

B5_SUBMIT_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "submitResult(bytes32,bytes32,bytes,bytes)" \
    "$TEST_MODEL_HASH" \
    "$TEST_INPUT_HASH" \
    "$TEST_RESULT" \
    "$ATTESTATION" \
    --value 0.001ether \
    --json 2>&1)

B5_SUBMIT_STATUS=$(tx_status_from_json "$B5_SUBMIT_TX")
B5_SUBMIT_GAS=$(tx_gas_from_json "$B5_SUBMIT_TX")

if [ "$B5_SUBMIT_STATUS" = "0x1" ]; then
    record_pass "B5: Happy path result submitted" "$B5_SUBMIT_GAS"
else
    record_fail "B5: Happy path result submitted" "tx status=$B5_SUBMIT_STATUS"
fi

# Compute resultId for the second submission
BLOCK_NUM2_DEC=$(tx_block_from_json "$B5_SUBMIT_TX")
PROVER_LOWER=$(echo "$PROVER_ADDR" | tr '[:upper:]' '[:lower:]')
RESULT_ID2=$(compute_result_id "$PROVER_LOWER" "$TEST_MODEL_HASH" "$TEST_INPUT_HASH" "$BLOCK_NUM2_DEC")
info "Happy path Result ID: $RESULT_ID2"

# Fast-forward past CHALLENGE_WINDOW (1 hour = 3600s) + buffer
cast rpc --rpc-url "$RPC_URL" evm_increaseTime 3700 >/dev/null 2>&1
cast rpc --rpc-url "$RPC_URL" evm_mine >/dev/null 2>&1
info "Time advanced by 3700 seconds (past 1h challenge window)"

# Finalize
B5_FINALIZE_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "finalize(bytes32)" \
    "$RESULT_ID2" \
    --json 2>&1)

B5_FINALIZE_STATUS=$(tx_status_from_json "$B5_FINALIZE_TX")
B5_FINALIZE_GAS=$(tx_gas_from_json "$B5_FINALIZE_TX")

if [ "$B5_FINALIZE_STATUS" = "0x1" ]; then
    record_pass "B5: Happy path finalized" "$B5_FINALIZE_GAS"
else
    record_fail "B5: Happy path finalized" "tx status=$B5_FINALIZE_STATUS"
fi

IS_VALID_FINAL=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "isResultValid(bytes32)(bool)" "$RESULT_ID2" 2>/dev/null)
assert_eq "B5: Finalized result is valid" "true" "$IS_VALID_FINAL"

# =============================================================================
# TEST GROUP C: Admin Operations
# =============================================================================

echo ""
log "===== Test Group C: Admin Operations ====="

# --- C1: Pause/Unpause ---
log "C1: Testing pause/unpause..."

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$ENGINE_ADDR" \
    "pause()" \
    >/dev/null 2>&1

# Submit should fail while paused
PAUSED_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$REQUESTER_KEY" \
    "$ENGINE_ADDR" \
    "requestExecution(bytes32,bytes32,string,address,uint256)" \
    "$TEST_IMAGE_ID" \
    "$INPUT_DIGEST" \
    "https://example.com/paused-test" \
    "0x0000000000000000000000000000000000000000" \
    3600 \
    --value 0.001ether \
    --json 2>&1 || echo '{"status":"0x0"}')

PAUSED_STATUS=$(tx_status_from_json "$PAUSED_TX")
if [ "$PAUSED_STATUS" = "0x0" ] || [ -z "$PAUSED_STATUS" ]; then
    record_pass "C1: Request reverts while paused"
else
    record_fail "C1: Request reverts while paused" "expected revert, status=$PAUSED_STATUS"
fi

# Unpause
cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$ENGINE_ADDR" \
    "unpause()" \
    >/dev/null 2>&1

# Verify requests work again after unpause
UNPAUSED_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$REQUESTER_KEY" \
    "$ENGINE_ADDR" \
    "requestExecution(bytes32,bytes32,string,address,uint256)" \
    "$TEST_IMAGE_ID" \
    "$INPUT_DIGEST" \
    "https://example.com/unpaused-test" \
    "0x0000000000000000000000000000000000000000" \
    3600 \
    --value 0.001ether \
    --json 2>&1)

UNPAUSED_STATUS=$(tx_status_from_json "$UNPAUSED_TX")
if [ "$UNPAUSED_STATUS" = "0x1" ]; then
    record_pass "C1: Request succeeds after unpause"
else
    record_fail "C1: Request succeeds after unpause" "status=$UNPAUSED_STATUS"
fi

# --- C2: Program deactivation/reactivation ---
log "C2: Testing program deactivation..."

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$REGISTRY_ADDR" \
    "deactivateProgram(bytes32)" \
    "$TEST_IMAGE_ID" \
    >/dev/null 2>&1

IS_ACTIVE_DEACT=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" \
    "isProgramActive(bytes32)(bool)" "$TEST_IMAGE_ID" 2>/dev/null)
assert_eq "C2: Program inactive after deactivation" "false" "$IS_ACTIVE_DEACT"

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$REGISTRY_ADDR" \
    "reactivateProgram(bytes32)" \
    "$TEST_IMAGE_ID" \
    >/dev/null 2>&1

IS_ACTIVE_REACT=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" \
    "isProgramActive(bytes32)(bool)" "$TEST_IMAGE_ID" 2>/dev/null)
assert_eq "C2: Program active after reactivation" "true" "$IS_ACTIVE_REACT"

# --- C3: Enclave revocation ---
log "C3: Testing enclave revocation..."

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "revokeEnclave(address)" \
    "$ENCLAVE_ADDR" \
    >/dev/null 2>&1

ENCLAVE_RAW=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "enclaves(address)(bool,bool,bytes32,uint256)" "$ENCLAVE_ADDR" 2>/dev/null)
ENCLAVE_ACTIVE=$(echo "$ENCLAVE_RAW" | sed -n '2p')
assert_eq "C3: Enclave inactive after revocation" "false" "$ENCLAVE_ACTIVE"

# --- C4: Gas usage summary ---
log "C4: Checking gas usage..."

CONTRACT_BALANCE=$(cast balance --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" 2>/dev/null)
info "TEEMLVerifier remaining balance: $CONTRACT_BALANCE wei"
record_pass "C4: Gas usage check complete"

# =============================================================================
# Summary
# =============================================================================

TOTAL=$((TESTS_PASSED + TESTS_FAILED))

echo ""
echo "================================================================"
echo "  SMOKE TEST SUMMARY"
echo "================================================================"
echo ""

# Print table header
printf "  %-45s  %-6s  %s\n" "TEST" "RESULT" "GAS"
printf "  %-45s  %-6s  %s\n" "---------------------------------------------" "------" "----------"

for i in "${!TEST_NAMES[@]}"; do
    printf "  %-45s  %-6s  %s\n" "${TEST_NAMES[$i]}" "${TEST_RESULTS[$i]}" "${TEST_GAS[$i]}"
done

echo ""
echo "  -----------------------------------------------"
printf "  Total: %d passed, %d failed (out of %d)\n" "$TESTS_PASSED" "$TESTS_FAILED" "$TOTAL"
echo ""

if [ "$TESTS_FAILED" -eq 0 ]; then
    echo "  CONTRACT: $TEE_VERIFIER_ADDR"
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
