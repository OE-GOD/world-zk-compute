"""ZKML Verifier — Python wrapper for the zkml-verifier Rust library.

Uses ctypes to call the C-FFI bindings from the shared library.

Usage:
    from zkml_verifier import verify_json, verify_bundle

    # From JSON string
    result = verify_json('{"proof_hex": "0x...", "gens_hex": "0x...", ...}')
    print(result["verified"], result["circuit_hash"])

    # From dict
    bundle = {"proof_hex": "0x...", "gens_hex": "0x...", "dag_circuit_description": {}}
    result = verify_bundle(bundle)
"""

import ctypes
import json
import os
import platform
from pathlib import Path
from typing import Any, Dict, Optional

__version__ = "0.1.0"


def _find_library() -> str:
    """Find the zkml-verifier shared library."""
    system = platform.system()
    if system == "Darwin":
        lib_name = "libzkml_verifier.dylib"
    elif system == "Linux":
        lib_name = "libzkml_verifier.so"
    elif system == "Windows":
        lib_name = "zkml_verifier.dll"
    else:
        raise OSError(f"Unsupported platform: {system}")

    # Search paths in order of priority
    search_dirs = [
        # Explicit env var
        os.environ.get("ZKML_LIB_DIR", ""),
        # Relative to this file (in-tree development: crate-level target)
        str(Path(__file__).parent.parent.parent / "target" / "release"),
        str(Path(__file__).parent.parent.parent / "target" / "debug"),
        # Workspace-level target (workspace root is 4 levels up from __init__.py)
        str(Path(__file__).parent.parent.parent.parent.parent / "target" / "release"),
        str(Path(__file__).parent.parent.parent.parent.parent / "target" / "debug"),
        # System paths
        "/usr/local/lib",
        "/usr/lib",
    ]

    for d in search_dirs:
        if not d:
            continue
        path = Path(d) / lib_name
        if path.exists():
            return str(path)

    raise FileNotFoundError(
        f"Could not find {lib_name}. "
        f"Build with: cargo build --release -p zkml-verifier\n"
        f"Or set ZKML_LIB_DIR to the directory containing {lib_name}"
    )


class _Library:
    """Lazy-loaded FFI library wrapper."""

    _instance: Optional["_Library"] = None
    _lib: Any = None

    @classmethod
    def get(cls) -> "_Library":
        if cls._instance is None:
            cls._instance = cls()
        return cls._instance

    def __init__(self) -> None:
        path = _find_library()
        self._lib = ctypes.CDLL(path)

        # zkml_verify_json(json_ptr, error_out) -> i32
        self._lib.zkml_verify_json.argtypes = [
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_char_p),
        ]
        self._lib.zkml_verify_json.restype = ctypes.c_int

        # zkml_verify_raw(proof_ptr, proof_len, pub_inputs_ptr, pub_inputs_len,
        #                 gens_ptr, gens_len, circuit_desc_ptr, circuit_desc_len,
        #                 error_out) -> i32
        self._lib.zkml_verify_raw.argtypes = [
            ctypes.c_char_p, ctypes.c_size_t,  # proof
            ctypes.c_char_p, ctypes.c_size_t,  # pub_inputs
            ctypes.c_char_p, ctypes.c_size_t,  # gens
            ctypes.c_char_p, ctypes.c_size_t,  # circuit_desc
            ctypes.POINTER(ctypes.c_char_p),   # error_out
        ]
        self._lib.zkml_verify_raw.restype = ctypes.c_int

        # zkml_free_string(ptr)
        self._lib.zkml_free_string.argtypes = [ctypes.c_char_p]
        self._lib.zkml_free_string.restype = None

    @property
    def lib(self) -> Any:
        return self._lib


class VerifyError(Exception):
    """Raised when proof verification encounters an error."""
    pass


def verify_json(json_str: str) -> Dict[str, Any]:
    """Verify a proof bundle from a JSON string.

    Args:
        json_str: JSON string containing the proof bundle.

    Returns:
        Dict with "verified" (bool), "circuit_hash" (str), and optionally "error" (str).

    Raises:
        VerifyError: If verification encounters a fatal error.
    """
    lib = _Library.get()
    error_ptr = ctypes.c_char_p()

    result = lib.lib.zkml_verify_json(
        json_str.encode("utf-8"),
        ctypes.byref(error_ptr),
    )

    error_msg = None
    if error_ptr.value is not None:
        error_msg = error_ptr.value.decode("utf-8")
        lib.lib.zkml_free_string(error_ptr)

    if result == -1:
        raise VerifyError(error_msg or "unknown error")

    return {
        "verified": result == 1,
        "error": error_msg,
    }


def verify_bundle(bundle: Dict[str, Any]) -> Dict[str, Any]:
    """Verify a proof bundle from a Python dict.

    Args:
        bundle: Dict with proof_hex, gens_hex, dag_circuit_description, etc.

    Returns:
        Dict with "verified" (bool) and optionally "error" (str).
    """
    return verify_json(json.dumps(bundle))
