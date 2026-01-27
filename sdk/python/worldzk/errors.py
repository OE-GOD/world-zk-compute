"""
World ZK Compute SDK - Error classes

Error codes follow the format WZK-XXXX:
- 1xxx: Client errors (validation, auth)
- 2xxx: Server errors
- 3xxx: Proof errors
- 4xxx: Contract errors
- 5xxx: Network errors
"""

from enum import Enum
from typing import Any, Dict, Optional


class ErrorCode(str, Enum):
    """Standardized error codes."""

    # 1xxx - Client Errors
    INVALID_REQUEST = "WZK-1000"
    INVALID_IMAGE_ID = "WZK-1001"
    INVALID_INPUT_HASH = "WZK-1002"
    INVALID_INPUT_URL = "WZK-1003"
    INVALID_REQUEST_ID = "WZK-1004"
    INVALID_PROOF = "WZK-1005"
    INVALID_SIGNATURE = "WZK-1006"
    INPUT_TOO_LARGE = "WZK-1010"
    INPUT_TOO_SMALL = "WZK-1011"
    REQUEST_EXPIRED = "WZK-1020"
    REQUEST_NOT_FOUND = "WZK-1021"
    REQUEST_ALREADY_CLAIMED = "WZK-1022"
    REQUEST_NOT_CLAIMED = "WZK-1023"
    REQUEST_ALREADY_COMPLETED = "WZK-1024"
    UNAUTHORIZED = "WZK-1030"
    FORBIDDEN = "WZK-1031"
    RATE_LIMITED = "WZK-1040"
    QUOTA_EXCEEDED = "WZK-1041"

    # 2xxx - Server Errors
    INTERNAL_ERROR = "WZK-2000"
    DATABASE_ERROR = "WZK-2001"
    CACHE_ERROR = "WZK-2002"
    CONFIGURATION_ERROR = "WZK-2003"
    SERVICE_UNAVAILABLE = "WZK-2010"
    SERVICE_OVERLOADED = "WZK-2011"
    MAINTENANCE_MODE = "WZK-2012"
    SHUTTING_DOWN = "WZK-2013"

    # 3xxx - Proof Errors
    PROOF_GENERATION_FAILED = "WZK-3000"
    PROOF_VERIFICATION_FAILED = "WZK-3001"
    PROOF_TIMEOUT = "WZK-3002"
    PROOF_CANCELLED = "WZK-3003"
    GUEST_PROGRAM_ERROR = "WZK-3010"
    GUEST_PROGRAM_PANIC = "WZK-3011"
    GUEST_PROGRAM_OOM = "WZK-3012"
    GUEST_PROGRAM_TIMEOUT = "WZK-3013"
    IMAGE_NOT_FOUND = "WZK-3020"
    IMAGE_NOT_REGISTERED = "WZK-3021"
    IMAGE_INACTIVE = "WZK-3022"
    BONSAI_ERROR = "WZK-3030"
    BONSAI_TIMEOUT = "WZK-3031"
    BONSAI_QUOTA_EXCEEDED = "WZK-3032"

    # 4xxx - Contract Errors
    CONTRACT_ERROR = "WZK-4000"
    TRANSACTION_FAILED = "WZK-4001"
    TRANSACTION_REVERTED = "WZK-4002"
    INSUFFICIENT_FUNDS = "WZK-4003"
    GAS_ESTIMATION_FAILED = "WZK-4004"
    NONCE_ERROR = "WZK-4005"
    CHAIN_UNAVAILABLE = "WZK-4010"
    CHAIN_REORG = "WZK-4011"
    WRONG_CHAIN = "WZK-4012"

    # 5xxx - Network Errors
    NETWORK_ERROR = "WZK-5000"
    CONNECTION_TIMEOUT = "WZK-5001"
    CONNECTION_REFUSED = "WZK-5002"
    DNS_ERROR = "WZK-5003"
    TLS_ERROR = "WZK-5004"
    IPFS_ERROR = "WZK-5010"
    IPFS_FETCH_FAILED = "WZK-5011"
    IPFS_UPLOAD_FAILED = "WZK-5012"
    IPFS_TIMEOUT = "WZK-5013"
    EXTERNAL_SERVICE_ERROR = "WZK-5020"

    @property
    def is_retryable(self) -> bool:
        """Check if this error is retryable."""
        return self in {
            ErrorCode.RATE_LIMITED,
            ErrorCode.SERVICE_UNAVAILABLE,
            ErrorCode.SERVICE_OVERLOADED,
            ErrorCode.PROOF_TIMEOUT,
            ErrorCode.BONSAI_TIMEOUT,
            ErrorCode.CHAIN_UNAVAILABLE,
            ErrorCode.NETWORK_ERROR,
            ErrorCode.CONNECTION_TIMEOUT,
            ErrorCode.CONNECTION_REFUSED,
            ErrorCode.IPFS_TIMEOUT,
        }

    @property
    def suggested_retry_delay_ms(self) -> Optional[int]:
        """Get suggested retry delay in milliseconds."""
        if not self.is_retryable:
            return None
        delays = {
            ErrorCode.RATE_LIMITED: 5000,
            ErrorCode.SERVICE_OVERLOADED: 10000,
            ErrorCode.SERVICE_UNAVAILABLE: 30000,
            ErrorCode.PROOF_TIMEOUT: 60000,
            ErrorCode.BONSAI_TIMEOUT: 60000,
        }
        return delays.get(self, 1000)


class WorldZKError(Exception):
    """Base exception for World ZK Compute SDK."""

    def __init__(self, message: str):
        self.message = message
        super().__init__(message)


class ApiError(WorldZKError):
    """API error with structured error code."""

    def __init__(
        self,
        code: str,
        message: str,
        http_status: int = 500,
        details: Optional[Dict[str, Any]] = None,
        request_id: Optional[str] = None,
        trace_id: Optional[str] = None,
        retry_after_ms: Optional[int] = None,
    ):
        super().__init__(message)
        self.code = code
        self.http_status = http_status
        self.details = details or {}
        self.request_id = request_id
        self.trace_id = trace_id
        self.retry_after_ms = retry_after_ms

    @classmethod
    def from_response(cls, data: Dict[str, Any], http_status: int = 500) -> "ApiError":
        """Create ApiError from API response."""
        code = data.get("code", "WZK-2000")
        message = data.get("message", "Unknown error")
        details = data.get("details")
        request_id = data.get("request_id")
        trace_id = data.get("trace_id")

        retry_after_ms = None
        if retry := data.get("retry"):
            retry_after_ms = retry.get("retry_after_ms")

        # Return specific error subclass based on code
        error_cls = cls._get_error_class(code)
        return error_cls(
            code=code,
            message=message,
            http_status=http_status,
            details=details,
            request_id=request_id,
            trace_id=trace_id,
            retry_after_ms=retry_after_ms,
        )

    @staticmethod
    def _get_error_class(code: str) -> type:
        """Get appropriate error class for code."""
        if code.startswith("WZK-103"):
            return AuthenticationError
        if code == "WZK-1021" or code == "WZK-3020":
            return NotFoundError
        if code in ("WZK-1040", "WZK-1041"):
            return RateLimitError
        if code.startswith("WZK-3"):
            return ProofError
        if code.startswith("WZK-5"):
            return NetworkError
        if code.startswith("WZK-1"):
            return ValidationError
        return ApiError

    @property
    def error_code(self) -> Optional[ErrorCode]:
        """Get ErrorCode enum if valid."""
        try:
            return ErrorCode(self.code)
        except ValueError:
            return None

    @property
    def is_retryable(self) -> bool:
        """Check if this error is retryable."""
        if ec := self.error_code:
            return ec.is_retryable
        return False

    def __str__(self) -> str:
        s = f"[{self.code}] {self.message}"
        if self.request_id:
            s += f" (request_id: {self.request_id})"
        return s

    def __repr__(self) -> str:
        return (
            f"ApiError(code={self.code!r}, message={self.message!r}, "
            f"http_status={self.http_status})"
        )


class ValidationError(ApiError):
    """Client-side validation error."""

    pass


class AuthenticationError(ApiError):
    """Authentication or authorization error."""

    pass


class NotFoundError(ApiError):
    """Resource not found error."""

    pass


class RateLimitError(ApiError):
    """Rate limit exceeded error."""

    @property
    def retry_after_seconds(self) -> Optional[float]:
        """Get retry delay in seconds."""
        if self.retry_after_ms:
            return self.retry_after_ms / 1000.0
        return None


class ProofError(ApiError):
    """Proof generation or verification error."""

    pass


class NetworkError(ApiError):
    """Network or external service error."""

    pass


class TimeoutError(WorldZKError):
    """Operation timed out."""

    def __init__(self, message: str = "Operation timed out", timeout_seconds: float = 0):
        super().__init__(message)
        self.timeout_seconds = timeout_seconds
