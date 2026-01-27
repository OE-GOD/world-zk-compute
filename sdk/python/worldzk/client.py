"""
World ZK Compute SDK - Client

Synchronous and asynchronous clients for the World ZK Compute API.
"""

import hashlib
import time
from typing import Any, Dict, List, Optional, Union
from urllib.parse import urljoin
import base64

try:
    import httpx

    HAS_HTTPX = True
except ImportError:
    HAS_HTTPX = False

try:
    import requests

    HAS_REQUESTS = True
except ImportError:
    HAS_REQUESTS = False

from .errors import ApiError, NetworkError, TimeoutError, WorldZKError
from .models import (
    ExecutionRequest,
    HealthResponse,
    Pagination,
    PaginatedList,
    Program,
    Prover,
    RequestStatus,
    StatusResponse,
)

DEFAULT_BASE_URL = "https://api.worldzk.compute/v1"
DEFAULT_TIMEOUT = 30.0
DEFAULT_MAX_RETRIES = 3


class BaseClient:
    """Base client with common functionality."""

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
        """Get request headers."""
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
            "User-Agent": "worldzk-python/1.0.0",
        }
        if self.api_key:
            headers["X-API-Key"] = self.api_key
        return headers

    def _build_url(self, path: str) -> str:
        """Build full URL from path."""
        return urljoin(self.base_url + "/", path.lstrip("/"))

    @staticmethod
    def _hash_input(data: bytes) -> str:
        """Compute SHA256 hash of input data."""
        return "0x" + hashlib.sha256(data).hexdigest()


class Client(BaseClient):
    """Synchronous client for World ZK Compute API."""

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
            self._session = requests.Session()
            self._use_httpx = False
        else:
            raise ImportError(
                "Either 'httpx' or 'requests' is required. "
                "Install with: pip install httpx"
            )

        # Sub-clients
        self.requests = RequestsClient(self)
        self.programs = ProgramsClient(self)
        self.provers = ProversClient(self)

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
        json: Optional[Dict[str, Any]] = None,
        retry: bool = True,
    ) -> Dict[str, Any]:
        """Make HTTP request."""
        url = self._build_url(path)
        headers = self._get_headers()

        last_error = None
        attempts = self.max_retries if retry else 1

        for attempt in range(attempts):
            try:
                if self._use_httpx:
                    response = self._session.request(
                        method, url, params=params, json=json, headers=headers
                    )
                    status_code = response.status_code
                    response_json = response.json() if response.content else {}
                else:
                    response = self._session.request(
                        method,
                        url,
                        params=params,
                        json=json,
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

            except (httpx.RequestError if HAS_HTTPX else requests.RequestException) as e:
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

    def health(self) -> HealthResponse:
        """Check service health."""
        data = self._request("GET", "/health")
        return HealthResponse.from_dict(data)

    def status(self) -> StatusResponse:
        """Get service status."""
        data = self._request("GET", "/status")
        return StatusResponse.from_dict(data)


class RequestsClient:
    """Client for execution request operations."""

    def __init__(self, client: Client):
        self._client = client

    def list(
        self,
        status: Optional[RequestStatus] = None,
        image_id: Optional[str] = None,
        limit: int = 20,
        offset: int = 0,
    ) -> PaginatedList:
        """List execution requests."""
        params: Dict[str, Any] = {"limit": limit, "offset": offset}
        if status:
            params["status"] = status.value
        if image_id:
            params["imageId"] = image_id

        data = self._client._request("GET", "/requests", params=params)

        requests = [ExecutionRequest.from_dict(r) for r in data.get("requests", [])]
        pagination = Pagination.from_dict(data.get("pagination", {}))

        return PaginatedList(items=requests, pagination=pagination)

    def get(self, request_id: int) -> ExecutionRequest:
        """Get execution request by ID."""
        data = self._client._request("GET", f"/requests/{request_id}")
        return ExecutionRequest.from_dict(data)

    def create(
        self,
        image_id: str,
        input_data: Optional[bytes] = None,
        input_url: Optional[str] = None,
        input_hash: Optional[str] = None,
        callback_contract: Optional[str] = None,
        expiration_seconds: int = 3600,
        tip: int = 0,
    ) -> ExecutionRequest:
        """
        Create a new execution request.

        Args:
            image_id: Program image ID (32-byte hex)
            input_data: Raw input bytes (will be base64 encoded)
            input_url: URL to fetch input (alternative to input_data)
            input_hash: SHA256 hash of input (computed if not provided)
            callback_contract: Contract to call with result
            expiration_seconds: Request expiration time
            tip: Tip amount in wei

        Returns:
            Created ExecutionRequest
        """
        if not input_data and not input_url:
            raise ValueError("Either input_data or input_url is required")

        body: Dict[str, Any] = {
            "image_id": image_id,
            "expiration_seconds": expiration_seconds,
        }

        if input_data:
            body["input_data"] = base64.b64encode(input_data).decode()
            body["input_hash"] = input_hash or self._client._hash_input(input_data)
        elif input_url:
            body["input_url"] = input_url
            if input_hash:
                body["input_hash"] = input_hash
            else:
                raise ValueError("input_hash is required when using input_url")

        if callback_contract:
            body["callback_contract"] = callback_contract
        if tip:
            body["tip"] = str(tip)

        data = self._client._request("POST", "/requests", json=body)
        return ExecutionRequest.from_dict(data)

    def cancel(self, request_id: int) -> ExecutionRequest:
        """Cancel a pending execution request."""
        data = self._client._request("DELETE", f"/requests/{request_id}")
        return ExecutionRequest.from_dict(data)

    def claim(self, request_id: int) -> Dict[str, Any]:
        """Claim an execution request (prover only)."""
        return self._client._request("POST", f"/requests/{request_id}/claim")

    def submit_proof(
        self,
        request_id: int,
        seal: bytes,
        journal: bytes,
    ) -> Dict[str, Any]:
        """Submit a proof for a claimed request."""
        body = {
            "seal": base64.b64encode(seal).decode(),
            "journal": base64.b64encode(journal).decode(),
        }
        return self._client._request("POST", f"/requests/{request_id}/proof", json=body)

    def wait(
        self,
        request_id: int,
        timeout: float = 300.0,
        poll_interval: float = 2.0,
    ) -> ExecutionRequest:
        """
        Wait for a request to complete.

        Args:
            request_id: Request ID to wait for
            timeout: Maximum time to wait in seconds
            poll_interval: Time between status checks

        Returns:
            Completed ExecutionRequest

        Raises:
            TimeoutError: If request doesn't complete within timeout
        """
        start_time = time.time()

        while True:
            request = self.get(request_id)

            if request.is_terminal:
                return request

            elapsed = time.time() - start_time
            if elapsed >= timeout:
                raise TimeoutError(
                    f"Request {request_id} did not complete within {timeout}s",
                    timeout,
                )

            time.sleep(poll_interval)


class ProgramsClient:
    """Client for program operations."""

    def __init__(self, client: Client):
        self._client = client

    def list(
        self,
        active: Optional[bool] = None,
        limit: int = 20,
        offset: int = 0,
    ) -> PaginatedList:
        """List registered programs."""
        params: Dict[str, Any] = {"limit": limit, "offset": offset}
        if active is not None:
            params["active"] = active

        data = self._client._request("GET", "/programs", params=params)

        programs = [Program.from_dict(p) for p in data.get("programs", [])]
        pagination = Pagination.from_dict(data.get("pagination", {}))

        return PaginatedList(items=programs, pagination=pagination)

    def get(self, image_id: str) -> Program:
        """Get program by image ID."""
        data = self._client._request("GET", f"/programs/{image_id}")
        return Program.from_dict(data)


class ProversClient:
    """Client for prover operations."""

    def __init__(self, client: Client):
        self._client = client

    def list(
        self,
        tier: Optional[str] = None,
        limit: int = 20,
        offset: int = 0,
    ) -> PaginatedList:
        """List registered provers."""
        params: Dict[str, Any] = {"limit": limit, "offset": offset}
        if tier:
            params["tier"] = tier

        data = self._client._request("GET", "/provers", params=params)

        provers = [Prover.from_dict(p) for p in data.get("provers", [])]
        pagination = Pagination.from_dict(data.get("pagination", {}))

        return PaginatedList(items=provers, pagination=pagination)

    def get(self, address: str) -> Prover:
        """Get prover by address."""
        data = self._client._request("GET", f"/provers/{address}")
        return Prover.from_dict(data)

    def register(
        self,
        name: Optional[str] = None,
        endpoint: Optional[str] = None,
    ) -> Prover:
        """Register as a prover."""
        body: Dict[str, Any] = {}
        if name:
            body["name"] = name
        if endpoint:
            body["endpoint"] = endpoint

        data = self._client._request("POST", "/provers/register", json=body)
        return Prover.from_dict(data)


class AsyncClient(BaseClient):
    """Asynchronous client for World ZK Compute API."""

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

        # Sub-clients
        self.requests = AsyncRequestsClient(self)
        self.programs = AsyncProgramsClient(self)
        self.provers = AsyncProversClient(self)

    async def _get_session(self) -> httpx.AsyncClient:
        """Get or create async session."""
        if self._session is None:
            self._session = httpx.AsyncClient(timeout=self.timeout)
        return self._session

    async def close(self) -> None:
        """Close the client session."""
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
        json: Optional[Dict[str, Any]] = None,
        retry: bool = True,
    ) -> Dict[str, Any]:
        """Make async HTTP request."""
        import asyncio

        url = self._build_url(path)
        headers = self._get_headers()
        session = await self._get_session()

        last_error = None
        attempts = self.max_retries if retry else 1

        for attempt in range(attempts):
            try:
                response = await session.request(
                    method, url, params=params, json=json, headers=headers
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

    async def health(self) -> HealthResponse:
        """Check service health."""
        data = await self._request("GET", "/health")
        return HealthResponse.from_dict(data)

    async def status(self) -> StatusResponse:
        """Get service status."""
        data = await self._request("GET", "/status")
        return StatusResponse.from_dict(data)


class AsyncRequestsClient:
    """Async client for execution request operations."""

    def __init__(self, client: AsyncClient):
        self._client = client

    async def list(
        self,
        status: Optional[RequestStatus] = None,
        image_id: Optional[str] = None,
        limit: int = 20,
        offset: int = 0,
    ) -> PaginatedList:
        """List execution requests."""
        params: Dict[str, Any] = {"limit": limit, "offset": offset}
        if status:
            params["status"] = status.value
        if image_id:
            params["imageId"] = image_id

        data = await self._client._request("GET", "/requests", params=params)

        requests = [ExecutionRequest.from_dict(r) for r in data.get("requests", [])]
        pagination = Pagination.from_dict(data.get("pagination", {}))

        return PaginatedList(items=requests, pagination=pagination)

    async def get(self, request_id: int) -> ExecutionRequest:
        """Get execution request by ID."""
        data = await self._client._request("GET", f"/requests/{request_id}")
        return ExecutionRequest.from_dict(data)

    async def create(
        self,
        image_id: str,
        input_data: Optional[bytes] = None,
        input_url: Optional[str] = None,
        input_hash: Optional[str] = None,
        callback_contract: Optional[str] = None,
        expiration_seconds: int = 3600,
        tip: int = 0,
    ) -> ExecutionRequest:
        """Create a new execution request."""
        if not input_data and not input_url:
            raise ValueError("Either input_data or input_url is required")

        body: Dict[str, Any] = {
            "image_id": image_id,
            "expiration_seconds": expiration_seconds,
        }

        if input_data:
            body["input_data"] = base64.b64encode(input_data).decode()
            body["input_hash"] = input_hash or self._client._hash_input(input_data)
        elif input_url:
            body["input_url"] = input_url
            if input_hash:
                body["input_hash"] = input_hash
            else:
                raise ValueError("input_hash is required when using input_url")

        if callback_contract:
            body["callback_contract"] = callback_contract
        if tip:
            body["tip"] = str(tip)

        data = await self._client._request("POST", "/requests", json=body)
        return ExecutionRequest.from_dict(data)

    async def cancel(self, request_id: int) -> ExecutionRequest:
        """Cancel a pending execution request."""
        data = await self._client._request("DELETE", f"/requests/{request_id}")
        return ExecutionRequest.from_dict(data)

    async def wait(
        self,
        request_id: int,
        timeout: float = 300.0,
        poll_interval: float = 2.0,
    ) -> ExecutionRequest:
        """Wait for a request to complete."""
        import asyncio

        start_time = time.time()

        while True:
            request = await self.get(request_id)

            if request.is_terminal:
                return request

            elapsed = time.time() - start_time
            if elapsed >= timeout:
                raise TimeoutError(
                    f"Request {request_id} did not complete within {timeout}s",
                    timeout,
                )

            await asyncio.sleep(poll_interval)


class AsyncProgramsClient:
    """Async client for program operations."""

    def __init__(self, client: AsyncClient):
        self._client = client

    async def list(
        self,
        active: Optional[bool] = None,
        limit: int = 20,
        offset: int = 0,
    ) -> PaginatedList:
        """List registered programs."""
        params: Dict[str, Any] = {"limit": limit, "offset": offset}
        if active is not None:
            params["active"] = active

        data = await self._client._request("GET", "/programs", params=params)

        programs = [Program.from_dict(p) for p in data.get("programs", [])]
        pagination = Pagination.from_dict(data.get("pagination", {}))

        return PaginatedList(items=programs, pagination=pagination)

    async def get(self, image_id: str) -> Program:
        """Get program by image ID."""
        data = await self._client._request("GET", f"/programs/{image_id}")
        return Program.from_dict(data)


class AsyncProversClient:
    """Async client for prover operations."""

    def __init__(self, client: AsyncClient):
        self._client = client

    async def list(
        self,
        tier: Optional[str] = None,
        limit: int = 20,
        offset: int = 0,
    ) -> PaginatedList:
        """List registered provers."""
        params: Dict[str, Any] = {"limit": limit, "offset": offset}
        if tier:
            params["tier"] = tier

        data = await self._client._request("GET", "/provers", params=params)

        provers = [Prover.from_dict(p) for p in data.get("provers", [])]
        pagination = Pagination.from_dict(data.get("pagination", {}))

        return PaginatedList(items=provers, pagination=pagination)

    async def get(self, address: str) -> Prover:
        """Get prover by address."""
        data = await self._client._request("GET", f"/provers/{address}")
        return Prover.from_dict(data)
