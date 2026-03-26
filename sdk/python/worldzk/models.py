"""Data models for the World ZK Compute SDK."""
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


# ---------------------------------------------------------------------------
# Gateway / WorldZK models
# ---------------------------------------------------------------------------


@dataclass
class ProofResult:
    """Result returned by ``WorldZK.prove()``."""

    proof_id: str
    model_hash: str
    circuit_hash: str
    output: str


@dataclass
class VerificationResult:
    """Result returned by ``WorldZK.verify()``."""

    verified: bool
    receipt_id: Optional[str] = None


@dataclass
class Receipt:
    """Verification receipt returned by ``WorldZK.get_receipt()``."""

    receipt_id: str
    proof_id: str
    verified: bool
    verified_at: str
    signature: str


@dataclass
class GatewayHealth:
    """Aggregated health response from the gateway.

    Returned by ``WorldZK.health()``.
    """

    status: str
    services: List[Dict[str, Any]] = field(default_factory=list)
    healthy_count: int = 0
    total_count: int = 0


# ---------------------------------------------------------------------------
# Legacy direct-service models (used by Client)
# ---------------------------------------------------------------------------


@dataclass
class ProofBundle:
    """A self-contained proof bundle."""

    proof_hex: str
    gens_hex: str
    dag_circuit_description: Dict[str, Any]
    public_inputs_hex: str = ""
    model_hash: Optional[str] = None
    timestamp: Optional[int] = None
    prover_version: Optional[str] = None
    circuit_hash: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        d = {
            "proof_hex": self.proof_hex,
            "gens_hex": self.gens_hex,
            "dag_circuit_description": self.dag_circuit_description,
        }
        if self.public_inputs_hex:
            d["public_inputs_hex"] = self.public_inputs_hex
        for key in ("model_hash", "timestamp", "prover_version", "circuit_hash"):
            val = getattr(self, key)
            if val is not None:
                d[key] = val
        return d


@dataclass
class ProofReceipt:
    """Receipt from the proof registry (legacy)."""

    id: str
    content_hash: str
    submitted_at: int = 0
    verified: Optional[bool] = None
