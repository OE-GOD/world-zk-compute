"""
World ZK Compute SDK - Data models
"""

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional
import hashlib


class RequestStatus(str, Enum):
    """Execution request status."""

    PENDING = "pending"
    CLAIMED = "claimed"
    COMPLETED = "completed"
    EXPIRED = "expired"
    CANCELLED = "cancelled"


class ReputationTier(str, Enum):
    """Prover reputation tier."""

    UNRANKED = "unranked"
    BRONZE = "bronze"
    SILVER = "silver"
    GOLD = "gold"
    PLATINUM = "platinum"
    DIAMOND = "diamond"


class ProofType(str, Enum):
    """ZK proof type."""

    GROTH16 = "groth16"
    SUCCINCT = "succinct"
    FAKE = "fake"


class HealthStatus(str, Enum):
    """Service health status."""

    HEALTHY = "healthy"
    DEGRADED = "degraded"
    UNHEALTHY = "unhealthy"


@dataclass
class ProofResult:
    """ZK proof result."""

    seal: bytes
    journal: bytes
    proof_type: ProofType = ProofType.GROTH16
    image_id: Optional[str] = None
    verified: bool = False
    verified_at: Optional[datetime] = None

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "ProofResult":
        """Create from API response."""
        import base64

        return cls(
            seal=base64.b64decode(data.get("seal", "")),
            journal=base64.b64decode(data.get("journal", "")),
            proof_type=ProofType(data.get("proof_type", "groth16")),
            image_id=data.get("image_id"),
            verified=data.get("verified", False),
            verified_at=_parse_datetime(data.get("verified_at")),
        )

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dict for API request."""
        import base64

        return {
            "seal": base64.b64encode(self.seal).decode(),
            "journal": base64.b64encode(self.journal).decode(),
            "proof_type": self.proof_type.value,
        }

    @property
    def journal_hash(self) -> str:
        """Get SHA256 hash of journal."""
        return "0x" + hashlib.sha256(self.journal).hexdigest()


@dataclass
class ExecutionRequest:
    """Execution request."""

    id: int
    requester: str
    image_id: str
    input_hash: str
    status: RequestStatus
    created_at: datetime
    input_url: Optional[str] = None
    callback_contract: Optional[str] = None
    tip: int = 0
    claimed_by: Optional[str] = None
    claim_deadline: Optional[datetime] = None
    expires_at: Optional[datetime] = None
    completed_at: Optional[datetime] = None
    proof: Optional[ProofResult] = None

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "ExecutionRequest":
        """Create from API response."""
        proof = None
        if proof_data := data.get("proof"):
            proof = ProofResult.from_dict(proof_data)

        return cls(
            id=data["id"],
            requester=data["requester"],
            image_id=data["image_id"],
            input_hash=data["input_hash"],
            status=RequestStatus(data["status"]),
            created_at=_parse_datetime(data["created_at"]) or datetime.now(),
            input_url=data.get("input_url"),
            callback_contract=data.get("callback_contract"),
            tip=int(data.get("tip", 0)),
            claimed_by=data.get("claimed_by"),
            claim_deadline=_parse_datetime(data.get("claim_deadline")),
            expires_at=_parse_datetime(data.get("expires_at")),
            completed_at=_parse_datetime(data.get("completed_at")),
            proof=proof,
        )

    @property
    def is_pending(self) -> bool:
        """Check if request is pending."""
        return self.status == RequestStatus.PENDING

    @property
    def is_completed(self) -> bool:
        """Check if request is completed."""
        return self.status == RequestStatus.COMPLETED

    @property
    def is_terminal(self) -> bool:
        """Check if request is in a terminal state."""
        return self.status in {
            RequestStatus.COMPLETED,
            RequestStatus.EXPIRED,
            RequestStatus.CANCELLED,
        }


@dataclass
class Program:
    """Registered program."""

    image_id: str
    name: str
    owner: str
    is_active: bool
    description: Optional[str] = None
    source_url: Optional[str] = None
    registered_at: Optional[datetime] = None
    execution_count: int = 0

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Program":
        """Create from API response."""
        return cls(
            image_id=data["image_id"],
            name=data.get("name", ""),
            owner=data.get("owner", ""),
            is_active=data.get("is_active", True),
            description=data.get("description"),
            source_url=data.get("source_url"),
            registered_at=_parse_datetime(data.get("registered_at")),
            execution_count=data.get("execution_count", 0),
        )


@dataclass
class Reputation:
    """Prover reputation."""

    score: int  # 0-10000 basis points
    tier: ReputationTier
    is_slashed: bool = False
    is_banned: bool = False

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Reputation":
        """Create from API response."""
        return cls(
            score=data.get("score", 5000),
            tier=ReputationTier(data.get("tier", "unranked")),
            is_slashed=data.get("is_slashed", False),
            is_banned=data.get("is_banned", False),
        )

    @property
    def score_percent(self) -> float:
        """Get score as percentage."""
        return self.score / 100.0

    @property
    def is_good_standing(self) -> bool:
        """Check if in good standing."""
        return not self.is_banned and self.score >= 5000


@dataclass
class ProverStats:
    """Prover statistics."""

    total_jobs: int = 0
    completed_jobs: int = 0
    failed_jobs: int = 0
    abandoned_jobs: int = 0
    avg_proof_time_ms: int = 0
    total_earnings: int = 0  # in wei

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "ProverStats":
        """Create from API response."""
        return cls(
            total_jobs=data.get("total_jobs", 0),
            completed_jobs=data.get("completed_jobs", 0),
            failed_jobs=data.get("failed_jobs", 0),
            abandoned_jobs=data.get("abandoned_jobs", 0),
            avg_proof_time_ms=data.get("avg_proof_time_ms", 0),
            total_earnings=int(data.get("total_earnings", 0)),
        )

    @property
    def success_rate(self) -> float:
        """Get success rate as percentage."""
        if self.total_jobs == 0:
            return 0.0
        return (self.completed_jobs / self.total_jobs) * 100.0

    @property
    def avg_proof_time_seconds(self) -> float:
        """Get average proof time in seconds."""
        return self.avg_proof_time_ms / 1000.0


@dataclass
class Prover:
    """Prover information."""

    address: str
    reputation: Reputation
    stats: ProverStats
    registered_at: Optional[datetime] = None
    last_active_at: Optional[datetime] = None

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Prover":
        """Create from API response."""
        return cls(
            address=data["address"],
            reputation=Reputation.from_dict(data.get("reputation", {})),
            stats=ProverStats.from_dict(data.get("stats", {})),
            registered_at=_parse_datetime(data.get("registered_at")),
            last_active_at=_parse_datetime(data.get("last_active_at")),
        )


@dataclass
class HealthCheck:
    """Individual health check result."""

    status: str  # pass, warn, fail
    message: Optional[str] = None


@dataclass
class HealthResponse:
    """Service health response."""

    status: HealthStatus
    timestamp: datetime
    version: Optional[str] = None
    checks: Dict[str, HealthCheck] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "HealthResponse":
        """Create from API response."""
        checks = {}
        for name, check_data in data.get("checks", {}).items():
            checks[name] = HealthCheck(
                status=check_data.get("status", "fail"),
                message=check_data.get("message"),
            )

        return cls(
            status=HealthStatus(data.get("status", "unhealthy")),
            timestamp=_parse_datetime(data.get("timestamp")) or datetime.now(),
            version=data.get("version"),
            checks=checks,
        )


@dataclass
class StatusResponse:
    """Service status response."""

    status: str
    queue_depth: int = 0
    active_jobs: int = 0
    total_completed: int = 0
    uptime_seconds: int = 0
    prover_count: int = 0

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "StatusResponse":
        """Create from API response."""
        return cls(
            status=data.get("status", "unknown"),
            queue_depth=data.get("queue_depth", 0),
            active_jobs=data.get("active_jobs", 0),
            total_completed=data.get("total_completed", 0),
            uptime_seconds=data.get("uptime_seconds", 0),
            prover_count=data.get("prover_count", 0),
        )


@dataclass
class Pagination:
    """Pagination info."""

    total: int
    limit: int
    offset: int
    has_more: bool

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "Pagination":
        """Create from API response."""
        return cls(
            total=data.get("total", 0),
            limit=data.get("limit", 20),
            offset=data.get("offset", 0),
            has_more=data.get("has_more", False),
        )


@dataclass
class PaginatedList:
    """Paginated list response."""

    items: List[Any]
    pagination: Pagination


def _parse_datetime(value: Optional[str]) -> Optional[datetime]:
    """Parse ISO datetime string."""
    if not value:
        return None
    try:
        # Handle ISO format with or without timezone
        if value.endswith("Z"):
            value = value[:-1] + "+00:00"
        return datetime.fromisoformat(value)
    except (ValueError, TypeError):
        return None
