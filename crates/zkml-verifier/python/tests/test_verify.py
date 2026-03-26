"""Tests for zkml_verifier Python wrapper.

These tests verify the wrapper logic without requiring the compiled
shared library (which needs `cargo build --release`).
"""

import json
import pytest
from unittest.mock import patch, MagicMock

from zkml_verifier import verify_json, verify_bundle, VerifyError, _find_library


def test_verify_bundle_calls_verify_json():
    """verify_bundle should serialize to JSON and call verify_json."""
    bundle = {
        "proof_hex": "0xdead",
        "gens_hex": "0xbeef",
        "dag_circuit_description": {},
    }
    with patch("zkml_verifier.verify_json") as mock_verify:
        mock_verify.return_value = {"verified": False, "error": "test"}
        result = verify_bundle(bundle)
        assert result == {"verified": False, "error": "test"}
        # Verify JSON was passed correctly
        call_json = mock_verify.call_args[0][0]
        parsed = json.loads(call_json)
        assert parsed["proof_hex"] == "0xdead"


def test_find_library_raises_on_missing():
    """_find_library should raise FileNotFoundError when lib is not found."""
    with patch.dict("os.environ", {"ZKML_LIB_DIR": "/nonexistent/path"}, clear=False):
        with pytest.raises(FileNotFoundError, match="Could not find"):
            _find_library()


def test_verify_error_is_exception():
    """VerifyError should be a proper exception."""
    err = VerifyError("test error")
    assert str(err) == "test error"
    assert isinstance(err, Exception)
