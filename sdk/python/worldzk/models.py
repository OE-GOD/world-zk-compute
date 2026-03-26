"""Data models for the World ZK Compute SDK."""
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


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
class VerificationResult:
    """Result of proof verification."""
    verified: bool
    circuit_hash: str = ""
    error: Optional[str] = None


@dataclass
class ProofReceipt:
    """Receipt from the proof registry."""
    id: str
    content_hash: str
    submitted_at: int = 0
    verified: Optional[bool] = None
