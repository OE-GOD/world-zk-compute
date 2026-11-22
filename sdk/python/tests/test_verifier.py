"""Tests for the batch verifier module (no Anvil required)."""

from unittest.mock import MagicMock, patch

import pytest

from worldzk.verifier import (
    BatchSession,
    BatchVerifier,
    BatchVerifyInput,
    StepResult,
    _ensure_hex,
)


class TestLoadFixture:
    """Tests for BatchVerifier.load_fixture()."""

    def test_load_phase1a_fixture(self):
        """Parse phase1a_dag_fixture format (proof_hex + public_inputs_hex)."""
        fixture = {
            "proof_hex": "0xdeadbeef",
            "circuit_hash_raw": "0x" + "ab" * 32,
            "public_inputs_hex": "0xcafe",
            "gens_hex": "0x1234",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert isinstance(result, BatchVerifyInput)
        assert result.proof == "0xdeadbeef"
        assert result.circuit_hash == "0x" + "ab" * 32
        assert result.public_inputs == "0xcafe"
        assert result.gens_data == "0x1234"

    def test_load_groth16_fixture(self):
        """Parse dag_groth16_e2e_fixture format (inner_proof_hex + public_values_abi)."""
        fixture = {
            "inner_proof_hex": "0xgroth16proof",
            "circuit_hash_raw": "0x" + "cd" * 32,
            "public_values_abi": "0xpubvals",
            "gens_hex": "0x5678",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result.proof == "0xgroth16proof"
        assert result.public_inputs == "0xpubvals"

    def test_groth16_keys_take_precedence(self):
        """When both formats are present, groth16 keys win."""
        fixture = {
            "proof_hex": "0xold_proof",
            "inner_proof_hex": "0xnew_proof",
            "circuit_hash_raw": "0x" + "00" * 32,
            "public_inputs_hex": "0xold_inputs",
            "public_values_abi": "0xnew_inputs",
            "gens_hex": "0xgens",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result.proof == "0xnew_proof"
        assert result.public_inputs == "0xnew_inputs"

    def test_missing_proof_raises(self):
        """Error when neither proof key is present."""
        fixture = {
            "circuit_hash_raw": "0x" + "00" * 32,
            "public_inputs_hex": "0xcafe",
            "gens_hex": "0x1234",
        }
        with pytest.raises(ValueError, match="proof_hex"):
            BatchVerifier.load_fixture(fixture)

    def test_missing_public_inputs_raises(self):
        """Error when neither public inputs key is present."""
        fixture = {
            "proof_hex": "0xdeadbeef",
            "circuit_hash_raw": "0x" + "00" * 32,
            "gens_hex": "0x1234",
        }
        with pytest.raises(ValueError, match="public_inputs_hex"):
            BatchVerifier.load_fixture(fixture)

    def test_missing_circuit_hash_raises(self):
        """Error when circuit_hash_raw is missing."""
        fixture = {
            "proof_hex": "0xdeadbeef",
            "public_inputs_hex": "0xcafe",
            "gens_hex": "0x1234",
        }
        with pytest.raises(ValueError, match="circuit_hash_raw"):
            BatchVerifier.load_fixture(fixture)

    def test_missing_gens_raises(self):
        """Error when gens_hex is missing."""
        fixture = {
            "proof_hex": "0xdeadbeef",
            "circuit_hash_raw": "0x" + "00" * 32,
            "public_inputs_hex": "0xcafe",
        }
        with pytest.raises(ValueError, match="gens_hex"):
            BatchVerifier.load_fixture(fixture)

    def test_adds_hex_prefix(self):
        """Values without 0x prefix get one added."""
        fixture = {
            "proof_hex": "deadbeef",
            "circuit_hash_raw": "ab" * 32,
            "public_inputs_hex": "cafe",
            "gens_hex": "1234",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result.proof == "0xdeadbeef"
        assert result.circuit_hash == "0x" + "ab" * 32
        assert result.public_inputs == "0xcafe"
        assert result.gens_data == "0x1234"


class TestBatchSession:
    """Tests for BatchSession dataclass."""

    def test_field_access(self):
        session = BatchSession(
            circuit_hash="0x" + "ab" * 32,
            next_batch_idx=5,
            total_batches=11,
            finalized=False,
            finalize_input_idx=0,
            finalize_groups_done=0,
        )
        assert session.circuit_hash == "0x" + "ab" * 32
        assert session.next_batch_idx == 5
        assert session.total_batches == 11
        assert session.finalized is False
        assert session.finalize_input_idx == 0
        assert session.finalize_groups_done == 0

    def test_finalized_session(self):
        session = BatchSession(
            circuit_hash="0x" + "00" * 32,
            next_batch_idx=11,
            total_batches=11,
            finalized=True,
            finalize_input_idx=34,
            finalize_groups_done=34,
        )
        assert session.finalized is True
        assert session.finalize_groups_done == 34


class TestEstimateTransactionCount:
    """Tests for BatchVerifier.estimate_transaction_count()."""

    def test_xgboost_88_layers_34_groups(self):
        """88 compute layers / 8 = 11 continue, 34 groups / 16 = 3 finalize."""
        result = BatchVerifier.estimate_transaction_count(88, 34)
        assert result["start"] == 1
        assert result["continue"] == 11
        assert result["finalize"] == 3
        assert result["total"] == 15

    def test_exact_division(self):
        """When layers/groups divide evenly."""
        result = BatchVerifier.estimate_transaction_count(16, 16)
        assert result["continue"] == 2
        assert result["finalize"] == 1
        assert result["total"] == 4

    def test_single_batch(self):
        """Minimum case: 1 layer, 1 group."""
        result = BatchVerifier.estimate_transaction_count(1, 1)
        assert result["start"] == 1
        assert result["continue"] == 1
        assert result["finalize"] == 1
        assert result["total"] == 3

    def test_large_circuit(self):
        """Large circuit with many layers and groups."""
        result = BatchVerifier.estimate_transaction_count(200, 100)
        assert result["continue"] == 25  # ceil(200/8)
        assert result["finalize"] == 7   # ceil(100/16)
        assert result["total"] == 33

    def test_just_over_boundary(self):
        """One extra layer/group pushes to next batch."""
        result = BatchVerifier.estimate_transaction_count(9, 17)
        assert result["continue"] == 2   # ceil(9/8)
        assert result["finalize"] == 2   # ceil(17/16)
        assert result["total"] == 5


class TestEnsureHexPrefix:
    """Tests for the _ensure_hex helper."""

    def test_adds_prefix(self):
        assert _ensure_hex("abc") == "0xabc"

    def test_preserves_prefix(self):
        assert _ensure_hex("0xabc") == "0xabc"

    def test_preserves_uppercase_prefix(self):
        assert _ensure_hex("0Xabc") == "0Xabc"

    def test_empty_string(self):
        assert _ensure_hex("") == "0x"

    def test_already_prefixed_long(self):
        val = "0x" + "ab" * 32
        assert _ensure_hex(val) == val


class TestStepResult:
    """Tests for StepResult dataclass."""

    def test_field_access(self):
        step = StepResult(
            tx_hash="0x" + "ff" * 32,
            gas_used=1_500_000,
            block_number=42,
        )
        assert step.tx_hash == "0x" + "ff" * 32
        assert step.gas_used == 1_500_000
        assert step.block_number == 42


def _make_verifier(**kwargs):
    """Create a BatchVerifier with mocked web3 internals."""
    with patch("worldzk.verifier.Web3") as mock_web3_cls:
        mock_web3_cls.return_value = MagicMock()
        mock_web3_cls.to_checksum_address.side_effect = lambda addr: addr

        with patch("worldzk.verifier.Account") as mock_account_cls:
            mock_account = MagicMock()
            mock_account.address = "0xSIGNER"
            mock_account_cls.from_key.return_value = mock_account

            return BatchVerifier(
                rpc_url="http://localhost:8545",
                private_key="0x" + "cc" * 32,
                contract_address="0x" + "ab" * 20,
                **kwargs,
            )


class TestSendTransaction:
    """Tests for gas estimation and revert handling in _send_transaction."""

    @staticmethod
    def _prepare(verifier, status=1):
        verifier._w3.eth.wait_for_transaction_receipt.return_value = {
            "status": status,
            "transactionHash": bytes.fromhex("ab" * 32),
            "gasUsed": 1,
            "blockNumber": 1,
        }
        return MagicMock()

    def test_estimate_is_not_capped_by_gas_limit(self):
        """A step needing more than the configured limit still gets its estimate."""
        verifier = _make_verifier(gas_limit=30_000_000)
        fn = self._prepare(verifier)
        fn.estimate_gas.return_value = 32_000_000

        verifier._send_transaction(fn)

        # The estimate call carries no "gas" field, so the node does not cap it.
        fn.estimate_gas.assert_called_once_with({"from": "0xSIGNER"})
        assert fn.build_transaction.call_args[0][0]["gas"] == int(32_000_000 * 1.1)

    def test_falls_back_to_gas_limit_when_estimation_fails(self):
        """The configured limit is used only when estimation fails."""
        verifier = _make_verifier(gas_limit=30_000_000)
        fn = self._prepare(verifier)
        fn.estimate_gas.side_effect = ValueError("execution reverted")

        verifier._send_transaction(fn)

        assert fn.build_transaction.call_args[0][0]["gas"] == 30_000_000

    def test_reverted_receipt_raises(self):
        """A mined-but-reverted transaction is an error, not a completed step."""
        verifier = _make_verifier()
        fn = self._prepare(verifier, status=0)
        fn.estimate_gas.return_value = 100_000

        with pytest.raises(RuntimeError, match="Transaction reverted: 0x" + "ab" * 32):
            verifier._send_transaction(fn)
