"""Tests for the worldzk Client and AsyncClient targeting the TEE Indexer API.

These tests use ``respx`` to mock httpx requests so no real indexer is needed.
"""

from __future__ import annotations

import pytest
import httpx
import respx

from worldzk.client import (
    Client,
    AsyncClient,
    DEFAULT_BASE_URL,
    DEFAULT_TIMEOUT,
    DEFAULT_MAX_RETRIES,
)
from worldzk.models import (
    IndexerHealthResponse,
    IndexerStatsResponse,
    ResultRow,
    ResultStatus,
)
from worldzk.errors import ApiError


# ---------------------------------------------------------------------------
# Fixtures: sample JSON responses matching the indexer OpenAPI spec
# ---------------------------------------------------------------------------

HEALTH_JSON = {
    "status": "ok",
    "last_indexed_block": 19542100,
    "total_results": 256,
}

STATS_JSON = {
    "total_submitted": 100,
    "total_challenged": 5,
    "total_finalized": 90,
    "total_resolved": 3,
}

RESULT_ROW_JSON = {
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

FINALIZED_ROW_JSON = {
    "id": "0xdef456",
    "model_hash": "0x" + "11" * 32,
    "input_hash": "0x" + "22" * 32,
    "output": "0xdeadbeef",
    "submitter": "0x" + "33" * 20,
    "status": "finalized",
    "block_number": 19542080,
    "timestamp": 1700001000,
    "challenger": None,
}

NOT_FOUND_JSON = {"error": "result not found"}


# ---------------------------------------------------------------------------
# Client defaults
# ---------------------------------------------------------------------------


class TestClientDefaults:
    """Verify default configuration values."""

    def test_default_base_url(self):
        assert DEFAULT_BASE_URL == "http://localhost:8081"

    def test_default_timeout(self):
        assert DEFAULT_TIMEOUT == 30.0

    def test_default_max_retries(self):
        assert DEFAULT_MAX_RETRIES == 3

    def test_client_stores_base_url(self):
        client = Client(base_url="http://myhost:9090")
        assert client.base_url == "http://myhost:9090"
        client.close()

    def test_client_strips_trailing_slash(self):
        client = Client(base_url="http://myhost:9090/")
        assert client.base_url == "http://myhost:9090"
        client.close()

    def test_client_stores_api_key(self):
        client = Client(api_key="test-key")
        assert client.api_key == "test-key"
        client.close()

    def test_client_context_manager(self):
        with Client() as client:
            assert client is not None


# ---------------------------------------------------------------------------
# Client.health()
# ---------------------------------------------------------------------------


class TestClientHealth:
    """Tests for GET /health."""

    @respx.mock
    def test_health_returns_parsed_response(self):
        respx.get("http://localhost:8081/health").mock(
            return_value=httpx.Response(200, json=HEALTH_JSON)
        )

        with Client() as client:
            health = client.health()

        assert isinstance(health, IndexerHealthResponse)
        assert health.status == "ok"
        assert health.last_indexed_block == 19542100
        assert health.total_results == 256

    @respx.mock
    def test_health_custom_base_url(self):
        respx.get("http://custom:9090/health").mock(
            return_value=httpx.Response(200, json=HEALTH_JSON)
        )

        with Client(base_url="http://custom:9090") as client:
            health = client.health()

        assert health.status == "ok"


# ---------------------------------------------------------------------------
# Client.stats()
# ---------------------------------------------------------------------------


class TestClientStats:
    """Tests for GET /api/v1/stats."""

    @respx.mock
    def test_stats_returns_parsed_response(self):
        respx.get("http://localhost:8081/api/v1/stats").mock(
            return_value=httpx.Response(200, json=STATS_JSON)
        )

        with Client() as client:
            stats = client.stats()

        assert isinstance(stats, IndexerStatsResponse)
        assert stats.total_submitted == 100
        assert stats.total_challenged == 5
        assert stats.total_finalized == 90
        assert stats.total_resolved == 3

    @respx.mock
    def test_stats_total_property(self):
        respx.get("http://localhost:8081/api/v1/stats").mock(
            return_value=httpx.Response(200, json=STATS_JSON)
        )

        with Client() as client:
            stats = client.stats()

        assert stats.total == 100 + 5 + 90 + 3


# ---------------------------------------------------------------------------
# Client.list_results()
# ---------------------------------------------------------------------------


class TestClientListResults:
    """Tests for GET /api/v1/results."""

    @respx.mock
    def test_list_results_no_filters(self):
        respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[RESULT_ROW_JSON, FINALIZED_ROW_JSON])
        )

        with Client() as client:
            results = client.list_results()

        assert len(results) == 2
        assert isinstance(results[0], ResultRow)
        assert results[0].id == "0xabc123"
        assert results[0].status == ResultStatus.SUBMITTED
        assert results[1].id == "0xdef456"
        assert results[1].status == ResultStatus.FINALIZED

    @respx.mock
    def test_list_results_with_status_filter(self):
        route = respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[FINALIZED_ROW_JSON])
        )

        with Client() as client:
            results = client.list_results(status="finalized")

        assert len(results) == 1
        assert results[0].status == ResultStatus.FINALIZED
        # Verify query parameter was sent
        assert route.calls[0].request.url.params["status"] == "finalized"

    @respx.mock
    def test_list_results_with_submitter_filter(self):
        submitter = "0x" + "cc" * 20
        route = respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[RESULT_ROW_JSON])
        )

        with Client() as client:
            results = client.list_results(submitter=submitter)

        assert len(results) == 1
        assert route.calls[0].request.url.params["submitter"] == submitter

    @respx.mock
    def test_list_results_with_model_hash_filter(self):
        model_hash = "0x" + "aa" * 32
        route = respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[RESULT_ROW_JSON])
        )

        with Client() as client:
            results = client.list_results(model_hash=model_hash)

        assert len(results) == 1
        assert route.calls[0].request.url.params["model_hash"] == model_hash

    @respx.mock
    def test_list_results_with_limit(self):
        route = respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[RESULT_ROW_JSON])
        )

        with Client() as client:
            results = client.list_results(limit=5)

        assert len(results) == 1
        assert route.calls[0].request.url.params["limit"] == "5"

    @respx.mock
    def test_list_results_with_all_filters(self):
        route = respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[])
        )

        with Client() as client:
            results = client.list_results(
                status="submitted",
                submitter="0xalice",
                model_hash="0xmodel",
                limit=10,
            )

        assert results == []
        params = route.calls[0].request.url.params
        assert params["status"] == "submitted"
        assert params["submitter"] == "0xalice"
        assert params["model_hash"] == "0xmodel"
        assert params["limit"] == "10"

    @respx.mock
    def test_list_results_empty(self):
        respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[])
        )

        with Client() as client:
            results = client.list_results()

        assert results == []

    @respx.mock
    def test_list_results_server_error(self):
        respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(
                500, json={"error": "storage lock poisoned"}
            )
        )

        with Client(max_retries=1) as client:
            with pytest.raises(ApiError) as exc_info:
                client.list_results()

        assert exc_info.value.http_status == 500


# ---------------------------------------------------------------------------
# Client.get_result()
# ---------------------------------------------------------------------------


class TestClientGetResult:
    """Tests for GET /api/v1/results/{id}."""

    @respx.mock
    def test_get_result_found(self):
        respx.get("http://localhost:8081/api/v1/results/0xabc123").mock(
            return_value=httpx.Response(200, json=RESULT_ROW_JSON)
        )

        with Client() as client:
            result = client.get_result("0xabc123")

        assert result is not None
        assert isinstance(result, ResultRow)
        assert result.id == "0xabc123"
        assert result.submitter == "0x" + "cc" * 20
        assert result.block_number == 19542050
        assert result.timestamp == 1700000000
        assert result.challenger is None

    @respx.mock
    def test_get_result_not_found_returns_none(self):
        respx.get("http://localhost:8081/api/v1/results/0xnonexistent").mock(
            return_value=httpx.Response(404, json=NOT_FOUND_JSON)
        )

        with Client(max_retries=1) as client:
            result = client.get_result("0xnonexistent")

        assert result is None

    @respx.mock
    def test_get_result_server_error_raises(self):
        respx.get("http://localhost:8081/api/v1/results/0xbad").mock(
            return_value=httpx.Response(
                500, json={"error": "database error"}
            )
        )

        with Client(max_retries=1) as client:
            with pytest.raises(ApiError) as exc_info:
                client.get_result("0xbad")

        assert exc_info.value.http_status == 500


# ---------------------------------------------------------------------------
# ResultRow model properties
# ---------------------------------------------------------------------------


class TestResultRowProperties:
    """Tests for ResultRow dataclass properties."""

    def test_is_finalized(self):
        row = ResultRow.from_dict(FINALIZED_ROW_JSON)
        assert row.is_finalized is True
        assert row.is_challenged is False

    def test_is_challenged(self):
        data = {**RESULT_ROW_JSON, "status": "challenged", "challenger": "0x" + "dd" * 20}
        row = ResultRow.from_dict(data)
        assert row.is_challenged is True
        assert row.is_finalized is False
        assert row.challenger == "0x" + "dd" * 20

    def test_submitted_is_neither(self):
        row = ResultRow.from_dict(RESULT_ROW_JSON)
        assert row.is_finalized is False
        assert row.is_challenged is False


# ---------------------------------------------------------------------------
# IndexerStatsResponse model
# ---------------------------------------------------------------------------


class TestIndexerStatsResponse:
    """Tests for IndexerStatsResponse dataclass."""

    def test_from_dict(self):
        stats = IndexerStatsResponse.from_dict(STATS_JSON)
        assert stats.total_submitted == 100
        assert stats.total_challenged == 5
        assert stats.total_finalized == 90
        assert stats.total_resolved == 3

    def test_total(self):
        stats = IndexerStatsResponse.from_dict(STATS_JSON)
        assert stats.total == 198

    def test_defaults_to_zero(self):
        stats = IndexerStatsResponse.from_dict({})
        assert stats.total_submitted == 0
        assert stats.total_challenged == 0
        assert stats.total_finalized == 0
        assert stats.total_resolved == 0
        assert stats.total == 0


# ---------------------------------------------------------------------------
# IndexerHealthResponse model
# ---------------------------------------------------------------------------


class TestIndexerHealthResponse:
    """Tests for IndexerHealthResponse dataclass."""

    def test_from_dict(self):
        health = IndexerHealthResponse.from_dict(HEALTH_JSON)
        assert health.status == "ok"
        assert health.last_indexed_block == 19542100
        assert health.total_results == 256

    def test_defaults(self):
        health = IndexerHealthResponse.from_dict({})
        assert health.status == "unknown"
        assert health.last_indexed_block == 0
        assert health.total_results == 0


# ---------------------------------------------------------------------------
# Client headers
# ---------------------------------------------------------------------------


class TestClientHeaders:
    """Tests for request headers."""

    @respx.mock
    def test_default_headers(self):
        route = respx.get("http://localhost:8081/health").mock(
            return_value=httpx.Response(200, json=HEALTH_JSON)
        )

        with Client() as client:
            client.health()

        headers = route.calls[0].request.headers
        assert headers["accept"] == "application/json"
        assert headers["user-agent"] == "worldzk-python/1.0.0"

    @respx.mock
    def test_api_key_header(self):
        route = respx.get("http://localhost:8081/health").mock(
            return_value=httpx.Response(200, json=HEALTH_JSON)
        )

        with Client(api_key="my-secret-key") as client:
            client.health()

        headers = route.calls[0].request.headers
        assert headers["x-api-key"] == "my-secret-key"

    @respx.mock
    def test_no_api_key_header_when_none(self):
        route = respx.get("http://localhost:8081/health").mock(
            return_value=httpx.Response(200, json=HEALTH_JSON)
        )

        with Client() as client:
            client.health()

        headers = route.calls[0].request.headers
        assert "x-api-key" not in headers


# ---------------------------------------------------------------------------
# AsyncClient
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
class TestAsyncClientHealth:
    """Tests for AsyncClient.health()."""

    @respx.mock
    async def test_health(self):
        respx.get("http://localhost:8081/health").mock(
            return_value=httpx.Response(200, json=HEALTH_JSON)
        )

        async with AsyncClient() as client:
            health = await client.health()

        assert isinstance(health, IndexerHealthResponse)
        assert health.status == "ok"
        assert health.last_indexed_block == 19542100


@pytest.mark.asyncio
class TestAsyncClientStats:
    """Tests for AsyncClient.stats()."""

    @respx.mock
    async def test_stats(self):
        respx.get("http://localhost:8081/api/v1/stats").mock(
            return_value=httpx.Response(200, json=STATS_JSON)
        )

        async with AsyncClient() as client:
            stats = await client.stats()

        assert isinstance(stats, IndexerStatsResponse)
        assert stats.total_submitted == 100
        assert stats.total_finalized == 90


@pytest.mark.asyncio
class TestAsyncClientListResults:
    """Tests for AsyncClient.list_results()."""

    @respx.mock
    async def test_list_results(self):
        respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[RESULT_ROW_JSON])
        )

        async with AsyncClient() as client:
            results = await client.list_results()

        assert len(results) == 1
        assert results[0].id == "0xabc123"

    @respx.mock
    async def test_list_results_with_filters(self):
        route = respx.get("http://localhost:8081/api/v1/results").mock(
            return_value=httpx.Response(200, json=[FINALIZED_ROW_JSON])
        )

        async with AsyncClient() as client:
            results = await client.list_results(status="finalized", limit=5)

        assert len(results) == 1
        params = route.calls[0].request.url.params
        assert params["status"] == "finalized"
        assert params["limit"] == "5"


@pytest.mark.asyncio
class TestAsyncClientGetResult:
    """Tests for AsyncClient.get_result()."""

    @respx.mock
    async def test_get_result_found(self):
        respx.get("http://localhost:8081/api/v1/results/0xabc123").mock(
            return_value=httpx.Response(200, json=RESULT_ROW_JSON)
        )

        async with AsyncClient() as client:
            result = await client.get_result("0xabc123")

        assert result is not None
        assert result.id == "0xabc123"

    @respx.mock
    async def test_get_result_not_found(self):
        respx.get("http://localhost:8081/api/v1/results/0xmissing").mock(
            return_value=httpx.Response(404, json=NOT_FOUND_JSON)
        )

        async with AsyncClient(max_retries=1) as client:
            result = await client.get_result("0xmissing")

        assert result is None

    @respx.mock
    async def test_get_result_server_error(self):
        respx.get("http://localhost:8081/api/v1/results/0xbad").mock(
            return_value=httpx.Response(500, json={"error": "db error"})
        )

        async with AsyncClient(max_retries=1) as client:
            with pytest.raises(ApiError):
                await client.get_result("0xbad")


# ---------------------------------------------------------------------------
# Import paths / backward compat
# ---------------------------------------------------------------------------


class TestImports:
    """Verify that the public API is importable from worldzk."""

    def test_client_importable(self):
        from worldzk import Client, AsyncClient
        assert Client is not None
        assert AsyncClient is not None

    def test_result_row_importable(self):
        from worldzk import ResultRow, ResultStatus
        assert ResultRow is not None
        assert ResultStatus is not None

    def test_indexer_responses_importable(self):
        from worldzk import IndexerHealthResponse, IndexerStatsResponse
        assert IndexerHealthResponse is not None
        assert IndexerStatsResponse is not None

    def test_backward_compat_aliases(self):
        """HealthResponse and StatsResponse aliases in client module."""
        from worldzk.client import HealthResponse, StatsResponse
        assert HealthResponse is IndexerHealthResponse
        assert StatsResponse is IndexerStatsResponse
