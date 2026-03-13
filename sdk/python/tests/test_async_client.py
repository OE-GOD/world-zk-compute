"""Tests for the async TEE verifier and event watcher (no Anvil required)."""

from __future__ import annotations

import asyncio
from unittest.mock import AsyncMock, MagicMock, patch, PropertyMock

import pytest

from worldzk.async_client import AsyncTEEVerifier, AsyncEventWatcher
from worldzk.tee_verifier import MLResult, TEE_ML_VERIFIER_ABI
from worldzk.event_watcher import (
    ResultSubmitted,
    ResultFinalized,
    ResultChallenged,
    DisputeResolved,
    EnclaveRegistered,
    parse_log,
    topic_hash,
    _EVENT_SIGNATURES,
)

# Re-use the known topic hashes from the sync test.
KNOWN_TOPIC_HASHES = {
    "ResultSubmitted": bytes.fromhex(
        "cbbfd00437c68789269982ebb1774629efbc497b9dd3e767a3870820602c35f1"
    ),
    "ResultFinalized": bytes.fromhex(
        "531940df687601ed4136aa673e705589fdac82035576fb0b7379964b97193792"
    ),
    "ResultChallenged": bytes.fromhex(
        "ddad479d6cf293bb88e77dd5fad20b1133db1dcb443bf02abd352ac3a09842f1"
    ),
    "DisputeResolved": bytes.fromhex(
        "6309d9f2499864a4f9d4ddb22f2b493afde8c215a1cd2e178647596cffe2efa4"
    ),
}

FAKE_CONTRACT = "0x" + "ab" * 20
FAKE_RPC_URL = "http://localhost:8545"
FAKE_PRIVATE_KEY = "0x" + "aa" * 32
FAKE_RESULT_ID = b"\xaa" * 32
FAKE_MODEL_HASH = b"\xbb" * 32
FAKE_INPUT_HASH = b"\xcc" * 32
FAKE_TX_HASH = b"\x11" * 32
FAKE_SUBMITTER_ADDR = "0x" + "dd" * 20
FAKE_ENCLAVE_ADDR = "0x" + "ff" * 20
FAKE_IMAGE_HASH = b"\x99" * 32


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


def _make_verifier(with_key: bool = True) -> AsyncTEEVerifier:
    """Create an AsyncTEEVerifier, optionally without a private key."""
    kwargs = {
        "rpc_url": FAKE_RPC_URL,
        "contract_address": FAKE_CONTRACT,
    }
    if with_key:
        kwargs["private_key"] = FAKE_PRIVATE_KEY
    return AsyncTEEVerifier(**kwargs)


def _mock_send_tx(verifier: AsyncTEEVerifier, receipt: dict = None) -> None:
    """Replace _send_tx with an AsyncMock returning the given receipt."""
    if receipt is None:
        receipt = {"transactionHash": FAKE_TX_HASH}
    verifier._contract = MagicMock()
    verifier._send_tx = AsyncMock(return_value=receipt)


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- construction tests
# ---------------------------------------------------------------------------


class TestAsyncVerifierInit:
    """Tests for AsyncTEEVerifier construction."""

    def test_async_verifier_init(self):
        verifier = _make_verifier()
        assert verifier.address is not None
        assert verifier.address.startswith("0x")
        assert len(verifier.address) == 42
        assert verifier.account_address is not None

    def test_async_verifier_init_without_private_key(self):
        verifier = _make_verifier(with_key=False)
        assert verifier.account_address is None

    def test_async_verifier_init_custom_gas_limit(self):
        verifier = AsyncTEEVerifier(
            rpc_url=FAKE_RPC_URL,
            contract_address=FAKE_CONTRACT,
            private_key=FAKE_PRIVATE_KEY,
            gas_limit=1_000_000,
        )
        assert verifier._gas_limit == 1_000_000


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- submit_result
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncSubmitResult:
    """Tests for AsyncTEEVerifier.submit_result."""

    async def test_async_submit_result(self):
        verifier = _make_verifier()

        mock_receipt = {
            "transactionHash": FAKE_TX_HASH,
            "logs": [],
            "status": 1,
            "blockNumber": 42,
            "blockHash": b"\x00" * 32,
            "contractAddress": None,
        }

        mock_event_processor = MagicMock()
        mock_event_processor.process_receipt.return_value = [
            {"args": {"resultId": FAKE_RESULT_ID}}
        ]
        verifier._contract = MagicMock()
        verifier._contract.functions.submitResult.return_value = MagicMock()
        verifier._contract.events.ResultSubmitted.return_value = mock_event_processor

        verifier._send_tx = AsyncMock(return_value=mock_receipt)

        result_id = await verifier.submit_result(
            model_hash=FAKE_MODEL_HASH,
            input_hash=FAKE_INPUT_HASH,
            result=b"\xde\xad\xbe\xef",
            attestation=b"\x01\x02\x03",
            stake_wei=100,
        )
        assert result_id == "0x" + FAKE_RESULT_ID.hex()
        verifier._send_tx.assert_awaited_once()

    async def test_async_submit_result_no_private_key_raises(self):
        verifier = _make_verifier(with_key=False)
        with pytest.raises(RuntimeError, match="Cannot send transactions"):
            await verifier.submit_result(
                model_hash=FAKE_MODEL_HASH,
                input_hash=FAKE_INPUT_HASH,
                result=b"\xde\xad",
                attestation=b"\x01",
            )

    async def test_async_submit_result_fallback_to_tx_hash(self):
        """When no ResultSubmitted event is emitted, fall back to tx hash."""
        verifier = _make_verifier()

        mock_receipt = {"transactionHash": FAKE_TX_HASH}
        mock_event_processor = MagicMock()
        mock_event_processor.process_receipt.return_value = []
        verifier._contract = MagicMock()
        verifier._contract.functions.submitResult.return_value = MagicMock()
        verifier._contract.events.ResultSubmitted.return_value = mock_event_processor

        verifier._send_tx = AsyncMock(return_value=mock_receipt)

        result_id = await verifier.submit_result(
            model_hash=FAKE_MODEL_HASH,
            input_hash=FAKE_INPUT_HASH,
            result=b"\xde\xad",
            attestation=b"\x01",
        )
        assert result_id == "0x" + FAKE_TX_HASH.hex()


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- challenge_result
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncChallengeResult:
    """Tests for AsyncTEEVerifier.challenge_result."""

    async def test_async_challenge_result(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.challenge.return_value = MagicMock()

        tx_hash = await verifier.challenge_result(
            result_id=FAKE_RESULT_ID, bond_wei=50
        )
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._send_tx.assert_awaited_once()


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- finalize_result
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncFinalizeResult:
    """Tests for AsyncTEEVerifier.finalize_result."""

    async def test_async_finalize_result(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.finalize.return_value = MagicMock()

        tx_hash = await verifier.finalize_result(result_id=FAKE_RESULT_ID)
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._send_tx.assert_awaited_once()


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- get_result
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncGetResult:
    """Tests for AsyncTEEVerifier.get_result."""

    async def test_async_get_result(self):
        verifier = _make_verifier(with_key=False)

        raw_tuple = (
            "0x" + "11" * 20,   # enclave
            "0x" + "22" * 20,   # submitter
            b"\x00" * 32,       # model_hash
            b"\x00" * 32,       # input_hash
            b"\x00" * 32,       # result_hash
            b"\xde\xad",        # result
            1000,               # submitted_at
            2000,               # challenge_deadline
            3000,               # dispute_deadline
            100,                # challenge_bond
            200,                # prover_stake_amount
            True,               # finalized
            False,              # challenged
            "0x" + "33" * 20,   # challenger
        )

        mock_call = AsyncMock(return_value=raw_tuple)
        mock_fn = MagicMock()
        mock_fn.call = mock_call

        verifier._contract = MagicMock()
        verifier._contract.functions.getResult.return_value = mock_fn

        result = await verifier.get_result(FAKE_RESULT_ID)
        assert isinstance(result, MLResult)
        assert result.submitted_at == 1000
        assert result.finalized is True
        assert result.challenged is False
        assert result.prover_stake_amount == 200
        assert result.result == b"\xde\xad"

    async def test_async_get_result_accepts_hex_string(self):
        """get_result should accept hex-encoded result IDs."""
        verifier = _make_verifier(with_key=False)

        raw_tuple = (
            "0x" + "11" * 20, "0x" + "22" * 20,
            b"\x00" * 32, b"\x00" * 32, b"\x00" * 32,
            b"", 0, 0, 0, 0, 0, False, False, "0x" + "00" * 20,
        )
        mock_call = AsyncMock(return_value=raw_tuple)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.getResult.return_value = mock_fn

        hex_id = "0x" + "aa" * 32
        result = await verifier.get_result(hex_id)
        assert isinstance(result, MLResult)


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- is_result_valid
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncIsResultValid:
    """Tests for AsyncTEEVerifier.is_result_valid."""

    async def test_async_is_result_valid_true(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=True)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.isResultValid.return_value = mock_fn

        valid = await verifier.is_result_valid(FAKE_RESULT_ID)
        assert valid is True

    async def test_async_is_result_valid_false(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=False)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.isResultValid.return_value = mock_fn

        valid = await verifier.is_result_valid(FAKE_RESULT_ID)
        assert valid is False


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- register / revoke enclave
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncEnclaveManagement:
    """Tests for register_enclave and revoke_enclave."""

    async def test_async_register_enclave(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.registerEnclave.return_value = MagicMock()

        tx_hash = await verifier.register_enclave(
            enclave_key=FAKE_ENCLAVE_ADDR,
            image_hash=FAKE_IMAGE_HASH,
        )
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._send_tx.assert_awaited_once()

    async def test_async_revoke_enclave(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.revokeEnclave.return_value = MagicMock()

        tx_hash = await verifier.revoke_enclave(enclave_key=FAKE_ENCLAVE_ADDR)
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._send_tx.assert_awaited_once()


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- resolve dispute
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncResolveDispute:
    """Tests for resolve_dispute and resolve_dispute_by_timeout."""

    async def test_async_resolve_dispute(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.resolveDispute.return_value = MagicMock()

        tx_hash = await verifier.resolve_dispute(
            result_id=FAKE_RESULT_ID,
            proof=b"\x01\x02",
            circuit_hash=b"\x00" * 32,
            public_inputs=b"\x03\x04",
            gens_data=b"\x05\x06",
        )
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._send_tx.assert_awaited_once()

    async def test_async_resolve_dispute_by_timeout(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.resolveDisputeByTimeout.return_value = MagicMock()

        tx_hash = await verifier.resolve_dispute_by_timeout(result_id=FAKE_RESULT_ID)
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()
        verifier._send_tx.assert_awaited_once()


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- ownership and admin methods
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncOwnershipAndAdmin:
    """Tests for owner, transfer, pause, and view methods."""

    async def test_async_owner(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value="0x" + "11" * 20)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.owner.return_value = mock_fn

        owner = await verifier.owner()
        assert owner == "0x" + "11" * 20

    async def test_async_paused(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=False)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.paused.return_value = mock_fn

        is_paused = await verifier.paused()
        assert is_paused is False

    async def test_async_challenge_bond_amount(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=100000000000000000)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.challengeBondAmount.return_value = mock_fn

        amount = await verifier.challenge_bond_amount()
        assert amount == 100000000000000000

    async def test_async_prover_stake_amount(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=200000000000000000)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.proverStake.return_value = mock_fn

        amount = await verifier.prover_stake_amount()
        assert amount == 200000000000000000

    async def test_async_dispute_resolved(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=True)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.disputeResolved.return_value = mock_fn

        resolved = await verifier.dispute_resolved(FAKE_RESULT_ID)
        assert resolved is True

    async def test_async_dispute_prover_won(self):
        verifier = _make_verifier(with_key=False)
        mock_call = AsyncMock(return_value=True)
        mock_fn = MagicMock()
        mock_fn.call = mock_call
        verifier._contract = MagicMock()
        verifier._contract.functions.disputeProverWon.return_value = mock_fn

        won = await verifier.dispute_prover_won(FAKE_RESULT_ID)
        assert won is True

    async def test_async_pause_and_unpause(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.pause.return_value = MagicMock()

        tx_hash = await verifier.pause()
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()

    async def test_async_transfer_ownership(self):
        verifier = _make_verifier()
        _mock_send_tx(verifier)
        verifier._contract.functions.transferOwnership.return_value = MagicMock()

        tx_hash = await verifier.transfer_ownership("0x" + "55" * 20)
        assert tx_hash == "0x" + FAKE_TX_HASH.hex()


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- close
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncVerifierClose:
    """Tests for AsyncTEEVerifier.close."""

    async def test_async_close(self):
        verifier = _make_verifier(with_key=False)
        mock_session = AsyncMock()
        verifier._w3.provider._session = mock_session
        await verifier.close()
        mock_session.close.assert_awaited_once()

    async def test_async_close_no_session(self):
        verifier = _make_verifier(with_key=False)
        verifier._w3.provider._session = None
        await verifier.close()  # Should not raise


# ---------------------------------------------------------------------------
# AsyncTEEVerifier -- error handling
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncVerifierErrors:
    """Error handling tests for AsyncTEEVerifier."""

    async def test_send_tx_without_key_raises(self):
        """All write methods should raise when no private key is provided."""
        verifier = _make_verifier(with_key=False)
        with pytest.raises(RuntimeError, match="Cannot send transactions"):
            await verifier.challenge_result(FAKE_RESULT_ID)

    async def test_send_tx_without_key_finalize_raises(self):
        verifier = _make_verifier(with_key=False)
        with pytest.raises(RuntimeError, match="Cannot send transactions"):
            await verifier.finalize_result(FAKE_RESULT_ID)

    async def test_tx_hash_hex_with_string(self):
        """_tx_hash_hex should handle string tx hashes."""
        result = AsyncTEEVerifier._tx_hash_hex({"transactionHash": "0xabcdef"})
        assert result == "0xabcdef"


# ---------------------------------------------------------------------------
# AsyncEventWatcher -- construction tests
# ---------------------------------------------------------------------------


class TestAsyncEventWatcherInit:
    """Tests for AsyncEventWatcher construction."""

    def test_async_event_watcher_init(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        assert watcher.contract_address is not None
        assert watcher.contract_address.startswith("0x")
        assert len(watcher.contract_address) == 42
        assert watcher.rpc_url == FAKE_RPC_URL
        assert watcher.is_watching is False

    def test_async_event_watcher_topic_filter_none(self):
        assert AsyncEventWatcher._build_topic0_filter(None) is None

    def test_async_event_watcher_topic_filter_empty(self):
        assert AsyncEventWatcher._build_topic0_filter([]) is None

    def test_async_event_watcher_topic_filter_single(self):
        result = AsyncEventWatcher._build_topic0_filter(["ResultFinalized"])
        assert isinstance(result, bytes)
        assert len(result) == 32

    def test_async_event_watcher_topic_filter_multiple(self):
        result = AsyncEventWatcher._build_topic0_filter(
            ["ResultSubmitted", "ResultChallenged"]
        )
        assert isinstance(result, list)
        assert len(result) == 2

    def test_async_event_watcher_topic_filter_unknown_raises(self):
        with pytest.raises(ValueError, match="Unknown event type"):
            AsyncEventWatcher._build_topic0_filter(["BogusEvent"])


# ---------------------------------------------------------------------------
# AsyncEventWatcher -- poll_events
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncEventWatcherPoll:
    """Tests for AsyncEventWatcher.poll_events."""

    async def test_async_event_watcher_poll_empty(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=100)
        mock_w3.eth.get_logs = AsyncMock(return_value=[])
        watcher._w3 = mock_w3

        events, next_block = await watcher.poll_events(from_block=0)
        assert events == []
        assert next_block == 101

    async def test_async_event_watcher_poll_parses_logs(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=100)

        finalized_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=50,
        )
        mock_w3.eth.get_logs = AsyncMock(return_value=[finalized_log])
        watcher._w3 = mock_w3

        events, next_block = await watcher.poll_events(from_block=0)
        assert len(events) == 1
        assert isinstance(events[0], ResultFinalized)
        assert events[0].result_id == FAKE_RESULT_ID
        assert next_block == 101

    async def test_async_event_watcher_poll_from_block_gt_latest(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=50)
        watcher._w3 = mock_w3

        events, next_block = await watcher.poll_events(from_block=100)
        assert events == []
        assert next_block == 100

    async def test_async_event_watcher_poll_filters_event_types(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=100)

        finalized_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=50,
        )
        mock_w3.eth.get_logs = AsyncMock(return_value=[finalized_log])
        watcher._w3 = mock_w3

        events, _ = await watcher.poll_events(
            from_block=0, event_types=["ResultChallenged"]
        )
        assert len(events) == 0

    async def test_async_event_watcher_poll_explicit_to_block(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_logs = AsyncMock(return_value=[])
        watcher._w3 = mock_w3

        events, next_block = await watcher.poll_events(
            from_block=10, to_block=20
        )
        assert events == []
        assert next_block == 21

    async def test_async_event_watcher_poll_multiple_events(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=200)

        log1 = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=100,
        )
        prover_won_data = b"\x00" * 31 + b"\x01"
        log2 = _make_log(
            topic0=KNOWN_TOPIC_HASHES["DisputeResolved"],
            topics_extra=[FAKE_RESULT_ID],
            data=prover_won_data,
            block_number=150,
        )
        mock_w3.eth.get_logs = AsyncMock(return_value=[log1, log2])
        watcher._w3 = mock_w3

        events, next_block = await watcher.poll_events(from_block=0)
        assert len(events) == 2
        assert isinstance(events[0], ResultFinalized)
        assert isinstance(events[1], DisputeResolved)
        assert events[1].prover_won is True
        assert next_block == 201


# ---------------------------------------------------------------------------
# AsyncEventWatcher -- watch / stop lifecycle
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncEventWatcherWatch:
    """Tests for AsyncEventWatcher.watch and stop lifecycle."""

    async def test_async_watch_and_stop(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=0)
        mock_w3.eth.get_logs = AsyncMock(return_value=[])
        watcher._w3 = mock_w3

        received: list = []
        await watcher.watch(received.append, poll_interval=0.05, from_block=0)
        assert watcher.is_watching is True

        await asyncio.sleep(0.15)
        watcher.stop()
        assert watcher.is_watching is False

    async def test_async_watch_raises_if_already_watching(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=0)
        mock_w3.eth.get_logs = AsyncMock(return_value=[])
        watcher._w3 = mock_w3

        await watcher.watch(lambda e: None, poll_interval=0.1, from_block=0)
        try:
            with pytest.raises(RuntimeError, match="already running"):
                await watcher.watch(
                    lambda e: None, poll_interval=0.1, from_block=0
                )
        finally:
            watcher.stop()

    async def test_async_watch_delivers_events(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )

        finalized_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=1,
        )

        call_count = 0

        async def mock_get_logs(params):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                return [finalized_log]
            return []

        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=0)
        mock_w3.eth.get_logs = mock_get_logs
        watcher._w3 = mock_w3

        received: list = []
        await watcher.watch(received.append, poll_interval=0.05, from_block=0)

        await asyncio.sleep(0.3)
        watcher.stop()

        assert len(received) >= 1
        assert isinstance(received[0], ResultFinalized)
        assert received[0].result_id == FAKE_RESULT_ID

    async def test_async_watch_with_async_callback(self):
        """watch() should support async callbacks."""
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )

        finalized_log = _make_log(
            topic0=KNOWN_TOPIC_HASHES["ResultFinalized"],
            topics_extra=[FAKE_RESULT_ID],
            block_number=1,
        )

        call_count = 0

        async def mock_get_logs(params):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                return [finalized_log]
            return []

        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=0)
        mock_w3.eth.get_logs = mock_get_logs
        watcher._w3 = mock_w3

        received: list = []

        async def async_callback(event):
            received.append(event)

        await watcher.watch(async_callback, poll_interval=0.05, from_block=0)

        await asyncio.sleep(0.3)
        watcher.stop()

        assert len(received) >= 1
        assert isinstance(received[0], ResultFinalized)

    async def test_async_stop_is_safe_when_not_watching(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        watcher.stop()  # Should not raise.
        assert watcher.is_watching is False

    async def test_async_watch_survives_rpc_error(self):
        """The watch loop should swallow transient RPC errors and keep polling."""
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )

        call_count = 0

        async def mock_get_logs(params):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                raise ConnectionError("RPC down")
            return []

        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=0)
        mock_w3.eth.get_logs = mock_get_logs
        watcher._w3 = mock_w3

        await watcher.watch(lambda e: None, poll_interval=0.05, from_block=0)
        await asyncio.sleep(0.3)
        watcher.stop()

        # Should have retried after the first failure.
        assert call_count >= 2

    async def test_async_close(self):
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_session = AsyncMock()
        watcher._w3.provider._session = mock_session
        await watcher.close()
        mock_session.close.assert_awaited_once()
        assert watcher.is_watching is False

    async def test_async_close_stops_active_watch(self):
        """close() should stop an active watch loop."""
        watcher = AsyncEventWatcher(
            contract_address=FAKE_CONTRACT,
            rpc_url=FAKE_RPC_URL,
        )
        mock_w3 = MagicMock()
        mock_w3.eth.get_block_number = AsyncMock(return_value=0)
        mock_w3.eth.get_logs = AsyncMock(return_value=[])
        watcher._w3 = mock_w3

        await watcher.watch(lambda e: None, poll_interval=0.05, from_block=0)
        assert watcher.is_watching is True

        # Replace provider for close
        watcher._w3.provider._session = None
        await watcher.close()
        assert watcher.is_watching is False
