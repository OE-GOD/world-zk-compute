"""
On-chain batch verification for DAG GKR proofs.

Requires the `web3` optional dependency: ``pip install worldzk[web3]``

Example usage::

    from worldzk import BatchVerifier

    verifier = BatchVerifier(
        rpc_url="http://localhost:8545",
        private_key="0x...",
        contract_address="0x...",
    )

    fixture = BatchVerifier.load_fixture(json.load(open("fixture.json")))
    result = verifier.verify_batch(fixture)
    print(f"Total gas: {result.total_gas_used}")
"""

from __future__ import annotations

import logging
import math
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Literal

logger = logging.getLogger(__name__)

from eth_account import Account
from eth_account.signers.local import LocalAccount
from web3 import Web3
from web3.contract import Contract

from .abi import DAG_BATCH_VERIFIER_ABI

LAYERS_PER_BATCH = 8
GROUPS_PER_FINALIZE_BATCH = 16
DEFAULT_GAS_LIMIT = 30_000_000


def _ensure_hex(value: str) -> str:
    """Ensure a hex string has 0x prefix."""
    if value.startswith("0x") or value.startswith("0X"):
        return value
    return "0x" + value


@dataclass
class BatchVerifyInput:
    """Input data for batch verification."""

    proof: str
    circuit_hash: str
    public_inputs: str
    gens_data: str


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
class BatchVerifyResult:
    """Result of a complete batch verification."""

    session_id: str
    start_step: StepResult
    continue_steps: list[StepResult] = field(default_factory=list)
    finalize_steps: list[StepResult] = field(default_factory=list)
    cleanup_step: StepResult | None = None
    total_gas_used: int = 0
    duration_ms: int = 0


ProgressPhase = Literal["start", "continue", "finalize", "cleanup"]


@dataclass
class ProgressEvent:
    """Progress callback event."""

    phase: ProgressPhase
    step: int
    total_steps: int
    tx_hash: str | None = None
    gas_used: int | None = None


ProgressCallback = Callable[[ProgressEvent], None]


class BatchVerifier:
    """Multi-transaction batch verifier for DAG GKR proofs.

    Mirrors the TypeScript SDK's BatchVerifier API.
    """

    def __init__(
        self,
        rpc_url: str,
        private_key: str,
        contract_address: str,
        gas_limit: int = DEFAULT_GAS_LIMIT,
        max_retries: int = 3,
        retry_delay_s: float = 1.0,
    ):
        """Create a batch verifier.

        Args:
            rpc_url: JSON-RPC endpoint URL.
            private_key: Hex-encoded private key for signing transactions.
            contract_address: Deployed DAGBatchVerifier contract address.
            gas_limit: Maximum gas per transaction (default 30M).
            max_retries: Number of retries for transient failures.
            retry_delay_s: Base delay between retries in seconds.
        """
        self._w3 = Web3(Web3.HTTPProvider(rpc_url))
        self._account: LocalAccount = Account.from_key(private_key)
        self._gas_limit = gas_limit
        self._max_retries = max_retries
        self._retry_delay_s = retry_delay_s
        self._contract: Contract = self._w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=DAG_BATCH_VERIFIER_ABI,
        )

    def is_circuit_active(self, circuit_hash: str) -> bool:
        """Check if a circuit is registered and active on-chain."""
        return self._contract.functions.isDAGCircuitActive(
            bytes.fromhex(_ensure_hex(circuit_hash)[2:])
        ).call()

    def get_session(self, session_id: str) -> BatchSession:
        """Read on-chain session state."""
        sid = bytes.fromhex(_ensure_hex(session_id)[2:])
        result = self._contract.functions.getDAGBatchSession(sid).call()
        return BatchSession(
            circuit_hash="0x" + result[0].hex(),
            next_batch_idx=result[1],
            total_batches=result[2],
            finalized=result[3],
            finalize_input_idx=result[4],
            finalize_groups_done=result[5],
        )

    def verify_batch(
        self,
        input: BatchVerifyInput,
        on_progress: ProgressCallback | None = None,
        skip_cleanup: bool = False,
    ) -> BatchVerifyResult:
        """Execute full multi-tx batch verification (start -> continue*N -> finalize*M -> cleanup)."""
        start_time = time.monotonic()
        total_gas = 0

        proof = bytes.fromhex(_ensure_hex(input.proof)[2:])
        circuit_hash = bytes.fromhex(_ensure_hex(input.circuit_hash)[2:])
        public_inputs = bytes.fromhex(_ensure_hex(input.public_inputs)[2:])
        gens_data = bytes.fromhex(_ensure_hex(input.gens_data)[2:])

        # Step 1: Start
        if on_progress:
            on_progress(ProgressEvent(phase="start", step=1, total_steps=1))

        start_receipt = self._send_transaction(
            self._contract.functions.startDAGBatchVerify(
                proof, circuit_hash, public_inputs, gens_data
            )
        )
        session_id = self._extract_session_id(start_receipt)
        start_step = self._receipt_to_step(start_receipt)
        total_gas += start_step.gas_used

        if on_progress:
            on_progress(ProgressEvent(
                phase="start", step=1, total_steps=1,
                tx_hash=start_step.tx_hash, gas_used=start_step.gas_used,
            ))

        # Read session to get total batches
        session = self.get_session(session_id)

        # Step 2: Continue
        continue_steps: list[StepResult] = []
        total_continues = session.total_batches
        sid_bytes = bytes.fromhex(session_id[2:])

        for i in range(total_continues):
            if on_progress:
                on_progress(ProgressEvent(
                    phase="continue", step=i + 1, total_steps=total_continues,
                ))

            receipt = self._send_transaction(
                self._contract.functions.continueDAGBatchVerify(
                    sid_bytes, proof, public_inputs, gens_data
                )
            )
            step = self._receipt_to_step(receipt)
            continue_steps.append(step)
            total_gas += step.gas_used

            if on_progress:
                on_progress(ProgressEvent(
                    phase="continue", step=i + 1, total_steps=total_continues,
                    tx_hash=step.tx_hash, gas_used=step.gas_used,
                ))

        # Step 3: Finalize (may take multiple txs)
        finalize_steps: list[StepResult] = []
        finalize_idx = 0
        while True:
            finalize_idx += 1
            if on_progress:
                on_progress(ProgressEvent(
                    phase="finalize", step=finalize_idx, total_steps=finalize_idx,
                ))

            receipt = self._send_transaction(
                self._contract.functions.finalizeDAGBatchVerify(
                    sid_bytes, proof, public_inputs, gens_data
                )
            )
            step = self._receipt_to_step(receipt)
            finalize_steps.append(step)
            total_gas += step.gas_used

            if on_progress:
                on_progress(ProgressEvent(
                    phase="finalize", step=finalize_idx, total_steps=finalize_idx,
                    tx_hash=step.tx_hash, gas_used=step.gas_used,
                ))

            # Check if finalization is complete
            session = self.get_session(session_id)
            if session.finalized:
                break

        # Step 4: Cleanup
        cleanup_step = None
        if not skip_cleanup:
            if on_progress:
                on_progress(ProgressEvent(phase="cleanup", step=1, total_steps=1))

            receipt = self._send_transaction(
                self._contract.functions.cleanupDAGBatchSession(sid_bytes)
            )
            cleanup_step = self._receipt_to_step(receipt)
            total_gas += cleanup_step.gas_used

            if on_progress:
                on_progress(ProgressEvent(
                    phase="cleanup", step=1, total_steps=1,
                    tx_hash=cleanup_step.tx_hash, gas_used=cleanup_step.gas_used,
                ))

        duration_ms = int((time.monotonic() - start_time) * 1000)

        return BatchVerifyResult(
            session_id=session_id,
            start_step=start_step,
            continue_steps=continue_steps,
            finalize_steps=finalize_steps,
            cleanup_step=cleanup_step,
            total_gas_used=total_gas,
            duration_ms=duration_ms,
        )

    def resume_batch(
        self,
        session_id: str,
        input: BatchVerifyInput,
        on_progress: ProgressCallback | None = None,
        skip_cleanup: bool = False,
    ) -> BatchVerifyResult:
        """Resume a previously started batch verification session."""
        start_time = time.monotonic()
        total_gas = 0

        proof = bytes.fromhex(_ensure_hex(input.proof)[2:])
        public_inputs = bytes.fromhex(_ensure_hex(input.public_inputs)[2:])
        gens_data = bytes.fromhex(_ensure_hex(input.gens_data)[2:])
        sid_bytes = bytes.fromhex(_ensure_hex(session_id)[2:])

        session = self.get_session(session_id)

        # Use a dummy start step since we're resuming
        start_step = StepResult(tx_hash="0x" + "0" * 64, gas_used=0, block_number=0)

        # Continue from where we left off
        continue_steps: list[StepResult] = []
        remaining = session.total_batches - session.next_batch_idx
        for i in range(remaining):
            step_num = session.next_batch_idx + i + 1
            if on_progress:
                on_progress(ProgressEvent(
                    phase="continue", step=step_num, total_steps=session.total_batches,
                ))

            receipt = self._send_transaction(
                self._contract.functions.continueDAGBatchVerify(
                    sid_bytes, proof, public_inputs, gens_data
                )
            )
            step = self._receipt_to_step(receipt)
            continue_steps.append(step)
            total_gas += step.gas_used

            if on_progress:
                on_progress(ProgressEvent(
                    phase="continue", step=step_num, total_steps=session.total_batches,
                    tx_hash=step.tx_hash, gas_used=step.gas_used,
                ))

        # Finalize
        finalize_steps: list[StepResult] = []
        if not session.finalized:
            finalize_idx = 0
            while True:
                finalize_idx += 1
                if on_progress:
                    on_progress(ProgressEvent(
                        phase="finalize", step=finalize_idx, total_steps=finalize_idx,
                    ))

                receipt = self._send_transaction(
                    self._contract.functions.finalizeDAGBatchVerify(
                        sid_bytes, proof, public_inputs, gens_data
                    )
                )
                step = self._receipt_to_step(receipt)
                finalize_steps.append(step)
                total_gas += step.gas_used

                if on_progress:
                    on_progress(ProgressEvent(
                        phase="finalize", step=finalize_idx, total_steps=finalize_idx,
                        tx_hash=step.tx_hash, gas_used=step.gas_used,
                    ))

                session = self.get_session(session_id)
                if session.finalized:
                    break

        # Cleanup
        cleanup_step = None
        if not skip_cleanup:
            if on_progress:
                on_progress(ProgressEvent(phase="cleanup", step=1, total_steps=1))

            receipt = self._send_transaction(
                self._contract.functions.cleanupDAGBatchSession(sid_bytes)
            )
            cleanup_step = self._receipt_to_step(receipt)
            total_gas += cleanup_step.gas_used

            if on_progress:
                on_progress(ProgressEvent(
                    phase="cleanup", step=1, total_steps=1,
                    tx_hash=cleanup_step.tx_hash, gas_used=cleanup_step.gas_used,
                ))

        duration_ms = int((time.monotonic() - start_time) * 1000)

        return BatchVerifyResult(
            session_id=session_id,
            start_step=start_step,
            continue_steps=continue_steps,
            finalize_steps=finalize_steps,
            cleanup_step=cleanup_step,
            total_gas_used=total_gas,
            duration_ms=duration_ms,
        )

    @staticmethod
    def load_fixture(fixture_data: dict[str, Any]) -> BatchVerifyInput:
        """Parse a JSON fixture into BatchVerifyInput.

        Supports both fixture formats:
        - phase1a_dag_fixture: uses ``proof_hex``, ``public_inputs_hex``
        - dag_groth16_e2e_fixture: uses ``inner_proof_hex``, ``public_values_abi``

        The groth16 keys take precedence when both are present.
        """
        # Determine proof field (prefer groth16 format)
        if "inner_proof_hex" in fixture_data:
            proof = fixture_data["inner_proof_hex"]
        elif "proof_hex" in fixture_data:
            proof = fixture_data["proof_hex"]
        else:
            raise ValueError("Fixture must contain 'proof_hex' or 'inner_proof_hex'")

        # Determine public inputs field (prefer groth16 format)
        if "public_values_abi" in fixture_data:
            public_inputs = fixture_data["public_values_abi"]
        elif "public_inputs_hex" in fixture_data:
            public_inputs = fixture_data["public_inputs_hex"]
        else:
            raise ValueError(
                "Fixture must contain 'public_inputs_hex' or 'public_values_abi'"
            )

        if "circuit_hash_raw" not in fixture_data:
            raise ValueError("Fixture must contain 'circuit_hash_raw'")
        if "gens_hex" not in fixture_data:
            raise ValueError("Fixture must contain 'gens_hex'")

        return BatchVerifyInput(
            proof=_ensure_hex(proof),
            circuit_hash=_ensure_hex(fixture_data["circuit_hash_raw"]),
            public_inputs=_ensure_hex(public_inputs),
            gens_data=_ensure_hex(fixture_data["gens_hex"]),
        )

    @staticmethod
    def estimate_transaction_count(
        num_compute_layers: int, num_eval_groups: int
    ) -> dict[str, int]:
        """Estimate the number of transactions needed for batch verification.

        Returns a dict with keys: start, continue, finalize, total.
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

    def _send_transaction(self, fn: Any) -> Any:
        """Build, sign, and send a transaction with retry logic."""
        for attempt in range(self._max_retries):
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
                return self._w3.eth.wait_for_transaction_receipt(tx_hash)

            except Exception as e:
                err_msg = str(e).lower()
                is_transient = any(
                    keyword in err_msg
                    for keyword in ["nonce", "timeout", "connection", "already known"]
                )
                if is_transient and attempt < self._max_retries - 1:
                    time.sleep(self._retry_delay_s * (attempt + 1))
                    continue
                raise

    def _extract_session_id(self, receipt: Any) -> str:
        """Extract session ID from DAGBatchSessionStarted event logs."""
        logs = self._contract.events.DAGBatchSessionStarted().process_receipt(receipt)
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
