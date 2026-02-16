"""Tests for worldzk.xgboost — XGBoost model converter."""

import csv
import hashlib
import math
import os
import struct
import sys
import tempfile

import pytest

# Ensure the SDK package is importable
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from worldzk.xgboost import (
    InputValidationError,
    ModelParseError,
    Sample,
    Tree,
    TreeNode,
    XGBoostConverter,
    XGBoostModel,
    _parse_csv_id,
    _write_byte_array_32,
    _write_f64,
    _write_u32,
    _write_u8,
    _write_vec_f64,
)


# ═══════════════════════════════════════════════════════════════════════════════
# Fixtures — reference model from generate-test-input.py
# ═══════════════════════════════════════════════════════════════════════════════


def _build_reference_model() -> XGBoostModel:
    """Build the exact 3-tree, 2-feature model from generate-test-input.py."""
    tree0 = Tree(nodes=[
        TreeNode(is_leaf=0, feature_idx=0, threshold=5.0, left_child=1, right_child=2, value=0.0),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=-0.3),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=0.4),
    ])
    tree1 = Tree(nodes=[
        TreeNode(is_leaf=0, feature_idx=1, threshold=3.0, left_child=1, right_child=2, value=0.0),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=-0.2),
        TreeNode(is_leaf=0, feature_idx=0, threshold=7.0, left_child=3, right_child=4, value=0.0),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=0.1),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=0.5),
    ])
    tree2 = Tree(nodes=[
        TreeNode(is_leaf=0, feature_idx=0, threshold=6.0, left_child=1, right_child=2, value=0.0),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=-0.1),
        TreeNode(is_leaf=1, feature_idx=0, threshold=0.0, left_child=0, right_child=0, value=0.3),
    ])
    return XGBoostModel(
        num_features=2,
        num_classes=2,
        base_score=0.0,
        trees=[tree0, tree1, tree2],
    )


def _build_reference_samples():
    """Build the 5 reference samples from generate-test-input.py."""
    return [
        (bytes([0x01] + [0] * 31), [2.0, 1.0]),
        (bytes([0x02] + [0] * 31), [3.0, 2.0]),
        (bytes([0x03] + [0] * 31), [4.0, 1.5]),
        (bytes([0xA1] + [0] * 31), [8.0, 5.0]),
        (bytes([0xA2] + [0] * 31), [9.0, 4.0]),
    ]


def _generate_reference_bytes() -> bytes:
    """Generate the exact bytes that generate-test-input.py produces.

    Reimplements the serialization from the script for comparison.
    """
    def w_u8(v):
        return struct.pack("<I", v & 0xFF)

    def w_u32(v):
        return struct.pack("<I", v & 0xFFFFFFFF)

    def w_u64(v):
        low = v & 0xFFFFFFFF
        high = (v >> 32) & 0xFFFFFFFF
        return struct.pack("<II", low, high)

    def w_f64(v):
        bits = struct.unpack("<Q", struct.pack("<d", v))[0]
        return w_u64(bits)

    def w_byte_array_32(data):
        result = b""
        for b in data:
            result += w_u8(b)
        return result

    def w_vec_f64(values):
        result = w_u32(len(values))
        for v in values:
            result += w_f64(v)
        return result

    tree0 = [
        {"is_leaf": 0, "feature_idx": 0, "threshold": 5.0, "left_child": 1, "right_child": 2, "value": 0.0},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": -0.3},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": 0.4},
    ]
    tree1 = [
        {"is_leaf": 0, "feature_idx": 1, "threshold": 3.0, "left_child": 1, "right_child": 2, "value": 0.0},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": -0.2},
        {"is_leaf": 0, "feature_idx": 0, "threshold": 7.0, "left_child": 3, "right_child": 4, "value": 0.0},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": 0.1},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": 0.5},
    ]
    tree2 = [
        {"is_leaf": 0, "feature_idx": 0, "threshold": 6.0, "left_child": 1, "right_child": 2, "value": 0.0},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": -0.1},
        {"is_leaf": 1, "feature_idx": 0, "threshold": 0.0, "left_child": 0, "right_child": 0, "value": 0.3},
    ]
    trees = [tree0, tree1, tree2]

    samples = [
        {"id": bytes([0x01] + [0] * 31), "features": [2.0, 1.0]},
        {"id": bytes([0x02] + [0] * 31), "features": [3.0, 2.0]},
        {"id": bytes([0x03] + [0] * 31), "features": [4.0, 1.5]},
        {"id": bytes([0xA1] + [0] * 31), "features": [8.0, 5.0]},
        {"id": bytes([0xA2] + [0] * 31), "features": [9.0, 4.0]},
    ]
    threshold = 0.5

    buf = b""
    buf += w_u32(2)   # num_features
    buf += w_u32(2)   # num_classes
    buf += w_f64(0.0)  # base_score

    buf += w_u32(len(trees))
    for tree in trees:
        buf += w_u32(len(tree))
        for node in tree:
            buf += w_u32(node["is_leaf"])
            buf += w_u32(node["feature_idx"])
            buf += w_f64(node["threshold"])
            buf += w_u32(node["left_child"])
            buf += w_u32(node["right_child"])
            buf += w_f64(node["value"])

    buf += w_u32(len(samples))
    for sample in samples:
        buf += w_byte_array_32(sample["id"])
        buf += w_vec_f64(sample["features"])

    buf += w_f64(threshold)
    return buf


# ═══════════════════════════════════════════════════════════════════════════════
# Test: Round-trip binary compatibility
# ═══════════════════════════════════════════════════════════════════════════════


class TestRoundTripBinaryCompatibility:
    """Verify byte-identical output with generate-test-input.py."""

    def test_serialize_matches_reference(self):
        model = _build_reference_model()
        converter = XGBoostConverter.from_model(model)

        for sample_id, features in _build_reference_samples():
            converter.add_sample(id=sample_id, features=features)
        converter.set_threshold(0.5)

        result = converter.serialize()
        expected = _generate_reference_bytes()

        assert result == expected, (
            f"Byte mismatch: got {len(result)} bytes, expected {len(expected)} bytes.\n"
            f"First diff at byte {_find_first_diff(result, expected)}"
        )

    def test_data_url_format(self):
        model = _build_reference_model()
        converter = XGBoostConverter.from_model(model)
        for sample_id, features in _build_reference_samples():
            converter.add_sample(id=sample_id, features=features)
        converter.set_threshold(0.5)

        url = converter.to_data_url()
        assert url.startswith("data:application/octet-stream;base64,")

        # Decode and verify it matches raw serialize
        encoded = url.split(",", 1)[1]
        import base64
        decoded = base64.b64decode(encoded)
        assert decoded == converter.serialize()

    def test_input_digest_consistent(self):
        model = _build_reference_model()
        converter = XGBoostConverter.from_model(model)
        for sample_id, features in _build_reference_samples():
            converter.add_sample(id=sample_id, features=features)
        converter.set_threshold(0.5)

        digest = converter.input_digest()
        assert digest.startswith("0x")
        assert len(digest) == 66  # 0x + 64 hex chars

        # Verify against manual SHA-256
        raw = converter.serialize()
        expected = "0x" + hashlib.sha256(raw).hexdigest()
        assert digest == expected


def _find_first_diff(a: bytes, b: bytes) -> int:
    for i in range(min(len(a), len(b))):
        if a[i] != b[i]:
            return i
    if len(a) != len(b):
        return min(len(a), len(b))
    return -1


# ═══════════════════════════════════════════════════════════════════════════════
# Test: JSON parsing
# ═══════════════════════════════════════════════════════════════════════════════


class TestXGBoostJsonParsing:
    """Test parsing XGBoost JSON model format."""

    def _minimal_xgboost_json(self) -> dict:
        """Create a minimal valid XGBoost JSON structure."""
        return {
            "learner": {
                "learner_model_param": {
                    "num_feature": "3",
                    "num_class": "0",
                    "base_score": "0.5",
                },
                "gradient_booster": {
                    "model": {
                        "trees": [
                            {
                                "left_children": [-1, -1, -1],
                                "right_children": [-1, -1, -1],
                                "split_indices": [0, 0, 0],
                                "split_conditions": [0.0, 0.0, 0.0],
                                "base_weights": [0.1, 0.2, 0.3],
                            },
                            {
                                "left_children": [1, -1, -1],
                                "right_children": [2, -1, -1],
                                "split_indices": [1, 0, 0],
                                "split_conditions": [2.5, 0.0, 0.0],
                                "base_weights": [0.0, -0.1, 0.4],
                            },
                        ],
                    },
                },
            },
        }

    def test_parse_minimal_json(self):
        data = self._minimal_xgboost_json()
        model = XGBoostModel.from_dict(data)

        assert model.num_features == 3
        assert model.num_classes == 2  # 0 mapped to 2
        assert model.base_score == 0.5
        assert len(model.trees) == 2

    def test_tree_structure(self):
        data = self._minimal_xgboost_json()
        model = XGBoostModel.from_dict(data)

        # First tree: all leaves (all left_children == -1)
        tree0 = model.trees[0]
        assert len(tree0.nodes) == 3
        assert all(n.is_leaf == 1 for n in tree0.nodes)
        assert tree0.nodes[0].value == pytest.approx(0.1)
        assert tree0.nodes[1].value == pytest.approx(0.2)

        # Second tree: root is internal, children are leaves
        tree1 = model.trees[1]
        assert len(tree1.nodes) == 3
        assert tree1.nodes[0].is_leaf == 0
        assert tree1.nodes[0].feature_idx == 1
        assert tree1.nodes[0].threshold == pytest.approx(2.5)
        assert tree1.nodes[0].left_child == 1
        assert tree1.nodes[0].right_child == 2
        assert tree1.nodes[1].is_leaf == 1
        assert tree1.nodes[1].value == pytest.approx(-0.1)
        assert tree1.nodes[2].is_leaf == 1
        assert tree1.nodes[2].value == pytest.approx(0.4)

    def test_parse_missing_learner(self):
        with pytest.raises(ModelParseError, match="missing"):
            XGBoostModel.from_dict({})

    def test_parse_no_trees(self):
        data = self._minimal_xgboost_json()
        data["learner"]["gradient_booster"]["model"]["trees"] = []
        with pytest.raises(ModelParseError, match="no trees"):
            XGBoostModel.from_dict(data)

    def test_parse_zero_num_features(self):
        data = self._minimal_xgboost_json()
        data["learner"]["learner_model_param"]["num_feature"] = "0"
        with pytest.raises(ModelParseError, match="num_features must be positive"):
            XGBoostModel.from_dict(data)

    def test_from_model_file(self):
        data = self._minimal_xgboost_json()
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            import json
            json.dump(data, f)
            f.flush()
            path = f.name

        try:
            converter = XGBoostConverter.from_model_file(path)
            assert converter.model.num_features == 3
        finally:
            os.unlink(path)

    def test_from_model_file_not_found(self):
        with pytest.raises(ModelParseError, match="Failed to read"):
            XGBoostConverter.from_model_file("/nonexistent/model.json")


# ═══════════════════════════════════════════════════════════════════════════════
# Test: Validation errors
# ═══════════════════════════════════════════════════════════════════════════════


class TestValidation:
    """Test input validation."""

    def _converter(self) -> XGBoostConverter:
        return XGBoostConverter.from_model(_build_reference_model())

    def test_wrong_feature_count(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="Expected 2 features, got 3"):
            conv.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0])

    def test_wrong_id_length(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="exactly 32 bytes"):
            conv.add_sample(id=bytes(16), features=[1.0, 2.0])

    def test_id_not_bytes(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="must be bytes"):
            conv.add_sample(id="not-bytes", features=[1.0, 2.0])

    def test_nan_feature(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="NaN"):
            conv.add_sample(id=bytes(32), features=[float("nan"), 2.0])

    def test_inf_feature(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="Inf"):
            conv.add_sample(id=bytes(32), features=[float("inf"), 2.0])

    def test_nan_threshold(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="finite"):
            conv.set_threshold(float("nan"))

    def test_no_samples(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="At least one sample"):
            conv.serialize()

    def test_invalid_child_index(self):
        bad_model = XGBoostModel(
            num_features=1,
            num_classes=2,
            base_score=0.0,
            trees=[Tree(nodes=[
                TreeNode(is_leaf=0, feature_idx=0, threshold=1.0,
                         left_child=1, right_child=99, value=0.0),
                TreeNode(is_leaf=1, feature_idx=0, threshold=0.0,
                         left_child=0, right_child=0, value=0.5),
            ])],
        )
        conv = XGBoostConverter.from_model(bad_model)
        conv.add_sample(id=bytes(32), features=[1.0])
        with pytest.raises(InputValidationError, match="right_child 99 out of bounds"):
            conv.serialize()

    def test_invalid_feature_idx(self):
        bad_model = XGBoostModel(
            num_features=2,
            num_classes=2,
            base_score=0.0,
            trees=[Tree(nodes=[
                TreeNode(is_leaf=0, feature_idx=5, threshold=1.0,
                         left_child=1, right_child=2, value=0.0),
                TreeNode(is_leaf=1, feature_idx=0, threshold=0.0,
                         left_child=0, right_child=0, value=0.1),
                TreeNode(is_leaf=1, feature_idx=0, threshold=0.0,
                         left_child=0, right_child=0, value=0.2),
            ])],
        )
        conv = XGBoostConverter.from_model(bad_model)
        conv.add_sample(id=bytes(32), features=[1.0, 2.0])
        with pytest.raises(InputValidationError, match="feature_idx 5 out of bounds"):
            conv.serialize()


# ═══════════════════════════════════════════════════════════════════════════════
# Test: CSV import
# ═══════════════════════════════════════════════════════════════════════════════


class TestCsvImport:
    """Test CSV sample loading."""

    def test_csv_with_hex_ids(self):
        model = _build_reference_model()
        conv = XGBoostConverter.from_model(model)

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline=""
        ) as f:
            writer = csv.writer(f)
            writer.writerow(["uid", "f1", "f2"])
            writer.writerow(["0x01", "2.0", "1.0"])
            writer.writerow(["0xA1", "8.0", "5.0"])
            path = f.name

        try:
            conv.add_samples_from_csv(path, id_column="uid", feature_columns=["f1", "f2"])
            assert len(conv.samples) == 2

            # Hex 0x01 -> b'\x01' + 31 zero bytes
            assert conv.samples[0].id == bytes([0x01]) + bytes(31)
            assert conv.samples[0].features == [2.0, 1.0]

            assert conv.samples[1].id == bytes([0xA1]) + bytes(31)
            assert conv.samples[1].features == [8.0, 5.0]
        finally:
            os.unlink(path)

    def test_csv_with_string_ids(self):
        model = _build_reference_model()
        conv = XGBoostConverter.from_model(model)

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline=""
        ) as f:
            writer = csv.writer(f)
            writer.writerow(["uid", "f1", "f2"])
            writer.writerow(["alice", "1.0", "2.0"])
            path = f.name

        try:
            conv.add_samples_from_csv(path, id_column="uid", feature_columns=["f1", "f2"])
            assert len(conv.samples) == 1
            # Non-hex string -> SHA-256 hash
            expected_id = hashlib.sha256(b"alice").digest()
            assert conv.samples[0].id == expected_id
        finally:
            os.unlink(path)

    def test_csv_missing_column(self):
        model = _build_reference_model()
        conv = XGBoostConverter.from_model(model)

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline=""
        ) as f:
            writer = csv.writer(f)
            writer.writerow(["uid", "f1"])  # missing f2
            writer.writerow(["0x01", "2.0"])
            path = f.name

        try:
            with pytest.raises(InputValidationError, match="missing feature column"):
                conv.add_samples_from_csv(
                    path, id_column="uid", feature_columns=["f1", "f2"]
                )
        finally:
            os.unlink(path)

    def test_csv_invalid_float(self):
        model = _build_reference_model()
        conv = XGBoostConverter.from_model(model)

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline=""
        ) as f:
            writer = csv.writer(f)
            writer.writerow(["uid", "f1", "f2"])
            writer.writerow(["0x01", "not_a_number", "2.0"])
            path = f.name

        try:
            with pytest.raises(InputValidationError, match="invalid float"):
                conv.add_samples_from_csv(
                    path, id_column="uid", feature_columns=["f1", "f2"]
                )
        finally:
            os.unlink(path)


# ═══════════════════════════════════════════════════════════════════════════════
# Test: Serde helpers
# ═══════════════════════════════════════════════════════════════════════════════


class TestSerdeHelpers:
    """Test low-level serialization helpers."""

    def test_write_u32(self):
        assert _write_u32(0) == b"\x00\x00\x00\x00"
        assert _write_u32(1) == b"\x01\x00\x00\x00"
        assert _write_u32(0xFFFFFFFF) == b"\xFF\xFF\xFF\xFF"

    def test_write_f64_zero(self):
        result = _write_f64(0.0)
        assert len(result) == 8
        assert result == b"\x00" * 8

    def test_write_f64_one(self):
        result = _write_f64(1.0)
        assert len(result) == 8
        # IEEE 754: 1.0 = 0x3FF0000000000000
        bits = struct.unpack("<Q", result)[0]
        assert bits == 0x3FF0000000000000

    def test_write_byte_array_32(self):
        data = bytes(range(32))
        result = _write_byte_array_32(data)
        assert len(result) == 32 * 4  # each byte -> 4 bytes (u32)
        # First byte (0) -> u32 LE
        assert result[0:4] == b"\x00\x00\x00\x00"
        # Second byte (1) -> u32 LE
        assert result[4:8] == b"\x01\x00\x00\x00"

    def test_write_byte_array_32_wrong_length(self):
        with pytest.raises(InputValidationError, match="Expected 32 bytes"):
            _write_byte_array_32(bytes(16))

    def test_write_vec_f64(self):
        result = _write_vec_f64([1.0, 2.0])
        # 4 bytes (length u32) + 2 * 8 bytes (f64) = 20 bytes
        assert len(result) == 20
        # First 4 bytes: length = 2
        assert result[0:4] == b"\x02\x00\x00\x00"


# ═══════════════════════════════════════════════════════════════════════════════
# Test: CSV ID parsing
# ═══════════════════════════════════════════════════════════════════════════════


class TestCsvIdParsing:
    """Test _parse_csv_id helper."""

    def test_hex_with_prefix(self):
        result = _parse_csv_id("0xDEAD")
        assert result == bytes([0xDE, 0xAD]) + bytes(30)

    def test_hex_without_prefix(self):
        result = _parse_csv_id("BEEF")
        assert result == bytes([0xBE, 0xEF]) + bytes(30)

    def test_full_32_byte_hex(self):
        hex_str = "AA" * 32
        result = _parse_csv_id(hex_str)
        assert result == bytes([0xAA] * 32)

    def test_non_hex_hashed(self):
        result = _parse_csv_id("hello-world")
        expected = hashlib.sha256(b"hello-world").digest()
        assert result == expected

    def test_hex_too_long(self):
        hex_str = "AA" * 33
        with pytest.raises(InputValidationError, match="too long"):
            _parse_csv_id(hex_str)


# ═══════════════════════════════════════════════════════════════════════════════
# Test: Method chaining
# ═══════════════════════════════════════════════════════════════════════════════


class TestMethodChaining:
    """Verify fluent API returns self."""

    def test_chaining(self):
        model = _build_reference_model()
        result = (
            XGBoostConverter.from_model(model)
            .set_threshold(0.3)
            .add_sample(id=bytes(32), features=[1.0, 2.0])
            .add_sample(id=bytes([1] + [0] * 31), features=[3.0, 4.0])
        )
        assert isinstance(result, XGBoostConverter)
        assert len(result.samples) == 2
        assert result.threshold == 0.3


# ═══════════════════════════════════════════════════════════════════════════════
# Test: to_dict round-trip
# ═══════════════════════════════════════════════════════════════════════════════


class TestToDict:
    """Test dataclass to_dict methods."""

    def test_model_to_dict(self):
        model = _build_reference_model()
        d = model.to_dict()
        assert d["num_features"] == 2
        assert d["num_classes"] == 2
        assert d["base_score"] == 0.0
        assert len(d["trees"]) == 3
        assert len(d["trees"][0]["nodes"]) == 3

    def test_sample_to_dict(self):
        s = Sample(id=bytes([0x01] + [0] * 31), features=[1.0, 2.0])
        d = s.to_dict()
        assert d["id"] == (bytes([0x01] + [0] * 31)).hex()
        assert d["features"] == [1.0, 2.0]
