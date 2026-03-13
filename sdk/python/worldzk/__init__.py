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

from .client import Client, AsyncClient, ResultRow, StatsResponse, HealthResponse
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
from .hash_utils import (
    compute_model_hash,
    compute_model_hash_from_bytes,
    compute_input_hash,
    compute_input_hash_from_json,
    compute_result_hash,
    compute_result_hash_from_bytes,
)
from .xgboost import XGBoostConverter, XGBoostModel, ModelParseError
from .lightgbm import LightGBMConverter

try:
    from .verifier import (
        BatchVerifier,
        BatchVerifyInput,
        BatchVerifyResult,
        BatchSession,
        StepResult,
        ProgressEvent,
    )
    from .batch_verifier import (
        BatchVerifier as StepBatchVerifier,
        BatchVerificationCancelledError,
        ProgressTracker,
    )
    from .tee_verifier import TEEVerifier, MLResult
    from .event_watcher import (
        TEEEventWatcher,
        TEEEventType,
        TEEEvent,
        ResultSubmitted as EventResultSubmitted,
        ResultChallenged as EventResultChallenged,
        ResultFinalized as EventResultFinalized,
        ResultExpired as EventResultExpired,
        DisputeResolved as EventDisputeResolved,
        EnclaveRegistered as EventEnclaveRegistered,
        EnclaveRevoked as EventEnclaveRevoked,
        parse_log,
        topic_hash,
    )
    from .async_client import AsyncTEEVerifier, AsyncEventWatcher
except ImportError:
    pass  # web3 not installed

__version__ = "1.0.0"
__all__ = [
    # Client
    "Client",
    "AsyncClient",
    "ResultRow",
    "StatsResponse",
    "HealthResponse",
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
    # XGBoost
    "XGBoostConverter",
    "XGBoostModel",
    "ModelParseError",
    # LightGBM
    "LightGBMConverter",
    # Batch Verifier (requires web3 optional dependency)
    "BatchVerifier",
    "BatchVerifyInput",
    "BatchVerifyResult",
    "BatchSession",
    "StepResult",
    "ProgressEvent",
    # Step-level Batch Verifier (requires web3 optional dependency)
    "StepBatchVerifier",
    "BatchVerificationCancelledError",
    "ProgressTracker",
    # TEE Verifier (requires web3 optional dependency)
    "TEEVerifier",
    "MLResult",
    # TEE Event Watcher (requires web3 optional dependency)
    "TEEEventWatcher",
    "TEEEventType",
    "TEEEvent",
    "EventResultSubmitted",
    "EventResultChallenged",
    "EventResultFinalized",
    "EventResultExpired",
    "EventDisputeResolved",
    "EventEnclaveRegistered",
    "EventEnclaveRevoked",
    "parse_log",
    "topic_hash",
    # Async TEE Verifier & Event Watcher (requires web3 optional dependency)
    "AsyncTEEVerifier",
    "AsyncEventWatcher",
    # Hash utilities
    "compute_model_hash",
    "compute_model_hash_from_bytes",
    "compute_input_hash",
    "compute_input_hash_from_json",
    "compute_result_hash",
    "compute_result_hash_from_bytes",
]
