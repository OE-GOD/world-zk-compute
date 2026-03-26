#!/usr/bin/env python3
"""Quickstart: Verify a ZKML proof bundle in Python.

Usage:
    python verify.py <proof_bundle.json>
    python verify.py  # uses test fixture if available
"""

import json
import sys
import os


def main():
    # Find proof bundle
    if len(sys.argv) > 1:
        path = sys.argv[1]
    else:
        # Try test fixture
        fixture = os.path.join(
            os.path.dirname(__file__),
            "../../contracts/test/fixtures/phase1a_dag_fixture.json",
        )
        if os.path.exists(fixture):
            path = fixture
            print(f"Using test fixture: {path}")
        else:
            print("Usage: python verify.py <proof_bundle.json>")
            sys.exit(1)

    # Load bundle
    with open(path) as f:
        bundle = json.load(f)

    print(f"Proof size: {len(bundle.get('proof_hex', ''))} hex chars")
    print(f"Gens size: {len(bundle.get('gens_hex', ''))} hex chars")

    # Try native library first
    try:
        from zkml_verifier import verify_json

        result = verify_json(json.dumps(bundle))
        print(f"Verified (native): {result['verified']}")
        if result.get("error"):
            print(f"Error: {result['error']}")
        return
    except ImportError:
        print("Native library not found, trying REST API...")

    # Fall back to REST API
    try:
        import urllib.request

        url = os.environ.get("VERIFIER_URL", "http://localhost:3000")
        req = urllib.request.Request(
            f"{url}/verify",
            data=json.dumps(bundle).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=30) as resp:
            result = json.loads(resp.read())
            print(f"Verified (API): {result.get('verified')}")
            if "circuit_hash" in result:
                print(f"Circuit hash: {result['circuit_hash']}")
    except Exception as e:
        print(f"REST API error: {e}")
        print("Start the verifier: docker compose -f docker-compose.bank-demo.yml up -d")
        sys.exit(1)


if __name__ == "__main__":
    main()
