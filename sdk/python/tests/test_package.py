"""Tests for the top-level ``worldzk`` package: version and public exports."""

from __future__ import annotations

import inspect
import re
from pathlib import Path

import pytest

import worldzk

PYPROJECT = Path(__file__).resolve().parents[1] / "pyproject.toml"


def _pyproject_version() -> str:
    match = re.search(r'^version = "([^"]+)"', PYPROJECT.read_text(), re.MULTILINE)
    assert match is not None, "no version in pyproject.toml"
    return match.group(1)


class TestVersion:
    def test_version_matches_pyproject(self):
        assert worldzk.__version__ == _pyproject_version()


class TestExports:
    def test_readme_names_importable(self):
        # Names README.md imports from the package root.
        for name in (
            "WorldZK",
            "IndexerClient",
            "AsyncIndexerClient",
            "ApiError",
            "NetworkError",
            "TimeoutError",
            "XGBoostConverter",
            "compute_model_hash",
            "compute_input_hash",
        ):
            assert name in worldzk.__all__, name

    def test_every_name_in_all_resolves(self):
        for name in worldzk.__all__:
            assert getattr(worldzk, name) is not None, name

    def test_unknown_attribute_raises_attribute_error(self):
        with pytest.raises(AttributeError):
            getattr(worldzk, "does_not_exist")

    def test_getmembers_does_not_raise(self):
        # inspect.getmembers (and help()) call getattr on every name in dir().
        names = {name for name, _ in inspect.getmembers(worldzk)}
        assert "WorldZK" in names

    def test_dir_hides_unresolvable_names_without_web3(self, monkeypatch):
        monkeypatch.setattr(worldzk, "_WEB3_AVAILABLE", False)
        unloaded = set(worldzk._WEB3_EXPORTS) - set(vars(worldzk))
        assert not unloaded & set(dir(worldzk))

    def test_web3_exports(self):
        pytest.importorskip("web3")
        from worldzk import AsyncTEEVerifier, BatchVerifier, StepBatchVerifier, TEEVerifier
        from worldzk.async_client import AsyncTEEVerifier as AsyncTEEVerifierImpl
        from worldzk.batch_verifier import BatchVerifier as StepBatchVerifierImpl
        from worldzk.tee_verifier import TEEVerifier as TEEVerifierImpl
        from worldzk.verifier import BatchVerifier as BatchVerifierImpl

        assert TEEVerifier is TEEVerifierImpl
        assert AsyncTEEVerifier is AsyncTEEVerifierImpl
        assert BatchVerifier is BatchVerifierImpl
        assert StepBatchVerifier is StepBatchVerifierImpl
        assert "TEEVerifier" in worldzk.__all__
