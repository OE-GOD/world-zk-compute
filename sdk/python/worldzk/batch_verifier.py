"""
Low-level batch verification client for DAG GKR proofs.

Provides individual step methods (start, continue, finalize, cleanup) for
fine-grained control over multi-transaction batch verification. For a
higher-level "run everything" approach, see :class:`worldzk.verifier.BatchVerifier`.

Requires the ``web3`` optional dependency: ``pip install worldzk[web3]``

Example usage::

    from worldzk.batch_verifier import BatchVerifier

    verifier = BatchVerifier(
        contract_address="0x...",
        rpc_url="http://localhost:8545",
        private_key="0x...",
    )

    session_id = verifier.start_verification(
        circuit_hash="0x...",
        proof_data=b"...",
        public_inputs=b"...",
        gens_data=b"...",
    )

    while verifier.continue_verification(session_id, proof_data, public_inputs, gens_data):
        pass  # keep sending continue txs

    while not verifier.finalize_verification(session_id, proof_data, public_inputs, gens_data):
        pass  # keep sending finalize txs

    verifier.cleanup_session(session_id)
"""

from __future__ import annotations

import logging
import math
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Optional

logger = logging.getLogger(__name__)

from eth_account import Account
from eth_account.signers.local import LocalAccount
from web3 import Web3
from web3.contract import Contract

from .abi import DAG_BATCH_VERIFIER_ABI

LAYERS_PER_BATCH = 8
GROUPS_PER_FINALIZE_BATCH = 16
DEFAULT_GAS_LIMIT = 30_000_000
DEFAULT_MAX_RETRIES = 3
DEFAULT_RETRY_DELAY_S = 2.0


def _ensure_hex(value: str) -> str:
    """Ensure a hex string has 0x prefix."""
    if value.startswith("0x") or value.startswith("0X"):
        return value
    return "0x" + value


def _to_bytes(value: bytes | str) -> bytes:
    """Convert hex string or bytes to bytes."""
    if isinstance(value, bytes):
        return value
    if isinstance(value, str):
        clean = value[2:] if value.startswith(("0x", "0X")) else value
        return bytes.fromhex(clean)
    raise TypeError(f"Expected bytes or str, got {type(value)}")


@dataclass
class BatchSession:
    """On-chain batch session state."""

    circuit_hash: str
    next_batch_idx: int
    total_batches: int
    finalized: bool
    finalize_input_idx: int
    finalize_groups_done: int


@dataclass
class StepResult:
    """Result of a single transaction step."""

    tx_hash: str
    gas_used: int
    block_number: int


@dataclass
class ProgressEvent:
    """Progress callback event."""

    step: int
    total: int
    elapsed_ms: int
    phase: str = ""
    tx_hash: str | None = None
    gas_used: int | None = None
    estimated_remaining_ms: int | None = None


ProgressCallback = Callable[[ProgressEvent], None]


class ProgressTracker:
    """Tracks timing for completed steps to estimate time remaining."""

    def __init__(self, start_time: float | None = None) -> None:
        self._start_time = start_time if start_time is not None else time.monotonic()
        self._step_times: list[float] = []

    def record_step(self, timestamp: float | None = None) -> None:
        """Record that a step completed at the given timestamp (or now)."""
        self._step_times.append(timestamp if timestamp is not None else time.monotonic())

    @property
    def completed_steps(self) -> int:
        """Number of completed steps."""
        return len(self._step_times)

    @property
    def average_step_ms(self) -> float | None:
        """Average milliseconds per step, or None if no steps completed."""
        if not self._step_times:
            return None
        elapsed = self._step_times[-1] - self._start_time
        return (elapsed * 1000) / len(self._step_times)

    @property
    def elapsed_ms(self) -> int:
        """Elapsed milliseconds since start."""
        return int((time.monotonic() - self._start_time) * 1000)

    def estimate_remaining_ms(self, remaining_steps: int) -> int | None:
        """Estimate time remaining in milliseconds."""
        avg = self.average_step_ms
        if avg is None or remaining_steps < 0:
            return None
        return round(avg * remaining_steps)

    @property
    def start_time(self) -> float:
        """The start time."""
        return self._start_time


class BatchVerificationCancelledError(Exception):
    """Raised when batch verification is cancelled via cancel()."""

    def __init__(self) -> None:
        super().__init__("Batch verification was cancelled")


class BatchVerifier:
    """Low-level multi-transaction batch verifier for DAG GKR proofs.

    Provides individual step methods for fine-grained control:

    - :meth:`start_verification` -- submit the start transaction
    - :meth:`continue_verification` -- submit one continue transaction
    - :meth:`finalize_verification` -- submit one finalize transaction
    - :meth:`get_session` -- read on-chain session state
    - :meth:`cleanup_session` -- delete session storage for gas refund

    Also supports running all steps automatically via :meth:`verify_batch`.
    """

    def __init__(
        self,
        contract_address: str,
        rpc_url: str,
        private_key: str | None = None,
        abi: list[dict[str, Any]] | None = None,
        gas_limit: int = DEFAULT_GAS_LIMIT,
        max_retries: int = DEFAULT_MAX_RETRIES,
        retry_delay_s: float = DEFAULT_RETRY_DELAY_S,
    ) -> None:
        """Create a new batch verifier.

        Args:
            contract_address: The deployed DAGBatchVerifier contract address.
            rpc_url: JSON-RPC endpoint URL.
            private_key: Hex-encoded private key for signing transactions.
                Required for write operations (start, continue, finalize, cleanup).
                Can be omitted for read-only usage (get_session, is_circuit_active).
            abi: Custom ABI to use. Defaults to the built-in DAG_BATCH_VERIFIER_ABI.
            gas_limit: Maximum gas per transaction (default 30M).
            max_retries: Number of retries for transient failures (default 3).
            retry_delay_s: Base delay between retries in seconds (default 2.0).
        """
        if not contract_address:
            raise ValueError("contract_address is required")
        if not rpc_url:
            raise ValueError("rpc_url is required")

        self._w3 = Web3(Web3.HTTPProvider(rpc_url))
        self._private_key = private_key
        self._account: LocalAccount | None = None
        if private_key:
            self._account = Account.from_key(private_key)

        resolved_abi = abi if abi is not None else DAG_BATCH_VERIFIER_ABI
        self._contract = self._w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=resolved_abi,
        )
        self._gas_limit = gas_limit
        self._max_retries = max_retries
        self._retry_delay_s = retry_delay_s
        self._cancelled = False
        self._progress_callback: ProgressCallback | None = None
        self._tracker: ProgressTracker | None = None

    @property
    def contract_address(self) -> str:
        """The contract address."""
        return self._contract.address

    @property
    def account_address(self) -> str | None:
        """The signer's address, or None if no private key was provided."""
        if self._account is None:
            return None
        return self._account.address

    # ---- Progress + Cancel ----

    def on_progress(self, callback: ProgressCallback) -> None:
        """Register a progress callback.

        The callback receives a :class:`ProgressEvent` after each completed step.

        Args:
            callback: A callable ``(event: ProgressEvent) -> None``.
        """
        self._progress_callback = callback

    def cancel(self) -> None:
        """Cancel an in-progress batch verification.

        No further transactions will be submitted after the current one completes.
        The caller can check the session state via :meth:`get_session` and resume
        later with :meth:`continue_verification` / :meth:`finalize_verification`.
        """
        self._cancelled = True

    def _reset_cancel(self) -> None:
        """Reset the cancelled flag for a new verification run."""
        self._cancelled = False

    def _check_cancelled(self) -> None:
        """Raise if cancelled."""
        if self._cancelled:
            raise BatchVerificationCancelledError()

    def _emit_progress(
        self,
        step: int,
        total: int,
        phase: str = "",
        tx_hash: str | None = None,
        gas_used: int | None = None,
    ) -> None:
        """Emit a progress event if a callback is registered."""
        if self._progress_callback is None:
            return
        tracker = self._tracker
        remaining = total - step if total > 0 else None
        estimated = None
        if tracker and remaining is not None and remaining >= 0:
            estimated = tracker.estimate_remaining_ms(remaining)
        self._progress_callback(ProgressEvent(
            step=step,
            total=total,
            elapsed_ms=tracker.elapsed_ms if tracker else 0,
            phase=phase,
            tx_hash=tx_hash,
            gas_used=gas_used,
            estimated_remaining_ms=estimated,
        ))

    # ---- Read-only methods ----

    def is_circuit_active(self, circuit_hash: str | bytes) -> bool:
        """Check if a circuit is registered and active on-chain.

        Args:
            circuit_hash: The circuit hash (hex string or bytes32).

        Returns:
            True if the circuit is registered and active.
        """
        return self._contract.functions.isDAGCircuitActive(
            _to_bytes(circuit_hash)
        ).call()

    def get_session(self, session_id: str | bytes) -> BatchSession:
        """Read on-chain session state.

        Args:
            session_id: The session ID (hex string or bytes32).

        Returns:
            A :class:`BatchSession` with the current state.
        """
        sid = _to_bytes(session_id)
        result = self._contract.functions.getDAGBatchSession(sid).call()
        circuit_hash_val = result[0]
        if isinstance(circuit_hash_val, bytes):
            circuit_hash_val = "0x" + circuit_hash_val.hex()
        return BatchSession(
            circuit_hash=circuit_hash_val,
            next_batch_idx=result[1],
            total_batches=result[2],
            finalized=result[3],
            finalize_input_idx=result[4],
            finalize_groups_done=result[5],
        )

    # ---- Write methods (individual steps) ----

    def start_verification(
        self,
        circuit_hash: str | bytes,
        proof_data: str | bytes,
        public_inputs: str | bytes,
        gens_data: str | bytes,
    ) -> str:
        """Submit the start transaction for a new batch verification session.

        Args:
            circuit_hash: The circuit hash (hex string or bytes32).
            proof_data: The proof bytes (hex string or bytes).
            public_inputs: ABI-encoded public inputs (hex string or bytes).
            gens_data: Generator data (hex string or bytes).

        Returns:
            The session ID as a hex string (``0x...``).

        Raises:
            RuntimeError: If no private key was provided.
            RuntimeError: If no DAGBatchSessionStarted event is found.
        """
        self._require_signer()
        receipt = self._send_transaction(
            self._contract.functions.startDAGBatchVerify(
                _to_bytes(proof_data),
                _to_bytes(circuit_hash),
                _to_bytes(public_inputs),
                _to_bytes(gens_data),
            )
        )
        return self._extract_session_id(receipt)

    def continue_verification(
        self,
        session_id: str | bytes,
        proof_data: str | bytes,
        public_inputs: str | bytes,
        gens_data: str | bytes,
    ) -> bool:
        """Submit one continue transaction.

        Args:
            session_id: The session ID from :meth:`start_verification`.
            proof_data: The proof bytes.
            public_inputs: ABI-encoded public inputs.
            gens_data: Generator data.

        Returns:
            True if more continue transactions are needed, False if all
            compute batches are done (caller should move to finalize).

        Raises:
            RuntimeError: If no private key was provided.
        """
        self._require_signer()
        self._send_transaction(
            self._contract.functions.continueDAGBatchVerify(
                _to_bytes(session_id),
                _to_bytes(proof_data),
                _to_bytes(public_inputs),
                _to_bytes(gens_data),
            )
        )
        session = self.get_session(session_id)
        return session.next_batch_idx < session.total_batches

    def finalize_verification(
        self,
        session_id: str | bytes,
        proof_data: str | bytes,
        public_inputs: str | bytes,
        gens_data: str | bytes,
    ) -> bool:
        """Submit one finalize transaction.

        Args:
            session_id: The session ID.
            proof_data: The proof bytes.
            public_inputs: ABI-encoded public inputs.
            gens_data: Generator data.

        Returns:
            True if verification is complete (finalized), False if more
            finalize transactions are needed.

        Raises:
            RuntimeError: If no private key was provided.
        """
        self._require_signer()
        self._send_transaction(
            self._contract.functions.finalizeDAGBatchVerify(
                _to_bytes(session_id),
                _to_bytes(proof_data),
                _to_bytes(public_inputs),
                _to_bytes(gens_data),
            )
        )
        session = self.get_session(session_id)
        return session.finalized

    def cleanup_session(self, session_id: str | bytes) -> str:
        """Delete session storage for gas refund.

        Args:
            session_id: The session ID.

        Returns:
            The transaction hash as a hex string.

        Raises:
            RuntimeError: If no private key was provided.
        """
        self._require_signer()
        receipt = self._send_transaction(
            self._contract.functions.cleanupDAGBatchSession(
                _to_bytes(session_id),
            )
        )
        return self._tx_hash_hex(receipt)

    # ---- High-level "run everything" methods ----

    def verify_batch(
        self,
        circuit_hash: str | bytes,
        proof_data: str | bytes,
        public_inputs: str | bytes,
        gens_data: str | bytes,
        skip_cleanup: bool = False,
    ) -> dict[str, Any]:
        """Run the full multi-tx batch verification.

        Executes start -> continue * N -> finalize * M -> cleanup.
        Supports cancellation via :meth:`cancel` and progress reporting
        via :meth:`on_progress`.

        Args:
            circuit_hash: The circuit hash.
            proof_data: The proof bytes.
            public_inputs: ABI-encoded public inputs.
            gens_data: Generator data.
            skip_cleanup: If True, skip the cleanup transaction.

        Returns:
            A dict with keys: ``session_id``, ``start_step``, ``continue_steps``,
            ``finalize_steps``, ``cleanup_step``, ``total_gas_used``,
            ``duration_ms``, ``cancelled``.
        """
        self._require_signer()
        self._reset_cancel()
        self._tracker = ProgressTracker()

        proof_bytes = _to_bytes(proof_data)
        ch_bytes = _to_bytes(circuit_hash)
        pi_bytes = _to_bytes(public_inputs)
        gens_bytes = _to_bytes(gens_data)

        total_gas = 0
        cancelled = False

        # -- Start --
        self._emit_progress(0, -1, phase="start")
        self._check_cancelled()

        start_receipt = self._send_transaction(
            self._contract.functions.startDAGBatchVerify(
                proof_bytes, ch_bytes, pi_bytes, gens_bytes
            )
        )
        self._tracker.record_step()
        session_id = self._extract_session_id(start_receipt)
        start_step = self._receipt_to_step(start_receipt)
        total_gas += start_step.gas_used

        session = self.get_session(session_id)
        total_batches = session.total_batches
        estimated_finalize = 3
        overall_total = 1 + total_batches + estimated_finalize + (0 if skip_cleanup else 1)

        self._emit_progress(1, overall_total, phase="start",
                            tx_hash=start_step.tx_hash, gas_used=start_step.gas_used)

        # -- Continue --
        continue_steps: list[StepResult] = []
        sid_bytes = _to_bytes(session_id)

        for i in range(total_batches):
            if self._cancelled:
                cancelled = True
                break

            self._emit_progress(1 + i, overall_total, phase="continue")

            receipt = self._send_transaction(
                self._contract.functions.continueDAGBatchVerify(
                    sid_bytes, proof_bytes, pi_bytes, gens_bytes
                )
            )
            self._tracker.record_step()
            step = self._receipt_to_step(receipt)
            continue_steps.append(step)
            total_gas += step.gas_used

            self._emit_progress(1 + i + 1, overall_total, phase="continue",
                                tx_hash=step.tx_hash, gas_used=step.gas_used)

        # -- Finalize --
        finalize_steps: list[StepResult] = []
        if not cancelled:
            finalize_idx = 0
            finalized = False
            while not finalized:
                if self._cancelled:
                    cancelled = True
                    break

                self._emit_progress(
                    1 + total_batches + finalize_idx, overall_total, phase="finalize"
                )

                receipt = self._send_transaction(
                    self._contract.functions.finalizeDAGBatchVerify(
                        sid_bytes, proof_bytes, pi_bytes, gens_bytes
                    )
                )
                self._tracker.record_step()
                step = self._receipt_to_step(receipt)
                finalize_steps.append(step)
                total_gas += step.gas_used

                session_state = self.get_session(session_id)
                finalized = session_state.finalized

                self._emit_progress(
                    1 + total_batches + finalize_idx + 1, overall_total,
                    phase="finalize", tx_hash=step.tx_hash, gas_used=step.gas_used
                )
                finalize_idx += 1

        # -- Cleanup --
        cleanup_step = None
        if not cancelled and not skip_cleanup:
            self._emit_progress(overall_total - 1, overall_total, phase="cleanup")

            receipt = self._send_transaction(
                self._contract.functions.cleanupDAGBatchSession(sid_bytes)
            )
            self._tracker.record_step()
            cleanup_step = self._receipt_to_step(receipt)
            total_gas += cleanup_step.gas_used

            self._emit_progress(overall_total, overall_total, phase="cleanup",
                                tx_hash=cleanup_step.tx_hash, gas_used=cleanup_step.gas_used)

        duration_ms = self._tracker.elapsed_ms

        return {
            "session_id": session_id,
            "start_step": start_step,
            "continue_steps": continue_steps,
            "finalize_steps": finalize_steps,
            "cleanup_step": cleanup_step,
            "total_gas_used": total_gas,
            "duration_ms": duration_ms,
            "cancelled": cancelled,
        }

    # ---- Static helpers ----

    @staticmethod
    def load_fixture(fixture_data: dict[str, Any]) -> dict[str, str]:
        """Parse a JSON fixture into verification input fields.

        Supports both fixture formats:

        - ``phase1a_dag_fixture``: ``proof_hex``, ``public_inputs_hex``
        - ``dag_groth16_e2e_fixture``: ``inner_proof_hex``, ``public_values_abi``

        Groth16 keys take precedence when both are present.

        Returns:
            A dict with keys: ``proof``, ``circuit_hash``, ``public_inputs``,
            ``gens_data`` -- all hex strings with ``0x`` prefix.
        """
        proof = fixture_data.get("inner_proof_hex") or fixture_data.get("proof_hex")
        if not proof:
            raise ValueError(
                "Fixture must contain 'proof_hex' or 'inner_proof_hex'"
            )

        public_inputs = fixture_data.get("public_values_abi") or fixture_data.get(
            "public_inputs_hex"
        )
        if not public_inputs:
            raise ValueError(
                "Fixture must contain 'public_inputs_hex' or 'public_values_abi'"
            )

        circuit_hash = fixture_data.get("circuit_hash_raw")
        if not circuit_hash:
            raise ValueError("Fixture must contain 'circuit_hash_raw'")

        gens_data = fixture_data.get("gens_hex")
        if not gens_data:
            raise ValueError("Fixture must contain 'gens_hex'")

        return {
            "proof": _ensure_hex(proof),
            "circuit_hash": _ensure_hex(circuit_hash),
            "public_inputs": _ensure_hex(public_inputs),
            "gens_data": _ensure_hex(gens_data),
        }

    @staticmethod
    def estimate_transaction_count(
        num_compute_layers: int, num_eval_groups: int
    ) -> dict[str, int]:
        """Estimate the number of transactions needed for batch verification.

        Args:
            num_compute_layers: Number of compute layers in the circuit.
            num_eval_groups: Number of input evaluation groups.

        Returns:
            A dict with keys: ``start``, ``continue``, ``finalize``, ``total``.
        """
        start = 1
        continue_count = math.ceil(num_compute_layers / LAYERS_PER_BATCH)
        finalize_count = math.ceil(num_eval_groups / GROUPS_PER_FINALIZE_BATCH)
        return {
            "start": start,
            "continue": continue_count,
            "finalize": finalize_count,
            "total": start + continue_count + finalize_count,
        }

    # ---- Internal helpers ----

    def _require_signer(self) -> None:
        """Raise if no private key was provided."""
        if self._account is None:
            raise RuntimeError(
                "A private_key is required for write operations. "
                "Pass private_key to the constructor."
            )

    def _send_transaction(self, fn: Any) -> Any:
        """Build, sign, and send a transaction with retry logic."""
        assert self._account is not None

        last_error: Exception | None = None
        for attempt in range(self._max_retries + 1):
            try:
                tx = fn.build_transaction({
                    "from": self._account.address,
                    "nonce": self._w3.eth.get_transaction_count(self._account.address),
                    "gas": self._gas_limit,
                    "gasPrice": self._w3.eth.gas_price,
                })

                # Try gas estimation with 10% buffer
                try:
                    estimated = self._w3.eth.estimate_gas(tx)
                    tx["gas"] = int(estimated * 1.1)
                except Exception:
                    logger.debug("Gas estimation failed, using configured limit", exc_info=True)

                signed = self._account.sign_transaction(tx)
                tx_hash = self._w3.eth.send_raw_transaction(signed.raw_transaction)
                receipt = self._w3.eth.wait_for_transaction_receipt(tx_hash)

                if receipt.get("status") == 0:
                    raise RuntimeError(
                        f"Transaction reverted: {self._tx_hash_hex(receipt)}"
                    )

                return receipt

            except Exception as e:
                last_error = e
                err_msg = str(e).lower()
                is_transient = any(
                    keyword in err_msg
                    for keyword in ["nonce", "timeout", "connection", "already known"]
                )
                if is_transient and attempt < self._max_retries:
                    time.sleep(self._retry_delay_s * (attempt + 1))
                    continue
                raise

        raise last_error  # type: ignore[misc]

    def _extract_session_id(self, receipt: Any) -> str:
        """Extract session ID from DAGBatchSessionStarted event logs."""
        logs = self._contract.events.DAGBatchSessionStarted().process_receipt(
            receipt
        )
        if not logs:
            raise RuntimeError(
                "No DAGBatchSessionStarted event found in transaction receipt"
            )
        session_id = logs[0]["args"]["sessionId"]
        if isinstance(session_id, bytes):
            return "0x" + session_id.hex()
        return str(session_id)

    @staticmethod
    def _receipt_to_step(receipt: Any) -> StepResult:
        """Convert a transaction receipt to a StepResult."""
        tx_hash = receipt["transactionHash"]
        if isinstance(tx_hash, bytes):
            tx_hash = "0x" + tx_hash.hex()
        return StepResult(
            tx_hash=str(tx_hash),
            gas_used=receipt["gasUsed"],
            block_number=receipt["blockNumber"],
        )

    @staticmethod
    def _tx_hash_hex(receipt: Any) -> str:
        """Extract tx hash as hex string."""
        h = receipt["transactionHash"]
        if isinstance(h, bytes):
            return "0x" + h.hex()
        return str(h)
