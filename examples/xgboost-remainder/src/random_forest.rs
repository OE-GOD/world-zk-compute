//! Scikit-learn Random Forest model parser
//!
//! Converts scikit-learn random forest JSON export format to the existing
//! `XgboostModel` / `DecisionTree` / `TreeNode` types used by the circuit
//! builder and prover.
//!
//! The circuit builder already handles tree-ensemble inference.  Random forests
//! differ from XGBoost only in the leaf encoding: instead of boosted residuals,
//! each leaf carries a class-probability value derived from the node's class
//! count histogram.  We scale by `FIXED_POINT_SCALE` (2^16) so the existing
//! fixed-point arithmetic in the circuit remains correct.
//!
//! # JSON format
//!
//! Scikit-learn does not expose a native JSON dump, so we define a simple
//! custom format (see module-level doc example):
//!
//! ```json
//! {
//!   "model_type": "random_forest",
//!   "n_estimators": 3,
//!   "n_features": 4,
//!   "n_classes": 2,
//!   "trees": [
//!     {
//!       "children_left":  [1, -1, -1],
//!       "children_right": [2, -1, -1],
//!       "feature":    [0, -2, -2],
//!       "threshold":  [5.0, -2.0, -2.0],
//!       "value":      [[0,0],[30,0],[0,20]]
//!     }
//!   ]
//! }
//! ```
//!
//! * `children_left[i]`  = left child index; -1 means leaf.
//! * `children_right[i]` = right child index; -1 means leaf.
//! * `feature[i]`        = split feature index; -2 means leaf.
//! * `threshold[i]`      = split threshold; -2.0 means leaf.
//! * `value[i]`          = class-count histogram at node i (all classes).

use crate::model::{tree_depth, DecisionTree, TreeNode, XgboostModel, FIXED_POINT_SCALE};
use serde::Deserialize;
use std::path::Path;

// ============================================================================
// Serde types for the custom sklearn JSON format
// ============================================================================

/// Top-level random forest model.
#[derive(Debug, Deserialize)]
struct RawRandomForest {
    /// Must be "random_forest" (validated at parse time).
    #[serde(default)]
    model_type: String,
    /// Number of trees.
    n_estimators: usize,
    /// Number of input features.
    n_features: usize,
    /// Number of output classes.
    n_classes: usize,
    /// Per-tree sklearn array representation.
    trees: Vec<RawSklearnTree>,
}

/// One sklearn tree (parallel-array representation).
///
/// All arrays have the same length (number of nodes in the tree).
#[derive(Debug, Deserialize)]
struct RawSklearnTree {
    /// Left child index; -1 = leaf.
    children_left: Vec<i32>,
    /// Right child index; -1 = leaf.
    children_right: Vec<i32>,
    /// Split feature index; -2 = leaf.
    feature: Vec<i32>,
    /// Split threshold; -2.0 = leaf.
    threshold: Vec<f64>,
    /// Class count histogram per node: `value[node_idx][class_idx]`.
    value: Vec<Vec<f64>>,
}

// ============================================================================
// Public API
// ============================================================================

/// Load a random forest model from a JSON file.
pub fn load_random_forest_json(path: &Path) -> Result<XgboostModel, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read model file: {}", e))?;
    parse_random_forest_json(&data)
}

/// Parse a random forest model from a JSON string and convert to `XgboostModel`.
///
/// Each sklearn tree (array-of-arrays) is converted to a flat `DecisionTree`
/// whose leaf values are the per-class probabilities scaled by `FIXED_POINT_SCALE`.
pub fn parse_random_forest_json(json_str: &str) -> Result<XgboostModel, String> {
    let raw: RawRandomForest = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse random forest JSON: {}", e))?;

    if !raw.model_type.is_empty() && raw.model_type != "random_forest" {
        return Err(format!(
            "Expected model_type \"random_forest\", got \"{}\"",
            raw.model_type
        ));
    }

    if raw.n_classes == 0 {
        return Err("n_classes must be >= 1".to_string());
    }
    if raw.n_features == 0 {
        return Err("n_features must be >= 1".to_string());
    }
    if raw.trees.len() != raw.n_estimators {
        return Err(format!(
            "n_estimators={} but {} trees provided",
            raw.n_estimators,
            raw.trees.len()
        ));
    }

    let n_classes = raw.n_classes;
    let mut trees = Vec::with_capacity(raw.trees.len());
    let mut max_depth = 0usize;

    for (tree_idx, raw_tree) in raw.trees.iter().enumerate() {
        let dt = convert_sklearn_tree(raw_tree, n_classes, tree_idx)?;
        let d = tree_depth(&dt);
        if d > max_depth {
            max_depth = d;
        }
        trees.push(dt);
    }

    Ok(XgboostModel {
        num_features: raw.n_features,
        num_classes: n_classes,
        max_depth,
        trees,
        // Random forests have no additive base score; bias handled by leaf probabilities.
        base_score: 0.0,
    })
}

// ============================================================================
// Internal conversion helpers
// ============================================================================

/// Convert one sklearn parallel-array tree to a `DecisionTree`.
///
/// Leaf value encoding (binary classification, n_classes == 2):
///   leaf_value = (count_class_1 / total_count) * FIXED_POINT_SCALE
///
/// For multi-class, the "score" is the probability of the argmax class:
///   leaf_value = (max_count / total_count) * FIXED_POINT_SCALE
///
/// This keeps compatibility with the existing circuit, which sums leaf values
/// and compares the sum to a threshold to determine the predicted class.
fn convert_sklearn_tree(
    raw: &RawSklearnTree,
    n_classes: usize,
    tree_idx: usize,
) -> Result<DecisionTree, String> {
    let n = raw.children_left.len();
    if n == 0 {
        return Err(format!("Tree {} has no nodes", tree_idx));
    }

    // All arrays must have the same length.
    if raw.children_right.len() != n
        || raw.feature.len() != n
        || raw.threshold.len() != n
        || raw.value.len() != n
    {
        return Err(format!(
            "Tree {}: array length mismatch (children_left={}, children_right={}, \
             feature={}, threshold={}, value={})",
            tree_idx,
            n,
            raw.children_right.len(),
            raw.feature.len(),
            raw.threshold.len(),
            raw.value.len()
        ));
    }

    let mut nodes = Vec::with_capacity(n);

    for i in 0..n {
        let is_leaf = raw.children_left[i] == -1;

        let leaf_value = if is_leaf {
            compute_leaf_value(&raw.value[i], n_classes, tree_idx, i)?
        } else {
            0.0
        };

        nodes.push(TreeNode {
            feature_index: if is_leaf { -1 } else { raw.feature[i] },
            threshold: if is_leaf { 0.0 } else { raw.threshold[i] },
            left_child: if is_leaf {
                0
            } else {
                raw.children_left[i] as usize
            },
            right_child: if is_leaf {
                0
            } else {
                raw.children_right[i] as usize
            },
            leaf_value,
            is_leaf,
        });
    }

    Ok(DecisionTree { nodes })
}

/// Compute the fixed-point leaf value from a class-count histogram.
///
/// For binary classification (n_classes == 2):
///   The leaf value represents the probability of class 1, scaled by
///   FIXED_POINT_SCALE.  Summing these across trees gives a score that can
///   be compared to (n_estimators / 2) * FIXED_POINT_SCALE for majority vote.
///
/// For multi-class:
///   The leaf value is the probability of the plurality class, scaled by
///   FIXED_POINT_SCALE.
fn compute_leaf_value(
    counts: &[f64],
    n_classes: usize,
    tree_idx: usize,
    node_idx: usize,
) -> Result<f64, String> {
    if counts.len() < n_classes {
        return Err(format!(
            "Tree {} node {}: value has {} entries, expected >= {} (n_classes)",
            tree_idx,
            node_idx,
            counts.len(),
            n_classes,
        ));
    }

    let total: f64 = counts.iter().sum();
    if total <= 0.0 {
        // No samples reached this leaf (should not happen in a fitted forest,
        // but handle gracefully by returning 0).
        return Ok(0.0);
    }

    let scaled_value = if n_classes == 2 {
        // Binary: probability of class 1.
        (counts[1] / total) * FIXED_POINT_SCALE as f64
    } else {
        // Multi-class: probability of the plurality (argmax) class.
        let max_count = counts[..n_classes]
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        (max_count / total) * FIXED_POINT_SCALE as f64
    };

    Ok(scaled_value.round())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{predict, quantize, traverse_tree};

    // -----------------------------------------------------------------------
    // Test constants
    // -----------------------------------------------------------------------

    /// Minimal 1-tree, 3-node forest (root + 2 leaves).
    const MINIMAL_RF_JSON: &str = r#"{
        "model_type": "random_forest",
        "n_estimators": 1,
        "n_features": 2,
        "n_classes": 2,
        "trees": [
            {
                "children_left":  [1, -1, -1],
                "children_right": [2, -1, -1],
                "feature":    [0, -2, -2],
                "threshold":  [5.0, -2.0, -2.0],
                "value":      [[50, 50], [30, 0], [0, 20]]
            }
        ]
    }"#;

    /// 2-tree, 4-feature model used in multiple tests.
    const SAMPLE_RANDOM_FOREST_JSON: &str = r#"{
        "model_type": "random_forest",
        "n_estimators": 2,
        "n_features": 4,
        "n_classes": 2,
        "trees": [
            {
                "children_left": [1, 3, 5, -1, -1, -1, -1],
                "children_right": [2, 4, 6, -1, -1, -1, -1],
                "feature": [0, 1, 2, -2, -2, -2, -2],
                "threshold": [5.0, 3.0, 1.5, -2.0, -2.0, -2.0, -2.0],
                "value": [[50, 50], [30, 0], [0, 20], [20, 0], [10, 0], [0, 15], [0, 5]]
            },
            {
                "children_left": [1, 3, 5, -1, -1, -1, -1],
                "children_right": [2, 4, 6, -1, -1, -1, -1],
                "feature": [2, 3, 0, -2, -2, -2, -2],
                "threshold": [2.0, 0.5, 6.0, -2.0, -2.0, -2.0, -2.0],
                "value": [[50, 50], [30, 10], [0, 30], [25, 5], [5, 5], [0, 20], [0, 10]]
            }
        ]
    }"#;

    // -----------------------------------------------------------------------
    // Parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_minimal_forest() {
        let model = parse_random_forest_json(MINIMAL_RF_JSON).unwrap();

        assert_eq!(model.trees.len(), 1);
        assert_eq!(model.num_features, 2);
        assert_eq!(model.num_classes, 2);
        assert!((model.base_score).abs() < 1e-15, "base_score should be 0");

        let tree = &model.trees[0];
        assert_eq!(tree.nodes.len(), 3);

        // Root node
        let root = &tree.nodes[0];
        assert!(!root.is_leaf);
        assert_eq!(root.feature_index, 0);
        assert!((root.threshold - 5.0).abs() < 1e-9);
        assert_eq!(root.left_child, 1);
        assert_eq!(root.right_child, 2);

        // Node 1: left leaf (all class-0 votes → prob of class 1 = 0)
        let leaf_left = &tree.nodes[1];
        assert!(leaf_left.is_leaf);
        assert!((leaf_left.leaf_value).abs() < 1.0, "left leaf: prob class1 ≈ 0");

        // Node 2: right leaf (all class-1 votes → prob of class 1 = 1.0 * SCALE)
        let leaf_right = &tree.nodes[2];
        assert!(leaf_right.is_leaf);
        let expected = FIXED_POINT_SCALE as f64;
        assert!(
            (leaf_right.leaf_value - expected).abs() < 1.0,
            "right leaf: prob class1 ≈ FIXED_POINT_SCALE, got {}",
            leaf_right.leaf_value
        );
    }

    #[test]
    fn test_parse_sample_forest() {
        let model = parse_random_forest_json(SAMPLE_RANDOM_FOREST_JSON).unwrap();

        assert_eq!(model.trees.len(), 2);
        assert_eq!(model.num_features, 4);
        assert_eq!(model.num_classes, 2);

        // Both trees are depth-2 complete binary trees (7 nodes each).
        assert_eq!(model.trees[0].nodes.len(), 7);
        assert_eq!(model.trees[1].nodes.len(), 7);
        assert_eq!(model.max_depth, 2);
    }

    #[test]
    fn test_parse_3_tree_forest() {
        let json = r#"{
            "model_type": "random_forest",
            "n_estimators": 3,
            "n_features": 2,
            "n_classes": 2,
            "trees": [
                {
                    "children_left":  [1, -1, -1],
                    "children_right": [2, -1, -1],
                    "feature":    [0, -2, -2],
                    "threshold":  [1.0, -2.0, -2.0],
                    "value":      [[10,10],[8,2],[2,8]]
                },
                {
                    "children_left":  [1, 3, 5, -1, -1, -1, -1],
                    "children_right": [2, 4, 6, -1, -1, -1, -1],
                    "feature":    [1, 0, 1, -2, -2, -2, -2],
                    "threshold":  [2.0, 0.5, 3.0, -2.0, -2.0, -2.0, -2.0],
                    "value":      [[20,20],[15,5],[5,15],[10,0],[5,5],[2,13],[3,2]]
                },
                {
                    "children_left":  [-1],
                    "children_right": [-1],
                    "feature":    [-2],
                    "threshold":  [-2.0],
                    "value":      [[3,7]]
                }
            ]
        }"#;

        let model = parse_random_forest_json(json).unwrap();
        assert_eq!(model.trees.len(), 3);
        // Tree depths: 1, 2, 0 → max = 2
        assert_eq!(model.max_depth, 2);
        // Single-node tree (depth 0)
        assert_eq!(model.trees[2].nodes.len(), 1);
        assert!(model.trees[2].nodes[0].is_leaf);
    }

    // -----------------------------------------------------------------------
    // Leaf value tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_leaf_value_binary_pure_class0() {
        // All votes for class 0 → prob(class1) = 0
        let counts = vec![100.0f64, 0.0];
        let val = compute_leaf_value(&counts, 2, 0, 0).unwrap();
        assert!((val).abs() < 1.0, "expected ~0, got {}", val);
    }

    #[test]
    fn test_leaf_value_binary_pure_class1() {
        // All votes for class 1 → prob(class1) = 1.0 * FIXED_POINT_SCALE
        let counts = vec![0.0f64, 50.0];
        let val = compute_leaf_value(&counts, 2, 0, 0).unwrap();
        let expected = FIXED_POINT_SCALE as f64;
        assert!((val - expected).abs() < 1.0, "expected {}, got {}", expected, val);
    }

    #[test]
    fn test_leaf_value_binary_equal_split() {
        // Equal votes → prob(class1) = 0.5 * FIXED_POINT_SCALE
        let counts = vec![25.0f64, 25.0];
        let val = compute_leaf_value(&counts, 2, 0, 0).unwrap();
        let expected = (FIXED_POINT_SCALE / 2) as f64;
        assert!((val - expected).abs() < 1.0, "expected ~{}, got {}", expected, val);
    }

    #[test]
    fn test_leaf_value_binary_asymmetric() {
        // 30 class-0, 20 class-1 → prob(class1) = 20/50 = 0.4
        let counts = vec![30.0f64, 20.0];
        let val = compute_leaf_value(&counts, 2, 0, 0).unwrap();
        let expected = (0.4 * FIXED_POINT_SCALE as f64).round();
        assert!((val - expected).abs() < 1.0, "expected {}, got {}", expected, val);
    }

    #[test]
    fn test_leaf_value_multiclass() {
        // 3 classes: [10, 30, 20] → argmax = class 1 (30 out of 60) = 0.5 * SCALE
        let counts = vec![10.0f64, 30.0, 20.0];
        let val = compute_leaf_value(&counts, 3, 0, 0).unwrap();
        let expected = (0.5 * FIXED_POINT_SCALE as f64).round();
        assert!((val - expected).abs() < 1.0, "expected {}, got {}", expected, val);
    }

    #[test]
    fn test_leaf_value_zero_total() {
        // No samples → value should be 0 (no panic)
        let counts = vec![0.0f64, 0.0];
        let val = compute_leaf_value(&counts, 2, 0, 0).unwrap();
        assert!((val).abs() < 1.0, "expected 0, got {}", val);
    }

    // -----------------------------------------------------------------------
    // Error handling tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_wrong_model_type() {
        let json = r#"{
            "model_type": "xgboost",
            "n_estimators": 1,
            "n_features": 2,
            "n_classes": 2,
            "trees": [
                {"children_left":[-1],"children_right":[-1],
                 "feature":[-2],"threshold":[-2.0],"value":[[5,5]]}
            ]
        }"#;
        let err = parse_random_forest_json(json).unwrap_err();
        assert!(err.contains("random_forest"), "error: {}", err);
    }

    #[test]
    fn test_error_estimator_count_mismatch() {
        let json = r#"{
            "model_type": "random_forest",
            "n_estimators": 3,
            "n_features": 2,
            "n_classes": 2,
            "trees": [
                {"children_left":[-1],"children_right":[-1],
                 "feature":[-2],"threshold":[-2.0],"value":[[5,5]]}
            ]
        }"#;
        let err = parse_random_forest_json(json).unwrap_err();
        assert!(
            err.contains("n_estimators") || err.contains("1 trees"),
            "error: {}",
            err
        );
    }

    #[test]
    fn test_error_array_length_mismatch() {
        let json = r#"{
            "model_type": "random_forest",
            "n_estimators": 1,
            "n_features": 2,
            "n_classes": 2,
            "trees": [
                {"children_left":[1,-1,-1],
                 "children_right":[2,-1],
                 "feature":[0,-2,-2],
                 "threshold":[1.0,-2.0,-2.0],
                 "value":[[10,10],[8,2],[2,8]]}
            ]
        }"#;
        let err = parse_random_forest_json(json).unwrap_err();
        assert!(err.contains("mismatch"), "error: {}", err);
    }

    // -----------------------------------------------------------------------
    // Inference tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_inference_minimal_forest() {
        // Single tree: feature[0] < 5.0 → left leaf (class0), else → right leaf (class1).
        let model = parse_random_forest_json(MINIMAL_RF_JSON).unwrap();

        // feature[0] = 3.0 < 5.0 → left leaf → prob(class1) ≈ 0
        let val_left = traverse_tree(&model.trees[0], &[3.0, 0.0]);
        assert!(val_left < (FIXED_POINT_SCALE as f64 / 2.0), "left branch: {}", val_left);

        // feature[0] = 7.0 >= 5.0 → right leaf → prob(class1) ≈ SCALE
        let val_right = traverse_tree(&model.trees[0], &[7.0, 0.0]);
        assert!(val_right > (FIXED_POINT_SCALE as f64 / 2.0), "right branch: {}", val_right);
    }

    #[test]
    fn test_predict_sample_forest() {
        // In the sample model, we just verify predict() runs without panic
        // and returns a valid class index (0 or 1).
        let model = parse_random_forest_json(SAMPLE_RANDOM_FOREST_JSON).unwrap();
        let features = vec![4.0f64, 2.0, 1.0, 0.3];
        let class_pred = predict(&model, &features);
        assert!(class_pred < 2, "predicted class should be 0 or 1, got {}", class_pred);
    }

    #[test]
    fn test_quantize_consistency() {
        // Ensure that quantize(dequantize(x)) round-trips for typical leaf values.
        let val = 0.75_f64;
        let q = quantize(val);
        let expected = (val * FIXED_POINT_SCALE as f64).round() as i64;
        assert_eq!(q, expected);
    }

    // -----------------------------------------------------------------------
    // File-based tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_from_file() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-model/random_forest_sample.json");
        if !path.exists() {
            // File may not exist in all environments; skip gracefully.
            eprintln!("Skipping test_load_from_file: file not found at {:?}", path);
            return;
        }
        let model = load_random_forest_json(&path).unwrap();
        assert!(model.trees.len() >= 1, "expected at least 1 tree");
        assert!(model.num_features >= 1, "expected at least 1 feature");
    }

    // -----------------------------------------------------------------------
    // E2E prove + verify
    // -----------------------------------------------------------------------

    /// End-to-end test: parse random forest → build circuit → prove → verify.
    ///
    /// Run with:
    ///   cargo test test_random_forest_e2e -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_random_forest_e2e() {
        use crate::circuit::build_and_prove;

        let model = parse_random_forest_json(SAMPLE_RANDOM_FOREST_JSON)
            .expect("failed to parse sample random forest");

        eprintln!(
            "Random forest: {} trees, {} features, max_depth={}",
            model.trees.len(),
            model.num_features,
            model.max_depth
        );

        let features = vec![4.0f64, 2.0, 1.0, 0.3];
        let predicted_class = predict(&model, &features);
        eprintln!("Predicted class: {}", predicted_class);

        let (proof_bytes, circuit_hash, public_inputs) =
            build_and_prove(&model, &features, predicted_class)
                .expect("build_and_prove failed");

        eprintln!("Proof size: {} bytes", proof_bytes.len());
        eprintln!("Circuit hash: 0x{}", hex::encode(&circuit_hash));
        eprintln!("Public inputs: 0x{}", hex::encode(&public_inputs));

        assert_eq!(
            &proof_bytes[0..4],
            b"REM1",
            "proof should start with REM1 selector"
        );
        assert!(!circuit_hash.is_empty(), "circuit hash should be non-empty");
    }
}
