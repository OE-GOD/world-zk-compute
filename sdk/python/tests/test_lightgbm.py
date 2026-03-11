"""Tests for worldzk.lightgbm -- LightGBM model converter."""

import csv
import hashlib
import json
import math
import os
import struct
import sys
import tempfile
from typing import List

import pytest

# Ensure the SDK package is importable
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from worldzk.lightgbm import (
    LightGBMConverter,
    _parse_lightgbm_model,
)
from worldzk.xgboost import (
    InputValidationError,
    ModelParseError,
    Sample,
    Tree,
    TreeNode,
    XGBoostConverter,
    XGBoostModel,
)


# =============================================================================
# Inline sample LightGBM models
# =============================================================================


def _sample_binary_model() -> dict:
    """A minimal binary classification LightGBM model with 2 trees, 4 features.

    Tree 0 (3 nodes):
        split on feature 2, threshold 0.5
          left: leaf 0.1
          right: split on feature 0, threshold 1.5
            left: leaf -0.2
            right: leaf 0.3

    Tree 1 (3 nodes):
        split on feature 1, threshold 3.0
          left: leaf -0.15
          right: leaf 0.25
    """
    return {
        "name": "tree",
        "version": "v3.3.5",
        "num_class": 1,
        "num_tree_per_iteration": 1,
        "label_index": 0,
        "max_feature_idx": 3,
        "objective": "binary sigmoid:1",
        "tree_info": [
            {
                "tree_index": 0,
                "num_leaves": 3,
                "num_cat": 0,
                "shrinkage": 1,
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 2,
                    "split_gain": 100.5,
                    "threshold": 0.5,
                    "decision_type": "<=",
                    "default_left": True,
                    "internal_value": 0,
                    "left_child": {
                        "leaf_index": 0,
                        "leaf_value": 0.1,
                    },
                    "right_child": {
                        "split_index": 1,
                        "split_feature": 0,
                        "threshold": 1.5,
                        "decision_type": "<=",
                        "default_left": True,
                        "internal_value": 0,
                        "left_child": {
                            "leaf_index": 1,
                            "leaf_value": -0.2,
                        },
                        "right_child": {
                            "leaf_index": 2,
                            "leaf_value": 0.3,
                        },
                    },
                },
            },
            {
                "tree_index": 1,
                "num_leaves": 2,
                "num_cat": 0,
                "shrinkage": 1,
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 1,
                    "split_gain": 50.0,
                    "threshold": 3.0,
                    "decision_type": "<=",
                    "default_left": True,
                    "internal_value": 0,
                    "left_child": {
                        "leaf_index": 0,
                        "leaf_value": -0.15,
                    },
                    "right_child": {
                        "leaf_index": 1,
                        "leaf_value": 0.25,
                    },
                },
            },
        ],
    }


def _sample_multiclass_model() -> dict:
    """A multiclass LightGBM model with 3 classes, 2 features, 6 trees.

    num_tree_per_iteration=3 means each boosting round produces 3 trees
    (one per class). We have 2 rounds -> 6 trees total.
    """
    def _make_leaf(val):
        return {"leaf_index": 0, "leaf_value": val}

    def _make_simple_tree(feature, threshold, left_val, right_val):
        return {
            "tree_structure": {
                "split_index": 0,
                "split_feature": feature,
                "threshold": threshold,
                "decision_type": "<=",
                "left_child": {"leaf_index": 0, "leaf_value": left_val},
                "right_child": {"leaf_index": 1, "leaf_value": right_val},
            },
        }

    return {
        "name": "tree",
        "version": "v3.3.5",
        "num_class": 3,
        "num_tree_per_iteration": 3,
        "max_feature_idx": 1,
        "objective": "multiclass softmax num_class:3",
        "tree_info": [
            # Round 1: trees for class 0, 1, 2
            _make_simple_tree(0, 1.0, 0.1, -0.1),
            _make_simple_tree(1, 2.0, -0.2, 0.2),
            _make_simple_tree(0, 0.5, 0.3, -0.3),
            # Round 2: trees for class 0, 1, 2
            _make_simple_tree(1, 1.5, 0.05, -0.05),
            _make_simple_tree(0, 0.8, -0.1, 0.1),
            _make_simple_tree(1, 2.5, 0.15, -0.15),
        ],
    }


def _sample_single_leaf_model() -> dict:
    """A model where one tree is just a single leaf (stump-like)."""
    return {
        "name": "tree",
        "version": "v3.3.5",
        "num_class": 1,
        "num_tree_per_iteration": 1,
        "max_feature_idx": 1,
        "objective": "regression",
        "tree_info": [
            {
                "tree_structure": {
                    "leaf_index": 0,
                    "leaf_value": 0.42,
                },
            },
        ],
    }


def _sample_deep_tree_model() -> dict:
    """A model with a deeper tree (depth 3)."""
    return {
        "name": "tree",
        "version": "v3.3.5",
        "num_class": 1,
        "num_tree_per_iteration": 1,
        "max_feature_idx": 2,
        "objective": "binary sigmoid:1",
        "tree_info": [
            {
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 0,
                    "threshold": 5.0,
                    "decision_type": "<=",
                    "left_child": {
                        "split_index": 1,
                        "split_feature": 1,
                        "threshold": 2.0,
                        "decision_type": "<=",
                        "left_child": {
                            "leaf_index": 0,
                            "leaf_value": -0.5,
                        },
                        "right_child": {
                            "split_index": 3,
                            "split_feature": 2,
                            "threshold": 1.0,
                            "decision_type": "<=",
                            "left_child": {
                                "leaf_index": 1,
                                "leaf_value": 0.1,
                            },
                            "right_child": {
                                "leaf_index": 2,
                                "leaf_value": 0.2,
                            },
                        },
                    },
                    "right_child": {
                        "split_index": 2,
                        "split_feature": 1,
                        "threshold": 3.0,
                        "decision_type": "<=",
                        "left_child": {
                            "leaf_index": 3,
                            "leaf_value": 0.3,
                        },
                        "right_child": {
                            "leaf_index": 4,
                            "leaf_value": 0.8,
                        },
                    },
                },
            },
        ],
    }


# =============================================================================
# Test: JSON parsing
# =============================================================================


class TestLightGBMJsonParsing:
    """Test parsing LightGBM JSON model format."""

    def test_parse_binary_model(self):
        data = _sample_binary_model()
        model = _parse_lightgbm_model(data)

        assert model.num_features == 4  # max_feature_idx=3 -> 4 features
        assert model.num_classes == 2  # num_class=1 -> 2 (binary)
        assert model.base_score == 0.0
        assert len(model.trees) == 2

    def test_parse_multiclass_model(self):
        data = _sample_multiclass_model()
        model = _parse_lightgbm_model(data)

        assert model.num_features == 2  # max_feature_idx=1
        assert model.num_classes == 3
        assert len(model.trees) == 6

    def test_tree_structure_binary(self):
        data = _sample_binary_model()
        model = _parse_lightgbm_model(data)

        # Tree 0: root splits on feature 2 at threshold 0.5
        tree0 = model.trees[0]
        assert len(tree0.nodes) == 5  # root + left_leaf + right_internal + 2 right_leaves

        root = tree0.nodes[0]
        assert root.is_leaf == 0
        assert root.feature_idx == 2
        assert root.threshold == pytest.approx(0.5)

        # left child is a leaf with value 0.1
        left = tree0.nodes[root.left_child]
        assert left.is_leaf == 1
        assert left.value == pytest.approx(0.1)

        # right child is an internal node
        right = tree0.nodes[root.right_child]
        assert right.is_leaf == 0
        assert right.feature_idx == 0
        assert right.threshold == pytest.approx(1.5)

        # right's children
        right_left = tree0.nodes[right.left_child]
        assert right_left.is_leaf == 1
        assert right_left.value == pytest.approx(-0.2)

        right_right = tree0.nodes[right.right_child]
        assert right_right.is_leaf == 1
        assert right_right.value == pytest.approx(0.3)

        # Tree 1: root splits on feature 1 at threshold 3.0
        tree1 = model.trees[1]
        assert len(tree1.nodes) == 3

        root1 = tree1.nodes[0]
        assert root1.is_leaf == 0
        assert root1.feature_idx == 1
        assert root1.threshold == pytest.approx(3.0)

        left1 = tree1.nodes[root1.left_child]
        assert left1.is_leaf == 1
        assert left1.value == pytest.approx(-0.15)

        right1 = tree1.nodes[root1.right_child]
        assert right1.is_leaf == 1
        assert right1.value == pytest.approx(0.25)

    def test_single_leaf_tree(self):
        data = _sample_single_leaf_model()
        model = _parse_lightgbm_model(data)

        assert len(model.trees) == 1
        tree = model.trees[0]
        assert len(tree.nodes) == 1
        assert tree.nodes[0].is_leaf == 1
        assert tree.nodes[0].value == pytest.approx(0.42)

    def test_deep_tree_structure(self):
        data = _sample_deep_tree_model()
        model = _parse_lightgbm_model(data)

        tree = model.trees[0]
        # Depth-3 tree: 4 internal + 5 leaves = 9 nodes? No.
        # Actually: root(0) -> left_internal(1) -> left_leaf(2), right_internal(3) -> leaf(4), leaf(5)
        #                    -> right_internal(6) -> leaf(7), leaf(8)
        # Count: root=split, left=split, left.left=leaf, left.right=split, left.right.left=leaf,
        #        left.right.right=leaf, right=split, right.left=leaf, right.right=leaf
        # = 3 splits + 5 leaves... wait, let me recount from the JSON.
        #
        # root: split(f0, 5.0)
        #   left: split(f1, 2.0)
        #     left: leaf(-0.5)
        #     right: split(f2, 1.0)
        #       left: leaf(0.1)
        #       right: leaf(0.2)
        #   right: split(f1, 3.0)
        #     left: leaf(0.3)
        #     right: leaf(0.8)
        # = 3 splits + 5 leaves... wait, 4 splits? No:
        # root, root.left, root.left.right, root.right = 4 splits? No:
        # root=split, root.left=split, root.left.left=leaf, root.left.right=split,
        # root.left.right.left=leaf, root.left.right.right=leaf,
        # root.right=split, root.right.left=leaf, root.right.right=leaf
        # = 4 split nodes + 5 leaf nodes = 9 total
        assert len(tree.nodes) == 9

        # Verify root
        root = tree.nodes[0]
        assert root.is_leaf == 0
        assert root.feature_idx == 0
        assert root.threshold == pytest.approx(5.0)

    def test_parse_missing_max_feature_idx(self):
        data = _sample_binary_model()
        del data["max_feature_idx"]
        with pytest.raises(ModelParseError, match="missing 'max_feature_idx'"):
            _parse_lightgbm_model(data)

    def test_parse_no_trees(self):
        data = _sample_binary_model()
        data["tree_info"] = []
        with pytest.raises(ModelParseError, match="no trees"):
            _parse_lightgbm_model(data)

    def test_parse_missing_tree_structure(self):
        data = _sample_binary_model()
        del data["tree_info"][0]["tree_structure"]
        with pytest.raises(ModelParseError, match="missing 'tree_structure'"):
            _parse_lightgbm_model(data)

    def test_parse_invalid_feature_idx(self):
        data = {
            "num_class": 1,
            "max_feature_idx": 1,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "split_index": 0,
                        "split_feature": 99,  # out of range
                        "threshold": 1.0,
                        "decision_type": "<=",
                        "left_child": {"leaf_index": 0, "leaf_value": 0.1},
                        "right_child": {"leaf_index": 1, "leaf_value": 0.2},
                    },
                },
            ],
        }
        with pytest.raises(ModelParseError, match="split_feature 99 out of bounds"):
            _parse_lightgbm_model(data)

    def test_parse_missing_left_child(self):
        data = {
            "num_class": 1,
            "max_feature_idx": 1,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "split_index": 0,
                        "split_feature": 0,
                        "threshold": 1.0,
                        "decision_type": "<=",
                        # missing left_child
                        "right_child": {"leaf_index": 1, "leaf_value": 0.2},
                    },
                },
            ],
        }
        with pytest.raises(ModelParseError, match="missing 'left_child'"):
            _parse_lightgbm_model(data)


# =============================================================================
# Test: Converter API
# =============================================================================


class TestLightGBMConverterAPI:
    """Test LightGBMConverter public API mirrors XGBoostConverter."""

    def test_from_dict(self):
        data = _sample_binary_model()
        conv = LightGBMConverter.from_dict(data)
        assert conv.model.num_features == 4
        assert conv.model.num_classes == 2
        assert len(conv.model.trees) == 2

    def test_from_model_file(self):
        data = _sample_binary_model()
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False
        ) as f:
            json.dump(data, f)
            f.flush()
            path = f.name

        try:
            conv = LightGBMConverter.from_model_file(path)
            assert conv.model.num_features == 4
        finally:
            os.unlink(path)

    def test_from_model_file_not_found(self):
        with pytest.raises(ModelParseError, match="Failed to read"):
            LightGBMConverter.from_model_file("/nonexistent/model.json")

    def test_from_model(self):
        model = XGBoostModel(
            num_features=2,
            num_classes=2,
            base_score=0.0,
            trees=[Tree(nodes=[
                TreeNode(is_leaf=1, feature_idx=0, threshold=0.0,
                         left_child=0, right_child=0, value=0.5),
            ])],
        )
        conv = LightGBMConverter.from_model(model)
        assert conv.model.num_features == 2

    def test_add_sample(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())
        result = conv.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])
        assert result is conv  # returns self for chaining
        assert len(conv.samples) == 1

    def test_set_threshold(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())
        result = conv.set_threshold(0.7)
        assert result is conv
        assert conv.threshold == pytest.approx(0.7)

    def test_default_threshold(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())
        assert conv.threshold == pytest.approx(0.5)

    def test_chaining(self):
        data = _sample_binary_model()
        result = (
            LightGBMConverter.from_dict(data)
            .set_threshold(0.3)
            .add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])
            .add_sample(id=bytes([1] + [0] * 31), features=[5.0, 6.0, 7.0, 8.0])
        )
        assert isinstance(result, LightGBMConverter)
        assert len(result.samples) == 2
        assert result.threshold == pytest.approx(0.3)


# =============================================================================
# Test: Serialization
# =============================================================================


class TestSerialization:
    """Test serialization output format and consistency."""

    def test_serialize_produces_bytes(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())
        conv.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])
        result = conv.serialize()
        assert isinstance(result, bytes)
        assert len(result) > 0

    def test_serialize_deterministic(self):
        """Same input always produces same bytes."""
        data = _sample_binary_model()

        conv1 = LightGBMConverter.from_dict(data)
        conv1.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])

        conv2 = LightGBMConverter.from_dict(data)
        conv2.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])

        assert conv1.serialize() == conv2.serialize()

    def test_data_url_format(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())
        conv.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])

        url = conv.to_data_url()
        assert url.startswith("data:application/octet-stream;base64,")

        # Decode and verify it matches raw serialize
        import base64
        encoded = url.split(",", 1)[1]
        decoded = base64.b64decode(encoded)
        assert decoded == conv.serialize()

    def test_input_digest(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())
        conv.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])

        digest = conv.input_digest()
        assert digest.startswith("0x")
        assert len(digest) == 66  # 0x + 64 hex chars

        # Verify against manual SHA-256
        raw = conv.serialize()
        expected = "0x" + hashlib.sha256(raw).hexdigest()
        assert digest == expected

    def test_serialize_matches_xgboost_for_same_model(self):
        """LightGBMConverter and XGBoostConverter should produce identical bytes
        when given the same internal XGBoostModel representation."""
        # Build a model manually
        model = XGBoostModel(
            num_features=2,
            num_classes=2,
            base_score=0.0,
            trees=[
                Tree(nodes=[
                    TreeNode(is_leaf=0, feature_idx=0, threshold=1.0,
                             left_child=1, right_child=2, value=0.0),
                    TreeNode(is_leaf=1, feature_idx=0, threshold=0.0,
                             left_child=0, right_child=0, value=-0.3),
                    TreeNode(is_leaf=1, feature_idx=0, threshold=0.0,
                             left_child=0, right_child=0, value=0.7),
                ]),
            ],
        )

        xgb_conv = XGBoostConverter.from_model(model)
        xgb_conv.add_sample(id=bytes(32), features=[0.5, 1.5])
        xgb_conv.set_threshold(0.5)

        lgb_conv = LightGBMConverter.from_model(model)
        lgb_conv.add_sample(id=bytes(32), features=[0.5, 1.5])
        lgb_conv.set_threshold(0.5)

        assert xgb_conv.serialize() == lgb_conv.serialize()
        assert xgb_conv.input_digest() == lgb_conv.input_digest()


# =============================================================================
# Test: Validation errors
# =============================================================================


class TestValidation:
    """Test input validation."""

    def _converter(self) -> LightGBMConverter:
        return LightGBMConverter.from_dict(_sample_binary_model())

    def test_wrong_feature_count(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="Expected 4 features, got 2"):
            conv.add_sample(id=bytes(32), features=[1.0, 2.0])

    def test_wrong_id_length(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="exactly 32 bytes"):
            conv.add_sample(id=bytes(16), features=[1.0, 2.0, 3.0, 4.0])

    def test_id_not_bytes(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="must be bytes"):
            conv.add_sample(id="not-bytes", features=[1.0, 2.0, 3.0, 4.0])

    def test_nan_feature(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="NaN"):
            conv.add_sample(id=bytes(32), features=[float("nan"), 2.0, 3.0, 4.0])

    def test_inf_feature(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="Inf"):
            conv.add_sample(id=bytes(32), features=[float("inf"), 2.0, 3.0, 4.0])

    def test_nan_threshold(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="finite"):
            conv.set_threshold(float("nan"))

    def test_no_samples(self):
        conv = self._converter()
        with pytest.raises(InputValidationError, match="At least one sample"):
            conv.serialize()


# =============================================================================
# Test: CSV import
# =============================================================================


class TestCsvImport:
    """Test CSV sample loading."""

    def test_csv_with_hex_ids(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline=""
        ) as f:
            writer = csv.writer(f)
            writer.writerow(["uid", "f0", "f1", "f2", "f3"])
            writer.writerow(["0x01", "1.0", "2.0", "3.0", "4.0"])
            writer.writerow(["0xA1", "5.0", "6.0", "7.0", "8.0"])
            path = f.name

        try:
            conv.add_samples_from_csv(
                path,
                id_column="uid",
                feature_columns=["f0", "f1", "f2", "f3"],
            )
            assert len(conv.samples) == 2
            assert conv.samples[0].id == bytes([0x01]) + bytes(31)
            assert conv.samples[0].features == [1.0, 2.0, 3.0, 4.0]
        finally:
            os.unlink(path)

    def test_csv_with_string_ids(self):
        conv = LightGBMConverter.from_dict(_sample_binary_model())

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".csv", delete=False, newline=""
        ) as f:
            writer = csv.writer(f)
            writer.writerow(["uid", "f0", "f1", "f2", "f3"])
            writer.writerow(["alice", "1.0", "2.0", "3.0", "4.0"])
            path = f.name

        try:
            conv.add_samples_from_csv(
                path,
                id_column="uid",
                feature_columns=["f0", "f1", "f2", "f3"],
            )
            assert len(conv.samples) == 1
            expected_id = hashlib.sha256(b"alice").digest()
            assert conv.samples[0].id == expected_id
        finally:
            os.unlink(path)


# =============================================================================
# Test: Multiple trees and multiclass
# =============================================================================


class TestMultipleTrees:
    """Test handling of multiple trees and multiclass models."""

    def test_multiclass_tree_count(self):
        data = _sample_multiclass_model()
        conv = LightGBMConverter.from_dict(data)

        assert conv.model.num_classes == 3
        assert len(conv.model.trees) == 6

    def test_multiclass_serialize(self):
        data = _sample_multiclass_model()
        conv = LightGBMConverter.from_dict(data)
        conv.add_sample(id=bytes(32), features=[0.5, 1.5])

        result = conv.serialize()
        assert isinstance(result, bytes)
        assert len(result) > 0

    def test_multiclass_tree_structure(self):
        data = _sample_multiclass_model()
        model = _parse_lightgbm_model(data)

        # Each tree has 3 nodes (1 split + 2 leaves)
        for i, tree in enumerate(model.trees):
            assert len(tree.nodes) == 3, f"Tree {i} should have 3 nodes"
            assert tree.nodes[0].is_leaf == 0, f"Tree {i} root should be split"
            assert tree.nodes[1].is_leaf == 1, f"Tree {i} left should be leaf"
            assert tree.nodes[2].is_leaf == 1, f"Tree {i} right should be leaf"

    def test_single_leaf_model(self):
        data = _sample_single_leaf_model()
        conv = LightGBMConverter.from_dict(data)

        assert conv.model.num_features == 2
        assert len(conv.model.trees) == 1
        assert len(conv.model.trees[0].nodes) == 1
        assert conv.model.trees[0].nodes[0].is_leaf == 1
        assert conv.model.trees[0].nodes[0].value == pytest.approx(0.42)

        # Should serialize fine
        conv.add_sample(id=bytes(32), features=[1.0, 2.0])
        result = conv.serialize()
        assert isinstance(result, bytes)


# =============================================================================
# Test: Deep tree
# =============================================================================


class TestDeepTree:
    """Test deeper tree structures."""

    def test_deep_tree_node_count(self):
        data = _sample_deep_tree_model()
        model = _parse_lightgbm_model(data)

        tree = model.trees[0]
        # 4 internal + 5 leaves = 9 nodes
        assert len(tree.nodes) == 9

    def test_deep_tree_traversal(self):
        """Verify the flat node array preserves the correct tree topology."""
        data = _sample_deep_tree_model()
        model = _parse_lightgbm_model(data)
        tree = model.trees[0]

        # Root: split on feature 0 at 5.0
        root = tree.nodes[0]
        assert root.feature_idx == 0
        assert root.threshold == pytest.approx(5.0)

        # Traverse left subtree
        left = tree.nodes[root.left_child]
        assert left.is_leaf == 0
        assert left.feature_idx == 1
        assert left.threshold == pytest.approx(2.0)

        left_left = tree.nodes[left.left_child]
        assert left_left.is_leaf == 1
        assert left_left.value == pytest.approx(-0.5)

        left_right = tree.nodes[left.right_child]
        assert left_right.is_leaf == 0
        assert left_right.feature_idx == 2
        assert left_right.threshold == pytest.approx(1.0)

        lr_left = tree.nodes[left_right.left_child]
        assert lr_left.is_leaf == 1
        assert lr_left.value == pytest.approx(0.1)

        lr_right = tree.nodes[left_right.right_child]
        assert lr_right.is_leaf == 1
        assert lr_right.value == pytest.approx(0.2)

        # Traverse right subtree
        right = tree.nodes[root.right_child]
        assert right.is_leaf == 0
        assert right.feature_idx == 1
        assert right.threshold == pytest.approx(3.0)

        right_left = tree.nodes[right.left_child]
        assert right_left.is_leaf == 1
        assert right_left.value == pytest.approx(0.3)

        right_right = tree.nodes[right.right_child]
        assert right_right.is_leaf == 1
        assert right_right.value == pytest.approx(0.8)


# =============================================================================
# Test: Tree inference simulation (manual traversal)
# =============================================================================


def _traverse_tree(tree: Tree, features: List[float]) -> float:
    """Simulate tree inference by traversing from root to leaf."""
    node = tree.nodes[0]
    while node.is_leaf == 0:
        if features[node.feature_idx] <= node.threshold:
            node = tree.nodes[node.left_child]
        else:
            node = tree.nodes[node.right_child]
    return node.value


class TestInferenceOutput:
    """Test that the parsed tree structure produces correct inference values."""

    def test_binary_model_inference(self):
        data = _sample_binary_model()
        model = _parse_lightgbm_model(data)

        # features: [f0=0.0, f1=0.0, f2=0.0, f3=0.0]
        # Tree 0: f2=0.0 <= 0.5 -> left -> leaf 0.1
        # Tree 1: f1=0.0 <= 3.0 -> left -> leaf -0.15
        # Sum = 0.1 + (-0.15) = -0.05
        features = [0.0, 0.0, 0.0, 0.0]
        tree0_val = _traverse_tree(model.trees[0], features)
        tree1_val = _traverse_tree(model.trees[1], features)
        assert tree0_val == pytest.approx(0.1)
        assert tree1_val == pytest.approx(-0.15)
        assert (tree0_val + tree1_val) == pytest.approx(-0.05)

    def test_binary_model_inference_right_path(self):
        data = _sample_binary_model()
        model = _parse_lightgbm_model(data)

        # features: [f0=2.0, f1=4.0, f2=1.0, f3=0.0]
        # Tree 0: f2=1.0 > 0.5 -> right -> split(f0, 1.5)
        #         f0=2.0 > 1.5 -> right -> leaf 0.3
        # Tree 1: f1=4.0 > 3.0 -> right -> leaf 0.25
        # Sum = 0.3 + 0.25 = 0.55
        features = [2.0, 4.0, 1.0, 0.0]
        tree0_val = _traverse_tree(model.trees[0], features)
        tree1_val = _traverse_tree(model.trees[1], features)
        assert tree0_val == pytest.approx(0.3)
        assert tree1_val == pytest.approx(0.25)
        assert (tree0_val + tree1_val) == pytest.approx(0.55)

    def test_deep_tree_inference(self):
        data = _sample_deep_tree_model()
        model = _parse_lightgbm_model(data)
        tree = model.trees[0]

        # features: [f0=3.0, f1=1.0, f2=0.5]
        # root: f0=3.0 <= 5.0 -> left
        # left: f1=1.0 <= 2.0 -> left -> leaf -0.5
        assert _traverse_tree(tree, [3.0, 1.0, 0.5]) == pytest.approx(-0.5)

        # features: [f0=3.0, f1=3.0, f2=0.5]
        # root: f0=3.0 <= 5.0 -> left
        # left: f1=3.0 > 2.0 -> right (split f2, 1.0)
        # f2=0.5 <= 1.0 -> left -> leaf 0.1
        assert _traverse_tree(tree, [3.0, 3.0, 0.5]) == pytest.approx(0.1)

        # features: [f0=3.0, f1=3.0, f2=2.0]
        # root: f0=3.0 <= 5.0 -> left
        # left: f1=3.0 > 2.0 -> right (split f2, 1.0)
        # f2=2.0 > 1.0 -> right -> leaf 0.2
        assert _traverse_tree(tree, [3.0, 3.0, 2.0]) == pytest.approx(0.2)

        # features: [f0=6.0, f1=2.0, f2=0.0]
        # root: f0=6.0 > 5.0 -> right
        # right: f1=2.0 <= 3.0 -> left -> leaf 0.3
        assert _traverse_tree(tree, [6.0, 2.0, 0.0]) == pytest.approx(0.3)

        # features: [f0=6.0, f1=4.0, f2=0.0]
        # root: f0=6.0 > 5.0 -> right
        # right: f1=4.0 > 3.0 -> right -> leaf 0.8
        assert _traverse_tree(tree, [6.0, 4.0, 0.0]) == pytest.approx(0.8)


# =============================================================================
# Test: to_dict round-trip
# =============================================================================


class TestToDict:
    """Test that model.to_dict() works correctly for LightGBM-parsed models."""

    def test_model_to_dict(self):
        data = _sample_binary_model()
        model = _parse_lightgbm_model(data)
        d = model.to_dict()

        assert d["num_features"] == 4
        assert d["num_classes"] == 2
        assert d["base_score"] == 0.0
        assert len(d["trees"]) == 2

    def test_round_trip_via_dict(self):
        """Parse LightGBM -> to_dict -> reconstruct XGBoostModel -> serialize.
        Should produce identical bytes."""
        data = _sample_binary_model()
        model = _parse_lightgbm_model(data)

        conv1 = LightGBMConverter.from_model(model)
        conv1.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])

        # Reconstruct from dict
        d = model.to_dict()
        model2 = XGBoostModel(
            num_features=d["num_features"],
            num_classes=d["num_classes"],
            base_score=d["base_score"],
            trees=[
                Tree(nodes=[
                    TreeNode(**n) for n in t["nodes"]
                ])
                for t in d["trees"]
            ],
        )
        conv2 = LightGBMConverter.from_model(model2)
        conv2.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0, 4.0])

        assert conv1.serialize() == conv2.serialize()


# =============================================================================
# Test: Regression model (num_class=1 with regression objective)
# =============================================================================


class TestRegressionModel:
    """Test that regression models are handled correctly."""

    def test_regression_num_classes(self):
        data = _sample_single_leaf_model()
        # objective is "regression"
        model = _parse_lightgbm_model(data)
        assert model.num_classes == 2  # mapped to 2 for circuit compatibility

    def test_regression_custom_objective(self):
        data = _sample_binary_model()
        data["objective"] = "regression_l2"
        data["num_class"] = 1
        model = _parse_lightgbm_model(data)
        assert model.num_classes == 2


# =============================================================================
# Test: Edge cases
# =============================================================================


class TestEdgeCases:
    """Test edge cases and error handling."""

    def test_decision_type_less_than(self):
        """LightGBM can use '<' instead of '<='."""
        data = {
            "num_class": 1,
            "max_feature_idx": 0,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "split_index": 0,
                        "split_feature": 0,
                        "threshold": 1.0,
                        "decision_type": "<",
                        "left_child": {"leaf_index": 0, "leaf_value": 0.1},
                        "right_child": {"leaf_index": 1, "leaf_value": 0.2},
                    },
                },
            ],
        }
        model = _parse_lightgbm_model(data)
        assert len(model.trees) == 1
        assert model.trees[0].nodes[0].threshold == pytest.approx(1.0)

    def test_unsupported_decision_type(self):
        data = {
            "num_class": 1,
            "max_feature_idx": 0,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "split_index": 0,
                        "split_feature": 0,
                        "threshold": 1.0,
                        "decision_type": "==",
                        "left_child": {"leaf_index": 0, "leaf_value": 0.1},
                        "right_child": {"leaf_index": 1, "leaf_value": 0.2},
                    },
                },
            ],
        }
        with pytest.raises(ModelParseError, match="unsupported decision_type"):
            _parse_lightgbm_model(data)

    def test_negative_max_feature_idx(self):
        data = {
            "num_class": 1,
            "max_feature_idx": -1,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "leaf_index": 0,
                        "leaf_value": 0.1,
                    },
                },
            ],
        }
        with pytest.raises(ModelParseError, match="num_features must be positive"):
            _parse_lightgbm_model(data)

    def test_leaf_with_no_leaf_value_defaults_to_zero(self):
        data = {
            "num_class": 1,
            "max_feature_idx": 0,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "leaf_index": 0,
                        # no leaf_value key
                    },
                },
            ],
        }
        model = _parse_lightgbm_model(data)
        assert model.trees[0].nodes[0].value == pytest.approx(0.0)

    def test_large_leaf_values(self):
        """Test with extreme float values."""
        data = {
            "num_class": 1,
            "max_feature_idx": 0,
            "objective": "binary sigmoid:1",
            "tree_info": [
                {
                    "tree_structure": {
                        "split_index": 0,
                        "split_feature": 0,
                        "threshold": 0.0,
                        "decision_type": "<=",
                        "left_child": {"leaf_index": 0, "leaf_value": -1e100},
                        "right_child": {"leaf_index": 1, "leaf_value": 1e100},
                    },
                },
            ],
        }
        model = _parse_lightgbm_model(data)
        tree = model.trees[0]
        assert tree.nodes[1].value == pytest.approx(-1e100)
        assert tree.nodes[2].value == pytest.approx(1e100)
