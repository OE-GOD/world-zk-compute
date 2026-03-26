"""Tests for the TEE verifier module (no Anvil required)."""

import pytest

from worldzk.tee_verifier import (
    TEE_ML_VERIFIER_ABI,
    MLResult,
    TEEVerifier,
    _to_bytes,
    _to_bytes32,
)


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
