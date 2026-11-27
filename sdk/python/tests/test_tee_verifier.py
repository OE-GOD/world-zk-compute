"""Tests for the TEE verifier module (no Anvil required)."""

from unittest.mock import MagicMock

import pytest

from worldzk.tee_verifier import (
    TEE_ML_VERIFIER_ABI,
    MLResult,
    TEEVerifier,
    _to_bytes,
    _to_bytes32,
)

FAKE_CONTRACT = "0x" + "ab" * 20
FAKE_RPC_URL = "http://localhost:8545"
FAKE_PRIVATE_KEY = "0x" + "aa" * 32
FAKE_RESULT_ID = b"\xaa" * 32
FAKE_MODEL_HASH = b"\xbb" * 32
FAKE_INPUT_HASH = b"\xcc" * 32
FAKE_TX_HASH = b"\x11" * 32
FAKE_ENCLAVE_ADDR = "0x" + "ff" * 20
FAKE_ADMIN_ADDR = "0x" + "dd" * 20


def _make_verifier() -> TEEVerifier:
    """Create a TEEVerifier (construction does not touch the network)."""
    return TEEVerifier(
        rpc_url=FAKE_RPC_URL,
        private_key=FAKE_PRIVATE_KEY,
        contract_address=FAKE_CONTRACT,
    )


def _mock_send_tx(verifier: TEEVerifier, receipt: dict = None) -> None:
    """Replace the contract and _send_tx with mocks returning the given receipt."""
    if receipt is None:
        receipt = {"transactionHash": FAKE_TX_HASH}
    verifier._contract = MagicMock()
    verifier._send_tx = MagicMock(return_value=receipt)


class TestToBytes32:
    """Tests for _to_bytes32 helper."""

    def test_from_bytes(self):
        val = b"\x00" * 32
        assert _to_bytes32(val) == val

    def test_from_hex_string(self):
        hex_str = "0x" + "ab" * 32
        result = _to_bytes32(hex_str)
        assert len(result) == 32
        assert result == bytes.fromhex("ab" * 32)

    def test_from_hex_no_prefix(self):
        hex_str = "cd" * 32
        result = _to_bytes32(hex_str)
        assert len(result) == 32

    def test_wrong_length_raises(self):
        with pytest.raises(ValueError, match="Expected 32 bytes"):
            _to_bytes32(b"\x00" * 16)

    def test_short_hex_raises(self):
        with pytest.raises(ValueError, match="Expected 32 bytes"):
            _to_bytes32("0xdeadbeef")


class TestToBytes:
    """Tests for _to_bytes helper."""

    def test_from_bytes(self):
        val = b"\xde\xad\xbe\xef"
        assert _to_bytes(val) == val

    def test_from_hex_string(self):
        assert _to_bytes("0xdeadbeef") == b"\xde\xad\xbe\xef"

    def test_from_hex_no_prefix(self):
        assert _to_bytes("cafe") == b"\xca\xfe"

    def test_empty_hex(self):
        assert _to_bytes("0x") == b""


class TestMLResult:
    """Tests for MLResult dataclass."""

    def test_creation(self):
        result = MLResult(
            enclave="0x" + "11" * 20,
            submitter="0x" + "22" * 20,
            model_hash=b"\x00" * 32,
            input_hash=b"\x00" * 32,
            result_hash=b"\x00" * 32,
            result=b"\xde\xad",
            submitted_at=1000,
            challenge_deadline=2000,
            dispute_deadline=3000,
            challenge_bond=100,
            prover_stake_amount=200,
            finalized=False,
            challenged=True,
            challenger="0x" + "33" * 20,
        )
        assert result.submitted_at == 1000
        assert result.challenged is True
        assert result.finalized is False
        assert result.challenge_bond == 100


class TestABI:
    """Tests for the ABI definition."""

    def test_abi_has_all_functions(self):
        fn_names = {e["name"] for e in TEE_ML_VERIFIER_ABI if e["type"] == "function"}
        expected = {
            "registerEnclave",
            "revokeEnclave",
            "submitResult",
            "challenge",
            "finalize",
            "resolveDispute",
            "resolveDisputeByTimeout",
            "getResult",
            "isResultValid",
            "admin",
            "changeAdmin",
            "timelock",
            "setTimelock",
            "implementation",
            "pause",
            "unpause",
            "paused",
            "challengeBondAmount",
            "proverStake",
            "disputeResolved",
            "disputeProverWon",
        }
        assert fn_names == expected

    def test_submit_result_is_payable(self):
        submit = next(
            e for e in TEE_ML_VERIFIER_ABI
            if e.get("name") == "submitResult"
        )
        assert submit["stateMutability"] == "payable"

    def test_challenge_is_payable(self):
        challenge = next(
            e for e in TEE_ML_VERIFIER_ABI
            if e.get("name") == "challenge"
        )
        assert challenge["stateMutability"] == "payable"

    def test_get_result_returns_tuple(self):
        get_result = next(
            e for e in TEE_ML_VERIFIER_ABI
            if e.get("name") == "getResult"
        )
        assert get_result["outputs"][0]["type"] == "tuple"
        components = get_result["outputs"][0]["components"]
        assert len(components) == 14
        field_names = [c["name"] for c in components]
        assert "enclave" in field_names
        assert "challenger" in field_names

    def test_view_functions(self):
        views = [
            e for e in TEE_ML_VERIFIER_ABI
            if e.get("stateMutability") == "view"
        ]
        view_names = {e["name"] for e in views}
        assert "isResultValid" in view_names
        assert "admin" in view_names
        assert "timelock" in view_names
        assert "implementation" in view_names
        assert "paused" in view_names
        assert "getResult" in view_names

    def test_uups_admin_functions(self):
        fn_names = {e["name"] for e in TEE_ML_VERIFIER_ABI if e["type"] == "function"}
        assert "admin" in fn_names
        assert "changeAdmin" in fn_names
        assert "timelock" in fn_names
        assert "setTimelock" in fn_names
        assert "implementation" in fn_names

    def test_pausable_functions(self):
        fn_names = {e["name"] for e in TEE_ML_VERIFIER_ABI if e["type"] == "function"}
        assert "pause" in fn_names
        assert "unpause" in fn_names
        assert "paused" in fn_names

    def test_change_admin_has_address_input(self):
        fn = next(
            e for e in TEE_ML_VERIFIER_ABI
            if e.get("name") == "changeAdmin"
        )
        assert fn["inputs"][0]["type"] == "address"
        assert fn["stateMutability"] == "nonpayable"


class TestVerifierInit:
    """Tests for TEEVerifier construction."""

    def test_init(self):
        verifier = _make_verifier()
        assert verifier.address.lower() == FAKE_CONTRACT
        assert verifier.account_address.startswith("0x")
        assert len(verifier.account_address) == 42
        assert verifier._gas_limit == 500_000

    def test_init_custom_gas_limit(self):
        verifier = TEEVerifier(
            rpc_url=FAKE_RPC_URL,
            private_key=FAKE_PRIVATE_KEY,
            contract_address=FAKE_CONTRACT,
            gas_limit=1_000_000,
        )
        assert verifier._gas_limit == 1_000_000


class TestSubmitResult:
    """Tests for TEEVerifier.submit_result."""

    def test_returns_result_id_from_event(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.events.ResultSubmitted.return_value.process_receipt.return_value = [
            {"args": {"resultId": FAKE_RESULT_ID}}
        ]

        result_id = verifier.submit_result(
            model_hash="0x" + FAKE_MODEL_HASH.hex(),
            input_hash=FAKE_INPUT_HASH,
            result="0xdeadbeef",
            attestation=b"\x01" * 65,
            stake_wei=10**17,
        )

        assert result_id == "0x" + FAKE_RESULT_ID.hex()
        verifier._contract.functions.submitResult.assert_called_once_with(
            FAKE_MODEL_HASH, FAKE_INPUT_HASH, b"\xde\xad\xbe\xef", b"\x01" * 65
        )
        assert verifier._send_tx.call_args.kwargs == {"value": 10**17}

    def test_falls_back_to_tx_hash_without_event(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.events.ResultSubmitted.return_value.process_receipt.return_value = []

        result_id = verifier.submit_result(
            FAKE_MODEL_HASH, FAKE_INPUT_HASH, b"\x00", b"\x01" * 65
        )

        assert result_id == "0x" + FAKE_TX_HASH.hex()

    def test_rejects_short_model_hash(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        with pytest.raises(ValueError, match="Expected 32 bytes"):
            verifier.submit_result(b"\x00" * 4, FAKE_INPUT_HASH, b"\x00", b"\x01" * 65)
        verifier._send_tx.assert_not_called()


class TestTransactions:
    """Tests for the state-changing TEEVerifier methods."""

    def test_challenge_sends_bond(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)

        tx_hash = verifier.challenge(FAKE_RESULT_ID, bond_wei=10**17)

        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._contract.functions.challenge.assert_called_once_with(FAKE_RESULT_ID)
        assert verifier._send_tx.call_args.kwargs == {"value": 10**17}

    def test_finalize(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)

        assert verifier.finalize("0x" + FAKE_RESULT_ID.hex()) == "0x" + FAKE_TX_HASH.hex()
        verifier._contract.functions.finalize.assert_called_once_with(FAKE_RESULT_ID)

    def test_register_enclave_checksums_address(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)

        verifier.register_enclave(FAKE_ENCLAVE_ADDR, b"\x99" * 32)

        args = verifier._contract.functions.registerEnclave.call_args.args
        assert args[0].lower() == FAKE_ENCLAVE_ADDR
        assert args[0] != FAKE_ENCLAVE_ADDR  # converted to checksum case
        assert args[1] == b"\x99" * 32

    def test_change_admin(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)

        assert verifier.change_admin(FAKE_ADMIN_ADDR) == "0x" + FAKE_TX_HASH.hex()
        args = verifier._contract.functions.changeAdmin.call_args.args
        assert args[0].lower() == FAKE_ADMIN_ADDR


class TestReads:
    """Tests for the read-only TEEVerifier methods."""

    def test_get_result_maps_tuple(self):
        verifier = _make_verifier()
        verifier._contract = MagicMock()
        verifier._contract.functions.getResult.return_value.call.return_value = (
            FAKE_ENCLAVE_ADDR,
            FAKE_ADMIN_ADDR,
            FAKE_MODEL_HASH,
            FAKE_INPUT_HASH,
            b"\xee" * 32,
            b"\xde\xad",
            1000,
            4600,
            0,
            0,
            10**17,
            False,
            True,
            FAKE_CONTRACT,
        )

        result = verifier.get_result(FAKE_RESULT_ID)

        assert isinstance(result, MLResult)
        assert result.enclave == FAKE_ENCLAVE_ADDR
        assert result.submitter == FAKE_ADMIN_ADDR
        assert result.result == b"\xde\xad"
        assert result.submitted_at == 1000
        assert result.challenge_deadline == 4600
        assert result.prover_stake_amount == 10**17
        assert result.finalized is False
        assert result.challenged is True
        assert result.challenger == FAKE_CONTRACT
        verifier._contract.functions.getResult.assert_called_once_with(FAKE_RESULT_ID)

    def test_simple_reads(self):
        verifier = _make_verifier()
        verifier._contract = MagicMock()
        fns = verifier._contract.functions
        fns.isResultValid.return_value.call.return_value = True
        fns.admin.return_value.call.return_value = FAKE_ADMIN_ADDR
        fns.paused.return_value.call.return_value = False
        fns.proverStake.return_value.call.return_value = 10**17
        fns.challengeBondAmount.return_value.call.return_value = 2 * 10**17

        assert verifier.is_result_valid(FAKE_RESULT_ID) is True
        assert verifier.admin() == FAKE_ADMIN_ADDR
        assert verifier.paused() is False
        assert verifier.prover_stake_amount() == 10**17
        assert verifier.challenge_bond_amount() == 2 * 10**17


class TestSendTx:
    """Tests for TEEVerifier._send_tx (signing and revert handling)."""

    @staticmethod
    def _verifier_with_chain(status: int) -> TEEVerifier:
        verifier = _make_verifier()
        verifier._w3 = MagicMock()
        verifier._w3.eth.get_transaction_count.return_value = 7
        verifier._w3.eth.gas_price = 3
        verifier._w3.eth.estimate_gas.return_value = 100_000
        verifier._w3.eth.wait_for_transaction_receipt.return_value = {
            "transactionHash": FAKE_TX_HASH,
            "status": status,
        }
        verifier._account = MagicMock()
        verifier._account.address = FAKE_ADMIN_ADDR
        return verifier

    def test_builds_signs_and_sends(self):
        verifier = self._verifier_with_chain(status=1)
        fn = MagicMock()
        fn.build_transaction.side_effect = lambda params: dict(params)

        receipt = verifier._send_tx(fn, value=5)

        assert receipt["transactionHash"] == FAKE_TX_HASH
        signed_tx = verifier._account.sign_transaction.call_args.args[0]
        assert signed_tx["from"] == FAKE_ADMIN_ADDR
        assert signed_tx["nonce"] == 7
        assert signed_tx["gasPrice"] == 3
        assert signed_tx["value"] == 5
        assert signed_tx["gas"] == 110_000  # estimate plus 10%
        verifier._w3.eth.send_raw_transaction.assert_called_once_with(
            verifier._account.sign_transaction.return_value.raw_transaction
        )

    def test_uses_gas_limit_when_estimation_fails(self):
        verifier = self._verifier_with_chain(status=1)
        verifier._w3.eth.estimate_gas.side_effect = ValueError("execution reverted")
        fn = MagicMock()
        fn.build_transaction.side_effect = lambda params: dict(params)

        verifier._send_tx(fn)

        signed_tx = verifier._account.sign_transaction.call_args.args[0]
        assert signed_tx["gas"] == 500_000

    def test_reverted_transaction_raises(self):
        verifier = self._verifier_with_chain(status=0)
        fn = MagicMock()
        fn.build_transaction.side_effect = lambda params: dict(params)

        with pytest.raises(RuntimeError, match="Transaction reverted: 0x" + FAKE_TX_HASH.hex()):
            verifier._send_tx(fn)
