#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════════════════
# Python SDK -- TEE Verifier E2E Test
#
# Tests the Python SDK TEEVerifier against a live Anvil instance:
#   1. Start Anvil
#   2. Deploy TEEMLVerifier behind its UUPS proxy (forge script)
#   3. Install Python SDK
#   4. Run Python E2E covering:
#      - admin() / implementation() / timelock() reads
#      - pause / unpause / paused()
#      - challenge_bond_amount() / prover_stake_amount()
#      - register_enclave
#      - submit_result (with EIP-712 attestation signed in Python)
#      - submit_result with an invalid attestation (must revert)
#      - get_result
#      - is_result_valid (before and after finalize)
#      - fast-forward past challenge window + finalize
#      - admin transfer (change_admin)
#      - revoke_enclave
#
# Environment overrides: PYTHON (default python3), ANVIL_PORT (default 8553),
# CONTRACTS_DIR (default <repo>/contracts).
# ═══════════════════════════════════════════════════════════════════════════════

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONTRACTS_DIR="${CONTRACTS_DIR:-$ROOT_DIR/contracts}"
SDK_DIR="$ROOT_DIR/sdk/python"
PYTHON="${PYTHON:-python3}"

ANVIL_PORT="${ANVIL_PORT:-8553}"
RPC_URL="http://127.0.0.1:${ANVIL_PORT}"

# Anvil account #0 -- admin / submitter
ADMIN_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ADMIN_ADDR="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"

# Anvil account #1 -- enclave signer
ENCLAVE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
ENCLAVE_ADDR="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"

# Anvil account #2 -- new admin for transfer test
NEW_ADMIN_KEY="0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
NEW_ADMIN_ADDR="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"

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

for i in $(seq 1 30); do
    if cast block-number --rpc-url "$RPC_URL" &>/dev/null; then
        break
    fi
    if [ "$i" -eq 30 ]; then
        err "Anvil failed to start on port $ANVIL_PORT"
        exit 1
    fi
    sleep 0.5
done
ok "Anvil running (PID: $ANVIL_PID)"

# ── Step 2: Deploy TEEMLVerifier ──────────────────────────────────────────────

log "Step 2: Deploying TEEMLVerifier..."
cd "$CONTRACTS_DIR"

# Deploy TEEMLVerifier behind its UUPS proxy.
# TEEMLVerifier.initialize rejects address(0). This flow never resolves a ZK dispute,
# so a non-zero placeholder is enough unless a real RemainderVerifier is supplied.
REMAINDER_VERIFIER="${REMAINDER_VERIFIER:-0xDDdDddDdDdddDDddDDddDDDDdDdDDdDDdDDDDDDd}"
export REMAINDER_VERIFIER
# The deploy script reads DEPLOYER_KEY as uint via vm.envUint
export DEPLOYER_KEY="$ADMIN_KEY"
DEPLOY_OUTPUT=$(forge script script/DeployTEEMLVerifier.s.sol:DeployTEEMLVerifier \
    --rpc-url "$RPC_URL" \
    --broadcast 2>&1) || {
    err "Failed to deploy TEEMLVerifier"
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
}

# The script prints the proxy address; the broadcast entry named "TEEMLVerifier" is the
# implementation, which must not be used directly.
TEE_VERIFIER=$(echo "$DEPLOY_OUTPUT" | grep "TEEMLVerifier deployed at:" | head -1 | awk '{print $NF}')
if [ -z "$TEE_VERIFIER" ]; then
    err "TEEMLVerifier proxy address not found in deploy output"
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
fi
ok "TEEMLVerifier deployed at: $TEE_VERIFIER"

# ── Step 3: Install Python SDK ────────────────────────────────────────────────

log "Step 3: Installing Python SDK..."
cd "$SDK_DIR"
"$PYTHON" -m pip install ".[web3]" --quiet
ok "Python SDK installed"

# ── Step 4: Run Python E2E tests ──────────────────────────────────────────────

log "Step 4: Running Python SDK TEE E2E tests..."

# The SDK was installed above; run from a directory that does not shadow it.
cd "$ROOT_DIR"

"$PYTHON" - "$RPC_URL" "$ADMIN_KEY" "$ADMIN_ADDR" "$ENCLAVE_KEY" "$ENCLAVE_ADDR" \
            "$NEW_ADMIN_KEY" "$NEW_ADMIN_ADDR" "$TEE_VERIFIER" << 'PYEOF'
import sys
from web3 import Web3
from eth_account import Account

from worldzk.tee_verifier import TEEVerifier

# ── Configuration from argv ───────────────────────────────────────────────────

RPC_URL        = sys.argv[1]
ADMIN_KEY      = sys.argv[2]
ADMIN_ADDR     = sys.argv[3]
ENCLAVE_KEY    = sys.argv[4]
ENCLAVE_ADDR   = sys.argv[5]
NEW_ADMIN_KEY  = sys.argv[6]
NEW_ADMIN_ADDR = sys.argv[7]
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
    w3.provider.make_request("evm_increaseTime", [seconds])
    w3.provider.make_request("evm_mine", [])


def sign_attestation(model_hash_bytes, input_hash_bytes, result_data, enclave_private_key):
    """
    Sign the TEE attestation the way TEEMLVerifier.submitResult recovers it (EIP-712):
      structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, keccak256(result)))
      digest     = keccak256(0x1901 ++ DOMAIN_SEPARATOR ++ structHash)
    with domain (name "TEEMLVerifier", version "1", chainId, verifyingContract).
    """
    signed = Account.sign_typed_data(
        enclave_private_key,
        full_message={
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"},
                ],
                "TEEMLResult": [
                    {"name": "modelHash", "type": "bytes32"},
                    {"name": "inputHash", "type": "bytes32"},
                    {"name": "resultHash", "type": "bytes32"},
                ],
            },
            "primaryType": "TEEMLResult",
            "domain": {
                "name": "TEEMLVerifier",
                "version": "1",
                "chainId": w3.eth.chain_id,
                "verifyingContract": Web3.to_checksum_address(CONTRACT_ADDR),
            },
            "message": {
                "modelHash": model_hash_bytes,
                "inputHash": input_hash_bytes,
                "resultHash": w3.keccak(result_data),
            },
        },
    )
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

# ── Test 1: admin() / implementation() / timelock() ─────────────────────────

print("--- Test 1: admin() / implementation() / timelock() ---")
verifier = TEEVerifier(
    rpc_url=RPC_URL,
    private_key=ADMIN_KEY,
    contract_address=CONTRACT_ADDR,
    gas_limit=1_000_000,
)

admin = verifier.admin()
if admin.lower() == ADMIN_ADDR.lower():
    ok(f"admin() = {admin}")
else:
    fail(f"admin() expected {ADMIN_ADDR}, got {admin}")

implementation = verifier.implementation()
if int(implementation, 16) != 0 and implementation.lower() != CONTRACT_ADDR.lower():
    ok(f"implementation() = {implementation} (behind proxy {CONTRACT_ADDR})")
else:
    fail(f"implementation() unexpected: {implementation}")

timelock = verifier.timelock()
if int(timelock, 16) == 0:
    ok("timelock() = zero address (none set)")
else:
    fail(f"timelock() expected zero address, got {timelock}")

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

# An attestation that is not a signature by a registered enclave must be rejected
try:
    verifier.submit_result(
        model_hash=model_hash,
        input_hash=input_hash,
        result=result_data,
        attestation=b"\x00" * 65,
        stake_wei=10**17,
    )
    fail("submit_result() accepted an all-zero attestation")
except RuntimeError as e:
    ok(f"submit_result() with invalid attestation rejected ({e})"[:90])

result_id_hex = verifier.submit_result(
    model_hash=model_hash,
    input_hash=input_hash,
    result=result_data,
    attestation=attestation,
    stake_wei=10**17,  # 0.1 ETH
)
ok(f"submit_result() returned result ID = {result_id_hex[:18]}...")

# The SDK reads the ID from the ResultSubmitted event; check it against the on-chain formula
result_id = compute_result_id(ADMIN_ADDR, model_hash, input_hash, w3.eth.block_number)
if result_id_hex == "0x" + result_id.hex():
    ok("result ID matches keccak256(sender, modelHash, inputHash, blockNumber)")
else:
    fail(f"result ID mismatch: SDK {result_id_hex}, computed 0x{result_id.hex()}")

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

# ── Test 12: admin transfer ─────────────────────────────────────────────────

print("--- Test 12: admin transfer ---")
tx = verifier.change_admin(NEW_ADMIN_ADDR)
ok(f"change_admin() tx = {tx[:18]}...")

# changeAdmin is single-step: the new admin takes over immediately
new_admin_verifier = TEEVerifier(
    rpc_url=RPC_URL,
    private_key=NEW_ADMIN_KEY,
    contract_address=CONTRACT_ADDR,
    gas_limit=1_000_000,
)
admin = new_admin_verifier.admin()
if admin.lower() == NEW_ADMIN_ADDR.lower():
    ok(f"admin() = {admin} (after transfer)")
else:
    fail(f"admin() expected {NEW_ADMIN_ADDR}, got {admin}")

# Transfer back to original admin
tx = new_admin_verifier.change_admin(ADMIN_ADDR)
ok(f"change_admin() back tx = {tx[:18]}...")

admin = verifier.admin()
if admin.lower() == ADMIN_ADDR.lower():
    ok(f"admin() = {admin} (restored)")
else:
    fail(f"admin() expected {ADMIN_ADDR}, got {admin}")

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
echo "  Tested:    admin, paused, pause/unpause, bond/stake amounts,"
echo "             register_enclave, submit_result, get_result,"
echo "             is_result_valid, finalize, admin transfer,"
echo "             revoke_enclave"
echo "================================================================"
