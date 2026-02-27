#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# Remainder ZKML E2E Test
#
# Full end-to-end test:
# 1. Run XGBoost inference with Remainder (GKR+Hyrax proof)
# 2. Deploy RemainderVerifier to local Anvil
# 3. Register circuit and submit proof on-chain
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
EXAMPLE_DIR="$ROOT_DIR/examples/xgboost-remainder"
CONTRACTS_DIR="$ROOT_DIR/contracts"
PROOF_OUTPUT="/tmp/remainder_e2e_proof.json"

ANVIL_PORT=8546
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"
DEPLOYER_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

log() { echo "==> $*"; }
ok()  { echo "  ✓ $*"; }
err() { echo "  ✗ $*" >&2; }

# Check if Anvil is running
if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    log "Starting Anvil on port $ANVIL_PORT..."
    anvil --port "$ANVIL_PORT" --silent &
    ANVIL_PID=$!
    sleep 2
    trap "kill $ANVIL_PID 2>/dev/null || true" EXIT
else
    log "Anvil already running on port $ANVIL_PORT"
fi

# ── Step 1: Generate proof with Remainder ─────────────────────────────────────
log "Step 1: Generating Remainder (GKR+Hyrax) proof for XGBoost inference..."
cd "$EXAMPLE_DIR"

cargo run --release -- \
    --model sample_model.json \
    --input sample_input.json \
    --output "$PROOF_OUTPUT" 2>&1

# Extract proof data from JSON
CIRCUIT_HASH=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print('0x' + d['circuit_hash'])")
PREDICTED_CLASS=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['predicted_class'])")
PROOF_SIZE=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['proof_size_bytes'])")
PROVE_TIME=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print(d['prove_time_ms'])")

ok "Proof generated: ${PROOF_SIZE} bytes in ${PROVE_TIME}ms, predicted class: ${PREDICTED_CLASS}"

# The proof is bincode-serialized (Rust native format).
# For on-chain submission, we prepend the "REM1" selector.
# The raw proof hex is already ABI-wrapped in proof_hex.
PROOF_HEX=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print('0x' + d['proof_hex'])")
PUBLIC_INPUTS_HEX=$(python3 -c "import json; d=json.load(open('$PROOF_OUTPUT')); print('0x' + d['public_inputs_hex'])")

# For on-chain, we need proof starting with "REM1" selector
# The raw proof bytes start with the bincode data; prepend REM1
PROOF_WITH_SELECTOR="0x52454d31$(python3 -c "
import json
d = json.load(open('$PROOF_OUTPUT'))
# Raw proof bytes (not ABI-wrapped)
raw = bytes.fromhex(d['proof_hex'])
# Skip the ABI wrapper (32 bytes offset + 32 bytes length)
# Just use the actual proof data
actual_proof = raw[64:64 + int.from_bytes(raw[32:64], 'big')]
print(actual_proof.hex())
")"

ok "Proof data prepared for on-chain submission"

# ── Step 2: Deploy RemainderVerifier ──────────────────────────────────────────
log "Step 2: Deploying RemainderVerifier to Anvil..."
cd "$CONTRACTS_DIR"

CIRCUIT_HASH_CLEAN="${CIRCUIT_HASH}"

# Deploy using forge script
DEPLOYER_KEY="$DEPLOYER_KEY" \
CIRCUIT_HASH="$CIRCUIT_HASH_CLEAN" \
PROOF_DATA="$PROOF_WITH_SELECTOR" \
PUBLIC_INPUTS="$PUBLIC_INPUTS_HEX" \
forge script script/RemainderE2E.s.sol:RemainderE2E \
    --rpc-url "$RPC_URL" \
    --broadcast \
    -vvv 2>&1

ok "E2E test complete!"

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════════════════════"
echo "  Remainder ZKML E2E Test Results"
echo "════════════════════════════════════════════════════════════"
echo "  Proof System:     Remainder (GKR + Hyrax PCS)"
echo "  Model:            XGBoost (2 trees, 5 features, depth 3)"
echo "  Predicted Class:  $PREDICTED_CLASS"
echo "  Proof Size:       $PROOF_SIZE bytes"
echo "  Prove Time:       ${PROVE_TIME}ms"
echo "  Circuit Hash:     $CIRCUIT_HASH"
echo "════════════════════════════════════════════════════════════"
