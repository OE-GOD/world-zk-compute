"""Sepolia integration tests using raw HTTP RPC calls.

Skipped unless both ALCHEMY_SEPOLIA_RPC_URL and TEE_VERIFIER_ADDRESS env vars are set.

To run:
    ALCHEMY_SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
    TEE_VERIFIER_ADDRESS=0x... \
        python -m pytest tests/test_sepolia_integration.py -v

These tests verify basic RPC connectivity and contract state on the Sepolia
testnet without importing any SDK modules. They use only the ``requests``
library for HTTP JSON-RPC calls.
"""

from __future__ import annotations

import os

import pytest
import requests

# ---------------------------------------------------------------------------
# Environment gate -- skip entire module when env vars are missing
# ---------------------------------------------------------------------------

ALCHEMY_SEPOLIA_RPC_URL = os.environ.get("ALCHEMY_SEPOLIA_RPC_URL", "")
TEE_VERIFIER_ADDRESS = os.environ.get("TEE_VERIFIER_ADDRESS", "")

pytestmark = pytest.mark.skipif(
    not (ALCHEMY_SEPOLIA_RPC_URL and TEE_VERIFIER_ADDRESS),
    reason="Requires ALCHEMY_SEPOLIA_RPC_URL and TEE_VERIFIER_ADDRESS env vars",
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _rpc_call(method: str, params: list | None = None) -> dict:
    """Send a JSON-RPC request to the Sepolia endpoint and return the body."""
    payload = {
        "jsonrpc": "2.0",
        "method": method,
        "params": params or [],
        "id": 1,
    }
    resp = requests.post(ALCHEMY_SEPOLIA_RPC_URL, json=payload, timeout=30)
    resp.raise_for_status()
    return resp.json()


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestSepoliaIntegration:
    """Raw JSON-RPC integration tests against a live Sepolia node."""

    def test_sepolia_rpc_connectivity(self) -> None:
        """POST eth_blockNumber and assert the returned block is positive."""
        body = _rpc_call("eth_blockNumber")

        assert "result" in body, f"RPC response missing 'result': {body}"
        block_hex = body["result"]
        block_number = int(block_hex, 16)
        assert block_number > 0, f"Block number should be positive, got {block_number}"

    def test_sepolia_contract_has_code(self) -> None:
        """POST eth_getCode for TEE_VERIFIER_ADDRESS and assert code length > 2.

        An address with no deployed bytecode returns ``"0x"`` (length 2).
        Any deployed contract will have ``"0x<bytecode>"`` with length > 2.
        """
        body = _rpc_call("eth_getCode", [TEE_VERIFIER_ADDRESS, "latest"])

        assert "result" in body, f"RPC response missing 'result': {body}"
        code = body["result"]
        assert isinstance(code, str), f"Expected hex string, got {type(code)}"
        assert len(code) > 2, (
            f"No bytecode at {TEE_VERIFIER_ADDRESS} (got '{code}'). "
            "Is the contract deployed on Sepolia?"
        )

    def test_sepolia_contract_owner(self) -> None:
        """Call owner() on TEE_VERIFIER_ADDRESS and assert a non-zero address.

        The function selector for ``owner()`` is ``0x8da5cb5b``.
        The return value is a 32-byte ABI-encoded address (last 20 bytes).
        """
        # owner() selector = keccak256("owner()")[:4] = 0x8da5cb5b
        body = _rpc_call(
            "eth_call",
            [
                {
                    "to": TEE_VERIFIER_ADDRESS,
                    "data": "0x8da5cb5b",
                },
                "latest",
            ],
        )

        assert "result" in body, f"RPC response missing 'result': {body}"
        raw = body["result"]
        # Result is 0x + 64 hex chars (32 bytes, ABI-encoded address)
        assert len(raw) == 66, f"Unexpected return length: {raw}"

        # Extract the address from the last 40 hex characters
        owner_address = "0x" + raw[-40:]
        zero_address = "0x" + "0" * 40
        assert owner_address != zero_address, (
            f"owner() returned zero address; expected a real admin address"
        )
