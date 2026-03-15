"""
ExecutionEngine SDK client for on-chain verifiable computation lifecycle.

Requires the ``web3`` optional dependency: ``pip install worldzk[web3]``

Example usage::

    from worldzk.execution_engine import ExecutionEngineClient

    client = ExecutionEngineClient(
        rpc_url="http://localhost:8545",
        contract_address="0x...",
        private_key="0x...",
    )

    # Submit a request
    request_id = client.submit_request(
        image_id="0x" + "ab" * 32,
        input_hash="0x" + "cd" * 32,
        input_url="https://example.com/inputs.json",
        tip_wei=100000000000000,  # 0.0001 ETH
    )

    # Prover claims the job
    client.claim_job(request_id)

    # Prover submits proof
    client.submit_result(request_id, seal=b"...", journal=b"...")

    # Query status
    status = client.get_request_status(request_id)
    print(f"Status: {status.status}")
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from typing import Any, List, Optional

from eth_account import Account
from eth_account.signers.local import LocalAccount
from web3 import Web3
from web3.contract import Contract


class RequestStatus(IntEnum):
    """Execution request status matching the on-chain enum."""

    PENDING = 0
    CLAIMED = 1
    COMPLETED = 2
    EXPIRED = 3
    CANCELLED = 4


@dataclass
class ExecutionRequestInfo:
    """On-chain execution request data."""

    id: int
    image_id: bytes
    input_digest: bytes
    requester: str
    created_at: int
    expires_at: int
    callback_contract: str
    status: RequestStatus
    claimed_by: str
    claimed_at: int
    claim_deadline: int
    tip: int
    max_tip: int

    @property
    def is_pending(self) -> bool:
        """Check if request is pending."""
        return self.status == RequestStatus.PENDING

    @property
    def is_claimed(self) -> bool:
        """Check if request has been claimed."""
        return self.status == RequestStatus.CLAIMED

    @property
    def is_completed(self) -> bool:
        """Check if request is completed."""
        return self.status == RequestStatus.COMPLETED

    @property
    def is_expired(self) -> bool:
        """Check if request has expired."""
        return self.status == RequestStatus.EXPIRED

    @property
    def is_cancelled(self) -> bool:
        """Check if request was cancelled."""
        return self.status == RequestStatus.CANCELLED

    @property
    def is_terminal(self) -> bool:
        """Check if request is in a terminal state."""
        return self.status in {
            RequestStatus.COMPLETED,
            RequestStatus.EXPIRED,
            RequestStatus.CANCELLED,
        }


EXECUTION_ENGINE_ABI: list[dict[str, Any]] = [
    # Events
    {
        "type": "event",
        "name": "ExecutionRequested",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
            {"name": "requester", "type": "address", "indexed": True},
            {"name": "imageId", "type": "bytes32", "indexed": True},
            {"name": "inputDigest", "type": "bytes32", "indexed": False},
            {"name": "inputUrl", "type": "string", "indexed": False},
            {"name": "inputType", "type": "uint8", "indexed": False},
            {"name": "tip", "type": "uint256", "indexed": False},
            {"name": "expiresAt", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ExecutionClaimed",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "claimDeadline", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ExecutionCompleted",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "journalDigest", "type": "bytes32", "indexed": False},
            {"name": "payout", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ExecutionExpired",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ExecutionCancelled",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ClaimExpired",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
            {"name": "prover", "type": "address", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "CallbackFailed",
        "inputs": [
            {"name": "requestId", "type": "uint256", "indexed": True},
            {"name": "callbackContract", "type": "address", "indexed": True},
        ],
        "anonymous": False,
    },
    # Functions
    {
        "type": "function",
        "name": "requestExecution",
        "inputs": [
            {"name": "imageId", "type": "bytes32"},
            {"name": "inputDigest", "type": "bytes32"},
            {"name": "inputUrl", "type": "string"},
            {"name": "callbackContract", "type": "address"},
            {"name": "expirationSeconds", "type": "uint256"},
            {"name": "inputType", "type": "uint8"},
        ],
        "outputs": [{"name": "requestId", "type": "uint256"}],
        "stateMutability": "payable",
    },
    {
        "type": "function",
        "name": "cancelExecution",
        "inputs": [{"name": "requestId", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "claimExecution",
        "inputs": [{"name": "requestId", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "submitProof",
        "inputs": [
            {"name": "requestId", "type": "uint256"},
            {"name": "seal", "type": "bytes"},
            {"name": "journal", "type": "bytes"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "getRequest",
        "inputs": [{"name": "requestId", "type": "uint256"}],
        "outputs": [
            {
                "name": "",
                "type": "tuple",
                "components": [
                    {"name": "id", "type": "uint256"},
                    {"name": "imageId", "type": "bytes32"},
                    {"name": "inputDigest", "type": "bytes32"},
                    {"name": "requester", "type": "address"},
                    {"name": "createdAt", "type": "uint48"},
                    {"name": "expiresAt", "type": "uint48"},
                    {"name": "callbackContract", "type": "address"},
                    {"name": "status", "type": "uint8"},
                    {"name": "claimedBy", "type": "address"},
                    {"name": "claimedAt", "type": "uint48"},
                    {"name": "claimDeadline", "type": "uint48"},
                    {"name": "tip", "type": "uint256"},
                    {"name": "maxTip", "type": "uint256"},
                ],
            }
        ],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getCurrentTip",
        "inputs": [{"name": "requestId", "type": "uint256"}],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getPendingRequests",
        "inputs": [
            {"name": "offset", "type": "uint256"},
            {"name": "limit", "type": "uint256"},
        ],
        "outputs": [{"name": "", "type": "uint256[]"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getProverStats",
        "inputs": [{"name": "prover", "type": "address"}],
        "outputs": [
            {"name": "completed", "type": "uint256"},
            {"name": "earnings", "type": "uint256"},
        ],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "nextRequestId",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "protocolFeeBps",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "feeRecipient",
        "inputs": [],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "owner",
        "inputs": [],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "paused",
        "inputs": [],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "pause",
        "inputs": [],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "unpause",
        "inputs": [],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "setProtocolFee",
        "inputs": [{"name": "_feeBps", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "setFeeRecipient",
        "inputs": [{"name": "_recipient", "type": "address"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
]


def _to_bytes32(value: bytes | str) -> bytes:
    """Convert to 32-byte value."""
    if isinstance(value, str):
        if value.startswith("0x") or value.startswith("0X"):
            value = bytes.fromhex(value[2:])
        else:
            value = bytes.fromhex(value)
    if len(value) != 32:
        raise ValueError(f"Expected 32 bytes, got {len(value)}")
    return value


def _to_bytes(value: bytes | str) -> bytes:
    """Convert hex string or bytes to bytes."""
    if isinstance(value, str):
        if value.startswith("0x") or value.startswith("0X"):
            return bytes.fromhex(value[2:])
        return bytes.fromhex(value)
    return value


# Minimum tip required by the contract (0.0001 ETH)
MIN_TIP_WEI = 100_000_000_000_000


class ExecutionEngineClient:
    """Client for the ExecutionEngine smart contract.

    Provides methods for the full execution lifecycle:
    request submission, job claiming, proof submission, and status queries.

    Can be used in read-only mode (without private_key) for queries,
    or with a private_key for submitting transactions.
    """

    def __init__(
        self,
        rpc_url: str,
        contract_address: str,
        private_key: Optional[str] = None,
        gas_limit: int = 500_000,
    ):
        """Create a new ExecutionEngineClient.

        Args:
            rpc_url: JSON-RPC endpoint URL.
            contract_address: Deployed ExecutionEngine contract address.
            private_key: Hex-encoded private key for signing transactions.
                         If None, only read-only methods are available.
            gas_limit: Maximum gas per transaction (default 500000).
        """
        self._w3 = Web3(Web3.HTTPProvider(rpc_url))
        self._account: Optional[LocalAccount] = None
        if private_key is not None:
            self._account = Account.from_key(private_key)
        self._contract: Contract = self._w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=EXECUTION_ENGINE_ABI,
        )
        self._gas_limit = gas_limit

    @property
    def address(self) -> str:
        """The contract address."""
        return self._contract.address

    @property
    def account_address(self) -> Optional[str]:
        """The signer's address, or None if read-only."""
        if self._account is not None:
            return self._account.address
        return None

    def _require_signer(self) -> LocalAccount:
        """Ensure a private key was provided for transaction signing."""
        if self._account is None:
            raise ValueError(
                "Private key required for transactions. "
                "Pass private_key to the constructor."
            )
        return self._account

    # ===================================================================
    # TRANSACTION METHODS
    # ===================================================================

    def submit_request(
        self,
        image_id: bytes | str,
        input_hash: bytes | str,
        input_url: str = "",
        callback_contract: str = "0x0000000000000000000000000000000000000000",
        expiration_seconds: int = 0,
        input_type: int = 0,
        tip_wei: int = MIN_TIP_WEI,
    ) -> int:
        """Submit an execution request.

        Args:
            image_id: The program image ID (bytes32).
            input_hash: Hash of the inputs (bytes32).
            input_url: URL where provers can fetch the inputs.
            callback_contract: Contract to receive results (zero address for none).
            expiration_seconds: Expiration time in seconds (0 = default 1 hour).
            input_type: 0 = public, 1 = private.
            tip_wei: Tip amount in wei (must be >= MIN_TIP = 0.0001 ETH).

        Returns:
            The request ID assigned by the contract.

        Raises:
            ValueError: If tip is below minimum or private key not set.
        """
        if tip_wei < MIN_TIP_WEI:
            raise ValueError(
                f"Tip must be at least {MIN_TIP_WEI} wei (0.0001 ETH), got {tip_wei}"
            )

        receipt = self._send_tx(
            self._contract.functions.requestExecution(
                _to_bytes32(image_id),
                _to_bytes32(input_hash),
                input_url,
                Web3.to_checksum_address(callback_contract),
                expiration_seconds,
                input_type,
            ),
            value=tip_wei,
        )

        # Parse request ID from ExecutionRequested event
        logs = self._contract.events.ExecutionRequested().process_receipt(receipt)
        if logs:
            return logs[0]["args"]["requestId"]

        # Fallback: cannot determine request ID from receipt
        raise RuntimeError("ExecutionRequested event not found in transaction receipt")

    def claim_job(self, request_id: int) -> str:
        """Claim an execution request as a prover.

        Args:
            request_id: The request to claim.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.claimExecution(request_id)
        )
        return self._tx_hash_hex(receipt)

    def submit_result(
        self,
        request_id: int,
        seal: bytes | str,
        journal: bytes | str,
    ) -> str:
        """Submit proof for a claimed execution request.

        Args:
            request_id: The request ID.
            seal: The proof seal bytes.
            journal: The public outputs (journal) bytes.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.submitProof(
                request_id,
                _to_bytes(seal),
                _to_bytes(journal),
            )
        )
        return self._tx_hash_hex(receipt)

    def cancel_request(self, request_id: int) -> str:
        """Cancel a pending execution request and refund the tip.

        Only the original requester can cancel.

        Args:
            request_id: The request to cancel.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.cancelExecution(request_id)
        )
        return self._tx_hash_hex(receipt)

    # ===================================================================
    # READ-ONLY METHODS
    # ===================================================================

    def get_request_status(self, request_id: int) -> ExecutionRequestInfo:
        """Get on-chain request details.

        Args:
            request_id: The request to query.

        Returns:
            ExecutionRequestInfo with full request details.
        """
        raw = self._contract.functions.getRequest(request_id).call()
        return ExecutionRequestInfo(
            id=raw[0],
            image_id=raw[1],
            input_digest=raw[2],
            requester=raw[3],
            created_at=raw[4],
            expires_at=raw[5],
            callback_contract=raw[6],
            status=RequestStatus(raw[7]),
            claimed_by=raw[8],
            claimed_at=raw[9],
            claim_deadline=raw[10],
            tip=raw[11],
            max_tip=raw[12],
        )

    def get_current_tip(self, request_id: int) -> int:
        """Get the current tip for a request (decreases over time via decay).

        Args:
            request_id: The request to query.

        Returns:
            Current tip amount in wei.
        """
        return self._contract.functions.getCurrentTip(request_id).call()

    def get_pending_requests(
        self, offset: int = 0, limit: int = 20
    ) -> List[int]:
        """Get pending request IDs for provers to find work.

        Args:
            offset: Number of matching requests to skip.
            limit: Maximum number of request IDs to return.

        Returns:
            List of request IDs that are currently pending.
        """
        return self._contract.functions.getPendingRequests(offset, limit).call()

    def get_prover_stats(self, prover_address: str) -> tuple[int, int]:
        """Get prover statistics.

        Args:
            prover_address: The prover address to query.

        Returns:
            Tuple of (completed_count, total_earnings_wei).
        """
        result = self._contract.functions.getProverStats(
            Web3.to_checksum_address(prover_address)
        ).call()
        return (result[0], result[1])

    def get_next_request_id(self) -> int:
        """Get the next request ID that will be assigned."""
        return self._contract.functions.nextRequestId().call()

    def get_protocol_fee_bps(self) -> int:
        """Get the protocol fee in basis points."""
        return self._contract.functions.protocolFeeBps().call()

    def get_fee_recipient(self) -> str:
        """Get the protocol fee recipient address."""
        return self._contract.functions.feeRecipient().call()

    def owner(self) -> str:
        """Get the contract owner address."""
        return self._contract.functions.owner().call()

    def paused(self) -> bool:
        """Check if the contract is paused."""
        return self._contract.functions.paused().call()

    # ===================================================================
    # DISPUTE METHODS (maps to task requirement)
    # ===================================================================

    def dispute_result(self, request_id: int) -> str:
        """Initiate a dispute on a completed request.

        Note: The ExecutionEngine contract uses a proof-based verification
        model (submit proof at completion time), not a post-hoc dispute
        model. This method cancels a pending request as the closest
        available on-chain dispute mechanism. For TEE-style challenge/dispute
        flows, use TEEVerifier instead.

        Args:
            request_id: The request to dispute/cancel.

        Returns:
            Transaction hash as hex string.
        """
        return self.cancel_request(request_id)

    # ===================================================================
    # INTERNAL
    # ===================================================================

    def _send_tx(self, fn: Any, value: int = 0) -> Any:
        """Build, sign, and send a transaction."""
        account = self._require_signer()
        tx = fn.build_transaction({
            "from": account.address,
            "nonce": self._w3.eth.get_transaction_count(account.address),
            "gas": self._gas_limit,
            "gasPrice": self._w3.eth.gas_price,
            "value": value,
        })
        try:
            estimated = self._w3.eth.estimate_gas(tx)
            tx["gas"] = int(estimated * 1.1)
        except Exception:
            pass
        signed = account.sign_transaction(tx)
        tx_hash = self._w3.eth.send_raw_transaction(signed.raw_transaction)
        return self._w3.eth.wait_for_transaction_receipt(tx_hash)

    @staticmethod
    def _tx_hash_hex(receipt: Any) -> str:
        """Extract tx hash as hex string."""
        h = receipt["transactionHash"]
        if isinstance(h, bytes):
            return "0x" + h.hex()
        return str(h)
