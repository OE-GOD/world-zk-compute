"""Tests for the ProverRegistry SDK client (no Anvil required)."""

from unittest.mock import MagicMock, patch

import pytest

from worldzk.prover_registry import (
    PROVER_REGISTRY_ABI,
    ProverInfo,
    ProverRegistryClient,
)


# ===========================================================================
# ProverInfo dataclass tests
# ===========================================================================


class TestProverInfo:
    """Tests for the ProverInfo dataclass."""

    def _make_info(
        self,
        active: bool = True,
        reputation: int = 5000,
        proofs_submitted: int = 10,
        proofs_failed: int = 1,
        registered_at: int = 1700000000,
    ) -> ProverInfo:
        return ProverInfo(
            owner="0x" + "11" * 20,
            stake=1_000_000_000_000_000_000,
            reputation=reputation,
            proofs_submitted=proofs_submitted,
            proofs_failed=proofs_failed,
            total_earnings=500_000_000_000_000,
            registered_at=registered_at,
            last_active_at=1700001000,
            active=active,
            endpoint="https://prover.example.com:8080",
        )

    def test_basic_fields(self):
        info = self._make_info()
        assert info.owner == "0x" + "11" * 20
        assert info.stake == 1_000_000_000_000_000_000
        assert info.active is True
        assert info.endpoint == "https://prover.example.com:8080"

    def test_reputation_percent(self):
        info = self._make_info(reputation=5000)
        assert info.reputation_percent == 50.0

    def test_reputation_percent_full(self):
        info = self._make_info(reputation=10000)
        assert info.reputation_percent == 100.0

    def test_reputation_percent_zero(self):
        info = self._make_info(reputation=0)
        assert info.reputation_percent == 0.0

    def test_is_registered(self):
        info = self._make_info(registered_at=1700000000)
        assert info.is_registered is True

    def test_is_not_registered(self):
        info = self._make_info(registered_at=0)
        assert info.is_registered is False

    def test_success_rate(self):
        info = self._make_info(proofs_submitted=9, proofs_failed=1)
        assert info.success_rate == 0.9

    def test_success_rate_no_proofs(self):
        info = self._make_info(proofs_submitted=0, proofs_failed=0)
        assert info.success_rate == 0.0

    def test_success_rate_all_success(self):
        info = self._make_info(proofs_submitted=100, proofs_failed=0)
        assert info.success_rate == 1.0

    def test_inactive_prover(self):
        info = self._make_info(active=False)
        assert info.active is False


# ===========================================================================
# ABI structure tests
# ===========================================================================


class TestABI:
    """Tests for the ABI definition."""

    def test_abi_has_all_functions(self):
        fn_names = {e["name"] for e in PROVER_REGISTRY_ABI if e["type"] == "function"}
        expected = {
            "register",
            "addStake",
            "withdrawStake",
            "deactivate",
            "reactivate",
            "slash",
            "recordSuccess",
            "setMinStake",
            "setSlashBasisPoints",
            "setSlasher",
            "getProver",
            "activeProverCount",
            "getActiveProvers",
            "isProver",
            "isActive",
            "getWeight",
            "selectProver",
            "getTopProvers",
            "stakingToken",
            "minStake",
            "slashBasisPoints",
            "totalStaked",
            "owner",
        }
        assert fn_names == expected

    def test_abi_has_events(self):
        event_names = {e["name"] for e in PROVER_REGISTRY_ABI if e["type"] == "event"}
        expected = {
            "ProverRegistered",
            "ProverDeactivated",
            "ProverReactivated",
            "StakeAdded",
            "StakeWithdrawn",
            "ProverSlashed",
            "ReputationUpdated",
            "RewardDistributed",
            "SlasherUpdated",
        }
        assert event_names == expected

    def test_register_is_nonpayable(self):
        fn = next(
            e for e in PROVER_REGISTRY_ABI
            if e.get("name") == "register"
        )
        assert fn["stateMutability"] == "nonpayable"

    def test_get_prover_returns_tuple(self):
        fn = next(
            e for e in PROVER_REGISTRY_ABI
            if e.get("name") == "getProver"
        )
        assert fn["outputs"][0]["type"] == "tuple"
        components = fn["outputs"][0]["components"]
        assert len(components) == 10
        field_names = [c["name"] for c in components]
        assert "owner" in field_names
        assert "stake" in field_names
        assert "reputation" in field_names
        assert "active" in field_names
        assert "endpoint" in field_names

    def test_view_functions(self):
        views = [
            e for e in PROVER_REGISTRY_ABI
            if e.get("stateMutability") == "view"
        ]
        view_names = {e["name"] for e in views}
        assert "getProver" in view_names
        assert "isActive" in view_names
        assert "isProver" in view_names
        assert "getActiveProvers" in view_names
        assert "activeProverCount" in view_names
        assert "minStake" in view_names
        assert "totalStaked" in view_names
        assert "owner" in view_names


# ===========================================================================
# ProverRegistryClient tests (mocked web3)
# ===========================================================================


def _make_mock_client(with_key: bool = True) -> ProverRegistryClient:
    """Create a ProverRegistryClient with mocked web3 internals."""
    with patch("worldzk.prover_registry.Web3") as MockWeb3, \
         patch("worldzk.prover_registry.Account") as MockAccount:

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
            client = ProverRegistryClient(
                rpc_url="http://localhost:8545",
                contract_address="0x" + "aa" * 20,
                private_key="0x" + "cc" * 32,
            )
            client._account = mock_account
        else:
            client = ProverRegistryClient.__new__(ProverRegistryClient)
            client._w3 = mock_w3_instance
            client._contract = mock_contract
            client._account = None
            client._gas_limit = 500_000

        client._w3 = mock_w3_instance
        client._contract = mock_contract

    return client


class TestProverRegistryClientInit:
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


class TestRegisterProver:
    """Tests for register_prover method."""

    def test_register_returns_tx_hash(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\xab" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.register_prover(
            stake=1_000_000_000_000_000_000,
            endpoint="https://prover.example.com",
        )
        assert tx_hash.startswith("0x")
        assert len(tx_hash) == 66
        client._send_tx.assert_called_once()

    def test_register_no_signer_raises(self):
        client = _make_mock_client(with_key=False)
        with pytest.raises(ValueError, match="Private key required"):
            client.register_prover(stake=1000)


class TestDeregisterProver:
    """Tests for deregister_prover method."""

    def test_deregister_returns_tx_hash(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\xde\xad" + b"\x00" * 30}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.deregister_prover()
        assert tx_hash.startswith("0x")
        client._send_tx.assert_called_once()

    def test_deregister_no_signer_raises(self):
        client = _make_mock_client(with_key=False)
        with pytest.raises(ValueError, match="Private key required"):
            client.deregister_prover()


class TestGetProverInfo:
    """Tests for get_prover_info method."""

    def test_returns_prover_info(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = (
            "0x" + "11" * 20,       # owner
            1_000_000_000_000_000_000,  # stake
            5000,                    # reputation
            10,                      # proofsSubmitted
            1,                       # proofsFailed
            500_000_000_000_000,     # totalEarnings
            1700000000,              # registeredAt
            1700001000,              # lastActiveAt
            True,                    # active
            "https://prover.example.com:8080",  # endpoint
        )
        client._contract.functions.getProver.return_value = mock_fn

        info = client.get_prover_info("0x" + "11" * 20)

        assert isinstance(info, ProverInfo)
        assert info.owner == "0x" + "11" * 20
        assert info.stake == 1_000_000_000_000_000_000
        assert info.reputation == 5000
        assert info.proofs_submitted == 10
        assert info.proofs_failed == 1
        assert info.active is True
        assert info.endpoint == "https://prover.example.com:8080"
        assert info.reputation_percent == 50.0

    def test_returns_inactive_prover(self):
        client = _make_mock_client(with_key=False)

        mock_fn = MagicMock()
        mock_fn.call.return_value = (
            "0x" + "22" * 20,
            0,     # stake
            2500,  # reputation
            5,     # proofsSubmitted
            3,     # proofsFailed
            0,     # totalEarnings
            1600000000,
            1600000500,
            False,  # active
            "",     # endpoint
        )
        client._contract.functions.getProver.return_value = mock_fn

        info = client.get_prover_info("0x" + "22" * 20)
        assert info.active is False
        assert info.stake == 0
        assert info.reputation_percent == 25.0


class TestIsProverActive:
    """Tests for is_prover_active method."""

    def test_active_prover(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = True
        client._contract.functions.isActive.return_value = mock_fn

        assert client.is_prover_active("0x" + "11" * 20) is True

    def test_inactive_prover(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = False
        client._contract.functions.isActive.return_value = mock_fn

        assert client.is_prover_active("0x" + "11" * 20) is False


class TestGetActiveProvers:
    """Tests for get_active_provers method."""

    def test_returns_list(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        addresses = ["0x" + "11" * 20, "0x" + "22" * 20, "0x" + "33" * 20]
        mock_fn.call.return_value = addresses
        client._contract.functions.getActiveProvers.return_value = mock_fn

        result = client.get_active_provers()
        assert result == addresses

    def test_returns_empty_list(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = []
        client._contract.functions.getActiveProvers.return_value = mock_fn

        result = client.get_active_provers()
        assert result == []


class TestReadOnlyMethods:
    """Tests for other read-only methods."""

    def test_active_prover_count(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 5
        client._contract.functions.activeProverCount.return_value = mock_fn

        assert client.active_prover_count() == 5

    def test_min_stake(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 1_000_000_000_000_000_000
        client._contract.functions.minStake.return_value = mock_fn

        assert client.min_stake() == 1_000_000_000_000_000_000

    def test_total_staked(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 10_000_000_000_000_000_000
        client._contract.functions.totalStaked.return_value = mock_fn

        assert client.total_staked() == 10_000_000_000_000_000_000

    def test_slash_basis_points(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 500
        client._contract.functions.slashBasisPoints.return_value = mock_fn

        assert client.slash_basis_points() == 500

    def test_owner(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = "0x" + "dd" * 20
        client._contract.functions.owner.return_value = mock_fn

        assert client.owner() == "0x" + "dd" * 20

    def test_is_prover(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = True
        client._contract.functions.isProver.return_value = mock_fn

        assert client.is_prover("0x" + "11" * 20) is True

    def test_get_weight(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = 500_000_000_000_000_000
        client._contract.functions.getWeight.return_value = mock_fn

        assert client.get_weight("0x" + "11" * 20) == 500_000_000_000_000_000

    def test_select_prover(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = "0x" + "22" * 20
        client._contract.functions.selectProver.return_value = mock_fn

        assert client.select_prover(42) == "0x" + "22" * 20

    def test_get_top_provers(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = ["0x" + "11" * 20, "0x" + "22" * 20]
        client._contract.functions.getTopProvers.return_value = mock_fn

        result = client.get_top_provers(2)
        assert len(result) == 2

    def test_staking_token(self):
        client = _make_mock_client(with_key=False)
        mock_fn = MagicMock()
        mock_fn.call.return_value = "0x" + "ee" * 20
        client._contract.functions.stakingToken.return_value = mock_fn

        assert client.staking_token() == "0x" + "ee" * 20


class TestStakingMethods:
    """Tests for staking methods."""

    def test_add_stake(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.add_stake(500_000_000_000_000_000)
        assert tx_hash.startswith("0x")
        client._send_tx.assert_called_once()

    def test_withdraw_stake(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.withdraw_stake(100_000_000_000_000_000)
        assert tx_hash.startswith("0x")
        client._send_tx.assert_called_once()


class TestAdminMethods:
    """Tests for admin methods."""

    def test_set_min_stake(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.set_min_stake(2_000_000_000_000_000_000)
        assert tx_hash.startswith("0x")

    def test_set_slash_basis_points(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.set_slash_basis_points(1000)
        assert tx_hash.startswith("0x")

    def test_set_slasher(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.set_slasher("0x" + "ff" * 20, True)
        assert tx_hash.startswith("0x")


class TestSlashingMethods:
    """Tests for slashing and rewards."""

    def test_slash(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.slash("0x" + "11" * 20, "invalid proof")
        assert tx_hash.startswith("0x")

    def test_record_success(self):
        client = _make_mock_client()
        mock_receipt = {"transactionHash": b"\x00" * 32}
        client._send_tx = MagicMock(return_value=mock_receipt)

        tx_hash = client.record_success("0x" + "11" * 20, 100_000_000_000_000)
        assert tx_hash.startswith("0x")


class TestTxHashHex:
    """Tests for _tx_hash_hex static method."""

    def test_bytes_hash(self):
        receipt = {"transactionHash": b"\xde\xad\xbe\xef" + b"\x00" * 28}
        result = ProverRegistryClient._tx_hash_hex(receipt)
        assert result.startswith("0x")
        assert result == "0x" + (b"\xde\xad\xbe\xef" + b"\x00" * 28).hex()

    def test_string_hash(self):
        receipt = {"transactionHash": "0xabcdef"}
        result = ProverRegistryClient._tx_hash_hex(receipt)
        assert result == "0xabcdef"
