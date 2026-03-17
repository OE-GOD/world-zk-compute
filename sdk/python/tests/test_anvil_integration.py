"""Anvil-based integration tests for the TEEVerifier Python SDK.

These tests spin up a local Anvil instance, deploy the TEEMLVerifier contract
using ``forge create``, and exercise the SDK client against the live contract.

Requirements:
    - ``anvil`` on PATH (from Foundry)
    - ``forge`` on PATH (from Foundry)
    - ``web3`` Python package installed (``pip install worldzk[web3]``)

All tests are skipped automatically when ``anvil`` or ``forge`` are not found.
"""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import time
from typing import Generator

import pytest

# ---------------------------------------------------------------------------
# Skip the entire module when foundry tools or web3 are missing
# ---------------------------------------------------------------------------

_HAS_ANVIL = shutil.which("anvil") is not None
_HAS_FORGE = shutil.which("forge") is not None

try:
    from web3 import Web3
    from web3.exceptions import ContractLogicError

    _HAS_WEB3 = True
except ImportError:
    _HAS_WEB3 = False

pytestmark = pytest.mark.skipif(
    not (_HAS_ANVIL and _HAS_FORGE and _HAS_WEB3),
    reason="Requires anvil, forge, and web3 to be available",
)

# ---------------------------------------------------------------------------
# Constants -- Anvil default funded accounts
# ---------------------------------------------------------------------------

ANVIL_PORT = 8546
ANVIL_RPC = f"http://127.0.0.1:{ANVIL_PORT}"

# Anvil account #0 (deployer / owner)
DEPLOYER_ADDRESS = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# Anvil account #1 (used as a second user)
USER2_ADDRESS = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
USER2_KEY = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"

# Anvil account #2 (used as enclave key)
ENCLAVE_ADDRESS = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
ENCLAVE_KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"

# Contracts directory (for forge create)
CONTRACTS_DIR = os.path.normpath(
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "contracts")
)

# Dummy 32-byte values for tests
ZERO_BYTES32 = b"\x00" * 32
IMAGE_HASH = b"\xaa" * 32

# ABI fragment for the public ``enclaves`` mapping getter (not in the SDK).
_ENCLAVES_ABI = [
    {
        "type": "function",
        "name": "enclaves",
        "inputs": [{"name": "", "type": "address"}],
        "outputs": [
            {"name": "registered", "type": "bool"},
            {"name": "active", "type": "bool"},
            {"name": "enclaveImageHash", "type": "bytes32"},
            {"name": "registeredAt", "type": "uint256"},
        ],
        "stateMutability": "view",
    }
]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _sign_attestation(
    model_hash: bytes,
    input_hash: bytes,
    result_data: bytes,
    contract_address: str,
    chain_id: int = 31337,
) -> bytes:
    """Produce an EIP-712 typed-data attestation matching the contract logic.

    The contract computes::

        structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, resultHash))
        digest = keccak256("\\x19\\x01" || DOMAIN_SEPARATOR || structHash)

    where DOMAIN_SEPARATOR uses EIP712Domain(name="TEEMLVerifier", version="1",
    chainId, verifyingContract).  We replicate this with
    ``eth_account.Account.sign_typed_data``.
    """
    from eth_account import Account

    result_hash = Web3.keccak(result_data)

    domain_data = {
        "name": "TEEMLVerifier",
        "version": "1",
        "chainId": chain_id,
        "verifyingContract": Web3.to_checksum_address(contract_address),
    }

    message_data = {
        "modelHash": model_hash,
        "inputHash": input_hash,
        "resultHash": result_hash,
    }

    full_message = {
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
        "domain": domain_data,
        "message": message_data,
    }

    signed = Account.sign_typed_data(
        private_key=ENCLAVE_KEY,
        full_message=full_message,
    )
    return signed.signature


def _raw_contract(w3_inst: "Web3", address: str) -> "Contract":
    """Return a web3 Contract with the SDK ABI plus the ``enclaves`` getter."""
    from worldzk.tee_verifier import TEE_ML_VERIFIER_ABI

    combined_abi = TEE_ML_VERIFIER_ABI + _ENCLAVES_ABI
    return w3_inst.eth.contract(
        address=Web3.to_checksum_address(address),
        abi=combined_abi,
    )


def _compute_result_id(
    w3_inst: "Web3", tx_hash_hex: str, sender: str, model_hash: bytes, input_hash: bytes
) -> str:
    """Compute the on-chain result ID from a submit transaction.

    The contract uses::

        resultId = keccak256(abi.encodePacked(msg.sender, modelHash, inputHash, block.number))

    We fetch the block number from the transaction receipt.
    """
    receipt = w3_inst.eth.get_transaction_receipt(tx_hash_hex)
    block_number = receipt["blockNumber"]
    sender_bytes = bytes.fromhex(sender[2:].lower())
    result_id = Web3.keccak(
        sender_bytes + model_hash + input_hash + block_number.to_bytes(32, "big")
    )
    return "0x" + result_id.hex()


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def anvil_process() -> Generator[subprocess.Popen, None, None]:
    """Start an Anvil instance on a dedicated port for the test session."""
    proc = subprocess.Popen(
        ["anvil", "--port", str(ANVIL_PORT), "--silent"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    # Wait for Anvil to be ready by polling the RPC endpoint
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            w3 = Web3(Web3.HTTPProvider(ANVIL_RPC))
            if w3.is_connected():
                break
        except Exception:
            pass
        time.sleep(0.3)
    else:
        proc.kill()
        pytest.fail("Anvil did not start within 15 seconds")

    yield proc

    # Teardown: terminate Anvil
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


@pytest.fixture(scope="module")
def contract_address(anvil_process: subprocess.Popen) -> str:
    """Deploy TEEMLVerifier via ``forge create`` and return its address."""
    result = subprocess.run(
        [
            "forge",
            "create",
            "--broadcast",
            "src/tee/TEEMLVerifier.sol:TEEMLVerifier",
            "--rpc-url",
            ANVIL_RPC,
            "--private-key",
            DEPLOYER_KEY,
            # Constructor args: admin, remainderVerifier
            "--constructor-args",
            DEPLOYER_ADDRESS,
            "0x0000000000000000000000000000000000000000",
        ],
        capture_output=True,
        text=True,
        cwd=CONTRACTS_DIR,
        timeout=120,
    )
    if result.returncode != 0:
        pytest.fail(
            f"forge create failed (exit {result.returncode}):\n"
            f"stdout: {result.stdout}\n"
            f"stderr: {result.stderr}"
        )

    # Parse the "Deployed to: 0x..." line from forge create output
    addr = None
    for line in result.stdout.splitlines():
        if "Deployed to:" in line:
            addr = line.split("Deployed to:")[-1].strip()
            break
    if addr is None:
        pytest.fail(
            f"Could not parse deployed address from forge output:\n{result.stdout}"
        )

    return Web3.to_checksum_address(addr)


@pytest.fixture(scope="module")
def deployer_verifier(contract_address: str) -> "TEEVerifier":
    """TEEVerifier instance connected as the deployer/owner."""
    from worldzk.tee_verifier import TEEVerifier

    return TEEVerifier(
        rpc_url=ANVIL_RPC,
        private_key=DEPLOYER_KEY,
        contract_address=contract_address,
    )


@pytest.fixture(scope="module")
def user2_verifier(contract_address: str) -> "TEEVerifier":
    """TEEVerifier instance connected as a non-owner user."""
    from worldzk.tee_verifier import TEEVerifier

    return TEEVerifier(
        rpc_url=ANVIL_RPC,
        private_key=USER2_KEY,
        contract_address=contract_address,
    )


@pytest.fixture(scope="module")
def w3() -> "Web3":
    """Raw Web3 instance connected to Anvil."""
    return Web3(Web3.HTTPProvider(ANVIL_RPC))


# ---------------------------------------------------------------------------
# Tests: ownership
# ---------------------------------------------------------------------------


class TestOwnership:
    """Test Ownable2Step functionality via the SDK."""

    def test_owner_returns_deployer(self, deployer_verifier: "TEEVerifier") -> None:
        owner = deployer_verifier.owner()
        assert owner.lower() == DEPLOYER_ADDRESS.lower()

    def test_pending_owner_initially_zero(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        pending = deployer_verifier.pending_owner()
        assert pending == "0x0000000000000000000000000000000000000000"

    def test_transfer_ownership_sets_pending(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        tx_hash = deployer_verifier.transfer_ownership(USER2_ADDRESS)
        assert tx_hash.startswith("0x")
        pending = deployer_verifier.pending_owner()
        assert pending.lower() == USER2_ADDRESS.lower()

    def test_accept_ownership_completes_transfer(
        self,
        deployer_verifier: "TEEVerifier",
        user2_verifier: "TEEVerifier",
    ) -> None:
        # user2 accepts ownership
        tx_hash = user2_verifier.accept_ownership()
        assert tx_hash.startswith("0x")
        assert deployer_verifier.owner().lower() == USER2_ADDRESS.lower()

        # Transfer back to original deployer so other tests work
        user2_verifier.transfer_ownership(DEPLOYER_ADDRESS)
        deployer_verifier.accept_ownership()
        assert deployer_verifier.owner().lower() == DEPLOYER_ADDRESS.lower()


# ---------------------------------------------------------------------------
# Tests: pause / unpause
# ---------------------------------------------------------------------------


class TestPauseUnpause:
    """Test Pausable functionality via the SDK."""

    def test_initially_not_paused(self, deployer_verifier: "TEEVerifier") -> None:
        assert deployer_verifier.paused() is False

    def test_owner_can_pause(self, deployer_verifier: "TEEVerifier") -> None:
        tx_hash = deployer_verifier.pause()
        assert tx_hash.startswith("0x")
        assert deployer_verifier.paused() is True

    def test_owner_can_unpause(self, deployer_verifier: "TEEVerifier") -> None:
        # Ensure it is paused first
        if not deployer_verifier.paused():
            deployer_verifier.pause()
        tx_hash = deployer_verifier.unpause()
        assert tx_hash.startswith("0x")
        assert deployer_verifier.paused() is False

    def test_non_owner_cannot_pause(
        self, w3: "Web3", contract_address: str
    ) -> None:
        """Simulate pause() from non-owner via eth_call -- should revert.

        The SDK's _send_tx does not raise on reverted receipts, so we use a
        raw contract.functions.pause().call() which raises ContractLogicError.
        """
        contract = _raw_contract(w3, contract_address)
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.pause().call({"from": USER2_ADDRESS})


# ---------------------------------------------------------------------------
# Tests: register enclave
# ---------------------------------------------------------------------------


class TestRegisterEnclave:
    """Test enclave registration via the SDK."""

    def test_register_enclave_succeeds(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        tx_hash = deployer_verifier.register_enclave(
            ENCLAVE_ADDRESS, IMAGE_HASH
        )
        assert tx_hash.startswith("0x")

        # Verify on-chain via raw call
        contract = w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=_ENCLAVES_ABI,
        )
        info = contract.functions.enclaves(
            Web3.to_checksum_address(ENCLAVE_ADDRESS)
        ).call()
        assert info[0] is True  # registered
        assert info[1] is True  # active
        assert info[2] == IMAGE_HASH  # enclaveImageHash

    def test_register_duplicate_reverts(
        self, w3: "Web3", contract_address: str
    ) -> None:
        """Duplicate registration should revert with 'already registered'."""
        contract = _raw_contract(w3, contract_address)
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.registerEnclave(
                Web3.to_checksum_address(ENCLAVE_ADDRESS),
                IMAGE_HASH,
            ).call({"from": DEPLOYER_ADDRESS})

    def test_non_owner_cannot_register(
        self, w3: "Web3", contract_address: str
    ) -> None:
        """Non-owner should be denied registration."""
        contract = _raw_contract(w3, contract_address)
        random_addr = "0x1111111111111111111111111111111111111111"
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.registerEnclave(
                Web3.to_checksum_address(random_addr),
                IMAGE_HASH,
            ).call({"from": USER2_ADDRESS})

    def test_revoke_enclave(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        # Register a fresh enclave, then revoke it
        fresh = "0x2222222222222222222222222222222222222222"
        deployer_verifier.register_enclave(fresh, ZERO_BYTES32)

        tx_hash = deployer_verifier.revoke_enclave(fresh)
        assert tx_hash.startswith("0x")

        # Verify it is no longer active
        contract = w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=_ENCLAVES_ABI,
        )
        info = contract.functions.enclaves(
            Web3.to_checksum_address(fresh)
        ).call()
        assert info[0] is True  # still registered
        assert info[1] is False  # but no longer active


# ---------------------------------------------------------------------------
# Tests: config reads
# ---------------------------------------------------------------------------


class TestConfigReads:
    """Test reading configuration parameters via the SDK."""

    def test_prover_stake_default(self, deployer_verifier: "TEEVerifier") -> None:
        stake = deployer_verifier.prover_stake_amount()
        # Default is 0.1 ether = 10^17
        assert stake == 10**17

    def test_challenge_bond_default(self, deployer_verifier: "TEEVerifier") -> None:
        bond = deployer_verifier.challenge_bond_amount()
        # Default is 0.1 ether = 10^17
        assert bond == 10**17

    def test_prover_stake_readable_by_non_owner(
        self, user2_verifier: "TEEVerifier"
    ) -> None:
        # View functions should be callable by anyone
        stake = user2_verifier.prover_stake_amount()
        assert stake == 10**17

    def test_challenge_bond_readable_by_non_owner(
        self, user2_verifier: "TEEVerifier"
    ) -> None:
        bond = user2_verifier.challenge_bond_amount()
        assert bond == 10**17


# ---------------------------------------------------------------------------
# Tests: basic query functions
# ---------------------------------------------------------------------------


class TestQueryFunctions:
    """Test view/query functions on the contract."""

    def test_is_result_valid_for_nonexistent(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        valid = deployer_verifier.is_result_valid(ZERO_BYTES32)
        assert valid is False

    def test_dispute_resolved_for_nonexistent(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        resolved = deployer_verifier.dispute_resolved(ZERO_BYTES32)
        assert resolved is False

    def test_dispute_prover_won_for_nonexistent(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        won = deployer_verifier.dispute_prover_won(ZERO_BYTES32)
        assert won is False

    def test_get_result_for_nonexistent(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        result = deployer_verifier.get_result(ZERO_BYTES32)
        assert result.submitted_at == 0
        assert result.finalized is False
        assert result.challenged is False


# ---------------------------------------------------------------------------
# Tests: contract address and account
# ---------------------------------------------------------------------------


class TestAddressProperties:
    """Test SDK address/account properties."""

    def test_contract_address(
        self,
        deployer_verifier: "TEEVerifier",
        contract_address: str,
    ) -> None:
        assert deployer_verifier.address.lower() == contract_address.lower()

    def test_account_address(self, deployer_verifier: "TEEVerifier") -> None:
        assert deployer_verifier.account_address.lower() == DEPLOYER_ADDRESS.lower()

    def test_user2_account_address(self, user2_verifier: "TEEVerifier") -> None:
        assert user2_verifier.account_address.lower() == USER2_ADDRESS.lower()


# ---------------------------------------------------------------------------
# Tests: submit result (requires registered enclave + valid ECDSA signature)
# ---------------------------------------------------------------------------


class TestSubmitResult:
    """Test result submission flow.

    Uses Anvil account #2 as the enclave signer so we can produce a valid
    ECDSA attestation locally.

    NOTE: The SDK's ``submit_result`` returns the result ID parsed from the
    ``ResultSubmitted`` event.  The ABI now has correct indexed params so
    event decoding works reliably.
    """

    def test_submit_result_succeeds(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        model_hash = b"\x01" * 32
        input_hash = b"\x02" * 32
        result_data = b"\xde\xad\xbe\xef"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )

        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        assert returned.startswith("0x")
        assert len(returned) == 66  # "0x" + 64 hex chars

    def test_submitted_result_is_queryable(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
    ) -> None:
        model_hash = b"\x03" * 32
        input_hash = b"\x04" * 32
        result_data = b"\xca\xfe\xba\xbe"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )

        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )

        result_id = returned

        on_chain = deployer_verifier.get_result(result_id)
        assert on_chain.submitted_at > 0
        assert on_chain.model_hash == model_hash
        assert on_chain.input_hash == input_hash
        assert on_chain.result == result_data
        assert on_chain.finalized is False
        assert on_chain.challenged is False
        assert on_chain.enclave.lower() == ENCLAVE_ADDRESS.lower()
        assert on_chain.submitter.lower() == DEPLOYER_ADDRESS.lower()
        assert on_chain.prover_stake_amount == 10**17

    def test_submit_insufficient_stake_reverts(
        self, w3: "Web3", contract_address: str
    ) -> None:
        """Submitting with too little stake should revert.

        We use eth_call (via contract.functions.X().call()) which simulates
        the transaction and raises on revert.
        """
        model_hash = b"\x05" * 32
        input_hash = b"\x06" * 32
        result_data = b"\x00"
        attestation = _sign_attestation(
            model_hash, input_hash, result_data, contract_address,
        )

        contract = _raw_contract(w3, contract_address)
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.submitResult(
                model_hash,
                input_hash,
                result_data,
                attestation,
            ).call({"from": DEPLOYER_ADDRESS, "value": 1})

    def test_is_result_valid_before_finalize(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
    ) -> None:
        """A freshly submitted result should NOT be valid (not yet finalized)."""
        model_hash = b"\x07" * 32
        input_hash = b"\x08" * 32
        result_data = b"\xff"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )

        result_id = returned
        valid = deployer_verifier.is_result_valid(result_id)
        assert valid is False


# ---------------------------------------------------------------------------
# Tests: challenge flow
# ---------------------------------------------------------------------------


class TestChallengeFlow:
    """Test challenging a submitted result."""

    def test_challenge_submitted_result(
        self,
        deployer_verifier: "TEEVerifier",
        user2_verifier: "TEEVerifier",
        w3: "Web3",
    ) -> None:
        model_hash = b"\x10" * 32
        input_hash = b"\x11" * 32
        result_data = b"\xaa\xbb"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Challenge from user2
        tx_hash = user2_verifier.challenge(result_id, bond_wei=10**17)
        assert tx_hash.startswith("0x")

        # Verify on-chain state
        on_chain = deployer_verifier.get_result(result_id)
        assert on_chain.challenged is True
        assert on_chain.challenger.lower() == USER2_ADDRESS.lower()

    def test_cannot_challenge_after_window(
        self,
        deployer_verifier: "TEEVerifier",
        user2_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        model_hash = b"\x14" * 32
        input_hash = b"\x15" * 32
        result_data = b"\xee\xff"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Advance time past challenge window (1 hour + 1 second)
        w3.provider.make_request("evm_increaseTime", [3601])
        w3.provider.make_request("evm_mine", [])

        # Use raw contract .call() which raises on revert (SDK _send_tx swallows reverts)
        contract = _raw_contract(w3, contract_address)
        result_id_bytes = bytes.fromhex(result_id[2:])
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.challenge(result_id_bytes).call(
                {"from": USER2_ADDRESS, "value": 10**17}
            )


# ---------------------------------------------------------------------------
# Tests: finalize flow
# ---------------------------------------------------------------------------


class TestFinalizeFlow:
    """Test result finalization after challenge window."""

    def test_finalize_after_challenge_window(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
    ) -> None:
        model_hash = b"\x20" * 32
        input_hash = b"\x21" * 32
        result_data = b"\x01\x02\x03"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Advance time past challenge window (1 hour + 1 second)
        w3.provider.make_request("evm_increaseTime", [3601])
        w3.provider.make_request("evm_mine", [])

        tx_hash = deployer_verifier.finalize(result_id)
        assert tx_hash.startswith("0x")

        on_chain = deployer_verifier.get_result(result_id)
        assert on_chain.finalized is True

    def test_is_result_valid_after_finalize(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
    ) -> None:
        model_hash = b"\x22" * 32
        input_hash = b"\x23" * 32
        result_data = b"\x04\x05\x06"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Before finalize — not valid yet
        assert deployer_verifier.is_result_valid(result_id) is False

        # Advance time and finalize
        w3.provider.make_request("evm_increaseTime", [3601])
        w3.provider.make_request("evm_mine", [])
        deployer_verifier.finalize(result_id)

        # After finalize — should be valid
        assert deployer_verifier.is_result_valid(result_id) is True

    def test_cannot_finalize_before_window(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        model_hash = b"\x24" * 32
        input_hash = b"\x25" * 32
        result_data = b"\x07\x08\x09"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Use raw contract .call() which raises on revert (SDK _send_tx swallows reverts)
        contract = _raw_contract(w3, contract_address)
        result_id_bytes = bytes.fromhex(result_id[2:])
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.finalize(result_id_bytes).call(
                {"from": DEPLOYER_ADDRESS}
            )


# ---------------------------------------------------------------------------
# Tests: dispute timeout flow
# ---------------------------------------------------------------------------


class TestDisputeTimeout:
    """Test dispute resolution by timeout (challenger wins if no proof submitted)."""

    def test_resolve_dispute_by_timeout(
        self,
        deployer_verifier: "TEEVerifier",
        user2_verifier: "TEEVerifier",
        w3: "Web3",
    ) -> None:
        model_hash = b"\x30" * 32
        input_hash = b"\x31" * 32
        result_data = b"\xab\xcd"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Challenge
        user2_verifier.challenge(result_id, bond_wei=10**17)

        # Advance past dispute window (24 hours + 1 second)
        w3.provider.make_request("evm_increaseTime", [86401])
        w3.provider.make_request("evm_mine", [])

        # Resolve by timeout — challenger should win
        tx_hash = user2_verifier.resolve_dispute_by_timeout(result_id)
        assert tx_hash.startswith("0x")

        # Verify dispute state
        assert deployer_verifier.dispute_resolved(result_id) is True
        assert deployer_verifier.dispute_prover_won(result_id) is False

    def test_cannot_resolve_timeout_before_window(
        self,
        deployer_verifier: "TEEVerifier",
        user2_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        model_hash = b"\x32" * 32
        input_hash = b"\x33" * 32
        result_data = b"\xef\x01"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        # Challenge
        user2_verifier.challenge(result_id, bond_wei=10**17)

        # Use raw contract .call() which raises on revert (SDK _send_tx swallows reverts)
        contract = _raw_contract(w3, contract_address)
        result_id_bytes = bytes.fromhex(result_id[2:])
        with pytest.raises((ContractLogicError, Exception)):
            contract.functions.resolveDisputeByTimeout(result_id_bytes).call(
                {"from": USER2_ADDRESS}
            )


# ---------------------------------------------------------------------------
# Tests: event watcher integration (verifies event_watcher against live Anvil)
# ---------------------------------------------------------------------------


class TestEventWatcherIntegration:
    """Test TEEEventWatcher against live on-chain events."""

    def test_poll_catches_enclave_registered(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        from worldzk.event_watcher import TEEEventWatcher, TEEEventType

        watcher = TEEEventWatcher(
            contract_address=contract_address,
            rpc_url=ANVIL_RPC,
        )

        # Register a fresh enclave (a unique address to avoid conflicts)
        fresh = "0x4444444444444444444444444444444444444444"
        deployer_verifier.register_enclave(fresh, b"\xbb" * 32)

        events, next_block = watcher.poll_events(from_block=0)
        registered = [
            e for e in events
            if e.event_name == "EnclaveRegistered"
        ]
        assert len(registered) >= 1
        # At least one should match our fresh address
        addrs = [e.enclave_key.lower() for e in registered]
        assert fresh.lower() in addrs

    def test_poll_catches_result_submitted(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        from worldzk.event_watcher import TEEEventWatcher

        watcher = TEEEventWatcher(
            contract_address=contract_address,
            rpc_url=ANVIL_RPC,
        )

        block_before = w3.eth.block_number

        model_hash = b"\x40" * 32
        input_hash = b"\x41" * 32
        result_data = b"\xfa\xce"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )

        events, _ = watcher.poll_events(from_block=block_before)
        submitted = [
            e for e in events if e.event_name == "ResultSubmitted"
        ]
        assert len(submitted) >= 1
        assert submitted[-1].model_hash == model_hash
        assert submitted[-1].input_hash == input_hash
        assert submitted[-1].submitter.lower() == DEPLOYER_ADDRESS.lower()

    def test_poll_catches_result_challenged(
        self,
        deployer_verifier: "TEEVerifier",
        user2_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        from worldzk.event_watcher import TEEEventWatcher

        watcher = TEEEventWatcher(
            contract_address=contract_address,
            rpc_url=ANVIL_RPC,
        )

        model_hash = b"\x42" * 32
        input_hash = b"\x43" * 32
        result_data = b"\x12\x34"

        attestation = _sign_attestation(
            model_hash, input_hash, result_data, deployer_verifier.address,
        )
        returned = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )
        result_id = returned

        block_before = w3.eth.block_number
        user2_verifier.challenge(result_id, bond_wei=10**17)

        events, _ = watcher.poll_events(from_block=block_before)
        challenged = [
            e for e in events if e.event_name == "ResultChallenged"
        ]
        assert len(challenged) >= 1
        assert challenged[-1].challenger.lower() == USER2_ADDRESS.lower()

    def test_poll_with_event_type_filter(
        self,
        deployer_verifier: "TEEVerifier",
        w3: "Web3",
        contract_address: str,
    ) -> None:
        from worldzk.event_watcher import TEEEventWatcher

        watcher = TEEEventWatcher(
            contract_address=contract_address,
            rpc_url=ANVIL_RPC,
        )

        # Poll only for ResultFinalized events — should not include
        # EnclaveRegistered or ResultSubmitted
        events, _ = watcher.poll_events(
            from_block=0,
            event_types=["ResultFinalized"],
        )
        for e in events:
            assert e.event_name == "ResultFinalized"

    def test_poll_next_block_advances(
        self,
        w3: "Web3",
        contract_address: str,
    ) -> None:
        from worldzk.event_watcher import TEEEventWatcher

        watcher = TEEEventWatcher(
            contract_address=contract_address,
            rpc_url=ANVIL_RPC,
        )

        current = w3.eth.block_number
        _, next_block = watcher.poll_events(from_block=0)

        # next_block should be > 0 (past the genesis block)
        assert next_block > 0
        # next_block should be <= current + 1
        assert next_block <= current + 1
