#!/usr/bin/env bash
# =============================================================================
# TEE Local Demo -- Interactive Educational Demo of the TEE Lifecycle
#
# Walks through the full TEE-attested ML inference lifecycle on a local Anvil
# instance, pausing between steps to explain what is happening:
#
#   a. Start Anvil
#   b. Deploy contracts via DeployFullStack
#   c. Register a test program with ProgramRegistry
#   d. Register a TEE enclave
#   e. Submit an execution request
#   f. Simulate TEE inference (submit result on-chain via TEEMLVerifier)
#   g. Show result status (pending challenge period)
#   h. Fast-forward time on Anvil (past the challenge window)
#   i. Finalize result
#   j. Show final result as VALID
#   k. Clean up (kill Anvil)
#
# Usage:
#   ./scripts/tee-local-demo.sh            # Run full demo
#   ./scripts/tee-local-demo.sh --step      # Pause between each step (press Enter)
#   ./scripts/tee-local-demo.sh --help      # Show help
#
# Exit codes:
#   0 -- demo completed successfully
#   1 -- error
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
CONTRACTS_DIR="$PROJECT_ROOT/contracts"

# ---------------------------------------------------------------------------
# Anvil deterministic accounts (default mnemonic)
# ---------------------------------------------------------------------------
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
DEPLOYER_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

PROVER_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
PROVER_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"

# Account #4 used as TEE enclave signer
ENCLAVE_KEY="0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a"
ENCLAVE_ADDR="0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"

# ---------------------------------------------------------------------------
# Test constants
# ---------------------------------------------------------------------------
TEST_IMAGE_ID="0x1111111111111111111111111111111111111111111111111111111111111111"
TEST_MODEL_HASH="0x2222222222222222222222222222222222222222222222222222222222222222"
TEST_INPUT_HASH="0x3333333333333333333333333333333333333333333333333333333333333333"
INPUT_DIGEST="0x4444444444444444444444444444444444444444444444444444444444444444"
ENCLAVE_IMAGE_HASH="0x5555555555555555555555555555555555555555555555555555555555555555"
TEST_RESULT="0xaabbccdd"

# ---------------------------------------------------------------------------
# Contract addresses (filled after deployment)
# ---------------------------------------------------------------------------
REGISTRY_ADDR=""
ENGINE_ADDR=""
TEE_VERIFIER_ADDR=""

# ---------------------------------------------------------------------------
# Background state
# ---------------------------------------------------------------------------
ANVIL_PID=""
ANVIL_PORT=""

# ---------------------------------------------------------------------------
# Flags
# ---------------------------------------------------------------------------
STEP_MODE=false

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<'USAGE'
tee-local-demo.sh -- Interactive local demo of the TEE inference lifecycle.

USAGE:
  scripts/tee-local-demo.sh [OPTIONS]

OPTIONS:
  --step          Pause between each step (press Enter to continue)
  --help, -h      Show this help message

WHAT IT DOES:
  1.  Start a local Anvil blockchain
  2.  Deploy all World ZK Compute contracts
  3.  Register a test ML program in ProgramRegistry
  4.  Register a TEE enclave with TEEMLVerifier
  5.  Submit an execution request via ExecutionEngine
  6.  Simulate TEE inference: sign attestation + submit result on-chain
  7.  Show result status: pending (inside challenge window)
  8.  Fast-forward time past the 1-hour challenge window
  9.  Finalize the unchallenged result
  10. Confirm the result is now VALID on-chain

REQUIREMENTS:
  - Foundry (anvil, forge, cast)
  - python3 (for JSON parsing)
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --step)    STEP_MODE=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *)         err "Unknown option: $1"; usage; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Pause and wait for Enter if --step mode is active
step_pause() {
    if [[ "$STEP_MODE" == "true" ]]; then
        printf "\n${YELLOW}  Press Enter to continue...${RESET}"
        read -r
    fi
}

# Explain what is about to happen
explain() {
    printf "\n  ${CYAN}%s${RESET}\n\n" "$*"
}

# Extract gas from cast send output
extract_gas() {
    echo "$1" | grep -i "gasUsed" | head -1 | awk '{print $NF}'
}

# Sign a TEE attestation: message = keccak256(encodePacked(modelHash, inputHash, keccak256(result)))
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

# Compute resultId = keccak256(abi.encodePacked(sender, modelHash, inputHash, blockNumber))
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

# Extract a field from JSON output
tx_status_from_json() {
    echo "$1" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null || echo ""
}

tx_gas_from_json() {
    echo "$1" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('gasUsed','0x0'),16))" 2>/dev/null || echo "n/a"
}

tx_block_from_json() {
    echo "$1" | python3 -c "import sys,json; print(int(json.load(sys.stdin).get('blockNumber','0x0'),16))" 2>/dev/null || echo "0"
}

# ---------------------------------------------------------------------------
# Cleanup trap
# ---------------------------------------------------------------------------
cleanup() {
    if [[ -n "$ANVIL_PID" ]] && kill -0 "$ANVIL_PID" 2>/dev/null; then
        info "Stopping Anvil (PID $ANVIL_PID)..."
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# =============================================================================
#  BANNER
# =============================================================================

printf "\n"
printf "${BOLD}================================================================${RESET}\n"
printf "${BOLD}    World ZK Compute -- TEE Inference Lifecycle Demo${RESET}\n"
printf "${BOLD}================================================================${RESET}\n"
printf "\n"

explain "This demo walks through the full lifecycle of a TEE-attested ML
  inference result on the World ZK Compute protocol:

  1. Deploy smart contracts to a local blockchain (Anvil)
  2. Register an ML program and a TEE enclave
  3. Submit an execution request
  4. A TEE enclave runs inference and attests the result
  5. The result enters a challenge window where anyone can dispute it
  6. After the challenge window expires, the result is finalized
  7. The result is now VALID and trustworthy on-chain

  This is the 'happy path' -- no disputes, just honest inference."

if [[ "$STEP_MODE" == "true" ]]; then
    info "Running in step mode. Press Enter between each step."
fi

# =============================================================================
# Step 0: Prerequisites
# =============================================================================

header "Step 0: Checking prerequisites"

for cmd in anvil forge cast python3; do
    if ! command -v "$cmd" &>/dev/null; then
        err "Required command not found: $cmd"
        echo "  Install Foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup"
        exit 1
    fi
done

ok "All prerequisites met (anvil, forge, cast, python3)"
step_pause

# =============================================================================
# Step 1: Start Anvil
# =============================================================================

header "Step 1/10: Starting local Anvil blockchain"

explain "Anvil is Foundry's local Ethereum node. It gives us deterministic
  accounts pre-funded with 10,000 ETH each, instant mining, and the
  ability to manipulate time -- essential for testing challenge windows."

# Find a random available port
ANVIL_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()' 2>/dev/null || echo "0")
if [[ "$ANVIL_PORT" == "0" ]]; then
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
info "RPC URL: $RPC_URL"
info "Deployer:  $DEPLOYER_ADDR"
info "Prover:    $PROVER_ADDR"
info "Enclave:   $ENCLAVE_ADDR"
step_pause

# =============================================================================
# Step 2: Deploy contracts via DeployFullStack
# =============================================================================

header "Step 2/10: Deploying contracts via DeployFullStack"

explain "We deploy the entire World ZK Compute contract stack in one shot:
  - MockRiscZeroVerifier (mock verifier for local testing)
  - ProgramRegistry     (tracks registered ML programs)
  - ProverReputation    (tracks prover reliability scores)
  - ProverRegistry      (manages prover staking)
  - ExecutionEngine     (orchestrates request/claim/proof lifecycle)
  - TEEMLVerifier       (TEE attestation + challenge/dispute logic)"

cd "$CONTRACTS_DIR"

# Install forge dependencies if needed
if [[ ! -d "lib/forge-std" ]]; then
    info "Installing forge-std..."
    forge install foundry-rs/forge-std --no-git --no-commit 2>/dev/null || true
fi

FORGE_OUTPUT=$(PRIVATE_KEY="$DEPLOYER_KEY" FEE_RECIPIENT="$DEPLOYER_ADDR" \
    forge script script/DeployFullStack.s.sol:DeployFullStack \
    --rpc-url "$RPC_URL" \
    --broadcast 2>&1) || {
    err "Forge deployment failed"
    echo "$FORGE_OUTPUT" >&2
    exit 1
}

cd "$PROJECT_ROOT"

# Extract addresses from forge output
VERIFIER_ADDR=$(echo "$FORGE_OUTPUT" | grep "MockRiscZeroVerifier:" | head -1 | awk '{print $NF}')
REGISTRY_ADDR=$(echo "$FORGE_OUTPUT" | grep "ProgramRegistry:" | head -1 | awk '{print $NF}')
ENGINE_ADDR=$(echo "$FORGE_OUTPUT" | grep "ExecutionEngine:" | head -1 | awk '{print $NF}')
TEE_VERIFIER_ADDR=$(echo "$FORGE_OUTPUT" | grep "TEEMLVerifier:" | head -1 | awk '{print $NF}')

if [[ -z "$REGISTRY_ADDR" ]] || [[ -z "$ENGINE_ADDR" ]] || [[ -z "$TEE_VERIFIER_ADDR" ]]; then
    err "Failed to extract contract addresses from forge output"
    echo "$FORGE_OUTPUT" >&2
    exit 1
fi

ok "All contracts deployed successfully"
info "MockRiscZeroVerifier: ${VERIFIER_ADDR:-n/a}"
info "ProgramRegistry:      $REGISTRY_ADDR"
info "ExecutionEngine:      $ENGINE_ADDR"
info "TEEMLVerifier:        $TEE_VERIFIER_ADDR"
step_pause

# =============================================================================
# Step 3: Register a test program with ProgramRegistry
# =============================================================================

header "Step 3/10: Registering test ML program"

explain "Before any inference can happen, the ML program must be registered
  in the ProgramRegistry. This stores:
  - imageId:     A unique identifier (hash of the compiled binary)
  - name:        Human-readable program name
  - programUrl:  Where to download the binary
  - inputSchema: Hash describing expected input format

  Think of this as 'publishing' the ML model to the network."

cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$REGISTRY_ADDR" \
    "registerProgram(bytes32,string,string,bytes32)" \
    "$TEST_IMAGE_ID" \
    "xgboost-credit-scoring" \
    "ipfs://QmTestProgram123" \
    "0x0000000000000000000000000000000000000000000000000000000000000000" \
    >/dev/null 2>&1

IS_ACTIVE=$(cast call --rpc-url "$RPC_URL" "$REGISTRY_ADDR" \
    "isProgramActive(bytes32)(bool)" "$TEST_IMAGE_ID" 2>/dev/null)

if [[ "$IS_ACTIVE" == "true" ]]; then
    ok "Program 'xgboost-credit-scoring' registered and active"
    info "Image ID: $TEST_IMAGE_ID"
else
    err "Program registration failed"
    exit 1
fi
step_pause

# =============================================================================
# Step 4: Register TEE enclave
# =============================================================================

header "Step 4/10: Registering TEE enclave"

explain "A Trusted Execution Environment (TEE) like AWS Nitro Enclaves runs
  code in an isolated, tamper-proof hardware enclave. We register the
  enclave's signing key on-chain so the contract can verify attestations.

  Key components:
  - enclaveKey:       The ECDSA address that signs attestations
  - enclaveImageHash: Hash of the enclave image (like AWS Nitro's PCR0)
                      This proves WHICH code is running inside the TEE

  Only the contract admin can register enclaves (trust anchor)."

# First lower the stake amounts for this demo
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

info "Prover stake set to 0.001 ETH (lowered for demo)"
info "Challenge bond set to 0.001 ETH (lowered for demo)"

# Register the enclave
cast send --rpc-url "$RPC_URL" \
    --private-key "$DEPLOYER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "registerEnclave(address,bytes32)" \
    "$ENCLAVE_ADDR" \
    "$ENCLAVE_IMAGE_HASH" \
    >/dev/null 2>&1

ENCLAVE_RAW=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "enclaves(address)(bool,bool,bytes32,uint256)" "$ENCLAVE_ADDR" 2>/dev/null)

if echo "$ENCLAVE_RAW" | head -1 | grep -q "true"; then
    ok "Enclave registered successfully"
    info "Enclave address:    $ENCLAVE_ADDR"
    info "Enclave image hash: $ENCLAVE_IMAGE_HASH"
else
    err "Enclave registration failed"
    exit 1
fi
step_pause

# =============================================================================
# Step 5: Submit an execution request
# =============================================================================

header "Step 5/10: Submitting an execution request"

explain "A user (the 'requester') submits a request for ML inference:
  - imageId:     Which program to run
  - inputDigest: Hash of the private input data
  - inputUrl:    Where the prover can fetch the encrypted input
  - callback:    Optional contract to notify on completion
  - timeout:     How long the prover has to respond (seconds)

  The requester also sends ETH as a tip/payment for the prover.
  This creates request #1 in the ExecutionEngine."

A5_OUTPUT=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$ENGINE_ADDR" \
    "requestExecution(bytes32,bytes32,string,address,uint256)" \
    "$TEST_IMAGE_ID" \
    "$INPUT_DIGEST" \
    "https://storage.example.com/encrypted-input/12345" \
    "0x0000000000000000000000000000000000000000" \
    3600 \
    --value 0.001ether \
    2>&1)

A5_GAS=$(extract_gas "$A5_OUTPUT")

NEXT_ID=$(cast call --rpc-url "$RPC_URL" "$ENGINE_ADDR" \
    "nextRequestId()(uint256)" 2>/dev/null)

if [[ "$NEXT_ID" == "2" ]]; then
    ok "Execution request #1 submitted"
    info "Gas used: $A5_GAS"
    info "Input digest: $INPUT_DIGEST"
    info "Next request ID: $NEXT_ID"
else
    err "Failed to submit execution request (nextRequestId=$NEXT_ID, expected=2)"
    exit 1
fi
step_pause

# =============================================================================
# Step 6: Simulate TEE inference (submit result via TEEMLVerifier)
# =============================================================================

header "Step 6/10: Simulating TEE inference"

explain "In production, the TEE enclave would:
  1. Fetch the encrypted input from the URL
  2. Decrypt it inside the enclave (key managed by KMS)
  3. Run the ML model (e.g., XGBoost credit scoring)
  4. Sign the result: ECDSA(keccak256(modelHash || inputHash || resultHash))
  5. Return the signed attestation to the prover

  The prover then submits the result + attestation on-chain.
  The contract verifies the signature came from a registered enclave.

  Here we simulate this: we sign with the enclave's private key and
  submit the result to TEEMLVerifier with a 0.001 ETH prover stake."

# Generate the attestation signature
ATTESTATION=$(sign_attestation "$TEST_MODEL_HASH" "$TEST_INPUT_HASH" "$TEST_RESULT" "$ENCLAVE_KEY")
info "Attestation signature generated by enclave key"

# Submit the result
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

if [[ "$B2_STATUS" != "0x1" ]]; then
    err "Failed to submit TEE result (tx status=$B2_STATUS)"
    exit 1
fi

# Compute the resultId
BLOCK_NUM_DEC=$(tx_block_from_json "$B2_TX")
PROVER_LOWER=$(echo "$PROVER_ADDR" | tr '[:upper:]' '[:lower:]')
RESULT_ID=$(compute_result_id "$PROVER_LOWER" "$TEST_MODEL_HASH" "$TEST_INPUT_HASH" "$BLOCK_NUM_DEC")

ok "TEE-attested result submitted on-chain"
info "Gas used:     $B2_GAS"
info "Result ID:    $RESULT_ID"
info "Model hash:   $TEST_MODEL_HASH"
info "Input hash:   $TEST_INPUT_HASH"
info "Result data:  $TEST_RESULT"
info "Prover stake: 0.001 ETH"
step_pause

# =============================================================================
# Step 7: Show result status (pending challenge period)
# =============================================================================

header "Step 7/10: Checking result status"

explain "The result has been submitted, but it is NOT yet valid. It must
  survive a 1-hour challenge window first.

  During this window, anyone can challenge the result by posting a bond.
  If challenged, the prover must produce a full ZK proof (via GKR/Groth16
  on the RemainderVerifier) to prove the inference was correct.

  This is the 'optimistic' model:
  - Happy path (no challenge):  ~150K gas total, very cheap
  - Dispute path (challenged):  ~250M gas for ZK verification

  Right now, we are inside the challenge window."

IS_VALID=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "isResultValid(bytes32)(bool)" "$RESULT_ID" 2>/dev/null)

if [[ "$IS_VALID" == "false" ]]; then
    ok "Result status: PENDING (not yet valid)"
    info "isResultValid($RESULT_ID) = false"
    info "The result is inside the 1-hour challenge window."
    info "Anyone can call challenge(resultId) with a bond to dispute it."
else
    warn "Result is unexpectedly valid already"
fi
step_pause

# =============================================================================
# Step 8: Fast-forward time past the challenge window
# =============================================================================

header "Step 8/10: Fast-forwarding time past challenge window"

explain "In production, you would wait 1 hour for the challenge window to
  expire. On our local Anvil chain, we can manipulate time:

    evm_increaseTime(3700)  -- advance the clock by 3700 seconds (>1 hour)
    evm_mine                -- mine a new block with the updated timestamp

  This simulates the passage of time without actually waiting.
  After this, the challenge window has closed and no one can dispute."

cast rpc --rpc-url "$RPC_URL" evm_increaseTime 3700 >/dev/null 2>&1
cast rpc --rpc-url "$RPC_URL" evm_mine >/dev/null 2>&1

ok "Time advanced by 3700 seconds (past the 1-hour challenge window)"
info "No challenges were submitted during the window."
step_pause

# =============================================================================
# Step 9: Finalize the result
# =============================================================================

header "Step 9/10: Finalizing the unchallenged result"

explain "Now that the challenge window has passed with no disputes, anyone
  can call finalize(resultId) to:
  1. Mark the result as finalized
  2. Return the prover's 0.001 ETH stake

  This is the 'happy path' -- the cheapest and most common outcome.
  The prover gets their stake back, and the result becomes permanent."

B5_TX=$(cast send --rpc-url "$RPC_URL" \
    --private-key "$PROVER_KEY" \
    "$TEE_VERIFIER_ADDR" \
    "finalize(bytes32)" \
    "$RESULT_ID" \
    --json 2>&1)

B5_STATUS=$(tx_status_from_json "$B5_TX")
B5_GAS=$(tx_gas_from_json "$B5_TX")

if [[ "$B5_STATUS" == "0x1" ]]; then
    ok "Result finalized successfully"
    info "Gas used: $B5_GAS"
    info "Prover stake (0.001 ETH) returned to submitter"
else
    err "Failed to finalize result (tx status=$B5_STATUS)"
    exit 1
fi
step_pause

# =============================================================================
# Step 10: Show final result as VALID
# =============================================================================

header "Step 10/10: Verifying final result"

explain "The result is now finalized and valid on-chain. Any smart contract
  or off-chain system can call isResultValid(resultId) to check.

  This is the on-chain truth: the ML inference result 0xaabbccdd was
  produced by a registered TEE enclave, survived the challenge window
  with no disputes, and is now permanently recorded as valid."

IS_VALID_FINAL=$(cast call --rpc-url "$RPC_URL" "$TEE_VERIFIER_ADDR" \
    "isResultValid(bytes32)(bool)" "$RESULT_ID" 2>/dev/null)

if [[ "$IS_VALID_FINAL" == "true" ]]; then
    ok "Result is VALID"
    info "isResultValid($RESULT_ID) = true"
else
    err "Result is not valid after finalization"
    exit 1
fi

# =============================================================================
# Summary
# =============================================================================

printf "\n"
printf "${BOLD}================================================================${RESET}\n"
printf "${BOLD}    Demo Complete -- TEE Happy Path Lifecycle${RESET}\n"
printf "${BOLD}================================================================${RESET}\n"
printf "\n"

printf "  ${BOLD}%-24s${RESET} %s\n" "RPC URL:" "$RPC_URL"
printf "  ${BOLD}%-24s${RESET} %s\n" "Deployer:" "$DEPLOYER_ADDR"
printf "  ${BOLD}%-24s${RESET} %s\n" "Prover:" "$PROVER_ADDR"
printf "  ${BOLD}%-24s${RESET} %s\n" "Enclave:" "$ENCLAVE_ADDR"
printf "\n"
printf "  ${BOLD}%-24s${RESET} %s\n" "ProgramRegistry:" "$REGISTRY_ADDR"
printf "  ${BOLD}%-24s${RESET} %s\n" "ExecutionEngine:" "$ENGINE_ADDR"
printf "  ${BOLD}%-24s${RESET} %s\n" "TEEMLVerifier:" "$TEE_VERIFIER_ADDR"
printf "\n"
printf "  ${BOLD}%-24s${RESET} %s\n" "Result ID:" "$RESULT_ID"
printf "  ${BOLD}%-24s${RESET} ${GREEN}%s${RESET}\n" "Result Status:" "VALID"
printf "  ${BOLD}%-24s${RESET} %s\n" "Result Data:" "$TEST_RESULT"
printf "\n"

header "Lifecycle Summary"
printf "\n"
printf "  Step  Description                              Cost\n"
printf "  ----  ---------------------------------------- ----------\n"
printf "  1     Deploy contracts (one-time)               ~5M gas\n"
printf "  2     Register program (one-time)               ~100K gas\n"
printf "  3     Register enclave (one-time, admin)        ~50K gas\n"
printf "  4     Submit execution request                  %s gas\n" "${A5_GAS:-~100K}"
printf "  5     Submit TEE result + attestation           %s gas\n" "${B2_GAS:-~100K}"
printf "  6     Finalize (after challenge window)         %s gas\n" "${B5_GAS:-~50K}"
printf "\n"
printf "  ${BOLD}Total happy-path cost per inference:${RESET} ~150-200K gas\n"
printf "  ${BOLD}Compare to full ZK verification:${RESET}    ~250M gas (1000x more)\n"
printf "\n"

header "What would happen if challenged?"
printf "\n"
printf "  If someone called challenge(resultId) during the 1-hour window:\n"
printf "  - Challenger posts a bond (0.001 ETH in this demo)\n"
printf "  - A 24-hour dispute window opens\n"
printf "  - Prover must submit a full ZK proof via RemainderVerifier\n"
printf "  - If proof is valid: prover wins both stakes\n"
printf "  - If no proof submitted: challenger wins by timeout\n"
printf "\n"
printf "  This 'optimistic verification' model gives us:\n"
printf "  - Cheap happy path (~150K gas per inference)\n"
printf "  - Strong security guarantees (ZK fallback for disputes)\n"
printf "  - Economic incentives to be honest (stake slashing)\n"
printf "\n"
printf "${BOLD}================================================================${RESET}\n"

# Cleanup happens via the EXIT trap
ok "Demo complete. Anvil will be stopped on exit."
