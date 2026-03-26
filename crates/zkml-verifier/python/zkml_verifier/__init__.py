"""ZKML Verifier -- Python wrapper for the zkml-verifier Rust library.

Uses ctypes to call the C-FFI bindings from the compiled shared library.

Usage:
    from zkml_verifier import verify_json, verify_bundle, verify_file

    # From JSON string
    result = verify_json('{"proof_hex": "0x...", "gens_hex": "0x...", ...}')
    print(result["verified"])

    # From dict
    bundle = {"proof_hex": "0x...", "gens_hex": "0x...", "dag_circuit_description": {}}
    result = verify_bundle(bundle)

    # From file path
    result = verify_file("/path/to/proof_bundle.json")
"""

import ctypes
import json
import os
import platform
from pathlib import Path
from typing import Any, Dict, Optional

__version__ = "0.1.0"
__all__ = [
    "verify_json",
    "verify_bundle",
    "verify_file",
    "version",
    "VerifyError",
]


def _lib_filename() -> str:
    """Return the platform-specific shared library filename."""
    system = platform.system()
    if system == "Darwin":
        return "libzkml_verifier.dylib"
    elif system == "Linux":
        return "libzkml_verifier.so"
    elif system == "Windows":
        return "zkml_verifier.dll"
    else:
        raise OSError(f"Unsupported platform: {system}")


def _find_library() -> str:
    """Find the zkml-verifier shared library.

    Search order:
    1. ZKML_LIB_DIR environment variable
    2. Inside this package directory (wheel install)
    3. Cargo target/release or target/debug (development)
    4. Workspace-level target directories
    5. System library paths
    """
    lib_name = _lib_filename()

    search_dirs = [
        # 1. Explicit env var
        os.environ.get("ZKML_LIB_DIR", ""),
        # 2. Bundled in package (wheel / pip install)
        str(Path(__file__).parent),
        # 3. Crate-level target (development: crates/zkml-verifier/target/)
        str(Path(__file__).parent.parent.parent / "target" / "release"),
        str(Path(__file__).parent.parent.parent / "target" / "debug"),
        # 4. Workspace-level target (workspace root is 5 levels up)
        str(Path(__file__).parent.parent.parent.parent.parent / "target" / "release"),
        str(Path(__file__).parent.parent.parent.parent.parent / "target" / "debug"),
        # 5. Per-agent target dirs (CI / multi-agent builds)
    ]

    # Also check any CARGO_TARGET_DIR that might be set
    cargo_target = os.environ.get("CARGO_TARGET_DIR", "")
    if cargo_target:
        search_dirs.insert(1, str(Path(cargo_target) / "release"))
        search_dirs.insert(2, str(Path(cargo_target) / "debug"))

    # System paths (Linux)
    search_dirs.extend(["/usr/local/lib", "/usr/lib"])

    for d in search_dirs:
        if not d:
            continue
        path = Path(d) / lib_name
        if path.exists():
            return str(path)

    raise FileNotFoundError(
        f"Could not find {lib_name}. "
        f"Build with: cargo build --release -p zkml-verifier\n"
        f"Or install via: pip install zkml-verifier\n"
        f"Or set ZKML_LIB_DIR to the directory containing {lib_name}"
    )


class _Library:
    """Lazy-loaded FFI library wrapper (singleton)."""

    _instance: Optional["_Library"] = None
    _lib: Any = None

    @classmethod
    def get(cls) -> "_Library":
        if cls._instance is None:
            cls._instance = cls()
        return cls._instance

    @classmethod
    def reset(cls) -> None:
        """Reset the singleton (useful for testing)."""
        cls._instance = None
        cls._lib = None

    def __init__(self) -> None:
        path = _find_library()
        self._lib = ctypes.CDLL(path)

        # zkml_verify_json(json_ptr, error_out) -> i32
        self._lib.zkml_verify_json.argtypes = [
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_char_p),
        ]
        self._lib.zkml_verify_json.restype = ctypes.c_int

        # zkml_verify_file(path_ptr, error_out) -> i32
        self._lib.zkml_verify_file.argtypes = [
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_char_p),
        ]
        self._lib.zkml_verify_file.restype = ctypes.c_int

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

        # zkml_version() -> const char*
        self._lib.zkml_version.argtypes = []
        self._lib.zkml_version.restype = ctypes.c_char_p

    @property
    def lib(self) -> Any:
        return self._lib


class VerifyError(Exception):
    """Raised when proof verification encounters an error."""
    pass


def verify_json(json_str: str) -> Dict[str, Any]:
    """Verify a proof bundle from a JSON string.

    Args:
        json_str: JSON string containing the proof bundle with keys:
            - proof_hex: hex-encoded proof data
            - gens_hex: hex-encoded Pedersen generators
            - dag_circuit_description: circuit topology as JSON object

    Returns:
        Dict with "verified" (bool) and optionally "error" (str).

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

    This is a convenience wrapper around verify_json() that accepts
    a Python dict instead of a raw JSON string.

    Args:
        bundle: Dict with proof_hex, gens_hex, dag_circuit_description, etc.

    Returns:
        Dict with "verified" (bool) and optionally "error" (str).

    Raises:
        VerifyError: If verification encounters a fatal error.
    """
    return verify_json(json.dumps(bundle))


def verify_file(path: str) -> Dict[str, Any]:
    """Verify a proof bundle from a JSON file.

    Args:
        path: Path to a JSON file containing the proof bundle.

    Returns:
        Dict with "verified" (bool) and optionally "error" (str).

    Raises:
        VerifyError: If the file cannot be read or verification fails.
        FileNotFoundError: If the file does not exist.
    """
    if not Path(path).exists():
        raise FileNotFoundError(f"Proof bundle file not found: {path}")

    lib = _Library.get()
    error_ptr = ctypes.c_char_p()

    result = lib.lib.zkml_verify_file(
        path.encode("utf-8"),
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


def version() -> str:
    """Return the native library version string."""
    lib = _Library.get()
    v = lib.lib.zkml_version()
    return v.decode("utf-8") if v else "unknown"
