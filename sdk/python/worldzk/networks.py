"""
Pre-configured network definitions for World ZK Compute.

Example usage:
    from worldzk.networks import NETWORKS, SEPOLIA

    config = NETWORKS[11155111]  # same object as SEPOLIA
    print(config.name, config.chain_id, config.verifier_router_address)

An address left at the zero address has no deployment recorded here; pass the
contract address to the client explicitly in that case.
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
