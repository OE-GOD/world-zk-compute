"""
ProverRegistry SDK client for on-chain prover management.

Requires the ``web3`` optional dependency: ``pip install worldzk[web3]``

Example usage::

    from worldzk.prover_registry import ProverRegistryClient

    client = ProverRegistryClient(
        rpc_url="http://localhost:8545",
        contract_address="0x...",
        private_key="0x...",
    )

    # Register as a prover
    tx_hash = client.register_prover(
        stake=1000000000000000000,  # 1 ETH in wei
        endpoint="https://myprover.example.com:8080",
    )

    # Query prover info
    info = client.get_prover_info("0x...")
    print(f"Stake: {info.stake}, Reputation: {info.reputation}")

    # Check if active
    active = client.is_prover_active("0x...")
    print(f"Active: {active}")
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, List, Optional

from eth_account import Account
from eth_account.signers.local import LocalAccount
from web3 import Web3
from web3.contract import Contract


# ---------------------------------------------------------------------------
# ABI
# ---------------------------------------------------------------------------

PROVER_REGISTRY_ABI: list[dict[str, Any]] = [
    # Events
    {
        "type": "event",
        "name": "ProverRegistered",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "stake", "type": "uint256", "indexed": False},
            {"name": "endpoint", "type": "string", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ProverDeactivated",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ProverReactivated",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "StakeAdded",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "amount", "type": "uint256", "indexed": False},
            {"name": "newTotal", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "StakeWithdrawn",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "amount", "type": "uint256", "indexed": False},
            {"name": "newTotal", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ProverSlashed",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "amount", "type": "uint256", "indexed": False},
            {"name": "reason", "type": "string", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "ReputationUpdated",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "oldRep", "type": "uint256", "indexed": False},
            {"name": "newRep", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "RewardDistributed",
        "inputs": [
            {"name": "prover", "type": "address", "indexed": True},
            {"name": "amount", "type": "uint256", "indexed": False},
        ],
        "anonymous": False,
    },
    {
        "type": "event",
        "name": "SlasherUpdated",
        "inputs": [
            {"name": "slasher", "type": "address", "indexed": True},
            {"name": "authorized", "type": "bool", "indexed": False},
        ],
        "anonymous": False,
    },
    # --- Mutating Functions ---
    {
        "type": "function",
        "name": "register",
        "inputs": [
            {"name": "stake", "type": "uint256"},
            {"name": "endpoint", "type": "string"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "addStake",
        "inputs": [{"name": "amount", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "withdrawStake",
        "inputs": [{"name": "amount", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "deactivate",
        "inputs": [],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "reactivate",
        "inputs": [],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    # --- Slashing & Rewards ---
    {
        "type": "function",
        "name": "slash",
        "inputs": [
            {"name": "prover", "type": "address"},
            {"name": "reason", "type": "string"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "recordSuccess",
        "inputs": [
            {"name": "prover", "type": "address"},
            {"name": "reward", "type": "uint256"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    # --- Admin Functions ---
    {
        "type": "function",
        "name": "setMinStake",
        "inputs": [{"name": "_minStake", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "setSlashBasisPoints",
        "inputs": [{"name": "_slashBasisPoints", "type": "uint256"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "setSlasher",
        "inputs": [
            {"name": "slasher", "type": "address"},
            {"name": "authorized", "type": "bool"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    # --- View Functions ---
    {
        "type": "function",
        "name": "getProver",
        "inputs": [{"name": "prover", "type": "address"}],
        "outputs": [
            {
                "name": "",
                "type": "tuple",
                "components": [
                    {"name": "owner", "type": "address"},
                    {"name": "stake", "type": "uint256"},
                    {"name": "reputation", "type": "uint256"},
                    {"name": "proofsSubmitted", "type": "uint256"},
                    {"name": "proofsFailed", "type": "uint256"},
                    {"name": "totalEarnings", "type": "uint256"},
                    {"name": "registeredAt", "type": "uint256"},
                    {"name": "lastActiveAt", "type": "uint256"},
                    {"name": "active", "type": "bool"},
                    {"name": "endpoint", "type": "string"},
                ],
            }
        ],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "activeProverCount",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getActiveProvers",
        "inputs": [],
        "outputs": [{"name": "", "type": "address[]"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "isProver",
        "inputs": [{"name": "addr", "type": "address"}],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "isActive",
        "inputs": [{"name": "addr", "type": "address"}],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getWeight",
        "inputs": [{"name": "prover", "type": "address"}],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "selectProver",
        "inputs": [{"name": "seed", "type": "uint256"}],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "getTopProvers",
        "inputs": [{"name": "n", "type": "uint256"}],
        "outputs": [{"name": "", "type": "address[]"}],
        "stateMutability": "view",
    },
    # --- State Accessors ---
    {
        "type": "function",
        "name": "stakingToken",
        "inputs": [],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "minStake",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "slashBasisPoints",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "totalStaked",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "owner",
        "inputs": [],
        "outputs": [{"name": "", "type": "address"}],
        "stateMutability": "view",
    },
]


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------


@dataclass
class ProverInfo:
    """On-chain prover information."""

    owner: str
    stake: int
    reputation: int
    proofs_submitted: int
    proofs_failed: int
    total_earnings: int
    registered_at: int
    last_active_at: int
    active: bool
    endpoint: str

    @property
    def reputation_percent(self) -> float:
        """Reputation as a percentage (0.0 - 100.0)."""
        return self.reputation / 100.0

    @property
    def is_registered(self) -> bool:
        """Check if the prover is registered (registeredAt != 0)."""
        return self.registered_at != 0

    @property
    def success_rate(self) -> float:
        """Success rate as a fraction (0.0 - 1.0), or 0.0 if no proofs."""
        total = self.proofs_submitted + self.proofs_failed
        if total == 0:
            return 0.0
        return self.proofs_submitted / total


# ---------------------------------------------------------------------------
# Client
# ---------------------------------------------------------------------------


class ProverRegistryClient:
    """Client for the ProverRegistry smart contract.

    Provides methods for prover registration, staking, reputation queries,
    slashing, and admin operations.

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
        """Create a new ProverRegistryClient.

        Args:
            rpc_url: JSON-RPC endpoint URL.
            contract_address: Deployed ProverRegistry contract address.
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
            abi=PROVER_REGISTRY_ABI,
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
    # REGISTRATION
    # ===================================================================

    def register_prover(self, stake: int, endpoint: str = "") -> str:
        """Register as a prover with initial stake.

        The caller must have approved the staking token for transfer beforehand.

        Args:
            stake: Amount of staking tokens to deposit (must be >= minStake).
            endpoint: Optional P2P endpoint for coordination.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.register(stake, endpoint)
        )
        return self._tx_hash_hex(receipt)

    def deregister_prover(self) -> str:
        """Deactivate the caller's prover registration.

        Stops receiving new jobs. Returns tx hash.
        """
        receipt = self._send_tx(
            self._contract.functions.deactivate()
        )
        return self._tx_hash_hex(receipt)

    def reactivate_prover(self) -> str:
        """Reactivate the caller's prover registration.

        Must have at least minStake. Returns tx hash.
        """
        receipt = self._send_tx(
            self._contract.functions.reactivate()
        )
        return self._tx_hash_hex(receipt)

    # ===================================================================
    # STAKING
    # ===================================================================

    def add_stake(self, amount: int) -> str:
        """Add more stake to the caller's registration.

        Args:
            amount: Amount of staking tokens to add.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.addStake(amount)
        )
        return self._tx_hash_hex(receipt)

    def withdraw_stake(self, amount: int) -> str:
        """Withdraw stake from the caller's registration.

        If active, remaining stake must be >= minStake.

        Args:
            amount: Amount of staking tokens to withdraw.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.withdrawStake(amount)
        )
        return self._tx_hash_hex(receipt)

    # ===================================================================
    # READ-ONLY METHODS
    # ===================================================================

    def get_prover_info(self, prover_address: str) -> ProverInfo:
        """Get full on-chain prover information.

        Args:
            prover_address: The prover address to query.

        Returns:
            ProverInfo with all prover details.
        """
        raw = self._contract.functions.getProver(
            Web3.to_checksum_address(prover_address)
        ).call()
        return ProverInfo(
            owner=raw[0],
            stake=raw[1],
            reputation=raw[2],
            proofs_submitted=raw[3],
            proofs_failed=raw[4],
            total_earnings=raw[5],
            registered_at=raw[6],
            last_active_at=raw[7],
            active=raw[8],
            endpoint=raw[9],
        )

    def is_prover_active(self, prover_address: str) -> bool:
        """Check if an address is an active prover.

        Args:
            prover_address: The address to check.

        Returns:
            True if the prover is registered and active.
        """
        return self._contract.functions.isActive(
            Web3.to_checksum_address(prover_address)
        ).call()

    def is_prover(self, prover_address: str) -> bool:
        """Check if an address is a registered prover (active or inactive).

        Args:
            prover_address: The address to check.

        Returns:
            True if the address is registered.
        """
        return self._contract.functions.isProver(
            Web3.to_checksum_address(prover_address)
        ).call()

    def get_active_provers(self) -> List[str]:
        """Get all active prover addresses.

        Returns:
            List of active prover addresses.
        """
        return self._contract.functions.getActiveProvers().call()

    def active_prover_count(self) -> int:
        """Get the number of active provers.

        Returns:
            Count of active provers.
        """
        return self._contract.functions.activeProverCount().call()

    def get_weight(self, prover_address: str) -> int:
        """Get the effective weight of a prover (stake * reputation / 10000).

        Args:
            prover_address: The prover address.

        Returns:
            The prover's weight.
        """
        return self._contract.functions.getWeight(
            Web3.to_checksum_address(prover_address)
        ).call()

    def select_prover(self, seed: int) -> str:
        """Select a prover using weighted random selection.

        Args:
            seed: Random seed (e.g., blockhash or VRF output).

        Returns:
            Selected prover address.
        """
        return self._contract.functions.selectProver(seed).call()

    def get_top_provers(self, n: int) -> List[str]:
        """Get the top N provers by reputation.

        Args:
            n: Number of provers to return.

        Returns:
            List of top prover addresses.
        """
        return self._contract.functions.getTopProvers(n).call()

    def staking_token(self) -> str:
        """Get the staking token address."""
        return self._contract.functions.stakingToken().call()

    def min_stake(self) -> int:
        """Get the minimum stake required."""
        return self._contract.functions.minStake().call()

    def slash_basis_points(self) -> int:
        """Get the slash percentage in basis points."""
        return self._contract.functions.slashBasisPoints().call()

    def total_staked(self) -> int:
        """Get the total amount staked across all provers."""
        return self._contract.functions.totalStaked().call()

    def owner(self) -> str:
        """Get the contract owner address."""
        return self._contract.functions.owner().call()

    # ===================================================================
    # SLASHING & REWARDS (authorized callers)
    # ===================================================================

    def slash(self, prover_address: str, reason: str) -> str:
        """Slash a prover for misbehavior (authorized slashers or owner only).

        Args:
            prover_address: The prover to slash.
            reason: Human-readable reason for the slash.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.slash(
                Web3.to_checksum_address(prover_address),
                reason,
            )
        )
        return self._tx_hash_hex(receipt)

    def record_success(self, prover_address: str, reward: int) -> str:
        """Record a successful proof and distribute reward (authorized callers).

        Args:
            prover_address: The prover that succeeded.
            reward: Reward amount earned.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.recordSuccess(
                Web3.to_checksum_address(prover_address),
                reward,
            )
        )
        return self._tx_hash_hex(receipt)

    # ===================================================================
    # ADMIN (onlyOwner)
    # ===================================================================

    def set_min_stake(self, min_stake: int) -> str:
        """Set the minimum stake requirement (owner only).

        Args:
            min_stake: New minimum stake in token units.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.setMinStake(min_stake)
        )
        return self._tx_hash_hex(receipt)

    def set_slash_basis_points(self, bps: int) -> str:
        """Set the slash percentage (owner only, max 50% = 5000 bps).

        Args:
            bps: New slash basis points.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.setSlashBasisPoints(bps)
        )
        return self._tx_hash_hex(receipt)

    def set_slasher(self, slasher_address: str, authorized: bool) -> str:
        """Authorize or deauthorize a slasher (owner only).

        Args:
            slasher_address: The address to authorize/deauthorize.
            authorized: True to authorize, False to deauthorize.

        Returns:
            Transaction hash as hex string.
        """
        receipt = self._send_tx(
            self._contract.functions.setSlasher(
                Web3.to_checksum_address(slasher_address),
                authorized,
            )
        )
        return self._tx_hash_hex(receipt)

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
