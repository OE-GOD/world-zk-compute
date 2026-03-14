"""
Pre-configured network definitions for World ZK Compute.

Example usage:
    from worldzk.networks import SEPOLIA

    verifier = TEEVerifier(
        rpc_url="https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY",
        private_key="0x...",
        contract_address=SEPOLIA.tee_verifier_address,
        chain_id=SEPOLIA.chain_id,
    )
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class NetworkConfig:
    """Configuration for a known network."""

    name: str
    chain_id: int
    verifier_router_address: str
    tee_verifier_address: str
    execution_engine_address: str
    explorer_url: str
    tee_only: bool


SEPOLIA = NetworkConfig(
    name="Ethereum Sepolia",
    chain_id=11155111,
    verifier_router_address="0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187",
    tee_verifier_address="0x0000000000000000000000000000000000000000",
    execution_engine_address="0x0000000000000000000000000000000000000000",
    explorer_url="https://sepolia.etherscan.io",
    tee_only=True,
)

ANVIL = NetworkConfig(
    name="Anvil (Local)",
    chain_id=31337,
    verifier_router_address="0x0000000000000000000000000000000000000000",
    tee_verifier_address="0x0000000000000000000000000000000000000000",
    execution_engine_address="0x0000000000000000000000000000000000000000",
    explorer_url="",
    tee_only=False,
)

NETWORKS = {
    SEPOLIA.chain_id: SEPOLIA,
    ANVIL.chain_id: ANVIL,
}
