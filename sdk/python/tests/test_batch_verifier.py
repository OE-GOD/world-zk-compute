"""Tests for the batch_verifier module (no Anvil required)."""

import pytest
from unittest.mock import MagicMock, patch, PropertyMock

from worldzk.batch_verifier import (
    BatchSession,
    BatchVerifier,
    BatchVerificationCancelledError,
    ProgressEvent,
    ProgressTracker,
    StepResult,
    _ensure_hex,
    _to_bytes,
    LAYERS_PER_BATCH,
    GROUPS_PER_FINALIZE_BATCH,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

VALID_CONTRACT = "0x" + "ab" * 20
VALID_RPC = "http://localhost:8545"
VALID_KEY = "0x" + "cc" * 32


def _make_verifier(**kwargs):
    """Create a BatchVerifier with mocked web3 internals."""
    defaults = {
        "contract_address": VALID_CONTRACT,
        "rpc_url": VALID_RPC,
        "private_key": VALID_KEY,
    }
    defaults.update(kwargs)

    with patch("worldzk.batch_verifier.Web3") as MockWeb3:
        mock_w3 = MagicMock()
        MockWeb3.return_value = mock_w3
        MockWeb3.HTTPProvider.return_value = MagicMock()
        MockWeb3.to_checksum_address.side_effect = lambda addr: addr

        with patch("worldzk.batch_verifier.Account") as MockAccount:
            mock_account = MagicMock()
            mock_account.address = "0xSIGNER"
            MockAccount.from_key.return_value = mock_account

            verifier = BatchVerifier(**defaults)

    return verifier


# ---------------------------------------------------------------------------
# Construction
# ---------------------------------------------------------------------------


class TestConstruction:
    """Tests for BatchVerifier constructor."""

    def test_valid_params(self):
        """Construct with all required params succeeds."""
        verifier = _make_verifier()
        assert verifier.contract_address is not None
        assert verifier.account_address == "0xSIGNER"

    def test_missing_contract_address_raises(self):
        """Empty contract address raises ValueError."""
        with pytest.raises(ValueError, match="contract_address"):
            _make_verifier(contract_address="")

    def test_missing_rpc_url_raises(self):
        """Empty rpc_url raises ValueError."""
        with pytest.raises(ValueError, match="rpc_url"):
            _make_verifier(rpc_url="")

    def test_no_private_key_read_only(self):
        """Omitting private_key is allowed for read-only usage."""
        with patch("worldzk.batch_verifier.Web3") as MockWeb3:
            mock_w3 = MagicMock()
            MockWeb3.return_value = mock_w3
            MockWeb3.HTTPProvider.return_value = MagicMock()
            MockWeb3.to_checksum_address.side_effect = lambda addr: addr

            verifier = BatchVerifier(
                contract_address=VALID_CONTRACT,
                rpc_url=VALID_RPC,
            )
            assert verifier.account_address is None

    def test_custom_abi(self):
        """Custom ABI is accepted."""
        custom_abi = [
            {
                "type": "function",
                "name": "customFunc",
                "inputs": [],
                "outputs": [],
                "stateMutability": "view",
            }
        ]
        # Should not raise
        _make_verifier(abi=custom_abi)

    def test_custom_gas_limit(self):
        """Custom gas limit is stored."""
        verifier = _make_verifier(gas_limit=50_000_000)
        assert verifier._gas_limit == 50_000_000

    def test_default_gas_limit(self):
        """Default gas limit is 30M."""
        verifier = _make_verifier()
        assert verifier._gas_limit == 30_000_000


# ---------------------------------------------------------------------------
# ABI Loading
# ---------------------------------------------------------------------------


class TestABILoading:
    """Tests for ABI loading."""

    def test_default_abi_has_required_functions(self):
        """The built-in ABI has all required functions."""
        from worldzk.abi import DAG_BATCH_VERIFIER_ABI

        fn_names = {
            e["name"]
            for e in DAG_BATCH_VERIFIER_ABI
            if e["type"] == "function"
        }
        required = {
            "startDAGBatchVerify",
            "continueDAGBatchVerify",
            "finalizeDAGBatchVerify",
            "cleanupDAGBatchSession",
            "getDAGBatchSession",
            "isDAGCircuitActive",
        }
        assert required.issubset(fn_names)

    def test_default_abi_has_events(self):
        """The built-in ABI has expected events."""
        from worldzk.abi import DAG_BATCH_VERIFIER_ABI

        event_names = {
            e["name"]
            for e in DAG_BATCH_VERIFIER_ABI
            if e["type"] == "event"
        }
        assert "DAGBatchSessionStarted" in event_names


# ---------------------------------------------------------------------------
# Session Management
# ---------------------------------------------------------------------------


class TestSessionManagement:
    """Tests for get_session and session-related logic."""

    def test_get_session_parses_result(self):
        """get_session returns a BatchSession from contract call."""
        verifier = _make_verifier()

        circuit_hash_bytes = b"\xab" * 32
        verifier._contract.functions.getDAGBatchSession.return_value.call.return_value = (
            circuit_hash_bytes,  # circuitHash
            5,                   # nextBatchIdx
            11,                  # totalBatches
            False,               # finalized
            0,                   # finalizeInputIdx
            0,                   # finalizeGroupsDone
        )

        session = verifier.get_session("0x" + "ff" * 32)
        assert isinstance(session, BatchSession)
        assert session.circuit_hash == "0x" + "ab" * 32
        assert session.next_batch_idx == 5
        assert session.total_batches == 11
        assert session.finalized is False

    def test_get_session_finalized(self):
        """get_session correctly reports finalized state."""
        verifier = _make_verifier()

        verifier._contract.functions.getDAGBatchSession.return_value.call.return_value = (
            b"\x00" * 32,
            11,
            11,
            True,
            34,
            34,
        )

        session = verifier.get_session("0x" + "00" * 32)
        assert session.finalized is True
        assert session.finalize_groups_done == 34

    def test_get_session_accepts_bytes(self):
        """get_session accepts bytes as session_id."""
        verifier = _make_verifier()

        verifier._contract.functions.getDAGBatchSession.return_value.call.return_value = (
            b"\x00" * 32, 0, 11, False, 0, 0,
        )

        session = verifier.get_session(b"\xff" * 32)
        assert isinstance(session, BatchSession)


# ---------------------------------------------------------------------------
# Write operations require signer
# ---------------------------------------------------------------------------


class TestRequireSigner:
    """Tests that write operations require a private key."""

    def test_start_requires_signer(self):
        """start_verification raises without private_key."""
        with patch("worldzk.batch_verifier.Web3") as MockWeb3:
            mock_w3 = MagicMock()
            MockWeb3.return_value = mock_w3
            MockWeb3.HTTPProvider.return_value = MagicMock()
            MockWeb3.to_checksum_address.side_effect = lambda addr: addr

            verifier = BatchVerifier(
                contract_address=VALID_CONTRACT,
                rpc_url=VALID_RPC,
            )
            with pytest.raises(RuntimeError, match="private_key"):
                verifier.start_verification(
                    circuit_hash="0x" + "00" * 32,
                    proof_data="0xdead",
                    public_inputs="0xbeef",
                    gens_data="0x1234",
                )

    def test_cleanup_requires_signer(self):
        """cleanup_session raises without private_key."""
        with patch("worldzk.batch_verifier.Web3") as MockWeb3:
            mock_w3 = MagicMock()
            MockWeb3.return_value = mock_w3
            MockWeb3.HTTPProvider.return_value = MagicMock()
            MockWeb3.to_checksum_address.side_effect = lambda addr: addr

            verifier = BatchVerifier(
                contract_address=VALID_CONTRACT,
                rpc_url=VALID_RPC,
            )
            with pytest.raises(RuntimeError, match="private_key"):
                verifier.cleanup_session("0x" + "00" * 32)


# ---------------------------------------------------------------------------
# Cancel support
# ---------------------------------------------------------------------------


class TestCancel:
    """Tests for cancellation support."""

    def test_cancel_sets_flag(self):
        """cancel() sets the cancelled flag."""
        verifier = _make_verifier()
        assert verifier._cancelled is False
        verifier.cancel()
        assert verifier._cancelled is True

    def test_check_cancelled_raises(self):
        """_check_cancelled raises after cancel() is called."""
        verifier = _make_verifier()
        verifier.cancel()
        with pytest.raises(BatchVerificationCancelledError):
            verifier._check_cancelled()

    def test_reset_cancel(self):
        """_reset_cancel clears the cancelled flag."""
        verifier = _make_verifier()
        verifier.cancel()
        verifier._reset_cancel()
        assert verifier._cancelled is False


# ---------------------------------------------------------------------------
# Progress callback
# ---------------------------------------------------------------------------


class TestProgressCallback:
    """Tests for progress callback support."""

    def test_on_progress_registers_callback(self):
        """on_progress stores the callback."""
        verifier = _make_verifier()
        events = []
        verifier.on_progress(lambda e: events.append(e))
        assert verifier._progress_callback is not None

    def test_emit_progress_calls_callback(self):
        """_emit_progress invokes the registered callback."""
        verifier = _make_verifier()
        verifier._tracker = ProgressTracker()
        events = []
        verifier.on_progress(lambda e: events.append(e))
        verifier._emit_progress(3, 10, phase="continue", tx_hash="0xabc", gas_used=500000)
        assert len(events) == 1
        assert events[0].step == 3
        assert events[0].total == 10
        assert events[0].phase == "continue"
        assert events[0].tx_hash == "0xabc"
        assert events[0].gas_used == 500000

    def test_emit_progress_no_callback_is_noop(self):
        """_emit_progress does nothing without a registered callback."""
        verifier = _make_verifier()
        # Should not raise
        verifier._emit_progress(1, 5)


# ---------------------------------------------------------------------------
# Progress Tracker
# ---------------------------------------------------------------------------


class TestProgressTracker:
    """Tests for the ProgressTracker helper class."""

    def test_initial_state(self):
        """New tracker has zero completed steps."""
        tracker = ProgressTracker()
        assert tracker.completed_steps == 0
        assert tracker.average_step_ms is None

    def test_record_and_average(self):
        """Recording steps updates counts and averages."""
        t0 = 100.0
        tracker = ProgressTracker(start_time=t0)
        tracker.record_step(timestamp=100.5)  # 500ms
        tracker.record_step(timestamp=101.0)  # another 500ms
        assert tracker.completed_steps == 2
        avg = tracker.average_step_ms
        assert avg is not None
        assert abs(avg - 500.0) < 1.0  # ~500ms per step

    def test_estimate_remaining(self):
        """estimate_remaining_ms returns reasonable estimate."""
        t0 = 0.0
        tracker = ProgressTracker(start_time=t0)
        tracker.record_step(timestamp=1.0)  # 1 second = 1000ms
        estimate = tracker.estimate_remaining_ms(5)
        assert estimate == 5000

    def test_estimate_remaining_no_steps(self):
        """estimate_remaining_ms returns None with no completed steps."""
        tracker = ProgressTracker()
        assert tracker.estimate_remaining_ms(5) is None


# ---------------------------------------------------------------------------
# Fixture loading
# ---------------------------------------------------------------------------


class TestLoadFixture:
    """Tests for BatchVerifier.load_fixture()."""

    def test_load_phase1a_fixture(self):
        """Parse phase1a format (proof_hex + public_inputs_hex)."""
        fixture = {
            "proof_hex": "0xdeadbeef",
            "circuit_hash_raw": "0x" + "ab" * 32,
            "public_inputs_hex": "0xcafe",
            "gens_hex": "0x1234",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result["proof"] == "0xdeadbeef"
        assert result["circuit_hash"] == "0x" + "ab" * 32
        assert result["public_inputs"] == "0xcafe"
        assert result["gens_data"] == "0x1234"

    def test_load_groth16_fixture(self):
        """Parse groth16 format (inner_proof_hex + public_values_abi)."""
        fixture = {
            "inner_proof_hex": "0xgroth16proof",
            "circuit_hash_raw": "0x" + "cd" * 32,
            "public_values_abi": "0xpubvals",
            "gens_hex": "0x5678",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result["proof"] == "0xgroth16proof"
        assert result["public_inputs"] == "0xpubvals"

    def test_groth16_takes_precedence(self):
        """When both formats present, groth16 keys win."""
        fixture = {
            "proof_hex": "0xold",
            "inner_proof_hex": "0xnew",
            "circuit_hash_raw": "0x" + "00" * 32,
            "public_inputs_hex": "0xold_inputs",
            "public_values_abi": "0xnew_inputs",
            "gens_hex": "0xgens",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result["proof"] == "0xnew"
        assert result["public_inputs"] == "0xnew_inputs"

    def test_missing_proof_raises(self):
        fixture = {
            "circuit_hash_raw": "0x" + "00" * 32,
            "public_inputs_hex": "0xcafe",
            "gens_hex": "0x1234",
        }
        with pytest.raises(ValueError, match="proof_hex"):
            BatchVerifier.load_fixture(fixture)

    def test_missing_circuit_hash_raises(self):
        fixture = {
            "proof_hex": "0xdead",
            "public_inputs_hex": "0xcafe",
            "gens_hex": "0x1234",
        }
        with pytest.raises(ValueError, match="circuit_hash_raw"):
            BatchVerifier.load_fixture(fixture)

    def test_missing_gens_raises(self):
        fixture = {
            "proof_hex": "0xdead",
            "circuit_hash_raw": "0x" + "00" * 32,
            "public_inputs_hex": "0xcafe",
        }
        with pytest.raises(ValueError, match="gens_hex"):
            BatchVerifier.load_fixture(fixture)

    def test_adds_hex_prefix(self):
        fixture = {
            "proof_hex": "deadbeef",
            "circuit_hash_raw": "ab" * 32,
            "public_inputs_hex": "cafe",
            "gens_hex": "1234",
        }
        result = BatchVerifier.load_fixture(fixture)
        assert result["proof"] == "0xdeadbeef"
        assert result["circuit_hash"].startswith("0x")
        assert result["public_inputs"] == "0xcafe"
        assert result["gens_data"] == "0x1234"


# ---------------------------------------------------------------------------
# Transaction count estimation
# ---------------------------------------------------------------------------


class TestEstimateTransactionCount:
    """Tests for estimate_transaction_count()."""

    def test_xgboost_88_layers_34_groups(self):
        """88 layers / 8 = 11 continue, 34 groups / 16 = 3 finalize."""
        result = BatchVerifier.estimate_transaction_count(88, 34)
        assert result["start"] == 1
        assert result["continue"] == 11
        assert result["finalize"] == 3
        assert result["total"] == 15

    def test_exact_division(self):
        result = BatchVerifier.estimate_transaction_count(16, 16)
        assert result["continue"] == 2
        assert result["finalize"] == 1
        assert result["total"] == 4

    def test_single_batch(self):
        result = BatchVerifier.estimate_transaction_count(1, 1)
        assert result["total"] == 3

    def test_large_circuit(self):
        result = BatchVerifier.estimate_transaction_count(200, 100)
        assert result["continue"] == 25
        assert result["finalize"] == 7
        assert result["total"] == 33


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


class TestHelpers:
    """Tests for module-level helper functions."""

    def test_ensure_hex_adds_prefix(self):
        assert _ensure_hex("abc") == "0xabc"

    def test_ensure_hex_preserves_prefix(self):
        assert _ensure_hex("0xabc") == "0xabc"

    def test_ensure_hex_uppercase(self):
        assert _ensure_hex("0Xabc") == "0Xabc"

    def test_to_bytes_from_hex(self):
        assert _to_bytes("0xdeadbeef") == b"\xde\xad\xbe\xef"

    def test_to_bytes_no_prefix(self):
        assert _to_bytes("cafe") == b"\xca\xfe"

    def test_to_bytes_passthrough(self):
        val = b"\x01\x02\x03"
        assert _to_bytes(val) is val

    def test_to_bytes_invalid_type_raises(self):
        with pytest.raises(TypeError):
            _to_bytes(12345)  # type: ignore[arg-type]


# ---------------------------------------------------------------------------
# Dataclasses
# ---------------------------------------------------------------------------


class TestDataclasses:
    """Tests for dataclass fields."""

    def test_batch_session_fields(self):
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

    def test_step_result_fields(self):
        step = StepResult(
            tx_hash="0x" + "ff" * 32,
            gas_used=1_500_000,
            block_number=42,
        )
        assert step.gas_used == 1_500_000
        assert step.block_number == 42

    def test_progress_event_defaults(self):
        event = ProgressEvent(step=1, total=10, elapsed_ms=500)
        assert event.phase == ""
        assert event.tx_hash is None
        assert event.gas_used is None
        assert event.estimated_remaining_ms is None

    def test_progress_event_with_all_fields(self):
        event = ProgressEvent(
            step=3,
            total=10,
            elapsed_ms=1500,
            phase="continue",
            tx_hash="0xabc",
            gas_used=2_000_000,
            estimated_remaining_ms=3500,
        )
        assert event.step == 3
        assert event.phase == "continue"
        assert event.estimated_remaining_ms == 3500
