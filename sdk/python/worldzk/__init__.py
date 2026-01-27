"""
World ZK Compute Python SDK

A Python client library for the World ZK Compute decentralized proving network.

Example usage:
    from worldzk import Client

    client = Client(api_key="your-api-key")

    # Submit a computation request
    request = client.requests.create(
        image_id="0x1234...",
        input_data=b"hello world",
    )

    # Wait for completion
    result = client.requests.wait(request.id)
    print(f"Proof: {result.proof}")
"""

from .client import Client, AsyncClient
from .models import (
    ExecutionRequest,
    RequestStatus,
    Program,
    Prover,
    Reputation,
    ReputationTier,
    ProverStats,
    ProofResult,
    HealthStatus,
)
from .errors import (
    WorldZKError,
    ApiError,
    ErrorCode,
    ValidationError,
    AuthenticationError,
    NotFoundError,
    RateLimitError,
    ProofError,
    NetworkError,
)

__version__ = "1.0.0"
__all__ = [
    # Client
    "Client",
    "AsyncClient",
    # Models
    "ExecutionRequest",
    "RequestStatus",
    "Program",
    "Prover",
    "Reputation",
    "ReputationTier",
    "ProverStats",
    "ProofResult",
    "HealthStatus",
    # Errors
    "WorldZKError",
    "ApiError",
    "ErrorCode",
    "ValidationError",
    "AuthenticationError",
    "NotFoundError",
    "RateLimitError",
    "ProofError",
    "NetworkError",
]
