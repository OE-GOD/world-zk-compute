"""Sepolia integration tests for the Python SDK.

Skipped unless SEPOLIA_RPC_URL is set.

To run:
    SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
        python -m pytest tests/test_sepolia.py -v
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Skip the entire module unless SEPOLIA_RPC_URL is set
# ---------------------------------------------------------------------------

SEPOLIA_RPC_URL = os.environ.get("SEPOLIA_RPC_URL", "")

try:
    from web3 import Web3

    _HAS_WEB3 = True
except ImportError:
    _HAS_WEB3 = False

pytestmark = pytest.mark.skipif(
    not (SEPOLIA_RPC_URL and _HAS_WEB3),
    reason="Requires SEPOLIA_RPC_URL env var and web3 package",
)

# ---------------------------------------------------------------------------
# Load deployment addresses
# ---------------------------------------------------------------------------

DEPLOY_FILE = Path(__file__).resolve().parents[3] / "deployments" / "11155111.json"


def _load_contract_address(name: str) -> str | None:
    if not DEPLOY_FILE.exists():
        return None
    try:
        data = json.loads(DEPLOY_FILE.read_text())
        return data.get("contracts", {}).get(name, {}).get("address")
    except (json.JSONDecodeError, KeyError):
        return None


TEE_VERIFIER_ADDRESS = os.environ.get(
    "TEE_VERIFIER_ADDRESS", _load_contract_address("TEEMLVerifier") or ""
)


class TestSepoliaConnection:
    """Basic Sepolia connectivity tests."""

    def test_connect_to_sepolia(self) -> None:
        w3 = Web3(Web3.HTTPProvider(SEPOLIA_RPC_URL))
        assert w3.is_connected()

    def test_chain_id(self) -> None:
        w3 = Web3(Web3.HTTPProvider(SEPOLIA_RPC_URL))
        chain_id = w3.eth.chain_id
        assert chain_id == 11155111

    def test_get_block_number(self) -> None:
        w3 = Web3(Web3.HTTPProvider(SEPOLIA_RPC_URL))
        block = w3.eth.block_number
        assert block > 0


@pytest.mark.skipif(
    not TEE_VERIFIER_ADDRESS,
    reason="TEE_VERIFIER_ADDRESS not set and not found in deployment file",
)
class TestSepoliaContract:
    """Tests that verify contract deployment on Sepolia."""

    def test_contract_code_exists(self) -> None:
        w3 = Web3(Web3.HTTPProvider(SEPOLIA_RPC_URL))
        code = w3.eth.get_code(Web3.to_checksum_address(TEE_VERIFIER_ADDRESS))
        assert len(code) > 0, "No code at TEEMLVerifier address"

    def test_contract_is_not_paused(self) -> None:
        """Verify the contract is operational (not paused)."""
        w3 = Web3(Web3.HTTPProvider(SEPOLIA_RPC_URL))
        # paused() selector = 0x5c975abb
        result = w3.eth.call(
            {
                "to": Web3.to_checksum_address(TEE_VERIFIER_ADDRESS),
                "data": "0x5c975abb",
            }
        )
        # result is 32 bytes, last byte 0 = not paused
        paused = int.from_bytes(result, "big")
        assert paused == 0, "Contract is paused"


class TestSepoliaSDKImports:
    """Verify SDK modules can be imported and used."""

    def test_import_verifier(self) -> None:
        from worldzk.verifier import TEEVerifier

        assert TEEVerifier is not None

    def test_import_tee_verifier(self) -> None:
        from worldzk.tee_verifier import TEEVerifierClient

        assert TEEVerifierClient is not None

    def test_import_xgboost_converter(self) -> None:
        from worldzk.xgboost import XGBoostConverter

        assert XGBoostConverter is not None

    def test_import_lightgbm_converter(self) -> None:
        from worldzk.lightgbm import LightGBMConverter

        assert LightGBMConverter is not None

    def test_import_event_watcher(self) -> None:
        from worldzk.event_watcher import TEEEventWatcher

        assert TEEEventWatcher is not None
