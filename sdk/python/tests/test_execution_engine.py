"""Tests for the ExecutionEngine SDK client (no Anvil required)."""

from unittest.mock import MagicMock, PropertyMock, patch

import pytest

from worldzk.execution_engine import (
    EXECUTION_ENGINE_ABI,
    MIN_TIP_WEI,
    ExecutionEngineClient,
    ExecutionRequestInfo,
    RequestStatus,
    _to_bytes,
    _to_bytes32,
)


# ===========================================================================
# Helper conversion tests
# ===========================================================================


class TestToBytes32:
    """Tests for _to_bytes32 helper."""

    def test_from_bytes(self):
        val = b"\x00" * 32
        assert _to_bytes32(val) == val

    def test_from_hex_string_0x(self):
        hex_str = "0x" + "ab" * 32
        result = _to_bytes32(hex_str)
        assert len(result) == 32
        assert result == bytes.fromhex("ab" * 32)

    def test_from_hex_string_0X(self):
        hex_str = "0X" + "cd" * 32
        result = _to_bytes32(hex_str)
        assert len(result) == 32

    def test_from_hex_no_prefix(self):
        hex_str = "ef" * 32
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

    def test_empty_bytes(self):
        assert _to_bytes(b"") == b""


# ===========================================================================
# RequestStatus enum tests
# ===========================================================================


class TestRequestStatus:
    """Tests for the RequestStatus enum."""

    def test_pending_value(self):
        assert RequestStatus.PENDING == 0

    def test_claimed_value(self):
        assert RequestStatus.CLAIMED == 1

    def test_completed_value(self):
        assert RequestStatus.COMPLETED == 2

    def test_expired_value(self):
        assert RequestStatus.EXPIRED == 3

    def test_cancelled_value(self):
        assert RequestStatus.CANCELLED == 4

    def test_from_int(self):
        assert RequestStatus(0) == RequestStatus.PENDING
        assert RequestStatus(2) == RequestStatus.COMPLETED


# ===========================================================================
# ExecutionRequestInfo tests
# ===========================================================================


class TestExecutionRequestInfo:
    """Tests for the ExecutionRequestInfo dataclass."""

    def _make_info(self, status: RequestStatus = RequestStatus.PENDING) -> ExecutionRequestInfo:
        return ExecutionRequestInfo(
            id=1,
            image_id=b"\x00" * 32,
            input_digest=b"\x00" * 32,
            requester="0x" + "11" * 20,
            created_at=1000,
            expires_at=4600,
            callback_contract="0x" + "00" * 20,
            status=status,
            claimed_by="0x" + "00" * 20,
            claimed_at=0,
            claim_deadline=0,
            tip=100_000_000_000_000,
            max_tip=100_000_000_000_000,
        )

    def test_is_pending(self):
        info = self._make_info(RequestStatus.PENDING)
        assert info.is_pending is True
        assert info.is_claimed is False

    def test_is_claimed(self):
        info = self._make_info(RequestStatus.CLAIMED)
        assert info.is_claimed is True
        assert info.is_pending is False

    def test_is_completed(self):
        info = self._make_info(RequestStatus.COMPLETED)
        assert info.is_completed is True
        assert info.is_terminal is True

    def test_is_expired(self):
        info = self._make_info(RequestStatus.EXPIRED)
        assert info.is_expired is True
        assert info.is_terminal is True

    def test_is_cancelled(self):
        info = self._make_info(RequestStatus.CANCELLED)
        assert info.is_cancelled is True
        assert info.is_terminal is True

    def test_non_terminal_statuses(self):
        for status in [RequestStatus.PENDING, RequestStatus.CLAIMED]:
            info = self._make_info(status)
            assert info.is_terminal is False


# ===========================================================================
# ABI structure tests
# ===========================================================================


class TestABI:
    """Tests for the ABI definition."""

    def test_abi_has_all_functions(self):
        fn_names = {e["name"] for e in EXECUTION_ENGINE_ABI if e["type"] == "function"}
        expected = {
            "requestExecution",
            "cancelExecution",
            "claimExecution",
            "submitProof",
            "getRequest",
            "getCurrentTip",
            "getPendingRequests",
            "getProverStats",
            "nextRequestId",
            "protocolFeeBps",
            "feeRecipient",
            "owner",
            "paused",
            "pause",
            "unpause",
            "setProtocolFee",
            "setFeeRecipient",
        }
        assert fn_names == expected

    def test_abi_has_events(self):
        event_names = {e["name"] for e in EXECUTION_ENGINE_ABI if e["type"] == "event"}
        expected = {
            "ExecutionRequested",
            "ExecutionClaimed",
            "ExecutionCompleted",
            "ExecutionExpired",
            "ExecutionCancelled",
            "ClaimExpired",
            "CallbackFailed",
        }
        assert event_names == expected

    def test_request_execution_is_payable(self):
        fn = next(
            e for e in EXECUTION_ENGINE_ABI
            if e.get("name") == "requestExecution"
        )
        assert fn["stateMutability"] == "payable"

    def test_claim_execution_is_nonpayable(self):
        fn = next(
            e for e in EXECUTION_ENGINE_ABI
            if e.get("name") == "claimExecution"
        )
        assert fn["stateMutability"] == "nonpayable"

    def test_submit_proof_is_nonpayable(self):
        fn = next(
            e for e in EXECUTION_ENGINE_ABI
            if e.get("name") == "submitProof"
        )
        assert fn["stateMutability"] == "nonpayable"

    def test_get_request_returns_tuple(self):
        fn = next(
            e for e in EXECUTION_ENGINE_ABI
            if e.get("name") == "getRequest"
        )
        assert fn["outputs"][0]["type"] == "tuple"
        components = fn["outputs"][0]["components"]
        assert len(components) == 13
        field_names = [c["name"] for c in components]
        assert "id" in field_names
        assert "imageId" in field_names
        assert "inputDigest" in field_names
        assert "requester" in field_names
        assert "status" in field_names
        assert "claimedBy" in field_names
        assert "tip" in field_names

    def test_view_functions(self):
        views = [
            e for e in EXECUTION_ENGINE_ABI
            if e.get("stateMutability") == "view"
        ]
        view_names = {e["name"] for e in views}
        assert "getRequest" in view_names
        assert "getCurrentTip" in view_names
        assert "getPendingRequests" in view_names
        assert "getProverStats" in view_names
        assert "owner" in view_names
        assert "paused" in view_names


# ===========================================================================
# ExecutionEngineClient tests (mocked web3)
# ===========================================================================


def _make_mock_client(with_key: bool = True) -> ExecutionEngineClient:
    """Create an ExecutionEngineClient with mocked web3 internals."""
    with patch("worldzk.execution_engine.Web3") as MockWeb3, \
         patch("worldzk.execution_engine.Account") as MockAccount:

        mock_w3_instance = MagicMock()
        MockWeb3.return_value = mock_w3_instance
        MockWeb3.HTTPProvider = MagicMock()
        MockWeb3.to_checksum_address = lambda addr: addr

        mock_contract = MagicMock()
        mock_w3_instance.eth.contract.return_value = mock_contract
        mock_contract.address = "0x" + "aa" * 20

        if with_key:
            mock_account = MagicMock()
            mock_account.address = "0x" + "bb" * 20
            MockAccount.from_key.return_value = mock_account
            client = ExecutionEngineClient(
                rpc_url="http://localhost:8545",
                contract_address="0x" + "aa" * 20,
                private_key="0x" + "cc" * 32,
            )
            client._account = mock_account
        else:
            client = ExecutionEngineClient.__new__(ExecutionEngineClient)
            client._w3 = mock_w3_instance
            client._contract = mock_contract
            client._account = None
            client._gas_limit = 500_000

        client._w3 = mock_w3_instance
        client._contract = mock_contract

    return client


class TestExecutionEngineClientInit:
    """Tests for client construction."""

    def test_address_property(self):
        client = _make_mock_client()
        assert client.address == "0x" + "aa" * 20

    def test_account_address_with_key(self):
        client = _make_mock_client(with_key=True)
        assert client.account_address == "0x" + "bb" * 20

    def test_account_address_without_key(self):
        client = _make_mock_client(with_key=False)
        assert client.account_address is None

    def test_require_signer_raises_without_key(self):
        client = _make_mock_client(with_key=False)
        with pytest.raises(ValueError, match="Private key required"):
            client._require_signer()

    def test_require_signer_returns_account_with_key(self):
        client = _make_mock_client(with_key=True)
        account = client._require_signer()
        assert account.address == "0x" + "bb" * 20


class TestSubmitRequest:
    """Tests for submit_request method."""

    def test_tip_below_minimum_raises(self):
        client = _make_mock_client()
        with pytest.raises(ValueError, match="Tip must be at least"):
            client.submit_request(
                image_id=b"\x01" * 32,
                input_hash=b"\x02" * 32,
                tip_wei=1,  # way below minimum
            )

    def test_tip_at_minimum_accepted(self):
        client = _make_mock_client()

        # Mock _send_tx
        mock_receipt = {"transactionHash": b"\xab" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        # Mock event processing
        mock_event_processor = MagicMock()
        mock_event_processor.process_receipt.return_value = [
            {"args": {"requestId": 42}}
        ]
        client._contract.events.ExecutionRequested.return_value = mock_event_processor

        result = client.submit_request(
            image_id=b"\x01" * 32,
            input_hash=b"\x02" * 32,
            tip_wei=MIN_TIP_WEI,
        )
        assert result == 42

    def test_submit_request_calls_contract(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\xab" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        mock_event_processor = MagicMock()
        mock_event_processor.process_receipt.return_value = [
            {"args": {"requestId": 7}}
        ]
        client._contract.events.ExecutionRequested.return_value = mock_event_processor

        request_id = client.submit_request(
            image_id="0x" + "01" * 32,
            input_hash="0x" + "02" * 32,
            input_url="https://example.com/input.json",
            tip_wei=200_000_000_000_000,
        )

        assert request_id == 7
        client._send_tx.assert_called_once()

    def test_submit_request_no_event_raises(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\xab" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        mock_event_processor = MagicMock()
        mock_event_processor.process_receipt.return_value = []
        client._contract.events.ExecutionRequested.return_value = mock_event_processor

        with pytest.raises(RuntimeError, match="ExecutionRequested event not found"):
            client.submit_request(
                image_id=b"\x01" * 32,
                input_hash=b"\x02" * 32,
            )

    def test_submit_request_no_signer_raises(self):
        client = _make_mock_client(with_key=False)
        with pytest.raises(ValueError, match="Private key required"):
            client.submit_request(
                image_id=b"\x01" * 32,
                input_hash=b"\x02" * 32,
            )


class TestClaimJob:
    """Tests for claim_job method."""

    def test_claim_job_returns_tx_hash(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\xde\xad" + b"\x00" * 30}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.claim_job(request_id=5)
        assert tx_hash.startswith("0x")
        assert len(tx_hash) == 66  # 0x + 64 hex chars
        client._send_tx.assert_called_once()

    def test_claim_job_no_signer_raises(self):
        client = _make_mock_client(with_key=False)
        with pytest.raises(ValueError, match="Private key required"):
            client.claim_job(request_id=1)


class TestSubmitResult:
    """Tests for submit_result method."""

    def test_submit_result_returns_tx_hash(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\xca\xfe" + b"\x00" * 30}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.submit_result(
            request_id=3,
            seal=b"\x01\x02\x03",
            journal=b"\x04\x05\x06",
        )
        assert tx_hash.startswith("0x")
        client._send_tx.assert_called_once()

    def test_submit_result_hex_inputs(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.submit_result(
            request_id=3,
            seal="0xdeadbeef",
            journal="0xcafebabe",
        )
        assert tx_hash.startswith("0x")

    def test_submit_result_no_signer_raises(self):
        client = _make_mock_client(with_key=False)
        with pytest.raises(ValueError, match="Private key required"):
            client.submit_result(
                request_id=1,
                seal=b"\x01",
                journal=b"\x02",
            )


class TestGetRequestStatus:
    """Tests for get_request_status method (read-only, no signer needed)."""

    def test_get_request_status_returns_info(self):
        client = _make_mock_client(with_key=False)

        # Mock the contract call
        mock_fn = MagicMock()
        mock_fn.call.return_value = (
            1,                      # id
            b"\x01" * 32,           # imageId
            b"\x02" * 32,           # inputDigest
            "0x" + "11" * 20,       # requester
            1000,                   # createdAt
            4600,                   # expiresAt
            "0x" + "00" * 20,       # callbackContract
            0,                      # status (Pending)
            "0x" + "00" * 20,       # claimedBy
            0,                      # claimedAt
            0,                      # claimDeadline
            100_000_000_000_000,    # tip
            100_000_000_000_000,    # maxTip
        )
        client._contract.functions.getRequest.return_value = mock_fn

        info = client.get_request_status(1)

        assert isinstance(info, ExecutionRequestInfo)
        assert info.id == 1
        assert info.image_id == b"\x01" * 32
        assert info.status == RequestStatus.PENDING
        assert info.is_pending is True
        assert info.tip == 100_000_000_000_000

    def test_get_request_status_claimed(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = (
            2, b"\x01" * 32, b"\x02" * 32, "0x" + "11" * 20,
            1000, 4600, "0x" + "00" * 20,
            1,  # status = Claimed
            "0x" + "33" * 20,  # claimedBy
            1100, 1700,
            100_000_000_000_000, 100_000_000_000_000,
        )
        client._contract.functions.getRequest.return_value = mock_fn

        info = client.get_request_status(2)
        assert info.status == RequestStatus.CLAIMED
        assert info.is_claimed is True
        assert info.claimed_by == "0x" + "33" * 20


class TestGetCurrentTip:
    """Tests for get_current_tip."""

    def test_returns_tip_value(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = 90_000_000_000_000
        client._contract.functions.getCurrentTip.return_value = mock_fn

        tip = client.get_current_tip(1)
        assert tip == 90_000_000_000_000


class TestGetPendingRequests:
    """Tests for get_pending_requests."""

    def test_returns_list_of_ids(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = [1, 3, 5, 7]
        client._contract.functions.getPendingRequests.return_value = mock_fn

        ids = client.get_pending_requests(offset=0, limit=10)
        assert ids == [1, 3, 5, 7]

    def test_empty_list(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = []
        client._contract.functions.getPendingRequests.return_value = mock_fn

        ids = client.get_pending_requests()
        assert ids == []


class TestGetProverStats:
    """Tests for get_prover_stats."""

    def test_returns_tuple(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = (10, 500_000_000_000_000_000)
        client._contract.functions.getProverStats.return_value = mock_fn

        completed, earnings = client.get_prover_stats("0x" + "aa" * 20)
        assert completed == 10
        assert earnings == 500_000_000_000_000_000


class TestReadOnlyMethods:
    """Tests for other read-only methods."""

    def test_get_next_request_id(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 42
        client._contract.functions.nextRequestId.return_value = mock_fn

        assert client.get_next_request_id() == 42

    def test_get_protocol_fee_bps(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 250
        client._contract.functions.protocolFeeBps.return_value = mock_fn

        assert client.get_protocol_fee_bps() == 250

    def test_get_fee_recipient(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = "0x" + "ff" * 20
        client._contract.functions.feeRecipient.return_value = mock_fn

        assert client.get_fee_recipient() == "0x" + "ff" * 20

    def test_owner(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = "0x" + "dd" * 20
        client._contract.functions.owner.return_value = mock_fn

        assert client.owner() == "0x" + "dd" * 20

    def test_paused(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = False
        client._contract.functions.paused.return_value = mock_fn

        assert client.paused() is False


class TestDisputeResult:
    """Tests for dispute_result method."""

    def test_dispute_delegates_to_cancel(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.dispute_result(request_id=5)
        assert tx_hash.startswith("0x")
        # Verify it calls cancelExecution under the hood
        client._contract.functions.cancelExecution.assert_called_once_with(5)


class TestCancelRequest:
    """Tests for cancel_request method."""

    def test_cancel_request_returns_tx_hash(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.cancel_request(request_id=10)
        assert tx_hash.startswith("0x")
        client._send_tx.assert_called_once()


class TestTxHashHex:
    """Tests for _tx_hash_hex static method."""

    def test_bytes_hash(self):
        receipt = {"transactionHash": b"\xde\xad\xbe\xef" + b"\x00" * 28}
        result = ExecutionEngineClient._tx_hash_hex(receipt)
        assert result.startswith("0x")
        assert result == "0x" + (b"\xde\xad\xbe\xef" + b"\x00" * 28).hex()

    def test_string_hash(self):
        receipt = {"transactionHash": "0xabcdef"}
        result = ExecutionEngineClient._tx_hash_hex(receipt)
        assert result == "0xabcdef"
