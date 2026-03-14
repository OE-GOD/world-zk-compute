"""
World ZK Compute Python SDK

A Python client library for the World ZK Compute decentralized proving network.

Example usage:
    from worldzk import Client

    client = Client()  # defaults to http://localhost:8081

    # Check indexer health
    health = client.health()
    print(f"Status: {health.status}, block: {health.last_indexed_block}")

    # List inference results
    results = client.list_results(status="finalized", limit=10)
    for r in results:
        print(f"{r.id}: {r.status}")

    # Get a single result
    result = client.get_result("0xabc123")
    print(f"Submitter: {result.submitter}")

    # Get aggregate statistics
    stats = client.stats()
    print(f"Finalized: {stats.total_finalized}")
"""

from .client import Client, AsyncClient
from .models import (
    ExecutionRequest,
    RequestStatus,
    ResultRow,
    ResultStatus,
    IndexerHealthResponse,
    IndexerStatsResponse,
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
    TimeoutError,
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
from .networks import NetworkConfig, SEPOLIA, ANVIL, NETWORKS

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
    # Indexer response models
    "ResultRow",
    "ResultStatus",
    "IndexerHealthResponse",
    "IndexerStatsResponse",
    # Legacy models (kept for backward compatibility)
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
    "TimeoutError",
    # XGBoost
    "XGBoostConverter",
    "XGBoostModel",
    "ModelParseError",
    # LightGBM
    "LightGBMConverter",
    # Networks
    "NetworkConfig",
    "SEPOLIA",
    "ANVIL",
    "NETWORKS",
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
