"""Tests for IndexerClient / AsyncIndexerClient against the current indexer API.

``tests/test_client.py`` covers the original behaviour of these clients. The
tests here pin what the indexer service (``services/indexer``) does today:
``GET /api/v1/results`` answers with a pagination envelope and API-key
authentication uses ``Authorization: Bearer <key>``.
"""

from __future__ import annotations

import httpx
import pytest
import respx

from worldzk.errors import ApiError, NetworkError, TimeoutError
from worldzk.indexer import AsyncIndexerClient, IndexerClient
from worldzk.models import ResultRow, ResultStatus

ROW_JSON = {
    "id": "0xabc123",
    "model_hash": "0x" + "aa" * 32,
    "input_hash": "0x" + "bb" * 32,
    "output": "",
    "submitter": "0x" + "cc" * 20,
    "status": "submitted",
    "block_number": 19542050,
    "timestamp": 1700000000,
    "challenger": None,
}

# Shape of PaginatedResultResponse in services/indexer/openapi.yaml
PAGE_JSON = {
    "data": [ROW_JSON],
    "total": 1,
    "limit": 100,
    "offset": 0,
    "has_more": False,
}

EMPTY_PAGE_JSON = {"data": [], "total": 0, "limit": 100, "offset": 0, "has_more": False}

RESULTS_URL = "http://localhost:8081/api/v1/results"
HEALTH_URL = "http://localhost:8081/health"
HEALTH_JSON = {"status": "ok", "last_indexed_block": 1, "total_results": 0}


class TestPaginatedResults:
    """list_results() unwraps the pagination envelope."""

    @respx.mock
    def test_sync_list_results_envelope(self):
        respx.get(RESULTS_URL).mock(return_value=httpx.Response(200, json=PAGE_JSON))

        with IndexerClient() as client:
            results = client.list_results()

        assert len(results) == 1
        assert isinstance(results[0], ResultRow)
        assert results[0].id == "0xabc123"
        assert results[0].status == ResultStatus.SUBMITTED

    @respx.mock
    def test_sync_list_results_empty_envelope(self):
        respx.get(RESULTS_URL).mock(return_value=httpx.Response(200, json=EMPTY_PAGE_JSON))

        with IndexerClient() as client:
            assert client.list_results() == []

    @respx.mock
    async def test_async_list_results_envelope(self):
        respx.get(RESULTS_URL).mock(return_value=httpx.Response(200, json=PAGE_JSON))

        async with AsyncIndexerClient() as client:
            results = await client.list_results()

        assert len(results) == 1
        assert results[0].id == "0xabc123"


class TestBearerAuth:
    """The API key is sent the way the indexer checks it."""

    @respx.mock
    def test_sync_sends_bearer_token(self):
        route = respx.get(RESULTS_URL).mock(
            return_value=httpx.Response(200, json=EMPTY_PAGE_JSON)
        )

        with IndexerClient(api_key="my-secret-key") as client:
            client.list_results()

        assert route.calls[0].request.headers["authorization"] == "Bearer my-secret-key"

    @respx.mock
    async def test_async_sends_bearer_token(self):
        route = respx.get(RESULTS_URL).mock(
            return_value=httpx.Response(200, json=EMPTY_PAGE_JSON)
        )

        async with AsyncIndexerClient(api_key="my-secret-key") as client:
            await client.list_results()

        assert route.calls[0].request.headers["authorization"] == "Bearer my-secret-key"

    @respx.mock
    def test_no_authorization_header_without_key(self):
        route = respx.get(HEALTH_URL).mock(return_value=httpx.Response(200, json=HEALTH_JSON))

        with IndexerClient() as client:
            client.health()

        assert "authorization" not in route.calls[0].request.headers


class TestErrorMessages:
    """The indexer's ``{"error": "<text>"}`` bodies reach the caller."""

    @respx.mock
    def test_error_field_becomes_message(self):
        respx.get(RESULTS_URL).mock(
            return_value=httpx.Response(400, json={"error": "invalid status 'bogus'"})
        )

        with IndexerClient(max_retries=1) as client:
            with pytest.raises(ApiError) as exc_info:
                client.list_results(status="bogus")

        assert exc_info.value.http_status == 400
        assert exc_info.value.message == "invalid status 'bogus'"

    def test_message_field_wins_over_error_field(self):
        error = ApiError.from_response({"error": "Unauthorized", "message": "Valid API key required."}, 401)
        assert error.message == "Valid API key required."

    def test_unknown_error_fallback(self):
        assert ApiError.from_response({}, 500).message == "Unknown error"


class TestTransportErrors:
    """Transport failures surface as SDK errors once retries are exhausted."""

    @respx.mock
    def test_sync_timeout_raises_sdk_timeout(self):
        respx.get(HEALTH_URL).mock(side_effect=httpx.ConnectTimeout("timed out"))

        with IndexerClient(max_retries=1) as client:
            with pytest.raises(TimeoutError) as exc_info:
                client.health()

        assert exc_info.value.timeout_seconds == client.timeout

    @respx.mock
    async def test_async_timeout_raises_sdk_timeout(self):
        respx.get(HEALTH_URL).mock(side_effect=httpx.ConnectTimeout("timed out"))

        async with AsyncIndexerClient(max_retries=1) as client:
            with pytest.raises(TimeoutError):
                await client.health()

    @respx.mock
    def test_sync_connection_error_raises_network_error(self):
        respx.get(HEALTH_URL).mock(side_effect=httpx.ConnectError("refused"))

        with IndexerClient(max_retries=1) as client:
            with pytest.raises(NetworkError) as exc_info:
                client.health()

        assert exc_info.value.code == "WZK-5000"

    @respx.mock
    async def test_async_connection_error_raises_network_error(self):
        respx.get(HEALTH_URL).mock(side_effect=httpx.ConnectError("refused"))

        async with AsyncIndexerClient(max_retries=1) as client:
            with pytest.raises(NetworkError):
                await client.health()
