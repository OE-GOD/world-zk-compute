"""
World ZK Compute SDK - LightGBM Model Converter

Converts trained LightGBM models (dump_model() JSON format) to risc0 serde
binary input for the xgboost-inference guest program. Reuses the same internal
tree representation (TreeNode, Tree, XGBoostModel) and serialization format as
XGBoostConverter, so circuit building works identically.

Zero external dependencies.

Example usage:
    converter = LightGBMConverter.from_model_file("model.json")
    converter.add_sample(id=bytes(32), features=[1.0, 2.0, 3.0])
    converter.set_threshold(0.5)

    input_bytes = converter.serialize()
    data_url = converter.to_data_url()
    digest = converter.input_digest()
"""

import base64
import csv
import hashlib
import json
import math
from typing import Any, Dict, List, Optional

from .errors import WorldZKError
from .xgboost import (
    InputValidationError,
    ModelParseError,
    Sample,
    Tree,
    TreeNode,
    XGBoostModel,
    _parse_csv_id,
    _write_byte_array_32,
    _write_f64,
    _write_u32,
    _write_vec_f64,
)


# =============================================================================
# LightGBM tree parsing (private)
# =============================================================================


def _parse_lightgbm_node(
    node: Dict[str, Any],
    nodes: List[TreeNode],
    num_features: int,
    tree_idx: int,
) -> int:
    """Recursively parse a LightGBM tree node into the flat TreeNode array.

    LightGBM dump_model() uses a recursive nested structure:
      - Internal nodes have "split_index", "split_feature", "threshold",
        "left_child", "right_child" (each of which is another node dict).
      - Leaf nodes have "leaf_index" and "leaf_value".

    We convert to the same flat array format as XGBoost:
      TreeNode { is_leaf, feature_idx, threshold, left_child, right_child, value }

    Returns the index of the node in the flat array.
    """
    my_idx = len(nodes)

    if "leaf_index" in node or "leaf_value" in node:
        # Leaf node
        leaf_value = float(node.get("leaf_value", 0.0))
        nodes.append(TreeNode(
            is_leaf=1,
            feature_idx=0,
            threshold=0.0,
            left_child=0,
            right_child=0,
            value=leaf_value,
        ))
        return my_idx

    # Internal (split) node
    split_feature = int(node.get("split_feature", 0))
    threshold = float(node.get("threshold", 0.0))
    decision_type = node.get("decision_type", "<=")

    if decision_type not in ("<=", "<"):
        raise ModelParseError(
            f"Tree {tree_idx}: unsupported decision_type '{decision_type}'. "
            f"Only '<=' and '<' are supported (categorical splits are not supported)."
        )

    if split_feature < 0 or split_feature >= num_features:
        raise ModelParseError(
            f"Tree {tree_idx}: split_feature {split_feature} "
            f"out of bounds [0, {num_features})"
        )

    # Reserve a slot for this node (will fill in children later)
    nodes.append(TreeNode(
        is_leaf=0,
        feature_idx=split_feature,
        threshold=threshold,
        left_child=0,  # placeholder
        right_child=0,  # placeholder
        value=0.0,
    ))

    # Parse children recursively
    left_child_node = node.get("left_child")
    right_child_node = node.get("right_child")

    if left_child_node is None:
        raise ModelParseError(
            f"Tree {tree_idx}: internal node missing 'left_child'"
        )
    if right_child_node is None:
        raise ModelParseError(
            f"Tree {tree_idx}: internal node missing 'right_child'"
        )

    left_idx = _parse_lightgbm_node(left_child_node, nodes, num_features, tree_idx)
    right_idx = _parse_lightgbm_node(right_child_node, nodes, num_features, tree_idx)

    # Patch the placeholder child indices
    nodes[my_idx] = TreeNode(
        is_leaf=0,
        feature_idx=split_feature,
        threshold=threshold,
        left_child=left_idx,
        right_child=right_idx,
        value=0.0,
    )

    return my_idx


def _parse_lightgbm_tree(
    tree_info: Dict[str, Any],
    tree_idx: int,
    num_features: int,
) -> Tree:
    """Parse a single LightGBM tree from its tree_info dict."""
    tree_structure = tree_info.get("tree_structure")
    if tree_structure is None:
        raise ModelParseError(
            f"Tree {tree_idx}: missing 'tree_structure'"
        )

    nodes: List[TreeNode] = []
    _parse_lightgbm_node(tree_structure, nodes, num_features, tree_idx)

    if not nodes:
        raise ModelParseError(f"Tree {tree_idx} has no nodes")

    return Tree(nodes=nodes)


def _parse_lightgbm_model(data: Dict[str, Any]) -> XGBoostModel:
    """Parse a LightGBM dump_model() JSON dict into an XGBoostModel.

    LightGBM JSON format (top-level keys):
      - "max_feature_idx": highest feature index used (0-based)
      - "num_class": number of classes (1 for binary/regression)
      - "num_tree_per_iteration": trees per boosting round
      - "tree_info": list of tree dicts, each with "tree_structure"
      - "objective": e.g. "binary sigmoid:1", "multiclass softmax num_class:3"

    We convert to XGBoostModel with:
      - num_features = max_feature_idx + 1
      - num_classes: mapped from LightGBM conventions
      - base_score = 0.0 (LightGBM uses init_score per sample, not global)
      - trees: converted from nested to flat format
    """
    # Extract num_features
    max_feature_idx = data.get("max_feature_idx")
    if max_feature_idx is None:
        raise ModelParseError(
            "Invalid LightGBM JSON: missing 'max_feature_idx'"
        )
    try:
        num_features = int(max_feature_idx) + 1
    except (ValueError, TypeError) as e:
        raise ModelParseError(f"Invalid max_feature_idx: {e}")

    if num_features <= 0:
        raise ModelParseError(
            f"num_features must be positive, got {num_features}"
        )

    # Extract num_classes
    lgb_num_class = int(data.get("num_class", 1))
    objective = data.get("objective", "")

    # LightGBM conventions:
    # - Binary classification: num_class=1, objective contains "binary"
    #   -> map to num_classes=2 (same as XGBoost)
    # - Regression: num_class=1, objective contains "regression"
    #   -> map to num_classes=2 (treat as binary for circuit compatibility)
    # - Multiclass: num_class=N (N>=2), objective contains "multiclass"
    #   -> keep as-is
    if lgb_num_class <= 1:
        num_classes = 2
    else:
        num_classes = lgb_num_class

    # Parse trees
    tree_info_list = data.get("tree_info", [])
    if not tree_info_list:
        raise ModelParseError("Model contains no trees")

    trees = []
    for ti, tree_info in enumerate(tree_info_list):
        try:
            trees.append(_parse_lightgbm_tree(tree_info, ti, num_features))
        except (KeyError, IndexError, ValueError, TypeError) as e:
            raise ModelParseError(f"Error parsing tree {ti}: {e}")

    return XGBoostModel(
        num_features=num_features,
        num_classes=num_classes,
        base_score=0.0,
        trees=trees,
    )


# =============================================================================
# LightGBMConverter -- public API
# =============================================================================


class LightGBMConverter:
    """Converts LightGBM models + samples to risc0 serde binary input.

    Uses the same internal representation and serialization format as
    XGBoostConverter, so circuit building works identically.

    Usage:
        converter = LightGBMConverter.from_model_file("model.json")
        converter.add_sample(id=bytes(32), features=[1.0, 2.0])
        converter.set_threshold(0.5)

        input_bytes = converter.serialize()
        data_url = converter.to_data_url()
    """

    def __init__(self, model: XGBoostModel) -> None:
        self._model = model
        self._samples: List[Sample] = []
        self._threshold: float = 0.5

    @classmethod
    def from_model_file(cls, path: str) -> "LightGBMConverter":
        """Load a LightGBM model from a JSON file (model.dump_model())."""
        try:
            with open(path, "r") as f:
                data = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            raise ModelParseError(f"Failed to read model file: {e}")
        return cls.from_dict(data)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "LightGBMConverter":
        """Create from a parsed LightGBM dump_model() JSON dict."""
        model = _parse_lightgbm_model(data)
        return cls(model)

    @classmethod
    def from_model(cls, model: XGBoostModel) -> "LightGBMConverter":
        """Create from a pre-built XGBoostModel dataclass."""
        return cls(model)

    @property
    def model(self) -> XGBoostModel:
        return self._model

    @property
    def samples(self) -> List[Sample]:
        return list(self._samples)

    @property
    def threshold(self) -> float:
        return self._threshold

    def set_threshold(self, threshold: float) -> "LightGBMConverter":
        """Set the classification threshold. Returns self for chaining."""
        if math.isnan(threshold) or math.isinf(threshold):
            raise InputValidationError("Threshold must be a finite number")
        self._threshold = threshold
        return self

    def add_sample(self, id: bytes, features: List[float]) -> "LightGBMConverter":
        """Add a single sample. Returns self for chaining.

        Args:
            id: Exactly 32 bytes identifying the sample.
            features: Feature values; count must match model.num_features.
        """
        self._validate_sample(id, features)
        self._samples.append(Sample(id=id, features=list(features)))
        return self

    def add_samples_from_csv(
        self,
        path: str,
        id_column: str,
        feature_columns: List[str],
    ) -> "LightGBMConverter":
        """Add samples from a CSV file. Returns self for chaining.

        Args:
            path: Path to CSV file.
            id_column: Column name for sample IDs.
            feature_columns: Column names for features (order matters).

        ID handling:
            - Hex strings are decoded and zero-padded to 32 bytes.
            - Non-hex strings are SHA-256 hashed to produce 32 bytes.
        """
        try:
            with open(path, "r", newline="") as f:
                reader = csv.DictReader(f)
                for row_num, row in enumerate(reader, start=1):
                    if id_column not in row:
                        raise InputValidationError(
                            f"Row {row_num}: missing id column '{id_column}'"
                        )

                    raw_id = row[id_column].strip()
                    sample_id = _parse_csv_id(raw_id)

                    features = []
                    for col in feature_columns:
                        if col not in row:
                            raise InputValidationError(
                                f"Row {row_num}: missing feature column '{col}'"
                            )
                        try:
                            features.append(float(row[col]))
                        except ValueError:
                            raise InputValidationError(
                                f"Row {row_num}: invalid float in column '{col}': {row[col]!r}"
                            )

                    self._validate_sample(sample_id, features)
                    self._samples.append(Sample(id=sample_id, features=features))
        except OSError as e:
            raise InputValidationError(f"Failed to read CSV file: {e}")

        return self

    def serialize(self) -> bytes:
        """Serialize the full input to risc0 serde binary format.

        Layout (matching Rust XGBoostInput):
            model: XGBoostModel
            samples: Vec<Sample>
            threshold: f64
        """
        self._validate_all()

        buf = b""

        # model: XGBoostModel
        buf += _write_u32(self._model.num_features)
        buf += _write_u32(self._model.num_classes)
        buf += _write_f64(self._model.base_score)

        # trees: Vec<Tree>
        buf += _write_u32(len(self._model.trees))
        for tree in self._model.trees:
            # nodes: Vec<TreeNode>
            buf += _write_u32(len(tree.nodes))
            for node in tree.nodes:
                buf += _write_u32(node.is_leaf)
                buf += _write_u32(node.feature_idx)
                buf += _write_f64(node.threshold)
                buf += _write_u32(node.left_child)
                buf += _write_u32(node.right_child)
                buf += _write_f64(node.value)

        # samples: Vec<Sample>
        buf += _write_u32(len(self._samples))
        for sample in self._samples:
            buf += _write_byte_array_32(sample.id)
            buf += _write_vec_f64(sample.features)

        # threshold: f64
        buf += _write_f64(self._threshold)

        return buf

    def to_data_url(self) -> str:
        """Serialize and return as a data URL (base64-encoded)."""
        raw = self.serialize()
        encoded = base64.b64encode(raw).decode("ascii")
        return f"data:application/octet-stream;base64,{encoded}"

    def input_digest(self) -> str:
        """Return the SHA-256 hex digest of the serialized input (0x-prefixed)."""
        raw = self.serialize()
        return "0x" + hashlib.sha256(raw).hexdigest()

    def _validate_sample(self, id: bytes, features: List[float]) -> None:
        """Validate a single sample's id and features."""
        if not isinstance(id, (bytes, bytearray)):
            raise InputValidationError(
                f"Sample ID must be bytes, got {type(id).__name__}"
            )
        if len(id) != 32:
            raise InputValidationError(
                f"Sample ID must be exactly 32 bytes, got {len(id)}"
            )
        if len(features) != self._model.num_features:
            raise InputValidationError(
                f"Expected {self._model.num_features} features, got {len(features)}"
            )
        for i, f in enumerate(features):
            if math.isnan(f) or math.isinf(f):
                raise InputValidationError(
                    f"Feature {i} is {f}; NaN and Inf are not allowed"
                )

    def _validate_all(self) -> None:
        """Validate the full input before serialization."""
        self._model.validate()
        if not self._samples:
            raise InputValidationError("At least one sample is required")
