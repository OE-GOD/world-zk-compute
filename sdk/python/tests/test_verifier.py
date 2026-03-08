"""Tests for the batch verifier module (no Anvil required)."""

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
