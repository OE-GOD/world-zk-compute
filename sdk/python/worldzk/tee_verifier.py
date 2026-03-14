"""
TEE ML Verifier client for on-chain TEE attestation and dispute resolution.

Requires the ``web3`` optional dependency: ``pip install worldzk[web3]``

Example usage::

    from worldzk import TEEVerifier

    verifier = TEEVerifier(
        rpc_url="http://localhost:8545",
        private_key="0x...",
        contract_address="0x...",
    )

    result_id = verifier.submit_result(
        model_hash=b"\\x00" * 32,
        input_hash=b"\\x00" * 32,
        result=b"\\xde\\xad\\xbe\\xef",
        attestation=b"...",
        stake_wei=100000000000000000,  # 0.1 ETH
    )
    print(f"Result ID: {result_id}")
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from eth_account import Account
from eth_account.signers.local import LocalAccount
from web3 import Web3
from web3.contract import Contract

TEE_ML_VERIFIER_ABI: list[dict[str, Any]] = [
    {
        "type": "event",
        "name": "ResultSubmitted",
        "inputs": [
            {"name": "resultId", "type": "bytes32", "indexed": True},
            {"name": "modelHash", "type": "bytes32", "indexed": True},
            {"name": "inputHash", "type": "bytes32", "indexed": False},
            {"name": "submitter", "type": "address", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ResultChallenged",
        "inputs": [
            {"name": "resultId", "type": "bytes32", "indexed": True},
            {"name": "challenger", "type": "address", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "DisputeResolved",
        "inputs": [
            {"name": "resultId", "type": "bytes32", "indexed": True},
            {"name": "proverWon", "type": "bool", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ResultFinalized",
        "inputs": [
            {"name": "resultId", "type": "bytes32", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "EnclaveRegistered",
        "inputs": [
            {"name": "enclaveKey", "type": "address", "indexed": True},
            {"name": "enclaveImageHash", "type": "bytes32", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "EnclaveRevoked",
        "inputs": [
            {"name": "enclaveKey", "type": "address", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "function",
        "name": "registerEnclave",
        "inputs": [
            {"name": "enclaveKey", "type": "address"},
            {"name": "enclaveImageHash", "type": "bytes32"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "revokeEnclave",
        "inputs": [{"name": "enclaveKey", "type": "address"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "submitResult",
        "inputs": [
            {"name": "modelHash", "type": "bytes32"},
            {"name": "inputHash", "type": "bytes32"},
            {"name": "result", "type": "bytes"},
            {"name": "attestation", "type": "bytes"},
        ],
        "outputs": [{"name": "resultId", "type": "bytes32"}],
        "stateMutability": "payable",
    },
    {
        "type": "function",
        "name": "challenge",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [],
        "stateMutability": "payable",
    },
    {
        "type": "function",
        "name": "finalize",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "resolveDispute",
        "inputs": [
            {"name": "resultId", "type": "bytes32"},
            {"name": "proof", "type": "bytes"},
            {"name": "circuitHash", "type": "bytes32"},
            {"name": "publicInputs", "type": "bytes"},
            {"name": "gensData", "type": "bytes"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "resolveDisputeByTimeout",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "getResult",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [
            {
                "name": "",
                "type": "tuple",
                "components": [
                    {"name": "enclave", "type": "address"},
                    {"name": "submitter", "type": "address"},
                    {"name": "modelHash", "type": "bytes32"},
                    {"name": "inputHash", "type": "bytes32"},
                    {"name": "resultHash", "type": "bytes32"},
                    {"name": "result", "type": "bytes"},
                    {"name": "submittedAt", "type": "uint256"},
                    {"name": "challengeDeadline", "type": "uint256"},
                    {"name": "disputeDeadline", "type": "uint256"},
                    {"name": "challengeBond", "type": "uint256"},
                    {"name": "proverStakeAmount", "type": "uint256"},
                    {"name": "finalized", "type": "bool"},
                    {"name": "challenged", "type": "bool"},
                    {"name": "challenger", "type": "address"},
                ],
            }
        ],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "isResultValid",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [{"name": "", "type": "bool"}],
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
        "name": "pendingOwner",
        "inputs": [],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "transferOwnership",
        "inputs": [{"name": "newOwner", "type": "address"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "acceptOwnership",
        "inputs": [],
        "outputs": [],
        "stateMutability": "nonpayable",
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
        "name": "paused",
        "inputs": [],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "challengeBondAmount",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "proverStake",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "disputeResolved",
        "inputs": [{"name": "", "type": "bytes32"}],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "disputeProverWon",
        "inputs": [{"name": "", "type": "bytes32"}],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
]


@dataclass
class MLResult:
    """On-chain TEE ML result."""

    enclave: str
    submitter: str
    model_hash: bytes
    input_hash: bytes
    result_hash: bytes
    result: bytes
    submitted_at: int
    challenge_deadline: int
    dispute_deadline: int
    challenge_bond: int
    prover_stake_amount: int
    finalized: bool
    challenged: bool
    challenger: str


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


class TEEVerifier:
    """Client for the TEEMLVerifier smart contract.

    Provides methods for enclave management, result submission,
    challenging, dispute resolution, and finalization.
    """

    def __init__(
        self,
        rpc_url: str,
        private_key: str,
        contract_address: str,
        gas_limit: int = 500_000,
    ):
        """Create a new TEEVerifier.

        Args:
            rpc_url: JSON-RPC endpoint URL.
            private_key: Hex-encoded private key for signing transactions.
            contract_address: Deployed TEEMLVerifier contract address.
            gas_limit: Maximum gas per transaction (default 500000).
        """
        self._w3 = Web3(Web3.HTTPProvider(rpc_url))
        self._account: LocalAccount = Account.from_key(private_key)
        self._contract: Contract = self._w3.eth.contract(
            address=Web3.to_checksum_address(contract_address),
            abi=TEE_ML_VERIFIER_ABI,
        )
        self._gas_limit = gas_limit

    @property
    def address(self) -> str:
        """The contract address."""
        return self._contract.address

    @property
    def account_address(self) -> str:
        """The signer's address."""
        return self._account.address

    def register_enclave(self, enclave_key: str, image_hash: bytes | str) -> str:
        """Register an enclave signing key. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.registerEnclave(
                Web3.to_checksum_address(enclave_key),
                _to_bytes32(image_hash),
            )
        )
        return self._tx_hash_hex(receipt)

    def revoke_enclave(self, enclave_key: str) -> str:
        """Revoke an enclave. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.revokeEnclave(
                Web3.to_checksum_address(enclave_key),
            )
        )
        return self._tx_hash_hex(receipt)

    def submit_result(
        self,
        model_hash: bytes | str,
        input_hash: bytes | str,
        result: bytes | str,
        attestation: bytes | str,
        stake_wei: int = 0,
    ) -> str:
        """Submit a TEE-attested result. Returns the result ID as hex."""
        receipt = self._send_tx(
            self._contract.functions.submitResult(
                _to_bytes32(model_hash),
                _to_bytes32(input_hash),
                _to_bytes(result),
                _to_bytes(attestation),
            ),
            value=stake_wei,
        )
        # Result ID is returned from the function but also in logs
        # Parse from ResultSubmitted event
        logs = self._contract.events.ResultSubmitted().process_receipt(receipt)
        if logs:
            rid = logs[0]["args"]["resultId"]
            if isinstance(rid, bytes):
                return "0x" + rid.hex()
            return str(rid)
        return self._tx_hash_hex(receipt)

    def challenge(self, result_id: bytes | str, bond_wei: int = 0) -> str:
        """Challenge a result. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.challenge(_to_bytes32(result_id)),
            value=bond_wei,
        )
        return self._tx_hash_hex(receipt)

    def finalize(self, result_id: bytes | str) -> str:
        """Finalize a result after challenge window. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.finalize(_to_bytes32(result_id)),
        )
        return self._tx_hash_hex(receipt)

    def resolve_dispute(
        self,
        result_id: bytes | str,
        proof: bytes | str,
        circuit_hash: bytes | str,
        public_inputs: bytes | str,
        gens_data: bytes | str,
    ) -> str:
        """Resolve a dispute with a ZK proof. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.resolveDispute(
                _to_bytes32(result_id),
                _to_bytes(proof),
                _to_bytes32(circuit_hash),
                _to_bytes(public_inputs),
                _to_bytes(gens_data),
            )
        )
        return self._tx_hash_hex(receipt)

    def resolve_dispute_by_timeout(self, result_id: bytes | str) -> str:
        """Resolve dispute in challenger's favor after timeout. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.resolveDisputeByTimeout(
                _to_bytes32(result_id)
            ),
        )
        return self._tx_hash_hex(receipt)

    def get_result(self, result_id: bytes | str) -> MLResult:
        """Fetch an on-chain result."""
        raw = self._contract.functions.getResult(
            _to_bytes32(result_id)
        ).call()
        return MLResult(
            enclave=raw[0],
            submitter=raw[1],
            model_hash=raw[2],
            input_hash=raw[3],
            result_hash=raw[4],
            result=raw[5],
            submitted_at=raw[6],
            challenge_deadline=raw[7],
            dispute_deadline=raw[8],
            challenge_bond=raw[9],
            prover_stake_amount=raw[10],
            finalized=raw[11],
            challenged=raw[12],
            challenger=raw[13],
        )

    def is_result_valid(self, result_id: bytes | str) -> bool:
        """Check if a result is valid (finalized or dispute resolved in prover's favor)."""
        return self._contract.functions.isResultValid(
            _to_bytes32(result_id)
        ).call()

    def owner(self) -> str:
        """Get the contract owner address."""
        return self._contract.functions.owner().call()

    def pending_owner(self) -> str:
        """Get the pending owner address (for 2-step transfer)."""
        return self._contract.functions.pendingOwner().call()

    def transfer_ownership(self, new_owner: str) -> str:
        """Initiate ownership transfer (2-step). Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.transferOwnership(
                Web3.to_checksum_address(new_owner),
            )
        )
        return self._tx_hash_hex(receipt)

    def accept_ownership(self) -> str:
        """Accept pending ownership transfer. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.acceptOwnership()
        )
        return self._tx_hash_hex(receipt)

    def pause(self) -> str:
        """Pause the contract. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.pause()
        )
        return self._tx_hash_hex(receipt)

    def unpause(self) -> str:
        """Unpause the contract. Returns tx hash."""
        receipt = self._send_tx(
            self._contract.functions.unpause()
        )
        return self._tx_hash_hex(receipt)

    def paused(self) -> bool:
        """Check if the contract is paused."""
        return self._contract.functions.paused().call()

    def challenge_bond_amount(self) -> int:
        """Get the minimum challenge bond amount."""
        return self._contract.functions.challengeBondAmount().call()

    def prover_stake_amount(self) -> int:
        """Get the minimum prover stake amount."""
        return self._contract.functions.proverStake().call()

    def dispute_resolved(self, result_id: bytes | str) -> bool:
        """Check if a dispute has been resolved."""
        return self._contract.functions.disputeResolved(
            _to_bytes32(result_id)
        ).call()

    def dispute_prover_won(self, result_id: bytes | str) -> bool:
        """Check if the prover won the dispute."""
        return self._contract.functions.disputeProverWon(
            _to_bytes32(result_id)
        ).call()

    def _send_tx(self, fn: Any, value: int = 0) -> Any:
        """Build, sign, and send a transaction."""
        tx = fn.build_transaction({
            "from": self._account.address,
            "nonce": self._w3.eth.get_transaction_count(self._account.address),
            "gas": self._gas_limit,
            "gasPrice": self._w3.eth.gas_price,
            "value": value,
        })
        try:
            estimated = self._w3.eth.estimate_gas(tx)
            tx["gas"] = int(estimated * 1.1)
        except Exception:
            pass
        signed = self._account.sign_transaction(tx)
        tx_hash = self._w3.eth.send_raw_transaction(signed.raw_transaction)
        return self._w3.eth.wait_for_transaction_receipt(tx_hash)

    @staticmethod
    def _tx_hash_hex(receipt: Any) -> str:
        """Extract tx hash as hex string."""
        h = receipt["transactionHash"]
        if isinstance(h, bytes):
            return "0x" + h.hex()
        return str(h)
