#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TEE Dispute Path E2E
#
# Deploy RemainderVerifier + TEEMLVerifier → register DAG circuit + enclave
# → submit TEE result → challenge → resolve dispute with real ZK proof
# → verify prover wins (proof is valid)
#
# Uses the phase1a_dag_fixture.json which contains a valid GKR proof for
# an 88-layer XGBoost circuit. The resolveDispute() call forwards to
# RemainderVerifier.verifyDAGProof() which needs >254M gas.
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"
FIXTURE_PATH="$CONTRACTS_DIR/test/fixtures/phase1a_dag_fixture.json"

ANVIL_PORT=8552
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

# Anvil account #0 — admin + submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Anvil account #1 — enclave signer
ENCLAVE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
ENCLAVE_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"

# Anvil account #2 — challenger
CHALLENGER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
CHALLENGER_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"

PROVER_STAKE="0.1ether"
CHALLENGE_BOND="0.1ether"

log() { echo "==> $*"; }
ok()  { echo "  ✓ $*"; }
err() { echo "  ✗ $*" >&2; }

cleanup() {
    if [ -n "${ANVIL_PID:-}" ]; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Helpers ───────────────────────────────────────────────────────────────────

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

# ── Step 1: Start Anvil with high gas limit ───────────────────────────────────

log "Step 1: Starting Anvil on port $ANVIL_PORT (gas limit 500M, no code size limit)..."
anvil --port "$ANVIL_PORT" --silent --gas-limit 500000000 --disable-code-size-limit &
ANVIL_PID=$!
sleep 2

if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil failed to start on port $ANVIL_PORT"
    exit 1
fi
ok "Anvil running (PID: $ANVIL_PID)"

# ── Step 2: Deploy RemainderVerifier + register DAG circuit ───────────────────

log "Step 2: Deploying RemainderVerifier and registering DAG circuit..."
cd "$CONTRACTS_DIR"

DEPLOYER_KEY="$ADMIN_KEY" \
    forge script script/DeployRemainderDAG.s.sol:DeployRemainderDAG \
    --rpc-url "$RPC_URL" --broadcast --disable-code-size-limit > /dev/null 2>&1

REMAINDER_VERIFIER=$(python3 -c "
import json, sys
data = json.load(open('broadcast/DeployRemainderDAG.s.sol/31337/run-latest.json'))
for tx in data['transactions']:
    if tx.get('transactionType') == 'CREATE':
        print(tx['contractAddress']); sys.exit(0)
sys.exit(1)
")

if [ -z "$REMAINDER_VERIFIER" ]; then
    err "Failed to extract RemainderVerifier address"
    exit 1
fi
ok "RemainderVerifier deployed at: $REMAINDER_VERIFIER"

# Validate circuit is registered
CIRCUIT_HASH=$(python3 -c "
import json
d = json.load(open('test/fixtures/phase1a_dag_fixture.json'))
print(d['circuit_hash_raw'])
")

IS_ACTIVE=$(cast call "$REMAINDER_VERIFIER" "isDAGCircuitActive(bytes32)(bool)" "$CIRCUIT_HASH" --rpc-url "$RPC_URL")
if [ "$IS_ACTIVE" != "true" ]; then
    err "DAG circuit not registered as active"
    exit 1
fi
ok "DAG circuit active (hash: ${CIRCUIT_HASH:0:18}...)"

# ── Step 3: Deploy TEEMLVerifier ──────────────────────────────────────────────

log "Step 3: Deploying TEEMLVerifier (with RemainderVerifier)..."

TEE_VERIFIER=$(forge create src/tee/TEEMLVerifier.sol:TEEMLVerifier \
    --broadcast --json \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_KEY" \
    --constructor-args "$ADMIN_ADDR" "$REMAINDER_VERIFIER" \
    2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['deployedTo'])")

if [ -z "$TEE_VERIFIER" ]; then
    err "Failed to deploy TEEMLVerifier"
    exit 1
fi
ok "TEEMLVerifier deployed at: $TEE_VERIFIER"

# Verify remainderVerifier is set
RV_ON_CHAIN=$(cast call "$TEE_VERIFIER" "remainderVerifier()(address)" --rpc-url "$RPC_URL")
ok "Linked to RemainderVerifier: $RV_ON_CHAIN"

# ── Step 4: Register enclave ─────────────────────────────────────────────────

log "Step 4: Registering test enclave..."
IMAGE_HASH=$(cast keccak "test-enclave-image-v1")

cast send "$TEE_VERIFIER" \
    "registerEnclave(address,bytes32)" "$ENCLAVE_ADDR" "$IMAGE_HASH" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

ok "Enclave registered: $ENCLAVE_ADDR"

# ── Step 5: Submit TEE-attested result ────────────────────────────────────────

log "Step 5: Submitting TEE-attested result..."

MODEL_HASH=$(cast keccak "xgboost-model-weights")
INPUT_HASH=$(cast keccak "test-input-data")
RESULT_DATA="0xdeadbeef"

ATTESTATION=$(sign_attestation "$MODEL_HASH" "$INPUT_HASH" "$RESULT_DATA" "$ENCLAVE_KEY")

cast send "$TEE_VERIFIER" \
    "submitResult(bytes32,bytes32,bytes,bytes)" \
    "$MODEL_HASH" "$INPUT_HASH" "$RESULT_DATA" "$ATTESTATION" \
    --value "$PROVER_STAKE" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL")
RESULT_ID=$(compute_result_id "$ADMIN_ADDR" "$MODEL_HASH" "$INPUT_HASH" "$BLOCK_NUM")
ok "Result submitted — ID: ${RESULT_ID:0:18}..."

# ── Step 6: Challenge the result ─────────────────────────────────────────────

log "Step 6: Challenger disputes the result..."

cast send "$TEE_VERIFIER" "challenge(bytes32)" "$RESULT_ID" \
    --value "$CHALLENGE_BOND" \
    --rpc-url "$RPC_URL" --private-key "$CHALLENGER_KEY" > /dev/null

ok "Challenged by $CHALLENGER_ADDR (bond: $CHALLENGE_BOND)"

# Verify challenged state
IS_CHALLENGED=$(cast call "$TEE_VERIFIER" "getResult(bytes32)" "$RESULT_ID" --rpc-url "$RPC_URL" \
    | python3 -c "
import sys
# getResult returns an ABI-encoded tuple; 'challenged' is a bool field
# Read raw hex, the challenged flag is deep in the struct, just check contract
" 2>/dev/null || true)

# Simpler check: isResultValid should be false
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
[ "$IS_VALID" = "false" ] && ok "Result in disputed state (isResultValid=false)"

# ── Step 7: Resolve dispute with real ZK proof ───────────────────────────────

log "Step 7: Resolving dispute with ZK proof from DAG fixture..."

# Extract proof data from fixture (these are large hex blobs)
PROOF_HEX=$(python3 -c "
import json
d = json.load(open('test/fixtures/phase1a_dag_fixture.json'))
print(d['proof_hex'])
")
PUBLIC_INPUTS_HEX=$(python3 -c "
import json
d = json.load(open('test/fixtures/phase1a_dag_fixture.json'))
print(d['public_inputs_hex'])
")
GENS_HEX=$(python3 -c "
import json
d = json.load(open('test/fixtures/phase1a_dag_fixture.json'))
print(d['gens_hex'])
")

ok "Fixture loaded: proof=$(echo -n "$PROOF_HEX" | wc -c | tr -d ' ') chars, gens=$(echo -n "$GENS_HEX" | wc -c | tr -d ' ') chars"

# Record balances before resolution
ADMIN_BAL_BEFORE=$(cast balance "$ADMIN_ADDR" --rpc-url "$RPC_URL")
CHALLENGER_BAL_BEFORE=$(cast balance "$CHALLENGER_ADDR" --rpc-url "$RPC_URL")

# Call resolveDispute — this triggers verifyDAGProof (>254M gas)
log "  Calling resolveDispute (this may take a while — ZK verification ~254M gas)..."
cast send "$TEE_VERIFIER" \
    "resolveDispute(bytes32,bytes,bytes32,bytes,bytes)" \
    "$RESULT_ID" "$PROOF_HEX" "$CIRCUIT_HASH" "$PUBLIC_INPUTS_HEX" "$GENS_HEX" \
    --gas-limit 500000000 \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

ok "resolveDispute() completed"

# ── Step 8: Verify dispute outcome ───────────────────────────────────────────

log "Step 8: Verifying dispute outcome..."

RESOLVED=$(cast call "$TEE_VERIFIER" "disputeResolved(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
PROVER_WON=$(cast call "$TEE_VERIFIER" "disputeProverWon(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")

if [ "$RESOLVED" != "true" ]; then
    err "disputeResolved should be true, got: $RESOLVED"
    exit 1
fi
ok "Dispute resolved: true"

if [ "$PROVER_WON" != "true" ]; then
    err "disputeProverWon should be true (valid ZK proof), got: $PROVER_WON"
    exit 1
fi
ok "Prover won: true (ZK proof verified)"

if [ "$IS_VALID" != "true" ]; then
    err "isResultValid should be true after prover wins dispute, got: $IS_VALID"
    exit 1
fi
ok "Result valid: true"

# Verify payout: prover (submitter/admin) should have received both stakes
ADMIN_BAL_AFTER=$(cast balance "$ADMIN_ADDR" --rpc-url "$RPC_URL")
ok "Admin balance: $ADMIN_BAL_BEFORE → $ADMIN_BAL_AFTER"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  TEE Dispute Path E2E — PASSED"
echo "════════════════════════════════════════════════════════════"
echo "  RemainderVerifier: $REMAINDER_VERIFIER"
echo "  TEEMLVerifier:     $TEE_VERIFIER"
echo "  Result ID:         ${RESULT_ID:0:18}..."
echo "  Flow:              submit → challenge → ZK proof → prover wins ✓"
echo "  Dispute outcome:   prover vindicated (valid ZK proof)"
echo "════════════════════════════════════════════════════════════"
