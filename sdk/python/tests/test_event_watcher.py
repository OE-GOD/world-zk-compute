"""Tests for the TEE event watcher module (no Anvil required)."""

from __future__ import annotations

import threading
import time
from unittest.mock import MagicMock, patch, PropertyMock

import pytest

from worldzk.event_watcher import (
    TEEEventType,
    TEEEventWatcher,
    ResultSubmitted,
    ResultChallenged,
    ResultFinalized,
    ResultExpired,
    DisputeResolved,
    EnclaveRegistered,
    EnclaveRevoked,
    TEEEvent,
    EVENT_TYPES,
    ALL_EVENT_NAMES,
    topic_hash,
    parse_log,
    _EVENT_SIGNATURES,
    _TOPIC_TO_NAME,
    _bytes_val,
    _address_from_topic,
    _address_from_data,
)


# ---------------------------------------------------------------------------
# Known topic hashes (computed from the Solidity event signatures via
# keccak256). These serve as regression anchors.
# ---------------------------------------------------------------------------

KNOWN_TOPIC_HASHES = {
    "ResultSubmitted": bytes.fromhex(
        "cbbfd00437c68789269982ebb1774629efbc497b9dd3e767a3870820602c35f1"
    ),
    "ResultChallenged": bytes.fromhex(
        "ddad479d6cf293bb88e77dd5fad20b1133db1dcb443bf02abd352ac3a09842f1"
    ),
    "ResultFinalized": bytes.fromhex(
        "531940df687601ed4136aa673e705589fdac82035576fb0b7379964b97193792"
    ),
    "ResultExpired": bytes.fromhex(
        "ce91eec3d4a12846bcc25bfcd925bccaece98d36b8460504eef566bb9f0a92f6"
    ),
    "DisputeResolved": bytes.fromhex(
        "6309d9f2499864a4f9d4ddb22f2b493afde8c215a1cd2e178647596cffe2efa4"
    ),
    "EnclaveRegistered": bytes.fromhex(
        "f461d2fcd0b737b85ec49b0a48e24b2de72534d256bf45886fe727cbc47b6b68"
    ),
    "EnclaveRevoked": bytes.fromhex(
        "014328d04215ef25a11c630f007f31516a065906981503bcb8cdf4998651c18b"
    ),
}

# A fake contract address for tests.
FAKE_CONTRACT = "0x" + "ab" * 20
FAKE_TX_HASH = b"\x11" * 32
FAKE_RESULT_ID = b"\xaa" * 32
FAKE_MODEL_HASH = b"\xbb" * 32
FAKE_INPUT_HASH = b"\xcc" * 32
FAKE_SUBMITTER_ADDR = "0x" + "dd" * 20
FAKE_CHALLENGER_ADDR = "0x" + "ee" * 20
FAKE_ENCLAVE_ADDR = "0x" + "ff" * 20
FAKE_IMAGE_HASH = b"\x99" * 32


# ---------------------------------------------------------------------------
# TEEEventType enum tests
# ---------------------------------------------------------------------------


class TestTEEEventType:
    """Tests for the TEEEventType enumeration."""

    def test_all_members_exist(self):
        expected = {
            "ResultSubmitted",
            "ResultChallenged",
            "ResultFinalized",
            "ResultExpired",
            "DisputeResolved",
            "EnclaveRegistered",
            "EnclaveRevoked",
        }
        actual = {member.name for member in TEEEventType}
        assert actual == expected

    def test_values_equal_names(self):
        """Each enum value should equal its name (string)."""
        for member in TEEEventType:
            assert member.value == member.name

    def test_enum_access_by_name(self):
        assert TEEEventType["ResultSubmitted"] is TEEEventType.ResultSubmitted
        assert TEEEventType["DisputeResolved"] is TEEEventType.DisputeResolved

    def test_enum_access_by_value(self):
        assert TEEEventType("ResultChallenged") is TEEEventType.ResultChallenged

    def test_count(self):
        assert len(TEEEventType) == 7


# ---------------------------------------------------------------------------
# Topic hash computation tests
# ---------------------------------------------------------------------------


class TestTopicHashes:
    """Verify keccak256 topic hashes match known Solidity values."""

    @pytest.mark.parametrize("event_name", list(KNOWN_TOPIC_HASHES.keys()))
    def test_topic_hash_matches_known(self, event_name: str):
        computed = topic_hash(event_name)
        expected = KNOWN_TOPIC_HASHES[event_name]
        assert bytes(computed) == expected, (
            f"Topic hash mismatch for {event_name}: "
            f"got {computed.hex()}, expected {expected.hex()}"
        )

    def test_all_event_signatures_have_topic_hashes(self):
        """Every event signature must produce a valid topic hash."""
        for name in _EVENT_SIGNATURES:
            h = topic_hash(name)
            assert len(h) == 32

    def test_topic_hashes_are_unique(self):
        hashes = [topic_hash(name) for name in _EVENT_SIGNATURES]
        assert len(set(bytes(h) for h in hashes)) == len(hashes)

    def test_reverse_lookup_complete(self):
        """_TOPIC_TO_NAME should map every topic hash back to an event name."""
        for name in _EVENT_SIGNATURES:
            h = topic_hash(name)
            assert bytes(h) in _TOPIC_TO_NAME
            assert _TOPIC_TO_NAME[bytes(h)] == name

    def test_unknown_event_raises_key_error(self):
        with pytest.raises(KeyError):
            topic_hash("NonExistentEvent")


# ---------------------------------------------------------------------------
# Event signature tests
# ---------------------------------------------------------------------------


class TestEventSignatures:
    """Verify event signatures match the Solidity contract."""

    def test_result_submitted_signature(self):
        assert _EVENT_SIGNATURES["ResultSubmitted"] == (
            "ResultSubmitted(bytes32,bytes32,bytes32,address)"
        )

    def test_result_challenged_signature(self):
        assert _EVENT_SIGNATURES["ResultChallenged"] == (
            "ResultChallenged(bytes32,address)"
        )

    def test_result_finalized_signature(self):
        assert _EVENT_SIGNATURES["ResultFinalized"] == "ResultFinalized(bytes32)"

    def test_result_expired_signature(self):
        assert _EVENT_SIGNATURES["ResultExpired"] == "ResultExpired(bytes32)"

    def test_dispute_resolved_signature(self):
        assert _EVENT_SIGNATURES["DisputeResolved"] == "DisputeResolved(bytes32,bool)"

    def test_enclave_registered_signature(self):
        assert _EVENT_SIGNATURES["EnclaveRegistered"] == (
            "EnclaveRegistered(address,bytes32)"
        )

    def test_enclave_revoked_signature(self):
        assert _EVENT_SIGNATURES["EnclaveRevoked"] == "EnclaveRevoked(address)"


# ---------------------------------------------------------------------------
# Typed event dataclass tests
# ---------------------------------------------------------------------------


class TestEventDataclasses:
    """Tests for the individual event dataclasses."""

    def test_result_submitted_creation(self):
        event = ResultSubmitted(
            result_id=FAKE_RESULT_ID,
            model_hash=FAKE_MODEL_HASH,
            input_hash=FAKE_INPUT_HASH,
            submitter=FAKE_SUBMITTER_ADDR,
            block_number=42,
            transaction_hash=FAKE_TX_HASH,
        )
        assert event.result_id == FAKE_RESULT_ID
        assert event.model_hash == FAKE_MODEL_HASH
        assert event.input_hash == FAKE_INPUT_HASH
        assert event.submitter == FAKE_SUBMITTER_ADDR
        assert event.block_number == 42
        assert event.transaction_hash == FAKE_TX_HASH
        assert event.event_name == "ResultSubmitted"

    def test_result_challenged_creation(self):
        event = ResultChallenged(
            result_id=FAKE_RESULT_ID,
            challenger=FAKE_CHALLENGER_ADDR,
            block_number=100,
            transaction_hash=FAKE_TX_HASH,
        )
        assert event.result_id == FAKE_RESULT_ID
        assert event.challenger == FAKE_CHALLENGER_ADDR
        assert event.event_name == "ResultChallenged"

    def test_result_finalized_creation(self):
        event = ResultFinalized(
            result_id=FAKE_RESULT_ID,
            block_number=200,
        )
        assert event.result_id == FAKE_RESULT_ID
        assert event.event_name == "ResultFinalized"

    def test_result_expired_creation(self):
        event = ResultExpired(
            result_id=FAKE_RESULT_ID,
        )
        assert event.result_id == FAKE_RESULT_ID
        assert event.event_name == "ResultExpired"

    def test_dispute_resolved_creation_prover_won(self):
        event = DisputeResolved(
            result_id=FAKE_RESULT_ID,
            prover_won=True,
        )
        assert event.prover_won is True
        assert event.event_name == "DisputeResolved"

    def test_dispute_resolved_creation_prover_lost(self):
        event = DisputeResolved(
            result_id=FAKE_RESULT_ID,
            prover_won=False,
        )
        assert event.prover_won is False

    def test_enclave_registered_creation(self):
        event = EnclaveRegistered(
            enclave_key=FAKE_ENCLAVE_ADDR,
            enclave_image_hash=FAKE_IMAGE_HASH,
            block_number=300,
        )
        assert event.enclave_key == FAKE_ENCLAVE_ADDR
        assert event.enclave_image_hash == FAKE_IMAGE_HASH
        assert event.event_name == "EnclaveRegistered"

    def test_enclave_revoked_creation(self):
        event = EnclaveRevoked(
            enclave_key=FAKE_ENCLAVE_ADDR,
        )
        assert event.enclave_key == FAKE_ENCLAVE_ADDR
        assert event.event_name == "EnclaveRevoked"

    def test_dataclasses_are_frozen(self):
        event = ResultFinalized(result_id=FAKE_RESULT_ID)
        with pytest.raises(AttributeError):
            event.result_id = b"\x00" * 32  # type: ignore

    def test_default_values(self):
        """Block number and tx hash default to 0 and empty bytes."""
        event = ResultFinalized(result_id=FAKE_RESULT_ID)
        assert event.block_number == 0
        assert event.transaction_hash == b""


# ---------------------------------------------------------------------------
# EVENT_TYPES and ALL_EVENT_NAMES tests
# ---------------------------------------------------------------------------


class TestEventTypesMapping:
    """Tests for the EVENT_TYPES dict and ALL_EVENT_NAMES list."""

    def test_event_types_keys(self):
        expected = {
            "ResultSubmitted",
            "ResultChallenged",
            "ResultFinalized",
            "ResultExpired",
            "DisputeResolved",
            "EnclaveRegistered",
            "EnclaveRevoked",
        }
        assert set(EVENT_TYPES.keys()) == expected

    def test_event_types_map_to_correct_classes(self):
        assert EVENT_TYPES["ResultSubmitted"] is ResultSubmitted
        assert EVENT_TYPES["ResultChallenged"] is ResultChallenged
        assert EVENT_TYPES["ResultFinalized"] is ResultFinalized
        assert EVENT_TYPES["ResultExpired"] is ResultExpired
        assert EVENT_TYPES["DisputeResolved"] is DisputeResolved
        assert EVENT_TYPES["EnclaveRegistered"] is EnclaveRegistered
        assert EVENT_TYPES["EnclaveRevoked"] is EnclaveRevoked

    def test_all_event_names_complete(self):
        assert set(ALL_EVENT_NAMES) == set(EVENT_TYPES.keys())

    def test_all_event_names_matches_enum(self):
        """ALL_EVENT_NAMES should contain exactly the TEEEventType members."""
        enum_names = {member.value for member in TEEEventType}
        assert set(ALL_EVENT_NAMES) == enum_names


# ---------------------------------------------------------------------------
# Helper function tests
# ---------------------------------------------------------------------------


class TestHelpers:
    """Tests for internal helper functions."""

    def test_bytes_val_from_bytes(self):
        assert _bytes_val(b"\xab\xcd") == b"\xab\xcd"

    def test_bytes_val_from_bytearray(self):
        assert _bytes_val(bytearray(b"\xab")) == b"\xab"

    def test_bytes_val_from_hex_string(self):
        assert _bytes_val("0xabcd") == b"\xab\xcd"

    def test_bytes_val_from_hex_string_no_prefix(self):
        assert _bytes_val("abcd") == b"\xab\xcd"

    def test_address_from_topic(self):
        # Address right-aligned in 32 bytes.
        topic = b"\x00" * 12 + bytes.fromhex("dd" * 20)
        addr = _address_from_topic(topic)
        assert addr.lower() == ("0x" + "dd" * 20).lower()

    def test_address_from_data(self):
        data = b"\x00" * 12 + bytes.fromhex("ee" * 20)
        addr = _address_from_data(data)
        assert addr.lower() == ("0x" + "ee" * 20).lower()

    def test_address_from_data_with_offset(self):
        data = b"\x00" * 32 + b"\x00" * 12 + bytes.fromhex("ee" * 20)
        addr = _address_from_data(data, offset=32)
        assert addr.lower() == ("0x" + "ee" * 20).lower()


# ---------------------------------------------------------------------------
# Log parsing tests
# ---------------------------------------------------------------------------


def _make_log(
    topic0: bytes,
    topics_extra: list = None,
    data: bytes = b"",
    block_number: int = 10,
    tx_hash: bytes = FAKE_TX_HASH,
) -> dict:
    """Create a mock log entry dict."""
    topics = [topic0]
    if topics_extra:
        topics.extend(topics_extra)
    return {
        "topics": topics,
        "data": data,
        "blockNumber": block_number,
        "transactionHash": tx_hash,
        "address": FAKE_CONTRACT,
        "logIndex": 0,
        "transactionIndex": 0,
    }


class TestParseLog:
    """Tests for parse_log function."""

    def test_parse_result_submitted(self):
        submitter_topic = b"\x00" * 12 + bytes.fromhex("dd" * 20)
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultSubmitted"],
            topics_extra=[FAKE_RESULT_ID, FAKE_MODEL_HASH, submitter_topic],
            data=FAKE_INPUT_HASH,
            block_number=42,
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, ResultSubmitted)
        assert event.event_name == "ResultSubmitted"
        assert event.result_id == FAKE_RESULT_ID
        assert event.model_hash == FAKE_MODEL_HASH
        assert event.input_hash == FAKE_INPUT_HASH
        assert event.submitter.lower() == ("0x" + "dd" * 20).lower()
        assert event.block_number == 42
        assert event.transaction_hash == FAKE_TX_HASH

    def test_parse_result_challenged(self):
        challenger_data = b"\x00" * 12 + bytes.fromhex("ee" * 20)
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultChallenged"],
            topics_extra=[FAKE_RESULT_ID],
            data=challenger_data,
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, ResultChallenged)
        assert event.event_name == "ResultChallenged"
        assert event.result_id == FAKE_RESULT_ID
        assert event.challenger.lower() == ("0x" + "ee" * 20).lower()

    def test_parse_result_finalized(self):
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, ResultFinalized)
        assert event.event_name == "ResultFinalized"
        assert event.result_id == FAKE_RESULT_ID

    def test_parse_result_expired(self):
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultExpired"],
            topics_extra=[FAKE_RESULT_ID],
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, ResultExpired)
        assert event.event_name == "ResultExpired"
        assert event.result_id == FAKE_RESULT_ID

    def test_parse_dispute_resolved_prover_won(self):
        prover_won_data = b"\x00" * 31 + b"\x01"
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["DisputeResolved"],
            topics_extra=[FAKE_RESULT_ID],
            data=prover_won_data,
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, DisputeResolved)
        assert event.event_name == "DisputeResolved"
        assert event.result_id == FAKE_RESULT_ID
        assert event.prover_won is True

    def test_parse_dispute_resolved_prover_lost(self):
        prover_lost_data = b"\x00" * 32
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["DisputeResolved"],
            topics_extra=[FAKE_RESULT_ID],
            data=prover_lost_data,
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, DisputeResolved)
        assert event.prover_won is False

    def test_parse_enclave_registered(self):
        enclave_topic = b"\x00" * 12 + bytes.fromhex("ff" * 20)
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["EnclaveRegistered"],
            topics_extra=[enclave_topic],
            data=FAKE_IMAGE_HASH,
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, EnclaveRegistered)
        assert event.event_name == "EnclaveRegistered"
        assert event.enclave_key.lower() == ("0x" + "ff" * 20).lower()
        assert event.enclave_image_hash == FAKE_IMAGE_HASH

    def test_parse_enclave_revoked(self):
        enclave_topic = b"\x00" * 12 + bytes.fromhex("ff" * 20)
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["EnclaveRevoked"],
            topics_extra=[enclave_topic],
        )
        event = parse_log(log)
        assert event is not None
        assert isinstance(event, EnclaveRevoked)
        assert event.event_name == "EnclaveRevoked"
        assert event.enclave_key.lower() == ("0x" + "ff" * 20).lower()

    def test_parse_unknown_topic_returns_none(self):
        log = _make_log(topic0=b"\x00" * 32)
        assert parse_log(log) is None

    def test_parse_empty_topics_returns_none(self):
        log = {"topics": [], "data": b"", "blockNumber": 0, "transactionHash": b""}
        assert parse_log(log) is None

    def test_parse_no_topics_key_returns_none(self):
        log = {"data": b"", "blockNumber": 0}
        assert parse_log(log) is None

    def test_block_number_as_hex_string(self):
        log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
        )
        log["blockNumber"] = "0xa"
        event = parse_log(log)
        assert event is not None
        assert event.block_number == 10


# ---------------------------------------------------------------------------
# TEEEventWatcher initialization tests
# ---------------------------------------------------------------------------


class TestTEEEventWatcherInit:
    """Tests for TEEEventWatcher construction."""

    def test_init_stores_address(self):
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:8545",
        )
        # Address should be checksummed.
        assert watcher.contract_address is not None
        assert watcher.contract_address.startswith("0x")
        assert len(watcher.contract_address) == 42

    def test_init_stores_rpc_url(self):
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:9999",
        )
        assert watcher.rpc_url == "http://localhost:9999"

    def test_init_with_abi(self):
        custom_abi = [{"type": "event", "name": "Custom"}]
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:8545",
            abi=custom_abi,
        )
        assert watcher._abi == custom_abi

    def test_init_abi_defaults_to_none(self):
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:8545",
        )
        assert watcher._abi is None

    def test_is_watching_initially_false(self):
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:8545",
        )
        assert watcher.is_watching is False


# ---------------------------------------------------------------------------
# TEEEventWatcher topic0 filter tests
# ---------------------------------------------------------------------------


class TestBuildTopic0Filter:
    """Tests for the _build_topic0_filter static method."""

    def test_none_event_types_returns_none(self):
        assert TEEEventWatcher._build_topic0_filter(None) is None

    def test_empty_list_returns_none(self):
        assert TEEEventWatcher._build_topic0_filter([]) is None

    def test_single_event_type_returns_bytes(self):
        result = TEEEventWatcher._build_topic0_filter(["ResultFinalized"])
        assert isinstance(result, bytes)
        assert len(result) == 32
        assert bytes(result) == KNOWN_TOPIC_HASHES["ResultFinalized"]

    def test_multiple_event_types_returns_list(self):
        result = TEEEventWatcher._build_topic0_filter(
            ["ResultSubmitted", "ResultChallenged"]
        )
        assert isinstance(result, list)
        assert len(result) == 2
        hashes_set = {bytes(h) for h in result}
        assert KNOWN_TOPIC_HASHES["ResultSubmitted"] in hashes_set
        assert KNOWN_TOPIC_HASHES["ResultChallenged"] in hashes_set

    def test_unknown_event_type_raises(self):
        with pytest.raises(ValueError, match="Unknown event type"):
            TEEEventWatcher._build_topic0_filter(["BogusEvent"])


# ---------------------------------------------------------------------------
# TEEEventWatcher poll_events tests (mocked web3)
# ---------------------------------------------------------------------------


class TestPollEvents:
    """Tests for TEEEventWatcher.poll_events with mocked web3."""

    def _make_watcher(self) -> TEEEventWatcher:
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:8545",
        )
        return watcher

    def test_poll_events_returns_tuple(self):
        watcher = self._make_watcher()
        # Mock the web3 instance.
        mock_w3 = MagicMock()
        mock_w3.eth.block_number = 100
        mock_w3.eth.get_logs.return_value = []
        watcher._w3 = mock_w3

        result = watcher.poll_events(from_block=0)
        assert isinstance(result, tuple)
        assert len(result) == 2
        events, next_block = result
        assert events == []
        assert next_block == 101

    def test_poll_events_from_block_greater_than_latest(self):
        watcher = self._make_watcher()
        mock_w3 = MagicMock()
        mock_w3.eth.block_number = 50
        watcher._w3 = mock_w3

        events, next_block = watcher.poll_events(from_block=100)
        assert events == []
        assert next_block == 100

    def test_poll_events_parses_logs(self):
        watcher = self._make_watcher()
        mock_w3 = MagicMock()
        mock_w3.eth.block_number = 100

        submitter_topic = b"\x00" * 12 + bytes.fromhex("dd" * 20)
        mock_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultSubmitted"],
            topics_extra=[FAKE_RESULT_ID, FAKE_MODEL_HASH, submitter_topic],
            data=FAKE_INPUT_HASH,
            block_number=50,
        )
        mock_w3.eth.get_logs.return_value = [mock_log]
        watcher._w3 = mock_w3

        events, next_block = watcher.poll_events(from_block=0)
        assert len(events) == 1
        assert isinstance(events[0], ResultSubmitted)
        assert events[0].result_id == FAKE_RESULT_ID
        assert next_block == 101

    def test_poll_events_filters_by_event_type(self):
        watcher = self._make_watcher()
        mock_w3 = MagicMock()
        mock_w3.eth.block_number = 100

        # Return a ResultFinalized log.
        mock_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=50,
        )
        mock_w3.eth.get_logs.return_value = [mock_log]
        watcher._w3 = mock_w3

        # Filter for ResultChallenged only -- should skip ResultFinalized.
        events, _ = watcher.poll_events(
            from_block=0,
            event_types=["ResultChallenged"],
        )
        assert len(events) == 0

    def test_poll_events_with_explicit_to_block(self):
        watcher = self._make_watcher()
        mock_w3 = MagicMock()
        mock_w3.eth.get_logs.return_value = []
        watcher._w3 = mock_w3

        events, next_block = watcher.poll_events(from_block=10, to_block=20)
        assert events == []
        assert next_block == 21

    def test_poll_events_multiple_events(self):
        watcher = self._make_watcher()
        mock_w3 = MagicMock()
        mock_w3.eth.block_number = 200

        log1 = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=100,
        )
        log2 = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultExpired"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=100,
        )
        challenger_data = b"\x00" * 12 + bytes.fromhex("ee" * 20)
        log3 = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultChallenged"],
            topics_extra=[FAKE_RESULT_ID],
            data=challenger_data,
            block_number=150,
        )
        mock_w3.eth.get_logs.return_value = [log1, log2, log3]
        watcher._w3 = mock_w3

        events, next_block = watcher.poll_events(from_block=0)
        assert len(events) == 3
        assert isinstance(events[0], ResultFinalized)
        assert isinstance(events[1], ResultExpired)
        assert isinstance(events[2], ResultChallenged)
        assert next_block == 201


# ---------------------------------------------------------------------------
# Watch / stop lifecycle tests
# ---------------------------------------------------------------------------


class TestWatchStopLifecycle:
    """Tests for watch() and stop() background polling."""

    def _make_watcher(self) -> TEEEventWatcher:
        watcher = TEEEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url="http://localhost:8545",
        )
        # Mock web3 so poll_events returns empty immediately.
        mock_w3 = MagicMock()
        mock_w3.eth.block_number = 0
        mock_w3.eth.get_logs.return_value = []
        watcher._w3 = mock_w3
        return watcher

    def test_watch_starts_thread(self):
        watcher = self._make_watcher()
        watcher.watch(lambda e: None, poll_interval=0.1, from_block=0)
        assert watcher.is_watching is True
        watcher.stop()
        assert watcher.is_watching is False

    def test_stop_is_idempotent(self):
        watcher = self._make_watcher()
        watcher.stop()  # Should not raise.
        watcher.stop()  # Still safe.
        assert watcher.is_watching is False

    def test_watch_raises_if_already_watching(self):
        watcher = self._make_watcher()
        watcher.watch(lambda e: None, poll_interval=0.1, from_block=0)
        try:
            with pytest.raises(RuntimeError, match="already running"):
                watcher.watch(lambda e: None, poll_interval=0.1, from_block=0)
        finally:
            watcher.stop()

    def test_watch_delivers_events_to_callback(self):
        watcher = self._make_watcher()

        finalized_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=1,
        )

        # Make get_logs return one event on the first call, then empty.
        call_count = 0

        def mock_get_logs(params):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                return [finalized_log]
            return []

        watcher._w3.eth.get_logs.side_effect = mock_get_logs

        received: list = []
        watcher.watch(received.append, poll_interval=0.05, from_block=0)

        # Wait a bit for the thread to pick up events.
        time.sleep(0.3)
        watcher.stop()

        assert len(received) >= 1
        assert isinstance(received[0], ResultFinalized)
        assert received[0].result_id == FAKE_RESULT_ID

    def test_watch_with_event_type_filter(self):
        watcher = self._make_watcher()

        # Return both a ResultFinalized and a ResultChallenged.
        log_finalized = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=1,
        )
        challenger_data = b"\x00" * 12 + bytes.fromhex("ee" * 20)
        log_challenged = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultChallenged"],
            topics_extra=[FAKE_RESULT_ID],
            data=challenger_data,
            block_number=2,
        )

        call_count = 0

        def mock_get_logs(params):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                return [log_finalized, log_challenged]
            return []

        watcher._w3.eth.get_logs.side_effect = mock_get_logs

        received: list = []
        watcher.watch(
            received.append,
            poll_interval=0.05,
            from_block=0,
            event_types=["ResultChallenged"],
        )

        time.sleep(0.3)
        watcher.stop()

        # Only ResultChallenged should be delivered.
        assert all(isinstance(e, ResultChallenged) for e in received)

    def test_watch_from_latest(self):
        watcher = self._make_watcher()
        watcher._w3.eth.block_number = 42
        watcher._w3.eth.get_logs.return_value = []

        watcher.watch(lambda e: None, poll_interval=0.1, from_block="latest")
        time.sleep(0.15)
        watcher.stop()

        # The first get_logs call should have used block 42 as fromBlock.
        calls = watcher._w3.eth.get_logs.call_args_list
        if calls:
            first_call_params = calls[0][0][0]
            assert first_call_params["fromBlock"] == 42

    def test_watch_survives_rpc_error(self):
        """The watch loop should swallow RPC errors and keep polling."""
        watcher = self._make_watcher()

        call_count = 0

        def mock_get_logs(params):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                raise ConnectionError("RPC down")
            return []

        watcher._w3.eth.get_logs.side_effect = mock_get_logs

        watcher.watch(lambda e: None, poll_interval=0.05, from_block=0)
        time.sleep(0.3)
        watcher.stop()

        # Should have made multiple calls despite the first failure.
        assert call_count >= 2


# ---------------------------------------------------------------------------
# TEEEvent union type tests
# ---------------------------------------------------------------------------


class TestTEEEventUnion:
    """Tests for the TEEEvent union type."""

    def test_result_submitted_is_tee_event(self):
        event: TEEEvent = ResultSubmitted(
            result_id=FAKE_RESULT_ID,
            model_hash=FAKE_MODEL_HASH,
            input_hash=FAKE_INPUT_HASH,
            submitter=FAKE_SUBMITTER_ADDR,
        )
        assert hasattr(event, "event_name")
        assert event.event_name == "ResultSubmitted"

    def test_all_event_types_have_event_name(self):
        """Every event dataclass should have an event_name field."""
        for cls in EVENT_TYPES.values():
            assert hasattr(cls, "event_name")
