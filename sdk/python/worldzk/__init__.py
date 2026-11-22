"""World ZK Compute Python SDK.

Importing ``worldzk`` needs no optional dependency. The on-chain clients
(``TEEVerifier``, ``BatchVerifier``, the event watchers, ...) need the ``web3``
extra (``pip install worldzk[web3]``) and are loaded on first access, e.g.
``from worldzk import TEEVerifier``.
"""
from importlib import import_module
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _distribution_version
from importlib.util import find_spec
from typing import TYPE_CHECKING, Any

from .client import Client, WorldZK
from .errors import (
    ApiError,
    AuthenticationError,
    ErrorCode,
    NetworkError,
    NotFoundError,
    ProofError,
    RateLimitError,
    TimeoutError,
    ValidationError,
    WorldZKError,
)
from .hash_utils import (
    compute_input_hash,
    compute_input_hash_from_json,
    compute_model_hash,
    compute_model_hash_from_bytes,
    compute_result_hash,
    compute_result_hash_from_bytes,
)
from .indexer import AsyncIndexerClient, IndexerClient
from .lightgbm import LightGBMConverter
from .models import (
    GatewayHealth,
    IndexerHealthResponse,
    IndexerStatsResponse,
    ProofBundle,
    ProofReceipt,
    ProofResult,
    Receipt,
    ResultRow,
    ResultStatus,
    VerificationResult,
)
from .networks import ANVIL, NETWORKS, SEPOLIA, NetworkConfig
from .xgboost import ModelParseError, XGBoostConverter, XGBoostModel

if TYPE_CHECKING:
    from .async_client import AsyncEventWatcher, AsyncTEEVerifier
    from .batch_verifier import BatchVerificationCancelledError, ProgressTracker
    from .batch_verifier import BatchVerifier as StepBatchVerifier
    from .event_watcher import DisputeResolved as EventDisputeResolved
    from .event_watcher import EnclaveRegistered as EventEnclaveRegistered
    from .event_watcher import EnclaveRevoked as EventEnclaveRevoked
    from .event_watcher import ResultChallenged as EventResultChallenged
    from .event_watcher import ResultExpired as EventResultExpired
    from .event_watcher import ResultFinalized as EventResultFinalized
    from .event_watcher import ResultSubmitted as EventResultSubmitted
    from .event_watcher import TEEEvent, TEEEventType, TEEEventWatcher, parse_log, topic_hash
    from .execution_engine import (
        EXECUTION_ENGINE_ABI,
        ExecutionEngineClient,
        ExecutionRequestInfo,
    )
    from .prover_registry import PROVER_REGISTRY_ABI, ProverInfo, ProverRegistryClient
    from .tee_verifier import MLResult, TEEVerifier
    from .verifier import (
        BatchSession,
        BatchVerifier,
        BatchVerifyInput,
        BatchVerifyResult,
        ProgressEvent,
        StepResult,
    )

try:
    __version__ = _distribution_version("worldzk")
except PackageNotFoundError:  # source checkout on sys.path, package not installed
    __version__ = "0.0.0"

# Exports that need the ``web3`` extra: public name -> (submodule, attribute).
# They are resolved by ``__getattr__`` on first access so that ``import worldzk``
# works without web3 installed.
_WEB3_EXPORTS = {
    # High-level batch verifier
    "BatchVerifier": ("verifier", "BatchVerifier"),
    "BatchVerifyInput": ("verifier", "BatchVerifyInput"),
    "BatchVerifyResult": ("verifier", "BatchVerifyResult"),
    "BatchSession": ("verifier", "BatchSession"),
    "StepResult": ("verifier", "StepResult"),
    "ProgressEvent": ("verifier", "ProgressEvent"),
    # Step-level batch verifier
    "StepBatchVerifier": ("batch_verifier", "BatchVerifier"),
    "BatchVerificationCancelledError": ("batch_verifier", "BatchVerificationCancelledError"),
    "ProgressTracker": ("batch_verifier", "ProgressTracker"),
    # TEE verifier
    "TEEVerifier": ("tee_verifier", "TEEVerifier"),
    "MLResult": ("tee_verifier", "MLResult"),
    # TEE event watcher
    "TEEEventWatcher": ("event_watcher", "TEEEventWatcher"),
    "TEEEventType": ("event_watcher", "TEEEventType"),
    "TEEEvent": ("event_watcher", "TEEEvent"),
    "EventResultSubmitted": ("event_watcher", "ResultSubmitted"),
    "EventResultChallenged": ("event_watcher", "ResultChallenged"),
    "EventResultFinalized": ("event_watcher", "ResultFinalized"),
    "EventResultExpired": ("event_watcher", "ResultExpired"),
    "EventDisputeResolved": ("event_watcher", "DisputeResolved"),
    "EventEnclaveRegistered": ("event_watcher", "EnclaveRegistered"),
    "EventEnclaveRevoked": ("event_watcher", "EnclaveRevoked"),
    "parse_log": ("event_watcher", "parse_log"),
    "topic_hash": ("event_watcher", "topic_hash"),
    # Async TEE verifier and event watcher
    "AsyncTEEVerifier": ("async_client", "AsyncTEEVerifier"),
    "AsyncEventWatcher": ("async_client", "AsyncEventWatcher"),
    # ExecutionEngine
    "ExecutionEngineClient": ("execution_engine", "ExecutionEngineClient"),
    "ExecutionRequestInfo": ("execution_engine", "ExecutionRequestInfo"),
    "EXECUTION_ENGINE_ABI": ("execution_engine", "EXECUTION_ENGINE_ABI"),
    # ProverRegistry
    "ProverRegistryClient": ("prover_registry", "ProverRegistryClient"),
    "ProverInfo": ("prover_registry", "ProverInfo"),
    "PROVER_REGISTRY_ABI": ("prover_registry", "PROVER_REGISTRY_ABI"),
}


def __getattr__(name: str) -> Any:
    try:
        module_name, attr = _WEB3_EXPORTS[name]
    except KeyError:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}") from None
    try:
        module = import_module(f".{module_name}", __name__)
    except ImportError as e:
        raise ImportError(
            f"worldzk.{name} requires the 'web3' extra ({e}). "
            "Install with: pip install worldzk[web3]"
        ) from e
    value = getattr(module, attr)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(_WEB3_EXPORTS))


__all__ = [
    # Gateway client
    "WorldZK",
    "Client",
    "GatewayHealth",
    "ProofBundle",
    "ProofReceipt",
    "ProofResult",
    "Receipt",
    "VerificationResult",
    # Indexer client
    "IndexerClient",
    "AsyncIndexerClient",
    "ResultRow",
    "ResultStatus",
    "IndexerHealthResponse",
    "IndexerStatsResponse",
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
    # Hash utilities
    "compute_model_hash",
    "compute_model_hash_from_bytes",
    "compute_input_hash",
    "compute_input_hash_from_json",
    "compute_result_hash",
    "compute_result_hash_from_bytes",
    # Model converters
    "XGBoostConverter",
    "XGBoostModel",
    "ModelParseError",
    "LightGBMConverter",
    # Networks
    "NetworkConfig",
    "SEPOLIA",
    "ANVIL",
    "NETWORKS",
    # High-level batch verifier (web3 extra)
    "BatchVerifier",
    "BatchVerifyInput",
    "BatchVerifyResult",
    "BatchSession",
    "StepResult",
    "ProgressEvent",
    # Step-level batch verifier (web3 extra)
    "StepBatchVerifier",
    "BatchVerificationCancelledError",
    "ProgressTracker",
    # TEE verifier (web3 extra)
    "TEEVerifier",
    "MLResult",
    # TEE event watcher (web3 extra)
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
    # Async TEE verifier and event watcher (web3 extra)
    "AsyncTEEVerifier",
    "AsyncEventWatcher",
    # ExecutionEngine (web3 extra)
    "ExecutionEngineClient",
    "ExecutionRequestInfo",
    "EXECUTION_ENGINE_ABI",
    # ProverRegistry (web3 extra)
    "ProverRegistryClient",
    "ProverInfo",
    "PROVER_REGISTRY_ABI",
]

# Without web3 the names above cannot be resolved; drop them from ``__all__``
# so that ``from worldzk import *`` keeps working without the extra.
if find_spec("web3") is None:
    __all__ = [name for name in __all__ if name not in _WEB3_EXPORTS]
