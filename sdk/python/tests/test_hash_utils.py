"""Tests for hash_utils module."""

import json
import os
import tempfile

import pytest

from worldzk.hash_utils import (
    _keccak256,
    _serialize_f64_list,
    compute_input_hash,
    compute_input_hash_from_json,
    compute_model_hash,
    compute_model_hash_from_bytes,
    compute_result_hash,
    compute_result_hash_from_bytes,
)


class TestKeccak256:
    """Test the internal keccak256 implementation."""

    def test_empty_input(self):
        """keccak256 of empty bytes is a well-known constant."""
        result = _keccak256(b"")
        expected = bytes.fromhex(
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        )
        assert result == expected

    def test_deterministic(self):
        """Same input always produces same output."""
        data = b"hello world"
        assert _keccak256(data) == _keccak256(data)

    def test_different_inputs(self):
        """Different inputs produce different outputs."""
        assert _keccak256(b"hello") != _keccak256(b"world")

    def test_output_length(self):
        """keccak256 output is always 32 bytes."""
        assert len(_keccak256(b"")) == 32
        assert len(_keccak256(b"x" * 1000)) == 32


class TestSerializeF64List:
    """Test JSON serialization compatibility with Rust's serde_json."""

    def test_basic_floats(self):
        assert _serialize_f64_list([1.0, 2.5, 3.7]) == b"[1.0,2.5,3.7]"

    def test_integer_floats(self):
        """Integer floats must serialize with .0 suffix (matching Rust)."""
        assert _serialize_f64_list([1.0, 2.0, 3.0]) == b"[1.0,2.0,3.0]"

    def test_negative_floats(self):
        assert _serialize_f64_list([-1.5, 0.0, 2.3]) == b"[-1.5,0.0,2.3]"

    def test_single_element(self):
        assert _serialize_f64_list([42.5]) == b"[42.5]"

    def test_empty_list(self):
        assert _serialize_f64_list([]) == b"[]"

    def test_prediction_scores(self):
        assert _serialize_f64_list([0.85]) == b"[0.85]"

    def test_multiclass_scores(self):
        assert _serialize_f64_list([0.1, 0.8, 0.1]) == b"[0.1,0.8,0.1]"


class TestComputeModelHash:
    """Test model hash computation."""

    def test_from_file(self):
        """compute_model_hash reads a file and returns keccak256 of contents."""
        with tempfile.NamedTemporaryFile(mode="wb", suffix=".json", delete=False) as f:
            f.write(b'{"learner":{}}')
            path = f.name

        try:
            result = compute_model_hash(path)
            expected = "0x" + _keccak256(b'{"learner":{}}').hex()
            assert result == expected
            assert result.startswith("0x")
            assert len(result) == 66  # 0x + 64 hex chars
        finally:
            os.unlink(path)

    def test_from_bytes(self):
        """compute_model_hash_from_bytes hashes raw bytes."""
        data = b"model data"
        result = compute_model_hash_from_bytes(data)
        expected = "0x" + _keccak256(data).hex()
        assert result == expected

    def test_empty_bytes(self):
        """Empty bytes produce the well-known keccak256 of empty input."""
        result = compute_model_hash_from_bytes(b"")
        assert result == "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"

    def test_different_models_produce_different_hashes(self):
        hash_a = compute_model_hash_from_bytes(b"model_a")
        hash_b = compute_model_hash_from_bytes(b"model_b")
        assert hash_a != hash_b


class TestComputeInputHash:
    """Test input hash computation (must match enclave behavior)."""

    def test_basic_features(self):
        """Input hash matches keccak256 of JSON-serialized features."""
        features = [1.0, 2.5, 3.7]
        result = compute_input_hash(features)

        # Manually compute: keccak256(b"[1.0,2.5,3.7]")
        json_bytes = json.dumps(features, separators=(",", ":")).encode("utf-8")
        expected = "0x" + _keccak256(json_bytes).hex()
        assert result == expected

    def test_empty_features(self):
        """Empty feature list produces keccak256 of '[]'."""
        result = compute_input_hash([])
        expected = "0x" + _keccak256(b"[]").hex()
        assert result == expected

    def test_single_feature(self):
        result = compute_input_hash([42.5])
        expected = "0x" + _keccak256(b"[42.5]").hex()
        assert result == expected

    def test_negative_features(self):
        features = [-1.5, 0.0, 2.3]
        result = compute_input_hash(features)
        expected = "0x" + _keccak256(b"[-1.5,0.0,2.3]").hex()
        assert result == expected

    def test_feature_ordering_matters(self):
        hash_a = compute_input_hash([1.0, 2.0, 3.0])
        hash_b = compute_input_hash([3.0, 2.0, 1.0])
        assert hash_a != hash_b

    def test_from_json_matches(self):
        """compute_input_hash_from_json matches compute_input_hash."""
        features = [1.0, 2.5, 3.7]
        hash_from_list = compute_input_hash(features)

        json_bytes = json.dumps(features, separators=(",", ":")).encode("utf-8")
        hash_from_json = compute_input_hash_from_json(json_bytes)

        assert hash_from_list == hash_from_json


class TestComputeResultHash:
    """Test result hash computation."""

    def test_binary_prediction(self):
        """Result hash for binary classification."""
        scores = [0.85]
        result = compute_result_hash(scores)
        expected = "0x" + _keccak256(b"[0.85]").hex()
        assert result == expected

    def test_multiclass_prediction(self):
        """Result hash for multi-class classification."""
        scores = [0.1, 0.8, 0.1]
        result = compute_result_hash(scores)
        expected = "0x" + _keccak256(b"[0.1,0.8,0.1]").hex()
        assert result == expected

    def test_from_bytes_matches(self):
        """compute_result_hash_from_bytes matches compute_result_hash."""
        scores = [0.85]
        hash_from_list = compute_result_hash(scores)

        result_bytes = json.dumps(scores, separators=(",", ":")).encode("utf-8")
        hash_from_bytes = compute_result_hash_from_bytes(result_bytes)

        assert hash_from_list == hash_from_bytes


class TestCrossLanguageCompatibility:
    """Test that Python hashes match known values from the Rust SDK.

    These are golden values computed by the Rust implementation to ensure
    cross-language compatibility.
    """

    def test_known_empty_hash(self):
        """keccak256('') is a well-known constant across all implementations."""
        result = compute_model_hash_from_bytes(b"")
        assert result == "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"

    def test_model_hash_deterministic(self):
        """Model hash is just keccak256 of raw bytes -- trivially cross-language."""
        data = b"hello model"
        result = compute_model_hash_from_bytes(data)
        # Verify it is a proper hex hash
        assert result.startswith("0x")
        assert len(result) == 66
        # Verify determinism
        assert compute_model_hash_from_bytes(data) == result

    def test_json_serialization_format(self):
        """Verify JSON format matches what Rust serde_json produces."""
        # This is the critical compatibility point: Python and Rust must
        # produce identical JSON for the same float values.
        test_cases = [
            ([1.0, 2.5, 3.7], "[1.0,2.5,3.7]"),
            ([1.0, 2.0, 3.0], "[1.0,2.0,3.0]"),
            ([-1.5, 0.0, 2.3], "[-1.5,0.0,2.3]"),
            ([0.85], "[0.85]"),
            ([0.1, 0.8, 0.1], "[0.1,0.8,0.1]"),
            ([], "[]"),
        ]
        for values, expected_json in test_cases:
            json_bytes = _serialize_f64_list(values)
            assert json_bytes.decode("utf-8") == expected_json, (
                f"Mismatch for {values}: got {json_bytes!r}, expected {expected_json!r}"
            )
