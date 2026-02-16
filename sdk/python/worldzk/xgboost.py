"""
World ZK Compute SDK - XGBoost Model Converter

Converts trained XGBoost models (JSON format) to risc0 serde binary input
for the xgboost-inference guest program. Zero external dependencies.

Example usage:
    converter = XGBoostConverter.from_model_file("model.json")
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
import struct
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from .errors import WorldZKError


# ═══════════════════════════════════════════════════════════════════════════════
# Errors
# ═══════════════════════════════════════════════════════════════════════════════


class ModelParseError(WorldZKError):
    """Raised when an XGBoost model JSON cannot be parsed."""

    pass


class InputValidationError(WorldZKError):
    """Raised when input data fails validation."""

    pass


# ═══════════════════════════════════════════════════════════════════════════════
# risc0 word-aligned serde helpers (private)
#
# risc0's serde format serializes everything as u32 little-endian words.
# ═══════════════════════════════════════════════════════════════════════════════


def _write_u8(val: int) -> bytes:
    """u8 -> ONE u32 word (zero-extended)."""
    return struct.pack("<I", val & 0xFF)


def _write_u32(val: int) -> bytes:
    """u32 -> ONE u32 word."""
    return struct.pack("<I", val & 0xFFFFFFFF)


def _write_u64(val: int) -> bytes:
    """u64 -> TWO u32 words (low, high)."""
    low = val & 0xFFFFFFFF
    high = (val >> 32) & 0xFFFFFFFF
    return struct.pack("<II", low, high)


def _write_f64(val: float) -> bytes:
    """f64 -> reinterpret as u64 bits -> TWO u32 words."""
    bits = struct.unpack("<Q", struct.pack("<d", val))[0]
    return _write_u64(bits)


def _write_byte_array_32(data: bytes) -> bytes:
    """[u8; 32] -> 32 u32 words (each byte as separate u32)."""
    if len(data) != 32:
        raise InputValidationError(f"Expected 32 bytes, got {len(data)}")
    result = b""
    for b in data:
        result += _write_u8(b)
    return result


def _write_vec_f64(values: List[float]) -> bytes:
    """Vec<f64> -> ONE u32 word for length, then each f64 as TWO u32 words."""
    result = _write_u32(len(values))
    for v in values:
        result += _write_f64(v)
    return result


# ═══════════════════════════════════════════════════════════════════════════════
# Dataclasses (mirror Rust structs field-for-field)
# ═══════════════════════════════════════════════════════════════════════════════


@dataclass
class TreeNode:
    """A node in an XGBoost decision tree.

    Mirrors the Rust struct:
        TreeNode { is_leaf: u32, feature_idx: u32, threshold: f64,
                   left_child: u32, right_child: u32, value: f64 }
    """

    is_leaf: int  # 0 = internal, 1 = leaf
    feature_idx: int
    threshold: float
    left_child: int
    right_child: int
    value: float

    def to_dict(self) -> Dict[str, Any]:
        return {
            "is_leaf": self.is_leaf,
            "feature_idx": self.feature_idx,
            "threshold": self.threshold,
            "left_child": self.left_child,
            "right_child": self.right_child,
            "value": self.value,
        }


@dataclass
class Tree:
    """A single decision tree.

    Mirrors the Rust struct: Tree { nodes: Vec<TreeNode> }
    """

    nodes: List[TreeNode] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return {"nodes": [n.to_dict() for n in self.nodes]}


@dataclass
class XGBoostModel:
    """XGBoost model metadata + trees.

    Mirrors the Rust struct:
        XGBoostModel { num_features: u32, num_classes: u32, base_score: f64,
                       trees: Vec<Tree> }
    """

    num_features: int
    num_classes: int
    base_score: float
    trees: List[Tree] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "XGBoostModel":
        """Parse an XGBoost JSON model (from model.save_model('model.json')).

        Navigates learner.gradient_booster.model.trees, detects leaves via
        left_children[i] == -1, and maps base_weights to leaf values.
        """
        try:
            learner = data["learner"]
            gb = learner["gradient_booster"]
            model = gb["model"]
        except (KeyError, TypeError) as e:
            raise ModelParseError(
                f"Invalid XGBoost JSON structure: missing {e}"
            )

        # Extract model params
        try:
            learner_params = learner.get("learner_model_param", {})
            num_features = int(learner_params.get("num_feature", 0))
            num_classes = int(learner_params.get("num_class", 0))
            base_score = float(learner_params.get("base_score", "0.5"))
        except (ValueError, TypeError) as e:
            raise ModelParseError(f"Invalid model parameters: {e}")

        if num_features <= 0:
            raise ModelParseError(
                f"num_features must be positive, got {num_features}"
            )

        # XGBoost uses num_class=0 for binary classification; map to 2
        if num_classes == 0:
            num_classes = 2

        # Parse trees
        raw_trees = model.get("trees", [])
        if not raw_trees:
            raise ModelParseError("Model contains no trees")

        trees = []
        for ti, raw_tree in enumerate(raw_trees):
            try:
                trees.append(_parse_xgboost_tree(raw_tree, ti, num_features))
            except (KeyError, IndexError, ValueError, TypeError) as e:
                raise ModelParseError(f"Error parsing tree {ti}: {e}")

        return cls(
            num_features=num_features,
            num_classes=num_classes,
            base_score=base_score,
            trees=trees,
        )

    def to_dict(self) -> Dict[str, Any]:
        return {
            "num_features": self.num_features,
            "num_classes": self.num_classes,
            "base_score": self.base_score,
            "trees": [t.to_dict() for t in self.trees],
        }

    def validate(self) -> None:
        """Validate model structure. Raises InputValidationError."""
        if self.num_features <= 0:
            raise InputValidationError(
                f"num_features must be positive, got {self.num_features}"
            )
        if not self.trees:
            raise InputValidationError("Model must have at least one tree")
        for ti, tree in enumerate(self.trees):
            if not tree.nodes:
                raise InputValidationError(f"Tree {ti} has no nodes")
            for ni, node in enumerate(tree.nodes):
                if node.is_leaf == 0:
                    # Internal node: check child indices
                    if node.left_child < 0 or node.left_child >= len(tree.nodes):
                        raise InputValidationError(
                            f"Tree {ti} node {ni}: left_child {node.left_child} "
                            f"out of bounds [0, {len(tree.nodes)})"
                        )
                    if node.right_child < 0 or node.right_child >= len(tree.nodes):
                        raise InputValidationError(
                            f"Tree {ti} node {ni}: right_child {node.right_child} "
                            f"out of bounds [0, {len(tree.nodes)})"
                        )
                    if node.feature_idx < 0 or node.feature_idx >= self.num_features:
                        raise InputValidationError(
                            f"Tree {ti} node {ni}: feature_idx {node.feature_idx} "
                            f"out of bounds [0, {self.num_features})"
                        )


def _parse_xgboost_tree(
    raw: Dict[str, Any], tree_idx: int, num_features: int
) -> Tree:
    """Parse a single tree from XGBoost JSON format."""
    left_children = raw["left_children"]
    right_children = raw["right_children"]
    split_indices = raw.get("split_indices", [])
    split_conditions = raw.get("split_conditions", [])
    base_weights = raw.get("base_weights", [])

    num_nodes = len(left_children)
    if num_nodes == 0:
        raise ModelParseError(f"Tree {tree_idx} has no nodes")

    nodes = []
    for i in range(num_nodes):
        is_leaf = 1 if left_children[i] == -1 else 0
        if is_leaf:
            nodes.append(TreeNode(
                is_leaf=1,
                feature_idx=0,
                threshold=0.0,
                left_child=0,
                right_child=0,
                value=float(base_weights[i]) if i < len(base_weights) else 0.0,
            ))
        else:
            feat_idx = int(split_indices[i]) if i < len(split_indices) else 0
            threshold = float(split_conditions[i]) if i < len(split_conditions) else 0.0
            nodes.append(TreeNode(
                is_leaf=0,
                feature_idx=feat_idx,
                threshold=threshold,
                left_child=int(left_children[i]),
                right_child=int(right_children[i]),
                value=0.0,
            ))

    return Tree(nodes=nodes)


@dataclass
class Sample:
    """A single input sample.

    Mirrors the Rust struct: Sample { id: [u8; 32], features: Vec<f64> }
    """

    id: bytes  # exactly 32 bytes
    features: List[float] = field(default_factory=list)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "id": self.id.hex(),
            "features": list(self.features),
        }


# ═══════════════════════════════════════════════════════════════════════════════
# XGBoostConverter — public API
# ═══════════════════════════════════════════════════════════════════════════════


class XGBoostConverter:
    """Converts XGBoost models + samples to risc0 serde binary input.

    Usage:
        converter = XGBoostConverter.from_model_file("model.json")
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
    def from_model_file(cls, path: str) -> "XGBoostConverter":
        """Load an XGBoost model from a JSON file (model.save_model('model.json'))."""
        try:
            with open(path, "r") as f:
                data = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            raise ModelParseError(f"Failed to read model file: {e}")
        return cls.from_dict(data)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "XGBoostConverter":
        """Create from a parsed XGBoost JSON dict."""
        model = XGBoostModel.from_dict(data)
        return cls(model)

    @classmethod
    def from_model(cls, model: XGBoostModel) -> "XGBoostConverter":
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

    def set_threshold(self, threshold: float) -> "XGBoostConverter":
        """Set the classification threshold. Returns self for chaining."""
        if math.isnan(threshold) or math.isinf(threshold):
            raise InputValidationError("Threshold must be a finite number")
        self._threshold = threshold
        return self

    def add_sample(self, id: bytes, features: List[float]) -> "XGBoostConverter":
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
    ) -> "XGBoostConverter":
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
        """Serialize the full XGBoostInput to risc0 serde binary format.

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


def _parse_csv_id(raw: str) -> bytes:
    """Parse a CSV ID string to 32 bytes.

    - Hex strings (with optional 0x prefix) are decoded and zero-padded.
    - Non-hex strings are SHA-256 hashed.
    """
    clean = raw.strip()
    if clean.startswith("0x") or clean.startswith("0X"):
        clean = clean[2:]

    try:
        decoded = bytes.fromhex(clean)
        if len(decoded) <= 32:
            return decoded.ljust(32, b"\x00")
        raise InputValidationError(
            f"Hex ID too long: {len(decoded)} bytes (max 32)"
        )
    except ValueError:
        # Not valid hex — hash it
        return hashlib.sha256(raw.encode("utf-8")).digest()
