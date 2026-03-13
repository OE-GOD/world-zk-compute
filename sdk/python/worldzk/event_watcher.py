"""
TEE Event Watcher for monitoring TEEMLVerifier contract events.

Polls for on-chain events using ``web3.py``'s ``eth.get_logs`` and parses them
into typed Python dataclasses.  Supports both one-shot polling and continuous
background watching with an optional event-type filter.

Requires the ``web3`` optional dependency: ``pip install worldzk[web3]``

Example usage::

    from worldzk.event_watcher import TEEEventWatcher

    watcher = TEEEventWatcher(
        contract_address="0x...",
        rpc_url="http://localhost:8545",
    )

    # One-shot poll
    events, next_block = watcher.poll_events(from_block=0)
    for event in events:
        print(event)

    # Continuous background watch
    def on_event(event):
        print(f"New event: {event}")

    watcher.watch(on_event, poll_interval=2.0, from_block="latest")
    # ... later ...
    watcher.stop()
"""

from __future__ import annotations

import enum
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Sequence, Type, Union

from web3 import Web3


class TEEEventType(enum.Enum):
    """Enumeration of all event types emitted by the TEEMLVerifier contract."""

    ResultSubmitted = "ResultSubmitted"
    ResultChallenged = "ResultChallenged"
    ResultFinalized = "ResultFinalized"
    ResultExpired = "ResultExpired"
    DisputeResolved = "DisputeResolved"
    EnclaveRegistered = "EnclaveRegistered"
    EnclaveRevoked = "EnclaveRevoked"

# ---------------------------------------------------------------------------
# Event signatures (Solidity canonical form) and their keccak256 topic hashes.
# These MUST match the events declared in ITEEMLVerifier.sol.
# ---------------------------------------------------------------------------

_EVENT_SIGNATURES: Dict[str, str] = {
    "ResultSubmitted": "ResultSubmitted(bytes32,bytes32,bytes32,address)",
    "ResultChallenged": "ResultChallenged(bytes32,address)",
    "ResultFinalized": "ResultFinalized(bytes32)",
    "ResultExpired": "ResultExpired(bytes32)",
    "DisputeResolved": "DisputeResolved(bytes32,bool)",
    "EnclaveRegistered": "EnclaveRegistered(address,bytes32)",
    "EnclaveRevoked": "EnclaveRevoked(address)",
}


def _compute_topic_hash(signature: str) -> bytes:
    """Compute the keccak256 topic hash for an event signature string."""
    return Web3.keccak(text=signature)


# Pre-computed topic hash -> event name mapping.
_TOPIC_TO_NAME: Dict[bytes, str] = {
    _compute_topic_hash(sig): name for name, sig in _EVENT_SIGNATURES.items()
}


def topic_hash(event_name: str) -> bytes:
    """Return the keccak256 topic0 hash for a known event name.

    Raises ``KeyError`` if *event_name* is not one of the recognized events.
    """
    sig = _EVENT_SIGNATURES[event_name]
    return _compute_topic_hash(sig)


# ---------------------------------------------------------------------------
# Typed event dataclasses
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ResultSubmitted:
    """A new ML result was submitted to the contract."""

    result_id: bytes
    model_hash: bytes
    input_hash: bytes
    submitter: str
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="ResultSubmitted", init=False)


@dataclass(frozen=True)
class ResultChallenged:
    """A submitted result was challenged."""

    result_id: bytes
    challenger: str
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="ResultChallenged", init=False)


@dataclass(frozen=True)
class ResultFinalized:
    """An unchallenged result was finalized after the challenge window."""

    result_id: bytes
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="ResultFinalized", init=False)


@dataclass(frozen=True)
class ResultExpired:
    """A result passed its challenge window (legacy, emitted alongside ResultFinalized)."""

    result_id: bytes
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="ResultExpired", init=False)


@dataclass(frozen=True)
class DisputeResolved:
    """A dispute was resolved via ZK proof or timeout."""

    result_id: bytes
    prover_won: bool
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="DisputeResolved", init=False)


@dataclass(frozen=True)
class EnclaveRegistered:
    """A new TEE enclave was registered."""

    enclave_key: str
    enclave_image_hash: bytes
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="EnclaveRegistered", init=False)


@dataclass(frozen=True)
class EnclaveRevoked:
    """A TEE enclave was revoked."""

    enclave_key: str
    block_number: int = 0
    transaction_hash: bytes = b""
    event_name: str = field(default="EnclaveRevoked", init=False)


# Union type for all possible TEE events.
TEEEvent = Union[
    ResultSubmitted,
    ResultChallenged,
    ResultFinalized,
    ResultExpired,
    DisputeResolved,
    EnclaveRegistered,
    EnclaveRevoked,
]

# Map from event name to its dataclass type.
EVENT_TYPES: Dict[str, Type[TEEEvent]] = {
    "ResultSubmitted": ResultSubmitted,
    "ResultChallenged": ResultChallenged,
    "ResultFinalized": ResultFinalized,
    "ResultExpired": ResultExpired,
    "DisputeResolved": DisputeResolved,
    "EnclaveRegistered": EnclaveRegistered,
    "EnclaveRevoked": EnclaveRevoked,
}

# All recognized event names.
ALL_EVENT_NAMES: List[str] = list(EVENT_TYPES.keys())


# ---------------------------------------------------------------------------
# Log parsing helpers
# ---------------------------------------------------------------------------


def _bytes_val(raw: Any) -> bytes:
    """Coerce a web3 value (bytes, HexBytes, str) to plain ``bytes``."""
    if isinstance(raw, (bytes, bytearray)):
        return bytes(raw)
    if hasattr(raw, "hex"):
        # HexBytes
        return bytes(raw)
    if isinstance(raw, str):
        if raw.startswith("0x") or raw.startswith("0X"):
            return bytes.fromhex(raw[2:])
        return bytes.fromhex(raw)
    return bytes(raw)


def _address_from_topic(topic_bytes: bytes) -> str:
    """Extract a checksummed address from a 32-byte topic (right-aligned)."""
    return Web3.to_checksum_address(topic_bytes[-20:])


def _address_from_data(data: bytes, offset: int = 0) -> str:
    """Extract a checksummed address from ABI-encoded data at *offset*."""
    return Web3.to_checksum_address(data[offset + 12 : offset + 32])


def parse_log(log: Dict[str, Any]) -> Optional[TEEEvent]:
    """Parse a raw ``eth_getLogs`` entry into a typed :class:`TEEEvent`.

    Returns ``None`` if the log does not match any known event.
    """
    topics = log.get("topics", [])
    if not topics:
        return None

    topic0 = _bytes_val(topics[0])
    event_name = _TOPIC_TO_NAME.get(topic0)
    if event_name is None:
        return None

    data = _bytes_val(log.get("data", b""))
    block_number = log.get("blockNumber", 0)
    if isinstance(block_number, str):
        block_number = int(block_number, 16)
    tx_hash = _bytes_val(log.get("transactionHash", b""))

    if event_name == "ResultSubmitted":
        # Topics: [sig, resultId(indexed), modelHash(indexed), submitter(indexed)]
        # Data: [inputHash (non-indexed)]
        result_id = _bytes_val(topics[1])
        model_hash = _bytes_val(topics[2])
        submitter = _address_from_topic(_bytes_val(topics[3]))
        input_hash = data[:32] if len(data) >= 32 else b"\x00" * 32
        return ResultSubmitted(
            result_id=result_id,
            model_hash=model_hash,
            input_hash=input_hash,
            submitter=submitter,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    if event_name == "ResultChallenged":
        # Topics: [sig, resultId(indexed)]
        # Data: [challenger (non-indexed, address padded to 32 bytes)]
        result_id = _bytes_val(topics[1])
        challenger = _address_from_data(data) if len(data) >= 32 else "0x" + "00" * 20
        return ResultChallenged(
            result_id=result_id,
            challenger=challenger,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    if event_name == "ResultFinalized":
        # Topics: [sig, resultId(indexed)]
        result_id = _bytes_val(topics[1])
        return ResultFinalized(
            result_id=result_id,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    if event_name == "ResultExpired":
        # Topics: [sig, resultId(indexed)]
        result_id = _bytes_val(topics[1])
        return ResultExpired(
            result_id=result_id,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    if event_name == "DisputeResolved":
        # Topics: [sig, resultId(indexed)]
        # Data: [proverWon (bool, padded to 32 bytes)]
        result_id = _bytes_val(topics[1])
        prover_won = False
        if len(data) >= 32:
            prover_won = int.from_bytes(data[:32], "big") != 0
        return DisputeResolved(
            result_id=result_id,
            prover_won=prover_won,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    if event_name == "EnclaveRegistered":
        # Topics: [sig, enclaveKey(indexed)]
        # Data: [enclaveImageHash (non-indexed)]
        enclave_key = _address_from_topic(_bytes_val(topics[1]))
        enclave_image_hash = data[:32] if len(data) >= 32 else b"\x00" * 32
        return EnclaveRegistered(
            enclave_key=enclave_key,
            enclave_image_hash=enclave_image_hash,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    if event_name == "EnclaveRevoked":
        # Topics: [sig, enclaveKey(indexed)]
        enclave_key = _address_from_topic(_bytes_val(topics[1]))
        return EnclaveRevoked(
            enclave_key=enclave_key,
            block_number=block_number,
            transaction_hash=tx_hash,
        )

    return None  # pragma: no cover


# ---------------------------------------------------------------------------
# TEEEventWatcher
# ---------------------------------------------------------------------------


class TEEEventWatcher:
    """Watches TEEMLVerifier contract events via ``eth_getLogs`` polling.

    Parameters
    ----------
    contract_address:
        The deployed TEEMLVerifier contract address (hex string).
    rpc_url:
        An HTTP JSON-RPC endpoint URL.
    abi:
        Optional contract ABI (unused for raw log parsing but kept for
        forward-compatibility with higher-level decode paths).
    """

    def __init__(
        self,
        contract_address: str,
        rpc_url: str,
        abi: Optional[List[Dict[str, Any]]] = None,
    ) -> None:
        """Create a new TEEEventWatcher.

        Args:
            contract_address: Deployed TEEMLVerifier contract address.
            rpc_url: JSON-RPC endpoint URL.
            abi: Optional contract ABI (unused; kept for forward-compatibility).
        """
        self._w3 = Web3(Web3.HTTPProvider(rpc_url))
        self._contract_address = Web3.to_checksum_address(contract_address)
        self._abi = abi
        self._rpc_url = rpc_url

        # Background watcher state
        self._watch_thread: Optional[threading.Thread] = None
        self._stop_event = threading.Event()

    # -- Properties ---------------------------------------------------------

    @property
    def contract_address(self) -> str:
        """The checksummed contract address being watched."""
        return self._contract_address

    @property
    def rpc_url(self) -> str:
        """The RPC URL this watcher is connected to."""
        return self._rpc_url

    @property
    def is_watching(self) -> bool:
        """Whether the background watch loop is currently running."""
        return (
            self._watch_thread is not None
            and self._watch_thread.is_alive()
            and not self._stop_event.is_set()
        )

    # -- One-shot polling ---------------------------------------------------

    def poll_events(
        self,
        from_block: int,
        to_block: Union[int, str] = "latest",
        event_types: Optional[Sequence[str]] = None,
    ) -> tuple:
        """Poll for events in a block range.

        Parameters
        ----------
        from_block:
            Starting block number (inclusive).
        to_block:
            Ending block number (inclusive) or ``"latest"``.
        event_types:
            If provided, only return events whose ``event_name`` is in this
            list.  Passing an empty list returns all events.

        Returns
        -------
        tuple[list[TEEEvent], int]
            A two-element tuple of ``(events, next_block)`` where
            *next_block* is the block number to pass as ``from_block`` on the
            next call to avoid re-processing the same logs.
        """
        # Build topic0 filter
        topic0_filter = self._build_topic0_filter(event_types)

        # Resolve to_block
        if isinstance(to_block, str) and to_block == "latest":
            resolved_to_block = self._w3.eth.block_number
        else:
            resolved_to_block = int(to_block)

        if from_block > resolved_to_block:
            return ([], from_block)

        filter_params: Dict[str, Any] = {
            "fromBlock": from_block,
            "toBlock": resolved_to_block,
            "address": self._contract_address,
        }
        if topic0_filter is not None:
            filter_params["topics"] = [topic0_filter]

        logs = self._w3.eth.get_logs(filter_params)

        events: List[TEEEvent] = []
        for log_entry in logs:
            parsed = parse_log(log_entry)
            if parsed is not None:
                # Apply client-side filter as well (belt-and-suspenders)
                if event_types and parsed.event_name not in event_types:
                    continue
                events.append(parsed)

        next_block = resolved_to_block + 1
        return (events, next_block)

    # -- Continuous watch ---------------------------------------------------

    def watch(
        self,
        callback: Callable[[TEEEvent], None],
        poll_interval: float = 2.0,
        from_block: Union[int, str] = "latest",
        event_types: Optional[Sequence[str]] = None,
    ) -> None:
        """Start a background thread that continuously polls for events.

        Each discovered event is passed to *callback*.  Call :meth:`stop` to
        terminate the background thread.

        Parameters
        ----------
        callback:
            A callable invoked with each :class:`TEEEvent` as it is discovered.
        poll_interval:
            Seconds between successive polls (default 2.0).
        from_block:
            Starting block.  Use ``"latest"`` to start from the current head.
        event_types:
            Optional filter for specific event types.
        """
        if self.is_watching:
            raise RuntimeError("Watcher is already running. Call stop() first.")

        self._stop_event.clear()

        if isinstance(from_block, str) and from_block == "latest":
            start_block = self._w3.eth.block_number
        else:
            start_block = int(from_block)

        def _poll_loop() -> None:
            current_block = start_block
            while not self._stop_event.is_set():
                try:
                    events, next_block = self.poll_events(
                        from_block=current_block,
                        to_block="latest",
                        event_types=event_types,
                    )
                    for event in events:
                        if self._stop_event.is_set():
                            return
                        callback(event)
                    current_block = next_block
                except Exception:
                    # Swallow transient RPC errors; the next poll will retry.
                    pass

                self._stop_event.wait(timeout=poll_interval)

        thread = threading.Thread(target=_poll_loop, daemon=True, name="tee-event-watcher")
        self._watch_thread = thread
        thread.start()

    def stop(self) -> None:
        """Stop the background watch loop.

        This is safe to call even if the watcher is not running.
        """
        self._stop_event.set()
        if self._watch_thread is not None:
            self._watch_thread.join(timeout=5.0)
            self._watch_thread = None

    # -- Internal helpers ---------------------------------------------------

    @staticmethod
    def _build_topic0_filter(
        event_types: Optional[Sequence[str]],
    ) -> Optional[Any]:
        """Build a topic0 filter value for ``eth_getLogs``.

        If *event_types* is ``None`` or empty, returns ``None`` (no filter).
        If it contains a single type, returns the topic hash directly.
        If multiple, returns a list of topic hashes (OR semantics in the EVM
        log filter spec).
        """
        if not event_types:
            return None

        hashes = []
        for name in event_types:
            if name not in _EVENT_SIGNATURES:
                raise ValueError(
                    f"Unknown event type: {name!r}. "
                    f"Valid types: {', '.join(ALL_EVENT_NAMES)}"
                )
            hashes.append(topic_hash(name))

        if len(hashes) == 1:
            return hashes[0]
        return hashes
