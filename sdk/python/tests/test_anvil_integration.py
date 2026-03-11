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

import json
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

# Contracts directory (for forge create)
CONTRACTS_DIR = os.path.normpath(
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "contracts")
)

# Dummy 32-byte values for tests
ZERO_BYTES32 = b"\x00" * 32
IMAGE_HASH = b"\xaa" * 32


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
            "src/tee/TEEMLVerifier.sol:TEEMLVerifier",
            "--rpc-url",
            ANVIL_RPC,
            "--private-key",
            DEPLOYER_KEY,
            # Constructor args: admin, remainderVerifier
            "--constructor-args",
            DEPLOYER_ADDRESS,
            "0x0000000000000000000000000000000000000000",
            "--json",
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

    # Parse the JSON output to extract the deployed address
    try:
        data = json.loads(result.stdout)
        addr = data["deployedTo"]
    except (json.JSONDecodeError, KeyError):
        # Fallback: try to parse from non-JSON output
        for line in result.stdout.splitlines():
            if "Deployed to:" in line or "deployedTo" in line:
                addr = line.split()[-1].strip()
                break
        else:
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

    def test_non_owner_cannot_pause(self, user2_verifier: "TEEVerifier") -> None:
        with pytest.raises(Exception):
            user2_verifier.pause()


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
        from worldzk.tee_verifier import TEE_ML_VERIFIER_ABI

        contract = w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=[
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
            ],
        )
        info = contract.functions.enclaves(
            Web3.to_checksum_address(ENCLAVE_ADDRESS)
        ).call()
        assert info[0] is True  # registered
        assert info[1] is True  # active
        assert info[2] == IMAGE_HASH  # enclaveImageHash

    def test_register_duplicate_reverts(
        self, deployer_verifier: "TEEVerifier"
    ) -> None:
        # ENCLAVE_ADDRESS was already registered in the test above
        with pytest.raises(Exception, match="already registered"):
            deployer_verifier.register_enclave(ENCLAVE_ADDRESS, IMAGE_HASH)

    def test_non_owner_cannot_register(
        self, user2_verifier: "TEEVerifier"
    ) -> None:
        random_addr = "0x1111111111111111111111111111111111111111"
        with pytest.raises(Exception):
            user2_verifier.register_enclave(random_addr, IMAGE_HASH)

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
            abi=[
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
            ],
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
        # Querying a non-existent result should return False, not revert
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
        # Non-existent result has submittedAt == 0
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
        assert (
            deployer_verifier.address.lower() == contract_address.lower()
        )

    def test_account_address(self, deployer_verifier: "TEEVerifier") -> None:
        assert (
            deployer_verifier.account_address.lower()
            == DEPLOYER_ADDRESS.lower()
        )

    def test_user2_account_address(self, user2_verifier: "TEEVerifier") -> None:
        assert (
            user2_verifier.account_address.lower() == USER2_ADDRESS.lower()
        )


# ---------------------------------------------------------------------------
# Tests: submit result (requires registered enclave + valid ECDSA signature)
# ---------------------------------------------------------------------------


class TestSubmitResult:
    """Test result submission flow.

    This verifies the SDK can build and send a payable transaction with the
    correct stake amount.  We use Anvil account #2 as the enclave signer so
    we can produce a valid ECDSA attestation locally.
    """

    @staticmethod
    def _sign_attestation(
        model_hash: bytes, input_hash: bytes, result_data: bytes
    ) -> bytes:
        """Produce an eth_sign-style attestation matching the contract logic."""
        from eth_account import Account
        from eth_account.messages import encode_defunct

        result_hash = Web3.keccak(result_data)
        message_bytes = Web3.keccak(
            model_hash + input_hash + result_hash
        )

        # ENCLAVE_ADDRESS private key (Anvil account #2)
        enclave_key = (
            "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a"
        )
        signed = Account.sign_message(
            encode_defunct(primitive=message_bytes),
            private_key=enclave_key,
        )
        return signed.signature

    def test_submit_result_returns_result_id(
        self,
        deployer_verifier: "TEEVerifier",
    ) -> None:
        model_hash = b"\x01" * 32
        input_hash = b"\x02" * 32
        result_data = b"\xde\xad\xbe\xef"

        attestation = self._sign_attestation(model_hash, input_hash, result_data)

        result_id = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,  # 0.1 ETH minimum stake
        )
        assert result_id.startswith("0x")
        assert len(result_id) == 66  # "0x" + 64 hex chars

    def test_submitted_result_is_queryable(
        self,
        deployer_verifier: "TEEVerifier",
    ) -> None:
        model_hash = b"\x03" * 32
        input_hash = b"\x04" * 32
        result_data = b"\xca\xfe\xba\xbe"

        attestation = self._sign_attestation(model_hash, input_hash, result_data)

        result_id = deployer_verifier.submit_result(
            model_hash=model_hash,
            input_hash=input_hash,
            result=result_data,
            attestation=attestation,
            stake_wei=10**17,
        )

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
        self,
        deployer_verifier: "TEEVerifier",
    ) -> None:
        model_hash = b"\x05" * 32
        input_hash = b"\x06" * 32
        result_data = b"\x00"

        attestation = self._sign_attestation(model_hash, input_hash, result_data)

        with pytest.raises(Exception, match="insufficient stake"):
            deployer_verifier.submit_result(
                model_hash=model_hash,
                input_hash=input_hash,
                result=result_data,
                attestation=attestation,
                stake_wei=1,  # way too little
            )
