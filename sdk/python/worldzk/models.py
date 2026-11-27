"""Data models for the World ZK Compute SDK."""
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Optional

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
    #: True when the generator runs in mock mode and no proof was computed.
    mock: bool = False
    #: ID under which the generator stored the proof in the registry, if it did.
    registry_proof_id: Optional[str] = None


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
    services: list[dict[str, Any]] = field(default_factory=list)
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
    dag_circuit_description: dict[str, Any]
    public_inputs_hex: str = ""
    model_hash: Optional[str] = None
    timestamp: Optional[int] = None
    prover_version: Optional[str] = None
    circuit_hash: Optional[str] = None

    def to_dict(self) -> dict[str, Any]:
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


# ---------------------------------------------------------------------------
# TEE indexer models (used by IndexerClient / AsyncIndexerClient)
# ---------------------------------------------------------------------------


class ResultStatus(str, Enum):
    """Inference result status (from the indexer)."""

    SUBMITTED = "submitted"
    CHALLENGED = "challenged"
    FINALIZED = "finalized"
    RESOLVED = "resolved"


@dataclass
class ResultRow:
    """An indexed inference result from the TEE indexer."""

    id: str
    model_hash: str
    input_hash: str
    output: str
    submitter: str
    status: ResultStatus
    block_number: int
    timestamp: int = 0
    challenger: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ResultRow":
        """Create from indexer API response."""
        return cls(
            id=data["id"],
            model_hash=data["model_hash"],
            input_hash=data["input_hash"],
            output=data.get("output", ""),
            submitter=data["submitter"],
            status=ResultStatus(data["status"]),
            block_number=data["block_number"],
            timestamp=data.get("timestamp", 0),
            challenger=data.get("challenger"),
        )

    @property
    def is_finalized(self) -> bool:
        """Check if the result has been finalized."""
        return self.status == ResultStatus.FINALIZED

    @property
    def is_challenged(self) -> bool:
        """Check if the result has been challenged."""
        return self.status == ResultStatus.CHALLENGED


@dataclass
class IndexerHealthResponse:
    """Health response from the TEE indexer."""

    status: str
    last_indexed_block: int
    total_results: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "IndexerHealthResponse":
        """Create from indexer API response."""
        return cls(
            status=data.get("status", "unknown"),
            last_indexed_block=data.get("last_indexed_block", 0),
            total_results=data.get("total_results", 0),
        )


@dataclass
class IndexerStatsResponse:
    """Aggregate statistics from the TEE indexer."""

    total_submitted: int
    total_challenged: int
    total_finalized: int
    total_resolved: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "IndexerStatsResponse":
        """Create from indexer API response."""
        return cls(
            total_submitted=data.get("total_submitted", 0),
            total_challenged=data.get("total_challenged", 0),
            total_finalized=data.get("total_finalized", 0),
            total_resolved=data.get("total_resolved", 0),
        )

    @property
    def total(self) -> int:
        """Total number of results across all statuses."""
        return (
            self.total_submitted
            + self.total_challenged
            + self.total_finalized
            + self.total_resolved
        )
