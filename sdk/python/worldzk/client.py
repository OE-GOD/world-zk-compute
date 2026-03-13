"""
World ZK Compute SDK - Client

Synchronous and asynchronous clients for the TEE Indexer REST API.

The indexer exposes:
  GET /api/v1/results         -- list results (query: status, submitter, model_hash, limit)
  GET /api/v1/results/:id     -- get single result
  GET /api/v1/stats           -- aggregate statistics
  GET /health                 -- health check
"""

import time
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

try:
    import httpx

    HAS_HTTPX = True
except ImportError:
    HAS_HTTPX = False

try:
    import requests as _requests_lib

    HAS_REQUESTS = True
except ImportError:
    HAS_REQUESTS = False

from .errors import ApiError, NetworkError, TimeoutError, WorldZKError

DEFAULT_BASE_URL = "http://localhost:8081"
DEFAULT_TIMEOUT = 30.0
DEFAULT_MAX_RETRIES = 3


# ---------------------------------------------------------------------------
# Response types (matching the indexer JSON shapes)
# ---------------------------------------------------------------------------

@dataclass
class ResultRow:
    """A single indexed result from the TEE verifier contract."""

    id: str
    model_hash: str
    input_hash: str
    output: str
    submitter: str
    status: str
    block_number: int
    timestamp: int
    challenger: Optional[str] = None

    @staticmethod
    def from_dict(d: Dict[str, Any]) -> "ResultRow":
        return ResultRow(
            id=d.get("id", ""),
            model_hash=d.get("model_hash", ""),
            input_hash=d.get("input_hash", ""),
            output=d.get("output", ""),
            submitter=d.get("submitter", ""),
            status=d.get("status", ""),
            block_number=d.get("block_number", 0),
            timestamp=d.get("timestamp", 0),
            challenger=d.get("challenger"),
        )


@dataclass
class StatsResponse:
    """Aggregate statistics from the indexer."""

    total_submitted: int
    total_challenged: int
    total_finalized: int
    total_resolved: int

    @staticmethod
    def from_dict(d: Dict[str, Any]) -> "StatsResponse":
        return StatsResponse(
            total_submitted=d.get("total_submitted", 0),
            total_challenged=d.get("total_challenged", 0),
            total_finalized=d.get("total_finalized", 0),
            total_resolved=d.get("total_resolved", 0),
        )


@dataclass
class HealthResponse:
    """Health check response from the indexer."""

    status: str
    last_indexed_block: int
    total_results: int

    @staticmethod
    def from_dict(d: Dict[str, Any]) -> "HealthResponse":
        return HealthResponse(
            status=d.get("status", "unknown"),
            last_indexed_block=d.get("last_indexed_block", 0),
            total_results=d.get("total_results", 0),
        )


# ---------------------------------------------------------------------------
# Base client
# ---------------------------------------------------------------------------


class BaseClient:
    """Base client with common HTTP functionality."""

    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        api_key: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
        max_retries: int = DEFAULT_MAX_RETRIES,
    ):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout
        self.max_retries = max_retries

    def _get_headers(self) -> Dict[str, str]:
        headers = {
            "Accept": "application/json",
            "User-Agent": "worldzk-python/1.0.0",
        }
        if self.api_key:
            headers["X-API-Key"] = self.api_key
        return headers

    def _build_url(self, path: str) -> str:
        return f"{self.base_url}{path}"


# ---------------------------------------------------------------------------
# Synchronous client
# ---------------------------------------------------------------------------


class Client(BaseClient):
    """Synchronous client for the TEE Indexer REST API.

    Example::

        client = Client("http://localhost:8081")
        health = client.health()
        results = client.list_results(status="submitted", limit=10)
        stats = client.stats()
    """

    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        api_key: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
        max_retries: int = DEFAULT_MAX_RETRIES,
    ):
        super().__init__(base_url, api_key, timeout, max_retries)

        if HAS_HTTPX:
            self._session = httpx.Client(timeout=timeout)
            self._use_httpx = True
        elif HAS_REQUESTS:
            self._session = _requests_lib.Session()
            self._use_httpx = False
        else:
            raise ImportError(
                "Either 'httpx' or 'requests' is required. "
                "Install with: pip install httpx"
            )

    def close(self) -> None:
        """Close the client session."""
        self._session.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *args) -> None:
        self.close()

    def _request(
        self,
        method: str,
        path: str,
        params: Optional[Dict[str, Any]] = None,
    ) -> Any:
        """Make an HTTP request with retries."""
        url = self._build_url(path)
        headers = self._get_headers()

        last_error: Optional[Exception] = None
        attempts = self.max_retries

        for attempt in range(attempts):
            try:
                if self._use_httpx:
                    response = self._session.request(
                        method, url, params=params, headers=headers
                    )
                    status_code = response.status_code
                    response_json = response.json() if response.content else {}
                else:
                    response = self._session.request(
                        method,
                        url,
                        params=params,
                        headers=headers,
                        timeout=self.timeout,
                    )
                    status_code = response.status_code
                    response_json = response.json() if response.content else {}

                if status_code >= 400:
                    error = ApiError.from_response(response_json, status_code)
                    if error.is_retryable and attempt < attempts - 1:
                        delay = (error.retry_after_ms or 1000) / 1000.0
                        time.sleep(delay)
                        continue
                    raise error

                return response_json

            except (httpx.TimeoutException if HAS_HTTPX else Exception) as e:
                if "Timeout" in type(e).__name__:
                    last_error = TimeoutError(f"Request timed out: {e}", self.timeout)
                    if attempt < attempts - 1:
                        time.sleep(1.0)
                        continue
                raise

            except (
                httpx.RequestError if HAS_HTTPX else _requests_lib.RequestException
            ) as e:
                last_error = NetworkError(
                    code="WZK-5000",
                    message=f"Network error: {e}",
                    http_status=0,
                )
                if attempt < attempts - 1:
                    time.sleep(1.0)
                    continue
                raise last_error

        if last_error:
            raise last_error
        raise WorldZKError("Request failed")

    # -- Indexer endpoints --

    def health(self) -> HealthResponse:
        """Check indexer health."""
        data = self._request("GET", "/health")
        return HealthResponse.from_dict(data)

    def stats(self) -> StatsResponse:
        """Get aggregate statistics."""
        data = self._request("GET", "/api/v1/stats")
        return StatsResponse.from_dict(data)

    def list_results(
        self,
        status: Optional[str] = None,
        submitter: Optional[str] = None,
        model_hash: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[ResultRow]:
        """List indexed results with optional filters.

        Args:
            status: Filter by status (submitted, challenged, finalized, resolved).
            submitter: Filter by submitter address.
            model_hash: Filter by model hash.
            limit: Maximum number of results (1-1000, default 50).
        """
        params: Dict[str, Any] = {}
        if status is not None:
            params["status"] = status
        if submitter is not None:
            params["submitter"] = submitter
        if model_hash is not None:
            params["model_hash"] = model_hash
        if limit is not None:
            params["limit"] = limit

        data = self._request("GET", "/api/v1/results", params=params)
        return [ResultRow.from_dict(r) for r in data]

    def get_result(self, result_id: str) -> Optional[ResultRow]:
        """Get a single result by ID.

        Returns None if not found (404).
        """
        try:
            data = self._request("GET", f"/api/v1/results/{result_id}")
            return ResultRow.from_dict(data)
        except ApiError as e:
            if e.http_status == 404:
                return None
            raise


# ---------------------------------------------------------------------------
# Async client
# ---------------------------------------------------------------------------


class AsyncClient(BaseClient):
    """Asynchronous client for the TEE Indexer REST API.

    Example::

        async with AsyncClient("http://localhost:8081") as client:
            health = await client.health()
            results = await client.list_results(status="finalized")
    """

    def __init__(
        self,
        base_url: str = DEFAULT_BASE_URL,
        api_key: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
        max_retries: int = DEFAULT_MAX_RETRIES,
    ):
        super().__init__(base_url, api_key, timeout, max_retries)

        if not HAS_HTTPX:
            raise ImportError(
                "httpx is required for async client. Install with: pip install httpx"
            )

        self._session: Optional[httpx.AsyncClient] = None

    async def _get_session(self) -> httpx.AsyncClient:
        if self._session is None:
            self._session = httpx.AsyncClient(timeout=self.timeout)
        return self._session

    async def close(self) -> None:
        if self._session:
            await self._session.aclose()
            self._session = None

    async def __aenter__(self) -> "AsyncClient":
        return self

    async def __aexit__(self, *args) -> None:
        await self.close()

    async def _request(
        self,
        method: str,
        path: str,
        params: Optional[Dict[str, Any]] = None,
    ) -> Any:
        """Make an async HTTP request with retries."""
        import asyncio

        url = self._build_url(path)
        headers = self._get_headers()
        session = await self._get_session()

        last_error: Optional[Exception] = None
        attempts = self.max_retries

        for attempt in range(attempts):
            try:
                response = await session.request(
                    method, url, params=params, headers=headers
                )
                status_code = response.status_code
                response_json = response.json() if response.content else {}

                if status_code >= 400:
                    error = ApiError.from_response(response_json, status_code)
                    if error.is_retryable and attempt < attempts - 1:
                        delay = (error.retry_after_ms or 1000) / 1000.0
                        await asyncio.sleep(delay)
                        continue
                    raise error

                return response_json

            except httpx.TimeoutException as e:
                last_error = TimeoutError(f"Request timed out: {e}", self.timeout)
                if attempt < attempts - 1:
                    await asyncio.sleep(1.0)
                    continue
                raise last_error

            except httpx.RequestError as e:
                last_error = NetworkError(
                    code="WZK-5000",
                    message=f"Network error: {e}",
                    http_status=0,
                )
                if attempt < attempts - 1:
                    await asyncio.sleep(1.0)
                    continue
                raise last_error

        if last_error:
            raise last_error
        raise WorldZKError("Request failed")

    # -- Indexer endpoints --

    async def health(self) -> HealthResponse:
        """Check indexer health."""
        data = await self._request("GET", "/health")
        return HealthResponse.from_dict(data)

    async def stats(self) -> StatsResponse:
        """Get aggregate statistics."""
        data = await self._request("GET", "/api/v1/stats")
        return StatsResponse.from_dict(data)

    async def list_results(
        self,
        status: Optional[str] = None,
        submitter: Optional[str] = None,
        model_hash: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[ResultRow]:
        """List indexed results with optional filters."""
        params: Dict[str, Any] = {}
        if status is not None:
            params["status"] = status
        if submitter is not None:
            params["submitter"] = submitter
        if model_hash is not None:
            params["model_hash"] = model_hash
        if limit is not None:
            params["limit"] = limit

        data = await self._request("GET", "/api/v1/results", params=params)
        return [ResultRow.from_dict(r) for r in data]

    async def get_result(self, result_id: str) -> Optional[ResultRow]:
        """Get a single result by ID. Returns None if not found."""
        try:
            data = await self._request("GET", f"/api/v1/results/{result_id}")
            return ResultRow.from_dict(data)
        except ApiError as e:
            if e.http_status == 404:
                return None
            raise
