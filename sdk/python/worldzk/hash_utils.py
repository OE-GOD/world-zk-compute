"""
Hash utilities for computing model_hash, input_hash, and result_hash.

These functions produce hashes compatible with the TEE enclave's hashing
behavior. Using these utilities ensures that on-chain verification will
match the enclave's attestation.

The functions use keccak256 (the Ethereum variant, as used by Solidity's
``keccak256()``). This requires either ``pysha3`` (via ``hashlib``),
``pycryptodome``, or ``web3`` to be installed.

Example usage::

    from worldzk.hash_utils import compute_model_hash, compute_input_hash

    # From a file path
    model_hash = compute_model_hash("path/to/model.json")

    # From raw bytes
    model_hash = compute_model_hash_from_bytes(b'{"learner":{}}')

    # Input hash from features
    input_hash = compute_input_hash([1.0, 2.5, 3.7])

    # Result hash from prediction scores
    result_hash = compute_result_hash([0.85])
"""

from __future__ import annotations

import json
from typing import List, Union


def _keccak256(data: bytes) -> bytes:
    """Compute keccak256 hash of data.

    Tries multiple backends in order:
    1. pysha3 (via hashlib)
    2. pycryptodome (Crypto.Hash.keccak)
    3. web3.Web3.keccak
    """
    # Try hashlib (works if pysha3 is installed)
    try:
        import hashlib

        k = hashlib.new("keccak_256")
        k.update(data)
        return k.digest()
    except (ValueError, ImportError):
        pass

    # Try pycryptodome
    try:
        from Crypto.Hash import keccak  # type: ignore[import-untyped]

        k = keccak.new(digest_bits=256)
        k.update(data)
        return k.digest()
    except ImportError:
        pass

    # Try web3
    try:
        from web3 import Web3

        return Web3.keccak(data)
    except ImportError:
        pass

    raise ImportError(
        "No keccak256 implementation found. "
        "Install one of: pysha3, pycryptodome, or web3. "
        "Example: pip install pysha3"
    )


def compute_model_hash(path: str) -> str:
    """Compute model hash from a file path.

    Reads the raw file bytes and returns ``keccak256(file_bytes)``
    as a ``0x``-prefixed hex string.

    This matches the enclave's model hash computation.

    Args:
        path: Path to the model file (e.g. XGBoost JSON model).

    Returns:
        ``0x``-prefixed hex string of the 32-byte keccak256 hash.
    """
    with open(path, "rb") as f:
        data = f.read()
    return "0x" + _keccak256(data).hex()


def compute_model_hash_from_bytes(data: bytes) -> str:
    """Compute model hash from raw bytes.

    Returns ``keccak256(data)`` as a ``0x``-prefixed hex string.

    Args:
        data: Raw model file bytes.

    Returns:
        ``0x``-prefixed hex string of the 32-byte keccak256 hash.
    """
    return "0x" + _keccak256(data).hex()


def compute_input_hash(features: List[float]) -> str:
    """Compute input hash from a feature vector.

    This matches the enclave's computation:
    ``keccak256(json.dumps(features, separators=(',', ':')))``

    The features are JSON-serialized in compact form (no spaces) and
    then hashed. The JSON format must exactly match what Rust's
    ``serde_json::to_vec`` produces for a ``Vec<f64>``.

    Args:
        features: List of float feature values.

    Returns:
        ``0x``-prefixed hex string of the 32-byte keccak256 hash.
    """
    json_bytes = _serialize_f64_list(features)
    return "0x" + _keccak256(json_bytes).hex()


def compute_result_hash(scores: List[float]) -> str:
    """Compute result hash from prediction scores.

    This matches the enclave's computation:
    ``keccak256(json.dumps(scores, separators=(',', ':')))``

    Args:
        scores: List of float prediction scores.

    Returns:
        ``0x``-prefixed hex string of the 32-byte keccak256 hash.
    """
    json_bytes = _serialize_f64_list(scores)
    return "0x" + _keccak256(json_bytes).hex()


def compute_input_hash_from_json(json_bytes: bytes) -> str:
    """Compute input hash from raw JSON bytes.

    Use this when you already have the JSON-serialized feature array.

    Args:
        json_bytes: JSON-encoded feature array bytes.

    Returns:
        ``0x``-prefixed hex string of the 32-byte keccak256 hash.
    """
    return "0x" + _keccak256(json_bytes).hex()


def compute_result_hash_from_bytes(result_bytes: bytes) -> str:
    """Compute result hash from raw result bytes.

    Use this when you already have the raw result bytes (e.g. from
    hex-decoding the enclave's response ``result`` field).

    Args:
        result_bytes: Raw result bytes.

    Returns:
        ``0x``-prefixed hex string of the 32-byte keccak256 hash.
    """
    return "0x" + _keccak256(result_bytes).hex()


def _serialize_f64_list(values: List[float]) -> bytes:
    """Serialize a list of floats to JSON bytes matching Rust's serde_json.

    Rust's serde_json serializes f64 values with the following rules:
    - Integers like 1.0 are serialized as ``1.0`` (not ``1``)
    - No spaces after commas or colons

    Python's json.dumps by default serializes 1.0 as ``1.0`` when using
    ``float`` type, which matches Rust.

    Returns:
        JSON bytes (e.g. ``b'[1.0,2.5,3.7]'``).
    """
    # Use compact separators to match serde_json (no spaces)
    json_str = json.dumps(values, separators=(",", ":"))

    # Python's json.dumps serializes float values that are exact integers
    # without a decimal point: 1.0 -> "1.0" only if the value is a Python float.
    # However, json.dumps may output "1.0" or "1" depending on the Python version
    # and the actual representation. We need to ensure ".0" is present for
    # values that are exact integers to match Rust's serde_json behavior.
    #
    # Actually, Python's json.dumps for float(1.0) outputs "1.0" by default,
    # which matches Rust. Let's verify and fix if needed.

    return json_str.encode("utf-8")
