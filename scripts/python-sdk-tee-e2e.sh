#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# Python SDK -- TEE Verifier E2E Test
#
# Tests the Python SDK TEEVerifier against a live Anvil instance:
#   1. Start Anvil
#   2. Deploy TEEMLVerifier via forge create
#   3. Install Python SDK
#   4. Run Python E2E covering:
#      - owner() read
#      - pause / unpause / paused()
#      - challenge_bond_amount() / prover_stake_amount()
#      - register_enclave
#      - submit_result (with ECDSA attestation signed in Python)
#      - get_result
#      - is_result_valid (before and after finalize)
#      - fast-forward past challenge window + finalize
#      - 2-step ownership transfer (transfer_ownership / pending_owner / accept_ownership)
#      - revoke_enclave
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="$ROOT_DIR/contracts"
SDK_DIR="$ROOT_DIR/sdk/python"

ANVIL_PORT=8553
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

# Anvil account #0 -- admin / submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Anvil account #1 -- enclave signer
ENCLAVE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
ENCLAVE_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"

# Anvil account #2 -- new owner for transfer test
NEW_OWNER_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
NEW_OWNER_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"

log() { echo "==> $*"; }
ok()  { echo "  [OK] $*"; }
err() { echo "  [FAIL] $*" >&2; }

cleanup() {
    if [ -n "${ANVIL_PID:-}" ]; then
        kill "$ANVIL_PID" 2>/dev/null || true
        wait "$ANVIL_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Step 1: Start Anvil ──────────────────────────────────────────────────────

log "Step 1: Starting Anvil on port $ANVIL_PORT..."
anvil --port "$ANVIL_PORT" --silent &
ANVIL_PID=$!
sleep 2

if ! curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' > /dev/null 2>&1; then
    err "Anvil failed to start on port $ANVIL_PORT"
    exit 1
fi
ok "Anvil running (PID: $ANVIL_PID)"

# ── Step 2: Deploy TEEMLVerifier ──────────────────────────────────────────────

log "Step 2: Deploying TEEMLVerifier..."
cd "$CONTRACTS_DIR"

TEE_VERIFIER=$(forge create src/tee/TEEMLVerifier.sol:TEEMLVerifier \
    --broadcast --json \
    --rpc-url "$RPC_URL" \
    --private-key "$ADMIN_KEY" \
    --constructor-args "$ADMIN_ADDR" "0x0000000000000000000000000000000000000000" \
    2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['deployedTo'])")

if [ -z "$TEE_VERIFIER" ]; then
    err "Failed to deploy TEEMLVerifier"
    exit 1
fi
ok "TEEMLVerifier deployed at: $TEE_VERIFIER"

# ── Step 3: Install Python SDK ────────────────────────────────────────────────

log "Step 3: Installing Python SDK..."
cd "$SDK_DIR"
python3 -m pip install ".[web3]" --quiet
ok "Python SDK installed"

# ── Step 4: Run Python E2E tests ──────────────────────────────────────────────

log "Step 4: Running Python SDK TEE E2E tests..."

python3 - "$RPC_URL" "$ADMIN_KEY" "$ADMIN_ADDR" "$ENCLAVE_KEY" "$ENCLAVE_ADDR" \
          "$NEW_OWNER_KEY" "$NEW_OWNER_ADDR" "$TEE_VERIFIER" << 'PYEOF'
import sys
import requests
from web3 import Web3
from eth_account import Account
from eth_account.messages import encode_defunct

from worldzk.tee_verifier import TEEVerifier

# ── Configuration from argv ───────────────────────────────────────────────────

RPC_URL        = sys.argv[1]
ADMIN_KEY      = sys.argv[2]
ADMIN_ADDR     = sys.argv[3]
ENCLAVE_KEY    = sys.argv[4]
ENCLAVE_ADDR   = sys.argv[5]
NEW_OWNER_KEY  = sys.argv[6]
NEW_OWNER_ADDR = sys.argv[7]
CONTRACT_ADDR  = sys.argv[8]

# ── Helpers ───────────────────────────────────────────────────────────────────

w3 = Web3(Web3.HTTPProvider(RPC_URL))
assert w3.is_connected(), "Not connected to Anvil"

tests_passed = 0
tests_failed = 0


def ok(msg):
    global tests_passed
    tests_passed += 1
    print(f"  [OK] {msg}")


def fail(msg):
    global tests_failed
    tests_failed += 1
    print(f"  [FAIL] {msg}", file=sys.stderr)


def anvil_increase_time(seconds):
    """Fast-forward Anvil block time."""
    requests.post(RPC_URL, json={
        "jsonrpc": "2.0",
        "method": "evm_increaseTime",
        "params": [seconds],
        "id": 1,
    })
    requests.post(RPC_URL, json={
        "jsonrpc": "2.0",
        "method": "evm_mine",
        "params": [],
        "id": 2,
    })


def sign_attestation(model_hash_bytes, input_hash_bytes, result_data, enclave_private_key):
    """
    Sign the TEE attestation matching the contract verification logic:
      message = keccak256(encodePacked(modelHash, inputHash, keccak256(result)))
      then EIP-191 personal sign of that 32-byte message.
    """
    result_hash = w3.keccak(result_data)
    packed = model_hash_bytes + input_hash_bytes + result_hash
    message_hash = w3.keccak(packed)
    signable = encode_defunct(message_hash)
    signed = Account.sign_message(signable, private_key=enclave_private_key)
    return signed.signature


def compute_result_id(sender, model_hash_bytes, input_hash_bytes, block_number):
    """
    resultId = keccak256(abi.encodePacked(sender, modelHash, inputHash, blockNumber))
    sender: 20 bytes, modelHash/inputHash: 32 bytes each, blockNumber: uint256 (32 bytes)
    """
    sender_bytes = bytes.fromhex(sender[2:])
    block_bytes = block_number.to_bytes(32, byteorder="big")
    packed = sender_bytes + model_hash_bytes + input_hash_bytes + block_bytes
    return w3.keccak(packed)


# ══════════════════════════════════════════════════════════════════════════════
# Tests
# ══════════════════════════════════════════════════════════════════════════════

# ── Test 1: owner() ──────────────────────────────────────────────────────────

print("--- Test 1: owner() ---")
verifier = TEEVerifier(
    rpc_url=RPC_URL,
    private_key=ADMIN_KEY,
    contract_address=CONTRACT_ADDR,
    gas_limit=1_000_000,
)

owner = verifier.owner()
if owner.lower() == ADMIN_ADDR.lower():
    ok(f"owner() = {owner}")
else:
    fail(f"owner() expected {ADMIN_ADDR}, got {owner}")

# ── Test 2: paused() -- should be false initially ────────────────────────────

print("--- Test 2: paused() ---")
is_paused = verifier.paused()
if not is_paused:
    ok("paused() = False (initial)")
else:
    fail(f"paused() expected False, got {is_paused}")

# ── Test 3: pause / unpause ─────────────────────────────────────────────────

print("--- Test 3: pause / unpause ---")
tx = verifier.pause()
ok(f"pause() tx = {tx[:18]}...")

is_paused = verifier.paused()
if is_paused:
    ok("paused() = True after pause()")
else:
    fail(f"paused() expected True, got {is_paused}")

tx = verifier.unpause()
ok(f"unpause() tx = {tx[:18]}...")

is_paused = verifier.paused()
if not is_paused:
    ok("paused() = False after unpause()")
else:
    fail(f"paused() expected False, got {is_paused}")

# ── Test 4: challenge_bond_amount / prover_stake_amount ──────────────────────

print("--- Test 4: challenge_bond_amount / prover_stake_amount ---")
bond = verifier.challenge_bond_amount()
if bond == 10**17:  # 0.1 ether
    ok(f"challenge_bond_amount() = {bond} (0.1 ETH)")
else:
    fail(f"challenge_bond_amount() expected {10**17}, got {bond}")

stake = verifier.prover_stake_amount()
if stake == 10**17:  # 0.1 ether
    ok(f"prover_stake_amount() = {stake} (0.1 ETH)")
else:
    fail(f"prover_stake_amount() expected {10**17}, got {stake}")

# ── Test 5: register_enclave ────────────────────────────────────────────────

print("--- Test 5: register_enclave ---")
image_hash = w3.keccak(text="test-enclave-image-v1")
tx = verifier.register_enclave(ENCLAVE_ADDR, image_hash)
ok(f"register_enclave() tx = {tx[:18]}...")

# ── Test 6: submit_result ───────────────────────────────────────────────────

print("--- Test 6: submit_result ---")
model_hash = w3.keccak(text="xgboost-model-weights")
input_hash = w3.keccak(text="test-input-data")
result_data = b"\xde\xad\xbe\xef"

attestation = sign_attestation(model_hash, input_hash, result_data, ENCLAVE_KEY)
ok(f"attestation signed ({len(attestation)} bytes)")

# Record block number before submission
pre_block = w3.eth.block_number

result_ref = verifier.submit_result(
    model_hash=model_hash,
    input_hash=input_hash,
    result=result_data,
    attestation=attestation,
    stake_wei=10**17,  # 0.1 ETH
)
ok(f"submit_result() returned = {result_ref[:18]}...")

# Compute the result_id from on-chain formula
submit_block = pre_block + 1
result_id = compute_result_id(ADMIN_ADDR, model_hash, input_hash, submit_block)
result_id_hex = "0x" + result_id.hex()
ok(f"computed result_id = {result_id_hex[:18]}...")

# ── Test 7: get_result ──────────────────────────────────────────────────────

print("--- Test 7: get_result ---")
ml_result = verifier.get_result(result_id)
if ml_result.submitter.lower() == ADMIN_ADDR.lower():
    ok(f"get_result().submitter = {ml_result.submitter}")
else:
    fail(f"get_result().submitter expected {ADMIN_ADDR}, got {ml_result.submitter}")

if ml_result.enclave.lower() == ENCLAVE_ADDR.lower():
    ok(f"get_result().enclave = {ml_result.enclave}")
else:
    fail(f"get_result().enclave expected {ENCLAVE_ADDR}, got {ml_result.enclave}")

if not ml_result.finalized:
    ok("get_result().finalized = False (before finalization)")
else:
    fail("get_result().finalized expected False")

if not ml_result.challenged:
    ok("get_result().challenged = False (no challenge)")
else:
    fail("get_result().challenged expected False")

if ml_result.prover_stake_amount == 10**17:
    ok(f"get_result().prover_stake_amount = {ml_result.prover_stake_amount}")
else:
    fail(f"get_result().prover_stake_amount expected {10**17}, got {ml_result.prover_stake_amount}")

# ── Test 8: is_result_valid before finalization ─────────────────────────────

print("--- Test 8: is_result_valid (before finalize) ---")
is_valid = verifier.is_result_valid(result_id)
if not is_valid:
    ok("is_result_valid() = False (not yet finalized)")
else:
    fail("is_result_valid() expected False before finalize")

# ── Test 9: fast-forward + finalize ─────────────────────────────────────────

print("--- Test 9: finalize ---")
anvil_increase_time(3601)
ok("time fast-forwarded by 3601 seconds")

tx = verifier.finalize(result_id)
ok(f"finalize() tx = {tx[:18]}...")

# ── Test 10: is_result_valid after finalization ─────────────────────────────

print("--- Test 10: is_result_valid (after finalize) ---")
is_valid = verifier.is_result_valid(result_id)
if is_valid:
    ok("is_result_valid() = True (finalized)")
else:
    fail("is_result_valid() expected True after finalize")

# ── Test 11: get_result after finalization ───────────────────────────────────

print("--- Test 11: get_result (after finalize) ---")
ml_result = verifier.get_result(result_id)
if ml_result.finalized:
    ok("get_result().finalized = True")
else:
    fail("get_result().finalized expected True")

# ── Test 12: 2-step ownership transfer ──────────────────────────────────────

print("--- Test 12: 2-step ownership transfer ---")
tx = verifier.transfer_ownership(NEW_OWNER_ADDR)
ok(f"transfer_ownership() tx = {tx[:18]}...")

pending = verifier.pending_owner()
if pending.lower() == NEW_OWNER_ADDR.lower():
    ok(f"pending_owner() = {pending}")
else:
    fail(f"pending_owner() expected {NEW_OWNER_ADDR}, got {pending}")

# Accept from the new owner's perspective
new_owner_verifier = TEEVerifier(
    rpc_url=RPC_URL,
    private_key=NEW_OWNER_KEY,
    contract_address=CONTRACT_ADDR,
    gas_limit=1_000_000,
)
tx = new_owner_verifier.accept_ownership()
ok(f"accept_ownership() tx = {tx[:18]}...")

owner = new_owner_verifier.owner()
if owner.lower() == NEW_OWNER_ADDR.lower():
    ok(f"owner() = {owner} (after transfer)")
else:
    fail(f"owner() expected {NEW_OWNER_ADDR}, got {owner}")

# Transfer back to original admin
tx = new_owner_verifier.transfer_ownership(ADMIN_ADDR)
ok(f"transfer_ownership() back tx = {tx[:18]}...")
tx = verifier.accept_ownership()
ok(f"accept_ownership() by admin tx = {tx[:18]}...")

owner = verifier.owner()
if owner.lower() == ADMIN_ADDR.lower():
    ok(f"owner() = {owner} (restored)")
else:
    fail(f"owner() expected {ADMIN_ADDR}, got {owner}")

# ── Test 13: revoke_enclave ─────────────────────────────────────────────────

print("--- Test 13: revoke_enclave ---")
tx = verifier.revoke_enclave(ENCLAVE_ADDR)
ok(f"revoke_enclave() tx = {tx[:18]}...")

# ── Summary ──────────────────────────────────────────────────────────────────

print()
if tests_failed > 0:
    print(f"RESULT: {tests_passed} passed, {tests_failed} failed")
    sys.exit(1)
else:
    print(f"RESULT: {tests_passed} tests passed")
    sys.exit(0)
PYEOF

PYTHON_EXIT=$?
if [ "$PYTHON_EXIT" -ne 0 ]; then
    err "Python SDK TEE E2E tests FAILED"
    exit 1
fi
ok "Python SDK TEE E2E tests passed"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "================================================================"
echo "  Python SDK TEE Verifier E2E -- PASSED"
echo "================================================================"
echo "  Contract:  $TEE_VERIFIER"
echo "  Tested:    owner, paused, pause/unpause, bond/stake amounts,"
echo "             register_enclave, submit_result, get_result,"
echo "             is_result_valid, finalize, 2-step ownership,"
echo "             revoke_enclave"
echo "================================================================"
