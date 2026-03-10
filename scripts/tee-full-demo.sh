#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# TEE Full Pipeline Demo
#
# Demonstrates the complete automated pipeline:
#   Deploy → TEE inference → On-chain submit → Pre-compute proof →
#   Challenge → Auto-resolve dispute → Verify prover wins
#
# This differs from tee-dispute-e2e.sh by generating the proof LIVE from
# the same model + features rather than using a pre-existing fixture file.
#
# Requirements:
#   - Docker (for TEE enclave container)
#   - Foundry (anvil, forge, cast)
#   - Cargo (for precompute_proof binary)
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"

ANVIL_PORT=8554
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"
CONTAINER_NAME="tee-full-demo-$$"
DOCKER_IMAGE="tee-full-demo"

# Anvil account #0 — admin + submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Anvil account #1 — enclave signer
ENCLAVE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"

# Anvil account #2 — challenger
CHALLENGER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
CHALLENGER_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"

PROVER_STAKE="0.1ether"
CHALLENGE_BOND="0.1ether"
FEATURES='[5.0, 3.5, 1.5, 0.3]'

log() { echo "==> $*"; }
ok()  { echo "  ✓ $*"; }
err() { echo "  ✗ $*" >&2; }

cleanup() {
    echo ""
    echo "Cleaning up..."
    docker stop "$CONTAINER_NAME" 2>/dev/null || true
    docker rm "$CONTAINER_NAME" 2>/dev/null || true
    [ -n "${ANVIL_PID:-}" ] && kill "$ANVIL_PID" 2>/dev/null || true
    [ -n "${PROOF_PID:-}" ] && kill "$PROOF_PID" 2>/dev/null || true
    rm -f "${PROOF_OUTPUT:-}"
}
trap cleanup EXIT

# ── Helpers ───────────────────────────────────────────────────────────────────

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

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  TEE Full Pipeline Demo"
echo "  inference → attestation → on-chain → pre-compute"
echo "  → challenge → auto-resolve → verify"
echo "═══════════════════════════════════════════════════════════"
echo ""

# ══════════════════════════════════════════════════════════════════════════════
# Step 1: Start Anvil
# ══════════════════════════════════════════════════════════════════════════════
log "Step 1: Starting Anvil on port $ANVIL_PORT..."
anvil --port "$ANVIL_PORT" --silent --gas-limit 500000000 --code-size-limit 200000 &
ANVIL_PID=$!
sleep 2

if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil failed to start on port $ANVIL_PORT"
    exit 1
fi
ok "Anvil running (PID: $ANVIL_PID)"

# ══════════════════════════════════════════════════════════════════════════════
# Step 2: Deploy Contracts (RemainderVerifier + TEEMLVerifier + DAG circuit)
# ══════════════════════════════════════════════════════════════════════════════
log "Step 2: Deploying contracts..."
cd "$CONTRACTS_DIR"

DEPLOYER_KEY="$ADMIN_KEY" \
    forge script script/DeployAll.s.sol:DeployAll \
    --rpc-url "$RPC_URL" --broadcast --code-size-limit 200000 > /dev/null 2>&1

# Extract addresses from broadcast JSON
BROADCAST_FILE=$(find broadcast/DeployAll.s.sol -name "run-latest.json" 2>/dev/null | head -1)
if [ -z "$BROADCAST_FILE" ]; then
    err "No broadcast file found — deployment may have failed"
    exit 1
fi

REMAINDER_VERIFIER=$(python3 -c "
import json
data = json.load(open('$BROADCAST_FILE'))
for tx in data['transactions']:
    if tx.get('contractName') == 'RemainderVerifier':
        print(tx['contractAddress']); break
")

TEE_VERIFIER=$(python3 -c "
import json
data = json.load(open('$BROADCAST_FILE'))
for tx in data['transactions']:
    if tx.get('contractName') == 'TEEMLVerifier':
        print(tx['contractAddress']); break
")

if [ -z "$REMAINDER_VERIFIER" ] || [ -z "$TEE_VERIFIER" ]; then
    err "Failed to extract contract addresses from broadcast"
    exit 1
fi

ok "RemainderVerifier: $REMAINDER_VERIFIER"
ok "TEEMLVerifier:     $TEE_VERIFIER"

cd "$ROOT_DIR"

# ══════════════════════════════════════════════════════════════════════════════
# Step 3: Build & Start TEE Enclave (Docker)
# ══════════════════════════════════════════════════════════════════════════════
log "Step 3: Starting TEE enclave container..."
docker build -t "$DOCKER_IMAGE" "$ROOT_DIR/tee/" > /dev/null 2>&1
docker run -d --name "$CONTAINER_NAME" \
    -p 8080:8080 \
    --add-host=host.docker.internal:host-gateway \
    -e ENCLAVE_PRIVATE_KEY="$ENCLAVE_KEY" \
    "$DOCKER_IMAGE" > /dev/null

# Wait for container to be ready
for i in $(seq 1 15); do
    if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq 15 ]; then
        err "Container failed to start within 15 seconds"
        docker logs "$CONTAINER_NAME" 2>&1 | tail -10
        exit 1
    fi
    sleep 1
done
ok "Enclave container running"

# ══════════════════════════════════════════════════════════════════════════════
# Step 4: Register Enclave On-Chain
# ══════════════════════════════════════════════════════════════════════════════
log "Step 4: Registering enclave..."
ENCLAVE_ADDR=$(curl -sf http://localhost:8080/info | python3 -c "import sys,json; print(json.load(sys.stdin)['enclave_address'])")
IMAGE_HASH=$(cast keccak "docker-enclave-v1")

cast send "$TEE_VERIFIER" \
    "registerEnclave(address,bytes32)" "$ENCLAVE_ADDR" "$IMAGE_HASH" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "Enclave registered: $ENCLAVE_ADDR"

# ══════════════════════════════════════════════════════════════════════════════
# Step 5: Run Inference via TEE Enclave
# ══════════════════════════════════════════════════════════════════════════════
log "Step 5: Running inference via Docker container..."
RESPONSE=$(curl -sf -X POST http://localhost:8080/infer \
    -H "Content-Type: application/json" \
    -d "{\"features\": $FEATURES}")

MODEL_HASH=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['model_hash'])")
INPUT_HASH=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['input_hash'])")
RESULT_DATA=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['result'])")
ATTESTATION=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['attestation'])")

ok "Inference result: $RESULT_DATA"
ok "Model hash: ${MODEL_HASH:0:18}..."

# ══════════════════════════════════════════════════════════════════════════════
# Step 6: Submit Result On-Chain
# ══════════════════════════════════════════════════════════════════════════════
log "Step 6: Submitting TEE-attested result on-chain..."
cast send "$TEE_VERIFIER" \
    "submitResult(bytes32,bytes32,bytes,bytes)" \
    "$MODEL_HASH" "$INPUT_HASH" "$RESULT_DATA" "$ATTESTATION" \
    --value "$PROVER_STAKE" \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null

BLOCK_NUM=$(cast block-number --rpc-url "$RPC_URL")
RESULT_ID=$(compute_result_id "$ADMIN_ADDR" "$MODEL_HASH" "$INPUT_HASH" "$BLOCK_NUM")
ok "Result submitted — ID: ${RESULT_ID:0:18}..."

# ══════════════════════════════════════════════════════════════════════════════
# Step 7: Pre-compute ZK Proof in Background
# ══════════════════════════════════════════════════════════════════════════════
log "Step 7: Pre-computing ZK proof in background..."
PROOF_OUTPUT="/tmp/tee-full-demo-proof-$$.json"

# Build if needed (suppress output)
cargo build --release --bin precompute_proof \
    --manifest-path "$ROOT_DIR/examples/xgboost-remainder/Cargo.toml" 2>/dev/null || true

# Start proof generation
cargo run --release --bin precompute_proof \
    --manifest-path "$ROOT_DIR/examples/xgboost-remainder/Cargo.toml" -- \
    --model "$ROOT_DIR/tee/test-model/model.json" \
    --features "$FEATURES" \
    --output "$PROOF_OUTPUT" > /dev/null 2>&1 &
PROOF_PID=$!
ok "Proof generation started (PID $PROOF_PID)"

# ══════════════════════════════════════════════════════════════════════════════
# Step 8: Challenge the Result
# ══════════════════════════════════════════════════════════════════════════════
log "Step 8: Challenger disputes the result..."
cast send "$TEE_VERIFIER" "challenge(bytes32)" "$RESULT_ID" \
    --value "$CHALLENGE_BOND" \
    --rpc-url "$RPC_URL" --private-key "$CHALLENGER_KEY" > /dev/null

# Verify challenge state
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
if [ "$IS_VALID" = "false" ]; then
    ok "Result challenged by $CHALLENGER_ADDR (isResultValid=false)"
else
    err "Expected isResultValid=false after challenge, got: $IS_VALID"
    exit 1
fi

# ══════════════════════════════════════════════════════════════════════════════
# Step 9: Wait for Pre-computed Proof
# ══════════════════════════════════════════════════════════════════════════════
log "Step 9: Waiting for pre-computed proof..."
if wait "$PROOF_PID"; then
    ok "Proof generation completed"
else
    err "Proof generation failed (exit code $?)"
    exit 1
fi
PROOF_PID=""

if [ ! -f "$PROOF_OUTPUT" ]; then
    err "Proof output file not found: $PROOF_OUTPUT"
    exit 1
fi

PROOF_SIZE=$(wc -c < "$PROOF_OUTPUT" | tr -d ' ')
ok "Proof ready: ${PROOF_SIZE} bytes"

# ══════════════════════════════════════════════════════════════════════════════
# Step 10: Resolve Dispute with ZK Proof
# ══════════════════════════════════════════════════════════════════════════════
log "Step 10: Resolving dispute with live ZK proof..."

# Extract proof data from pre-computed output
PROOF_HEX=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['proof_hex'])")
CIRCUIT_HASH=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['circuit_hash'])")
PUBLIC_INPUTS=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['public_inputs_hex'])")
GENS_HEX=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['gens_hex'])")

log "  Calling resolveDispute (ZK verification ~254M gas)..."
cast send "$TEE_VERIFIER" \
    "resolveDispute(bytes32,bytes,bytes32,bytes,bytes)" \
    "$RESULT_ID" "$PROOF_HEX" "$CIRCUIT_HASH" "$PUBLIC_INPUTS" "$GENS_HEX" \
    --gas-limit 500000000 \
    --rpc-url "$RPC_URL" --private-key "$ADMIN_KEY" > /dev/null
ok "resolveDispute() completed"

# ══════════════════════════════════════════════════════════════════════════════
# Step 11: Verify Final State
# ══════════════════════════════════════════════════════════════════════════════
log "Step 11: Verifying final state..."

RESOLVED=$(cast call "$TEE_VERIFIER" "disputeResolved(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
PROVER_WON=$(cast call "$TEE_VERIFIER" "disputeProverWon(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")
IS_VALID=$(cast call "$TEE_VERIFIER" "isResultValid(bytes32)(bool)" "$RESULT_ID" --rpc-url "$RPC_URL")

if [ "$RESOLVED" != "true" ]; then
    err "disputeResolved should be true, got: $RESOLVED"
    exit 1
fi
ok "disputeResolved: true"

if [ "$PROVER_WON" != "true" ]; then
    err "disputeProverWon should be true (valid ZK proof), got: $PROVER_WON"
    exit 1
fi
ok "disputeProverWon: true"

if [ "$IS_VALID" != "true" ]; then
    err "isResultValid should be true after prover wins dispute, got: $IS_VALID"
    exit 1
fi
ok "isResultValid: true"

# ══════════════════════════════════════════════════════════════════════════════
# Summary
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  FULL PIPELINE DEMO PASSED!"
echo "═══════════════════════════════════════════════════════════"
echo "  RemainderVerifier:  $REMAINDER_VERIFIER"
echo "  TEEMLVerifier:      $TEE_VERIFIER"
echo "  Enclave (Docker):   $ENCLAVE_ADDR"
echo "  Result ID:          ${RESULT_ID:0:18}..."
echo ""
echo "  Flow:"
echo "    1. Docker TEE → XGBoost inference → ECDSA sign"
echo "    2. Submit on-chain with prover stake"
echo "    3. Pre-compute ZK proof (live, from same model+features)"
echo "    4. Challenger disputes result"
echo "    5. Resolve dispute with ZK proof → prover wins"
echo "    6. Result validated on-chain"
echo "═══════════════════════════════════════════════════════════"
