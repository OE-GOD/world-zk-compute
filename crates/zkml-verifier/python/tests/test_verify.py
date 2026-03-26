"""Tests for zkml_verifier Python wrapper.

Unit tests mock the native library to test wrapper logic.
Integration tests (marked with @pytest.mark.integration) require
a compiled shared library.
"""

import json
import os
import pytest
from pathlib import Path
from unittest.mock import patch, MagicMock

from zkml_verifier import (
    verify_json,
    verify_bundle,
    verify_file,
    version,
    VerifyError,
    _find_library,
    _lib_filename,
    __version__,
)


# ---------------------------------------------------------------------------
# Unit tests (no native library needed)
# ---------------------------------------------------------------------------


def test_version_string():
    """__version__ should be a valid semver string."""
    parts = __version__.split(".")
    assert len(parts) == 3
    assert all(p.isdigit() for p in parts)


def test_lib_filename_platform():
    """_lib_filename should return platform-appropriate name."""
    name = _lib_filename()
    assert name.startswith("libzkml_verifier") or name.startswith("zkml_verifier")
    assert name.endswith((".dylib", ".so", ".dll"))


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
    with patch.dict(
        "os.environ", {"ZKML_LIB_DIR": "/nonexistent/path"}, clear=False
    ):
        # Also need to ensure the package dir and other search paths fail
        with patch("zkml_verifier.Path.exists", return_value=False):
            with pytest.raises(FileNotFoundError, match="Could not find"):
                _find_library()


def test_verify_error_is_exception():
    """VerifyError should be a proper exception subclass."""
    err = VerifyError("test error")
    assert str(err) == "test error"
    assert isinstance(err, Exception)


def test_verify_error_message_preserved():
    """VerifyError should preserve the error message through raise/except."""
    try:
        raise VerifyError("detailed message about failure")
    except VerifyError as e:
        assert "detailed message about failure" in str(e)


def test_verify_file_missing_path():
    """verify_file should raise FileNotFoundError for missing files."""
    with pytest.raises(FileNotFoundError, match="not found"):
        verify_file("/nonexistent/path/proof.json")


def test_all_exports():
    """Package should export all documented symbols."""
    import zkml_verifier
    assert hasattr(zkml_verifier, "verify_json")
    assert hasattr(zkml_verifier, "verify_bundle")
    assert hasattr(zkml_verifier, "verify_file")
    assert hasattr(zkml_verifier, "version")
    assert hasattr(zkml_verifier, "VerifyError")
    assert hasattr(zkml_verifier, "__version__")


# ---------------------------------------------------------------------------
# Integration tests (require compiled native library)
# ---------------------------------------------------------------------------


def _has_native_library():
    """Check if the native library is available."""
    try:
        _find_library()
        return True
    except FileNotFoundError:
        return False


@pytest.mark.skipif(not _has_native_library(), reason="native library not built")
class TestIntegration:
    """Integration tests that load the real native library."""

    def test_version_from_native(self):
        """version() should return a version string from the native lib."""
        v = version()
        assert isinstance(v, str)
        assert len(v) > 0
        # Should match the Rust-side version
        assert v == "0.1.0"

    def test_verify_json_invalid_input(self):
        """verify_json with invalid JSON should raise VerifyError."""
        with pytest.raises(VerifyError, match="error"):
            verify_json("{}")

    def test_verify_json_malformed(self):
        """verify_json with a non-empty but incomplete bundle should error."""
        with pytest.raises(VerifyError):
            verify_json('{"proof_hex": "0xdead"}')

    def test_verify_bundle_invalid(self):
        """verify_bundle with minimal dict should raise VerifyError."""
        with pytest.raises(VerifyError):
            verify_bundle({"proof_hex": "0x00", "gens_hex": "0x00"})
