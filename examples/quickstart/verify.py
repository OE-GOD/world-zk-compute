#!/usr/bin/env python3
"""Quickstart: Verify a ZKML proof bundle in Python.

This script demonstrates three verification methods:
1. Native library (via zkml-verifier pip package)
2. REST API (via the verifier Docker service)
3. Direct file loading + JSON verification

Usage:
    python verify.py <proof_bundle.json>
    python verify.py                         # uses test fixture if available

Environment variables:
    VERIFIER_URL  -- REST API base URL (default: http://localhost:3000)
    ZKML_LIB_DIR  -- Directory containing libzkml_verifier.{so,dylib}
"""

import json
import os
import sys
from pathlib import Path


def find_proof_bundle() -> str:
    """Locate a proof bundle file from CLI args or test fixtures."""
    if len(sys.argv) > 1:
        path = sys.argv[1]
        if not os.path.exists(path):
            print(f"Error: file not found: {path}")
            sys.exit(1)
        return path

    # Try test fixtures in order of preference
    script_dir = Path(__file__).resolve().parent
    candidates = [
        script_dir / "sample_proof.json",
        script_dir.parent.parent / "contracts" / "test" / "fixtures" / "phase1a_dag_fixture.json",
        script_dir.parent.parent / "web" / "sample_proof.json",
    ]
    for candidate in candidates:
        if candidate.exists():
            print(f"Using fixture: {candidate}")
            return str(candidate)

    print("Usage: python verify.py <proof_bundle.json>")
    print()
    print("No proof bundle provided and no test fixture found.")
    print("Generate one with: cargo run -p zkml-verifier -- bundle ...")
    sys.exit(1)


def verify_native(bundle_path: str) -> bool:
    """Verify using the native Rust library via ctypes FFI.

    Requires: pip install zkml-verifier
    Or: cargo build --release -p zkml-verifier + set ZKML_LIB_DIR
    """
    try:
        from zkml_verifier import verify_file, version

        print(f"zkml-verifier version: {version()}")
        result = verify_file(bundle_path)
        print(f"Verified (native): {result['verified']}")
        if result.get("error"):
            print(f"  Error detail: {result['error']}")
        return result["verified"]
    except ImportError:
        print("Native library not available (pip install zkml-verifier)")
        return False
    except Exception as e:
        print(f"Native verification failed: {e}")
        return False


def verify_native_json(bundle_path: str) -> bool:
    """Verify using the native library with a JSON string."""
    try:
        from zkml_verifier import verify_json

        with open(bundle_path) as f:
            json_str = f.read()

        result = verify_json(json_str)
        print(f"Verified (native/json): {result['verified']}")
        return result["verified"]
    except ImportError:
        return False
    except Exception as e:
        print(f"Native JSON verification failed: {e}")
        return False


def verify_rest_api(bundle_path: str) -> bool:
    """Verify using the REST API service.

    Requires: docker compose -f docker-compose.verifier.yml up -d
    """
    import urllib.request
    import urllib.error

    url = os.environ.get("VERIFIER_URL", "http://localhost:3000")

    with open(bundle_path) as f:
        body = f.read().encode("utf-8")

    req = urllib.request.Request(
        f"{url}/verify",
        data=body,
        headers={"Content-Type": "application/json"},
    )

    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            result = json.loads(resp.read())
            print(f"Verified (REST API): {result.get('verified')}")
            if "circuit_hash" in result:
                print(f"  Circuit hash: {result['circuit_hash']}")
            return result.get("verified", False)
    except urllib.error.URLError as e:
        print(f"REST API not available: {e}")
        print(f"  Start with: docker compose -f docker-compose.verifier.yml up -d")
        return False
    except Exception as e:
        print(f"REST API error: {e}")
        return False


def main():
    bundle_path = find_proof_bundle()

    # Show bundle info
    with open(bundle_path) as f:
        bundle = json.load(f)
    print(f"Proof bundle: {bundle_path}")
    print(f"  proof_hex:  {len(bundle.get('proof_hex', ''))} hex chars")
    print(f"  gens_hex:   {len(bundle.get('gens_hex', ''))} hex chars")
    if bundle.get("circuit_hash"):
        print(f"  circuit:    {bundle['circuit_hash']}")
    if bundle.get("model_hash"):
        print(f"  model:      {bundle['model_hash']}")
    print()

    # Try native library first, then REST API
    print("--- Method 1: Native Library ---")
    native_ok = verify_native(bundle_path)

    if not native_ok:
        print()
        print("--- Method 2: REST API ---")
        verify_rest_api(bundle_path)


if __name__ == "__main__":
    main()
