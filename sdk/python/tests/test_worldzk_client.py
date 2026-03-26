"""Tests for the WorldZK gateway client.

Uses unittest.mock to mock urllib.request.urlopen so no real gateway is needed.
"""

from __future__ import annotations

import io
import json
import os
import tempfile
from typing import Any, Dict
from unittest import mock
from urllib.error import HTTPError, URLError

import pytest

from worldzk.client import WorldZK
from worldzk.models import GatewayHealth, ProofResult, Receipt, VerificationResult


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _mock_response(data: Any, status: int = 200) -> mock.MagicMock:
    """Build a mock urllib response that returns JSON-encoded *data*."""
    body = json.dumps(data).encode("utf-8")
    resp = mock.MagicMock()
    resp.read.return_value = body
    resp.status = status
    resp.__enter__ = mock.MagicMock(return_value=resp)
    resp.__exit__ = mock.MagicMock(return_value=False)
    return resp


def _http_error(code: int, body: Dict[str, Any]) -> HTTPError:
    """Build an HTTPError with a JSON body."""
    data = json.dumps(body).encode("utf-8")
    err = HTTPError(
        url="http://localhost:8080/test",
        code=code,
        msg=f"HTTP {code}",
        hdrs={},  # type: ignore[arg-type]
        fp=io.BytesIO(data),
    )
    return err


# ---------------------------------------------------------------------------
# Construction & defaults
# ---------------------------------------------------------------------------


class TestWorldZKDefaults:
    """Verify default configuration values."""

    def test_default_url(self):
        zk = WorldZK()
        assert zk.url == "http://localhost:8080"

    def test_custom_url(self):
        zk = WorldZK(url="http://myhost:9090")
        assert zk.url == "http://myhost:9090"

    def test_strips_trailing_slash(self):
        zk = WorldZK(url="http://myhost:9090/")
        assert zk.url == "http://myhost:9090"

    def test_api_key_stored(self):
        zk = WorldZK(api_key="secret")
        assert zk.api_key == "secret"

    def test_api_key_default_none(self):
        zk = WorldZK()
        assert zk.api_key is None

    def test_timeout_default(self):
        zk = WorldZK()
        assert zk.timeout == 120

    def test_custom_timeout(self):
        zk = WorldZK(timeout=60)
        assert zk.timeout == 60

    def test_context_manager(self):
        with WorldZK() as zk:
            assert isinstance(zk, WorldZK)


# ---------------------------------------------------------------------------
# Headers / auth
# ---------------------------------------------------------------------------


class TestWorldZKHeaders:
    """Verify request headers are set correctly."""

    @mock.patch("worldzk.client.urlopen")
    def test_no_auth_header_when_no_key(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({"status": "healthy"})
        zk = WorldZK()
        zk.health()

        req = mock_urlopen.call_args[0][0]
        assert req.get_header("Authorization") is None

    @mock.patch("worldzk.client.urlopen")
    def test_bearer_token_when_key_set(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({"status": "healthy"})
        zk = WorldZK(api_key="my-secret")
        zk.health()

        req = mock_urlopen.call_args[0][0]
        assert req.get_header("Authorization") == "Bearer my-secret"

    @mock.patch("worldzk.client.urlopen")
    def test_content_type_on_post(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({"verified": True})
        zk = WorldZK()
        zk.verify("proof-1")

        req = mock_urlopen.call_args[0][0]
        assert req.get_header("Content-type") == "application/json"


# ---------------------------------------------------------------------------
# upload_model
# ---------------------------------------------------------------------------


class TestWorldZKUploadModel:
    """Tests for WorldZK.upload_model()."""

    @mock.patch("worldzk.client.urlopen")
    def test_upload_model_basic(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "model_id": "m-123",
            "model_hash": "0xabc",
        })

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False
        ) as f:
            json.dump({"trees": [1, 2, 3]}, f)
            f.flush()
            tmp_path = f.name

        try:
            zk = WorldZK()
            result = zk.upload_model(tmp_path)

            assert result["model_id"] == "m-123"
            assert result["model_hash"] == "0xabc"

            # Verify the request payload
            req = mock_urlopen.call_args[0][0]
            assert req.full_url == "http://localhost:8080/models"
            assert req.get_method() == "POST"
            body = json.loads(req.data)
            assert "model_json" in body
        finally:
            os.unlink(tmp_path)

    @mock.patch("worldzk.client.urlopen")
    def test_upload_model_with_name_and_format(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({"model_id": "m-456"})

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False
        ) as f:
            f.write("{}")
            f.flush()
            tmp_path = f.name

        try:
            zk = WorldZK()
            zk.upload_model(tmp_path, name="my-model", format="xgboost")

            req = mock_urlopen.call_args[0][0]
            body = json.loads(req.data)
            assert body["name"] == "my-model"
            assert body["format"] == "xgboost"
        finally:
            os.unlink(tmp_path)

    def test_upload_model_file_not_found(self):
        zk = WorldZK()
        with pytest.raises(FileNotFoundError):
            zk.upload_model("/nonexistent/path/model.json")


# ---------------------------------------------------------------------------
# list_models / get_model
# ---------------------------------------------------------------------------


class TestWorldZKListModels:
    """Tests for WorldZK.list_models() and get_model()."""

    @mock.patch("worldzk.client.urlopen")
    def test_list_models(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response([
            {"model_id": "m-1", "name": "model-a"},
            {"model_id": "m-2", "name": "model-b"},
        ])

        zk = WorldZK()
        models = zk.list_models()

        assert len(models) == 2
        assert models[0]["model_id"] == "m-1"

    @mock.patch("worldzk.client.urlopen")
    def test_get_model(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "model_id": "m-1",
            "name": "test",
        })

        zk = WorldZK()
        model = zk.get_model("m-1")

        assert model["model_id"] == "m-1"
        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/models/m-1"


# ---------------------------------------------------------------------------
# prove
# ---------------------------------------------------------------------------


class TestWorldZKProve:
    """Tests for WorldZK.prove()."""

    @mock.patch("worldzk.client.urlopen")
    def test_prove_returns_proof_result(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "proof_id": "p-123",
            "model_hash": "0xaa",
            "circuit_hash": "0xbb",
            "output": "42",
        })

        zk = WorldZK()
        result = zk.prove("m-1", [1.0, 2.0, 3.0])

        assert isinstance(result, ProofResult)
        assert result.proof_id == "p-123"
        assert result.model_hash == "0xaa"
        assert result.circuit_hash == "0xbb"
        assert result.output == "42"

    @mock.patch("worldzk.client.urlopen")
    def test_prove_sends_correct_payload(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "proof_id": "p-1",
            "model_hash": "",
            "circuit_hash": "",
            "output": "",
        })

        zk = WorldZK()
        zk.prove("model-abc", [1.5, 2.5])

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/prove"
        body = json.loads(req.data)
        assert body["model_id"] == "model-abc"
        assert body["features"] == [1.5, 2.5]

    @mock.patch("worldzk.client.urlopen")
    def test_prove_handles_missing_fields(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({"proof_id": "p-2"})

        zk = WorldZK()
        result = zk.prove("m-1", [1.0])

        assert result.proof_id == "p-2"
        assert result.model_hash == ""
        assert result.circuit_hash == ""
        assert result.output == ""


# ---------------------------------------------------------------------------
# verify
# ---------------------------------------------------------------------------


class TestWorldZKVerify:
    """Tests for WorldZK.verify()."""

    @mock.patch("worldzk.client.urlopen")
    def test_verify_success(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "verified": True,
            "receipt_id": "r-123",
        })

        zk = WorldZK()
        result = zk.verify("p-1")

        assert isinstance(result, VerificationResult)
        assert result.verified is True
        assert result.receipt_id == "r-123"

    @mock.patch("worldzk.client.urlopen")
    def test_verify_failure(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "verified": False,
        })

        zk = WorldZK()
        result = zk.verify("p-bad")

        assert result.verified is False
        assert result.receipt_id is None

    @mock.patch("worldzk.client.urlopen")
    def test_verify_sends_to_correct_url(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({"verified": True})

        zk = WorldZK()
        zk.verify("proof-abc")

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/proofs/proof-abc/verify"
        assert req.get_method() == "POST"


# ---------------------------------------------------------------------------
# get_receipt
# ---------------------------------------------------------------------------


class TestWorldZKGetReceipt:
    """Tests for WorldZK.get_receipt()."""

    @mock.patch("worldzk.client.urlopen")
    def test_get_receipt(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "receipt_id": "r-1",
            "proof_id": "p-1",
            "verified": True,
            "verified_at": "2026-01-01T00:00:00Z",
            "signature": "0xsig",
        })

        zk = WorldZK()
        receipt = zk.get_receipt("p-1")

        assert isinstance(receipt, Receipt)
        assert receipt.receipt_id == "r-1"
        assert receipt.proof_id == "p-1"
        assert receipt.verified is True
        assert receipt.verified_at == "2026-01-01T00:00:00Z"
        assert receipt.signature == "0xsig"

    @mock.patch("worldzk.client.urlopen")
    def test_get_receipt_uses_proof_id_fallback(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "receipt_id": "r-2",
            "verified": False,
            "verified_at": "",
            "signature": "",
        })

        zk = WorldZK()
        receipt = zk.get_receipt("p-fallback")

        # proof_id should fall back to the argument
        assert receipt.proof_id == "p-fallback"

    @mock.patch("worldzk.client.urlopen")
    def test_get_receipt_url(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "receipt_id": "r-1",
            "proof_id": "p-1",
            "verified": True,
            "verified_at": "",
            "signature": "",
        })

        zk = WorldZK()
        zk.get_receipt("p-1")

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/proofs/p-1/receipt"


# ---------------------------------------------------------------------------
# list_proofs
# ---------------------------------------------------------------------------


class TestWorldZKListProofs:
    """Tests for WorldZK.list_proofs()."""

    @mock.patch("worldzk.client.urlopen")
    def test_list_proofs_default(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response([
            {"proof_id": "p-1"},
            {"proof_id": "p-2"},
        ])

        zk = WorldZK()
        proofs = zk.list_proofs()

        assert len(proofs) == 2
        req = mock_urlopen.call_args[0][0]
        assert "limit=50" in req.full_url

    @mock.patch("worldzk.client.urlopen")
    def test_list_proofs_with_model_filter(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response([])

        zk = WorldZK()
        zk.list_proofs(model="m-1", limit=10)

        req = mock_urlopen.call_args[0][0]
        assert "model=m-1" in req.full_url
        assert "limit=10" in req.full_url

    @mock.patch("worldzk.client.urlopen")
    def test_list_proofs_empty(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response([])

        zk = WorldZK()
        proofs = zk.list_proofs()

        assert proofs == []


# ---------------------------------------------------------------------------
# transparency_root
# ---------------------------------------------------------------------------


class TestWorldZKTransparency:
    """Tests for WorldZK.transparency_root()."""

    @mock.patch("worldzk.client.urlopen")
    def test_transparency_root(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "root": "0xdeadbeef",
            "total_proofs": 42,
        })

        zk = WorldZK()
        result = zk.transparency_root()

        assert result["root"] == "0xdeadbeef"
        assert result["total_proofs"] == 42

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/proofs/transparency/root"


# ---------------------------------------------------------------------------
# stats
# ---------------------------------------------------------------------------


class TestWorldZKStats:
    """Tests for WorldZK.stats()."""

    @mock.patch("worldzk.client.urlopen")
    def test_stats(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "total_proofs": 100,
            "total_verified": 90,
        })

        zk = WorldZK()
        result = zk.stats()

        assert result["total_proofs"] == 100

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/stats"


# ---------------------------------------------------------------------------
# list_circuits
# ---------------------------------------------------------------------------


class TestWorldZKCircuits:
    """Tests for WorldZK.list_circuits()."""

    @mock.patch("worldzk.client.urlopen")
    def test_list_circuits(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response([
            {"circuit_hash": "0xabc", "name": "xgboost-remainder"},
        ])

        zk = WorldZK()
        circuits = zk.list_circuits()

        assert len(circuits) == 1
        assert circuits[0]["circuit_hash"] == "0xabc"

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/circuits"


# ---------------------------------------------------------------------------
# health
# ---------------------------------------------------------------------------


class TestWorldZKHealth:
    """Tests for WorldZK.health() and health_service()."""

    @mock.patch("worldzk.client.urlopen")
    def test_health_aggregate(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "status": "healthy",
            "services": [
                {"name": "generator", "healthy": True},
                {"name": "registry", "healthy": True},
                {"name": "verifier", "healthy": True},
            ],
            "healthy_count": 3,
            "total_count": 3,
        })

        zk = WorldZK()
        health = zk.health()

        assert isinstance(health, GatewayHealth)
        assert health.status == "healthy"
        assert health.healthy_count == 3
        assert health.total_count == 3
        assert len(health.services) == 3

    @mock.patch("worldzk.client.urlopen")
    def test_health_degraded(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "status": "degraded",
            "services": [
                {"name": "generator", "healthy": True},
                {"name": "registry", "healthy": False},
                {"name": "verifier", "healthy": True},
            ],
            "healthy_count": 2,
            "total_count": 3,
        })

        zk = WorldZK()
        health = zk.health()

        assert health.status == "degraded"
        assert health.healthy_count == 2

    @mock.patch("worldzk.client.urlopen")
    def test_health_service(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "name": "verifier",
            "healthy": True,
            "response_ms": 12,
        })

        zk = WorldZK()
        result = zk.health_service("verifier")

        assert result["name"] == "verifier"
        assert result["healthy"] is True

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "http://localhost:8080/health/verifier"


# ---------------------------------------------------------------------------
# Error handling
# ---------------------------------------------------------------------------


class TestWorldZKErrors:
    """Tests for HTTP error handling."""

    @mock.patch("worldzk.client.urlopen")
    def test_401_unauthorized(self, mock_urlopen):
        mock_urlopen.side_effect = _http_error(
            401, {"error": "invalid or missing API key"}
        )

        zk = WorldZK()
        with pytest.raises(RuntimeError, match="HTTP 401"):
            zk.list_proofs()

    @mock.patch("worldzk.client.urlopen")
    def test_404_not_found(self, mock_urlopen):
        mock_urlopen.side_effect = _http_error(
            404, {"error": "proof not found"}
        )

        zk = WorldZK()
        with pytest.raises(RuntimeError, match="HTTP 404"):
            zk.get_receipt("nonexistent")

    @mock.patch("worldzk.client.urlopen")
    def test_500_server_error(self, mock_urlopen):
        mock_urlopen.side_effect = _http_error(
            500, {"error": "internal server error"}
        )

        zk = WorldZK()
        with pytest.raises(RuntimeError, match="HTTP 500"):
            zk.stats()

    @mock.patch("worldzk.client.urlopen")
    def test_502_bad_gateway(self, mock_urlopen):
        mock_urlopen.side_effect = _http_error(
            502, {"error": "upstream error"}
        )

        zk = WorldZK()
        with pytest.raises(RuntimeError, match="HTTP 502"):
            zk.prove("m-1", [1.0])

    @mock.patch("worldzk.client.urlopen")
    def test_connection_error(self, mock_urlopen):
        mock_urlopen.side_effect = URLError("Connection refused")

        zk = WorldZK()
        with pytest.raises(ConnectionError, match="Cannot connect"):
            zk.health()


# ---------------------------------------------------------------------------
# Custom gateway URL
# ---------------------------------------------------------------------------


class TestWorldZKCustomURL:
    """Verify custom gateway URL is used in all requests."""

    @mock.patch("worldzk.client.urlopen")
    def test_custom_url_in_prove(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "proof_id": "p-1",
            "model_hash": "",
            "circuit_hash": "",
            "output": "",
        })

        zk = WorldZK(url="https://api.example.com")
        zk.prove("m-1", [1.0])

        req = mock_urlopen.call_args[0][0]
        assert req.full_url.startswith("https://api.example.com/")

    @mock.patch("worldzk.client.urlopen")
    def test_custom_url_in_health(self, mock_urlopen):
        mock_urlopen.return_value = _mock_response({
            "status": "healthy",
            "services": [],
            "healthy_count": 0,
            "total_count": 0,
        })

        zk = WorldZK(url="https://api.example.com")
        zk.health()

        req = mock_urlopen.call_args[0][0]
        assert req.full_url == "https://api.example.com/health"


# ---------------------------------------------------------------------------
# Import / export tests
# ---------------------------------------------------------------------------


class TestWorldZKImports:
    """Verify that WorldZK is importable from the top-level package."""

    def test_worldzk_importable(self):
        from worldzk import WorldZK as WZK

        assert WZK is not None

    def test_models_importable(self):
        from worldzk import (
            GatewayHealth,
            ProofResult,
            Receipt,
            VerificationResult,
        )

        assert GatewayHealth is not None
        assert ProofResult is not None
        assert Receipt is not None
        assert VerificationResult is not None

    def test_legacy_client_still_importable(self):
        from worldzk import Client

        assert Client is not None

    def test_legacy_models_still_importable(self):
        from worldzk import ProofBundle, ProofReceipt

        assert ProofBundle is not None
        assert ProofReceipt is not None
