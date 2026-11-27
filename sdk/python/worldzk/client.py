"""World ZK Compute Python SDK — unified client for prove + verify + receipts.

Usage (gateway client)::

    from worldzk import WorldZK

    zk = WorldZK("http://localhost:8080", api_key="my-key")
    model = zk.upload_model("model.json")
    result = zk.prove(model["model_id"], [1.0, 2.0, 3.0])
    if result.registry_proof_id:  # set when the generator stored the proof
        verification = zk.verify(result.registry_proof_id)

Usage (direct-service client, legacy)::

    from worldzk import Client

    client = Client(verifier_url="http://localhost:3000", registry_url="http://localhost:3001")
    result = client.verify(bundle)
    receipt = client.submit(bundle)
    proofs = client.search(circuit_hash="0xabc...")
"""

import json
import os
from typing import Any, Optional
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from .models import GatewayHealth, ProofResult, Receipt, VerificationResult

# ---------------------------------------------------------------------------
# WorldZK — gateway-facing client
# ---------------------------------------------------------------------------


class WorldZK:
    """Client for the World ZK Compute off-chain verification platform.

    Talks to the unified API gateway which reverse-proxies requests to the
    proof-generator, proof-registry, and verifier backend services.

    Parameters
    ----------
    url:
        Base URL of the API gateway (default ``http://localhost:8080``).
    api_key:
        Optional Bearer token for authenticated endpoints.
    timeout:
        HTTP request timeout in seconds (default 120, matching the gateway's
        upstream timeout).
    """

    def __init__(
        self,
        url: str = "http://localhost:8080",
        api_key: Optional[str] = None,
        timeout: int = 120,
    ):
        self.url = url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout

    # -- context manager ------------------------------------------------

    def __enter__(self) -> "WorldZK":
        return self

    def __exit__(self, *args: Any) -> None:
        pass  # stdlib urllib has no persistent connection pool to close

    # -- Models ---------------------------------------------------------

    def upload_model(
        self,
        path: str,
        name: Optional[str] = None,
        format: Optional[str] = None,
    ) -> dict[str, Any]:
        """Upload an ML model file for proof generation.

        Parameters
        ----------
        path:
            Path to a model JSON file on disk.
        name:
            Human-readable model name. The service requires one; defaults
            to the file name without its extension.
        format:
            Model format hint, e.g. ``"xgboost"`` (optional; the service
            detects the format when omitted).

        Returns
        -------
        dict
            ``model_id``, ``model_hash``, ``circuit_hash``, ``format`` and
            ``active``.
        """
        with open(path) as f:
            model_json = json.load(f)
        payload: dict[str, Any] = {
            "name": name or os.path.splitext(os.path.basename(path))[0],
            "model_json": model_json,
        }
        if format:
            payload["format"] = format
        return self._post("/models", payload)

    def list_models(self) -> list[dict[str, Any]]:
        """List all uploaded models (each has ``id``, ``name``, ``model_hash``, ...)."""
        return _unwrap(self._get("/models"), "models")

    def get_model(self, model_id: str) -> dict[str, Any]:
        """Retrieve a model by its ID."""
        return self._get(f"/models/{model_id}")

    # -- Prove ----------------------------------------------------------

    def prove(
        self,
        model_id: str,
        features: list[float],
    ) -> ProofResult:
        """Generate a proof for model inference.

        Parameters
        ----------
        model_id:
            Identifier of the previously uploaded model.
        features:
            Input feature vector for inference.

        Returns
        -------
        ProofResult
            Contains ``proof_id``, ``model_hash``, ``circuit_hash``,
            ``output``, ``mock`` and ``registry_proof_id``. ``proof_id`` is
            the generator's own identifier; :meth:`verify` and
            :meth:`get_receipt` take ``registry_proof_id``, which is ``None``
            when the generator did not store the proof in the registry.
        """
        resp = self._post("/prove", {
            "model_id": model_id,
            "features": features,
        })
        return ProofResult(
            proof_id=resp.get("proof_id", ""),
            model_hash=resp.get("model_hash", ""),
            circuit_hash=resp.get("circuit_hash", ""),
            output=resp.get("output", ""),
            mock=resp.get("mock", False),
            registry_proof_id=resp.get("registry_proof_id"),
        )

    # -- Verify ---------------------------------------------------------

    def verify(self, proof_id: str) -> VerificationResult:
        """Verify a stored proof.

        Parameters
        ----------
        proof_id:
            Registry identifier of the proof to verify.

        Returns
        -------
        VerificationResult
            Contains ``verified`` flag and optional ``receipt_id``.
        """
        resp = self._post(f"/proofs/{proof_id}/verify", {})
        return VerificationResult(
            verified=resp.get("verified", False),
            receipt_id=resp.get("receipt_id"),
        )

    def get_receipt(self, proof_id: str) -> Receipt:
        """Download a verification receipt for a proof.

        Parameters
        ----------
        proof_id:
            Registry identifier of the proof whose receipt to retrieve.

        Returns
        -------
        Receipt
            Full receipt with ``receipt_id``, ``proof_id``, verification
            status, timestamp, and signature.
        """
        resp = self._get(f"/proofs/{proof_id}/receipt")
        return Receipt(
            receipt_id=resp.get("receipt_id", ""),
            proof_id=resp.get("proof_id", proof_id),
            verified=resp.get("verified", False),
            verified_at=resp.get("verified_at", ""),
            signature=resp.get("signature", ""),
        )

    # -- Search / list --------------------------------------------------

    def list_proofs(
        self,
        model: Optional[str] = None,
        limit: int = 50,
    ) -> list[dict[str, Any]]:
        """List proofs, optionally filtered by model.

        Parameters
        ----------
        model:
            Filter by exact model hash (optional).
        limit:
            Maximum number of results (default 50).
        """
        params = [f"limit={limit}"]
        if model:
            params.append(f"model={model}")
        query = "&".join(params)
        return _unwrap(self._get(f"/proofs?{query}"), "proofs")

    # -- Transparency ---------------------------------------------------

    def transparency_root(self) -> dict[str, Any]:
        """Get the current transparency log Merkle root.

        The proof registry serves this at ``/transparency/root``. The gateway
        does not forward that path today (it answers 404), so this call only
        works when ``url`` points at the proof registry itself.

        Returns
        -------
        dict
            Contains ``root`` hash and ``tree_size``.
        """
        return self._get("/transparency/root")

    # -- Stats ----------------------------------------------------------

    def stats(self) -> dict[str, Any]:
        """Get proof registry statistics."""
        return self._get("/stats")

    # -- Circuits -------------------------------------------------------

    def list_circuits(self) -> list[str]:
        """List the IDs of the circuits registered with the verifier."""
        return _unwrap(self._get("/circuits"), "circuits")

    # -- Health ---------------------------------------------------------

    def health(self) -> GatewayHealth:
        """Check aggregated health of all backend services.

        Returns
        -------
        GatewayHealth
            Overall status and per-service health details.
        """
        resp = self._get("/health")
        return GatewayHealth(
            status=resp.get("status", "unknown"),
            services=resp.get("services", []),
            healthy_count=resp.get("healthy_count", 0),
            total_count=resp.get("total_count", 0),
        )

    def health_service(self, service: str) -> dict[str, Any]:
        """Check health of a specific backend service.

        Parameters
        ----------
        service:
            One of ``"verifier"``, ``"registry"``, ``"generator"``.
        """
        return self._get(f"/health/{service}")

    # -- HTTP helpers ---------------------------------------------------

    def _headers(self) -> dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    def _post(self, path: str, data: Any) -> Any:
        url = f"{self.url}{path}"
        body = json.dumps(data).encode("utf-8")
        req = Request(url, data=body, headers=self._headers())
        return self._send(req, url)

    def _get(self, path: str) -> Any:
        url = f"{self.url}{path}"
        req = Request(url, headers=self._headers())
        return self._send(req, url)

    def _send(self, req: Request, url: str) -> Any:
        try:
            with urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except HTTPError as e:
            error_body = e.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"HTTP {e.code}: {error_body}") from e
        except URLError as e:
            raise ConnectionError(f"Cannot connect to {url}: {e}") from e


def _unwrap(resp: Any, key: str) -> Any:
    """Return the list inside a ``{key: [...]}`` response envelope."""
    if isinstance(resp, dict):
        return resp.get(key, [])
    return resp


# ---------------------------------------------------------------------------
# Client — direct-service client (legacy, pre-gateway)
# ---------------------------------------------------------------------------


class Client:
    """Direct client for individual World ZK Compute backend services.

    Prefer :class:`WorldZK` for new code -- it talks to the unified gateway
    instead of requiring separate URLs for each backend.
    """

    def __init__(
        self,
        verifier_url: str = "http://localhost:3000",
        registry_url: str = "http://localhost:3001",
        generator_url: str = "http://localhost:3002",
        timeout: int = 30,
    ):
        self.verifier_url = verifier_url.rstrip("/")
        self.registry_url = registry_url.rstrip("/")
        self.generator_url = generator_url.rstrip("/")
        self.timeout = timeout

    def verify(self, bundle: dict[str, Any]) -> dict[str, Any]:
        """Verify a proof bundle via the verifier API."""
        return self._post(f"{self.verifier_url}/verify", bundle)

    def verify_hybrid(self, bundle: dict[str, Any]) -> dict[str, Any]:
        """Hybrid verification (transcript only, no EC checks)."""
        return self._post(f"{self.verifier_url}/verify/hybrid", bundle)

    def verify_batch(self, bundles: list[dict[str, Any]]) -> dict[str, Any]:
        """Batch verify multiple proof bundles."""
        return self._post(f"{self.verifier_url}/verify/batch", {"bundles": bundles})

    def submit(self, bundle: dict[str, Any]) -> dict[str, Any]:
        """Submit a proof bundle to the registry."""
        return self._post(f"{self.registry_url}/proofs", bundle)

    def get_proof(self, proof_id: str) -> dict[str, Any]:
        """Retrieve a proof from the registry by ID."""
        return self._get(f"{self.registry_url}/proofs/{proof_id}")

    def search(
        self,
        circuit_hash: Optional[str] = None,
        model_hash: Optional[str] = None,
        limit: int = 50,
    ) -> dict[str, Any]:
        """Search proofs in the registry.

        Returns ``{"proofs": [...], "count": n}``. The registry filters by
        exact ``model_hash``. It has no circuit filter, so ``circuit_hash``
        is applied here to the (at most ``limit``) proofs it returned.
        """
        params = [f"limit={limit}"]
        if model_hash:
            params.append(f"model={model_hash}")
        query = "&".join(params)
        resp = self._get(f"{self.registry_url}/proofs?{query}")
        if circuit_hash:
            proofs = [p for p in resp.get("proofs", []) if p.get("circuit_hash") == circuit_hash]
            resp = {**resp, "proofs": proofs, "count": len(proofs)}
        return resp

    def prove(self, model_id: str, features: list[float]) -> dict[str, Any]:
        """Generate a proof via the proof generator service."""
        return self._post(
            f"{self.generator_url}/prove",
            {"model_id": model_id, "features": features},
        )

    def health(self) -> dict[str, dict[str, Any]]:
        """Check health of all services."""
        result = {}
        for name, url in [
            ("verifier", self.verifier_url),
            ("registry", self.registry_url),
            ("generator", self.generator_url),
        ]:
            try:
                resp = self._get(f"{url}/health")
                result[name] = {"healthy": True, **resp}
            except Exception as e:
                result[name] = {"healthy": False, "error": str(e)}
        return result

    def _post(self, url: str, data: Any) -> dict[str, Any]:
        body = json.dumps(data).encode("utf-8")
        req = Request(url, data=body, headers={"Content-Type": "application/json"})
        try:
            with urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except HTTPError as e:
            error_body = e.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"HTTP {e.code}: {error_body}") from e
        except URLError as e:
            raise ConnectionError(f"Cannot connect to {url}: {e}") from e

    def _get(self, url: str) -> dict[str, Any]:
        req = Request(url)
        try:
            with urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except HTTPError as e:
            error_body = e.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"HTTP {e.code}: {error_body}") from e
        except URLError as e:
            raise ConnectionError(f"Cannot connect to {url}: {e}") from e
