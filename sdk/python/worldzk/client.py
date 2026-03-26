"""World ZK Compute Python SDK — unified client for prove + verify + receipts.

Usage:
    from worldzk import Client

    client = Client(verifier_url="http://localhost:3000", registry_url="http://localhost:3001")

    # Verify a proof bundle
    result = client.verify(bundle)

    # Submit to registry
    receipt = client.submit(bundle)

    # Search proofs
    proofs = client.search(circuit_hash="0xabc...")
"""

import json
from typing import Any, Dict, List, Optional
from urllib.request import Request, urlopen
from urllib.error import HTTPError, URLError


class Client:
    """Unified client for World ZK Compute services."""

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

    def verify(self, bundle: Dict[str, Any]) -> Dict[str, Any]:
        """Verify a proof bundle via the verifier API."""
        return self._post(f"{self.verifier_url}/verify", bundle)

    def verify_hybrid(self, bundle: Dict[str, Any]) -> Dict[str, Any]:
        """Hybrid verification (transcript only, no EC checks)."""
        return self._post(f"{self.verifier_url}/verify/hybrid", bundle)

    def verify_batch(self, bundles: List[Dict[str, Any]]) -> Dict[str, Any]:
        """Batch verify multiple proof bundles."""
        return self._post(f"{self.verifier_url}/verify/batch", {"bundles": bundles})

    def submit(self, bundle: Dict[str, Any]) -> Dict[str, Any]:
        """Submit a proof bundle to the registry."""
        return self._post(f"{self.registry_url}/proofs", bundle)

    def get_proof(self, proof_id: str) -> Dict[str, Any]:
        """Retrieve a proof from the registry by ID."""
        return self._get(f"{self.registry_url}/proofs/{proof_id}")

    def search(
        self,
        circuit_hash: Optional[str] = None,
        model_hash: Optional[str] = None,
        limit: int = 50,
    ) -> Dict[str, Any]:
        """Search proofs in the registry."""
        params = [f"limit={limit}"]
        if circuit_hash:
            params.append(f"circuit_hash={circuit_hash}")
        if model_hash:
            params.append(f"model_hash={model_hash}")
        query = "&".join(params)
        return self._get(f"{self.registry_url}/proofs?{query}")

    def prove(self, model_id: str, features: List[float]) -> Dict[str, Any]:
        """Generate a proof via the proof generator service."""
        return self._post(
            f"{self.generator_url}/prove",
            {"model_id": model_id, "features": features},
        )

    def health(self) -> Dict[str, Dict[str, Any]]:
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

    def _post(self, url: str, data: Any) -> Dict[str, Any]:
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

    def _get(self, url: str) -> Dict[str, Any]:
        req = Request(url)
        try:
            with urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except HTTPError as e:
            error_body = e.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"HTTP {e.code}: {error_body}") from e
        except URLError as e:
            raise ConnectionError(f"Cannot connect to {url}: {e}") from e
