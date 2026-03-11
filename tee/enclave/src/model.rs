//! Model loading and inference for XGBoost and LightGBM.
//!
//! Supports two JSON formats:
//! - **XGBoost**: produced by `xgb.save_model("model.json")` (array-of-struct trees)
//! - **LightGBM**: produced by `lgb.booster_.save_model("model.json")` or
//!   `lgb.booster_.dump_model()` (recursive tree structures)
//!
//! Both formats are parsed into a common `XgboostModel` / `Tree` representation
//! so that inference uses a single code path.

use serde::Deserialize;
use serde_json::Value;

/// A complete XGBoost model (ensemble of decision trees).
#[derive(Debug)]
pub struct XgboostModel {
    /// Number of input features.
    pub num_features: usize,
    /// Number of output classes.
    /// For binary classification / regression, XGBoost stores `num_class = 0` in JSON;
    /// we normalize this to 2 internally.
    pub num_classes: usize,
    /// Base score (bias term added to predictions).
    pub base_score: f64,
    /// The decision trees in the ensemble.
    pub trees: Vec<Tree>,
}

/// A single decision tree in array-of-struct format (matching XGBoost JSON).
#[derive(Debug)]
pub struct Tree {
    pub left_children: Vec<i32>,
    pub right_children: Vec<i32>,
    pub split_indices: Vec<usize>,
    pub split_conditions: Vec<f64>,
    pub base_weights: Vec<f64>,
}

/// Model format selector for loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelFormat {
    /// Automatically detect the format by trying XGBoost first, then LightGBM.
    #[default]
    Auto,
    /// Force XGBoost JSON parsing.
    Xgboost,
    /// Force LightGBM JSON parsing.
    Lightgbm,
}

impl ModelFormat {
    /// Parse from a string (case-insensitive). Returns `None` for unrecognized values.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "xgboost" => Some(Self::Xgboost),
            "lightgbm" | "lgbm" => Some(Self::Lightgbm),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// XGBoost JSON schema (for deserialization only)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct XgbJsonModel {
    learner: XgbLearner,
}

#[derive(Deserialize)]
struct XgbLearner {
    learner_model_param: XgbModelParam,
    gradient_booster: XgbGradientBooster,
}

#[derive(Deserialize)]
struct XgbModelParam {
    num_feature: String,
    #[serde(default)]
    num_class: String,
    #[serde(default = "default_base_score")]
    base_score: String,
}

fn default_base_score() -> String {
    "5E-1".to_string()
}

#[derive(Deserialize)]
struct XgbGradientBooster {
    model: XgbGbtreeModel,
}

#[derive(Deserialize)]
struct XgbGbtreeModel {
    trees: Vec<XgbTree>,
    #[serde(default)]
    #[allow(dead_code)]
    tree_info: Vec<i32>,
}

#[derive(Deserialize)]
struct XgbTree {
    left_children: Vec<i32>,
    right_children: Vec<i32>,
    split_indices: Vec<i32>,
    split_conditions: Vec<f64>,
    base_weights: Vec<f64>,
}

// ---------------------------------------------------------------------------
// LightGBM JSON schema (for deserialization only)
// ---------------------------------------------------------------------------

/// Top-level LightGBM model JSON structure.
/// LightGBM JSON can come from `dump_model()` or `save_model()`.
#[derive(Deserialize)]
struct LgbJsonModel {
    /// Tree info array; each element contains a `tree_structure`.
    #[serde(default)]
    tree_info: Vec<LgbTreeInfo>,
    /// Number of features (string in LightGBM JSON).
    #[serde(default)]
    max_feature_idx: Option<i64>,
    /// Number of classes for multi-class (-1 or 1 for binary).
    #[serde(default)]
    #[allow(dead_code)]
    num_class: Option<i64>,
    /// Number of trees per iteration (equals num_class for multi-class, 1 for binary).
    #[serde(default)]
    num_tree_per_iteration: Option<i64>,
}

#[derive(Deserialize)]
struct LgbTreeInfo {
    #[serde(default)]
    tree_structure: Option<Value>,
}

/// Flatten a recursive LightGBM tree node (JSON `Value`) into the array-of-struct
/// `Tree` representation used by the rest of the crate.
///
/// LightGBM internal nodes have:
///   `split_feature`, `threshold`, `decision_type`, `left_child`, `right_child`
/// Leaf nodes have:
///   `leaf_value`
fn flatten_lgb_tree(root: &Value) -> Result<Tree, String> {
    let mut left_children: Vec<i32> = Vec::new();
    let mut right_children: Vec<i32> = Vec::new();
    let mut split_indices: Vec<usize> = Vec::new();
    let mut split_conditions: Vec<f64> = Vec::new();
    let mut base_weights: Vec<f64> = Vec::new();

    // BFS / pre-order traversal, assigning sequential indices.
    // We use a queue of (json_node, assigned_index).
    let mut queue: Vec<(Value, usize)> = Vec::new();
    queue.push((root.clone(), 0));

    // Pre-allocate slot 0
    left_children.push(0);
    right_children.push(0);
    split_indices.push(0);
    split_conditions.push(0.0);
    base_weights.push(0.0);

    let mut head = 0;
    while head < queue.len() {
        let (node, idx) = queue[head].clone();
        head += 1;

        if node.get("leaf_value").is_some() {
            // Leaf node
            let leaf_val = node["leaf_value"]
                .as_f64()
                .ok_or_else(|| "Invalid leaf_value".to_string())?;
            left_children[idx] = -1;
            right_children[idx] = -1;
            split_indices[idx] = 0;
            split_conditions[idx] = 0.0;
            base_weights[idx] = leaf_val;
        } else {
            // Internal node
            let split_feature = node
                .get("split_feature")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("Missing split_feature at node {}", idx))?
                as usize;
            let threshold = node
                .get("threshold")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("Missing threshold at node {}", idx))?;

            let left_child_json = node
                .get("left_child")
                .ok_or_else(|| format!("Missing left_child at node {}", idx))?;
            let right_child_json = node
                .get("right_child")
                .ok_or_else(|| format!("Missing right_child at node {}", idx))?;

            // Assign indices for children
            let left_idx = left_children.len();
            left_children.push(0);
            right_children.push(0);
            split_indices.push(0);
            split_conditions.push(0.0);
            base_weights.push(0.0);

            let right_idx = left_children.len();
            left_children.push(0);
            right_children.push(0);
            split_indices.push(0);
            split_conditions.push(0.0);
            base_weights.push(0.0);

            left_children[idx] = left_idx as i32;
            right_children[idx] = right_idx as i32;
            split_indices[idx] = split_feature;
            split_conditions[idx] = threshold;
            base_weights[idx] = 0.0;

            queue.push((left_child_json.clone(), left_idx));
            queue.push((right_child_json.clone(), right_idx));
        }
    }

    if left_children.is_empty() {
        return Err("LightGBM tree has no nodes".to_string());
    }

    Ok(Tree {
        left_children,
        right_children,
        split_indices,
        split_conditions,
        base_weights,
    })
}

/// Parse a LightGBM model from a JSON string.
///
/// LightGBM JSON format (from `dump_model()` or `save_model()`):
/// ```json
/// {
///   "name": "tree",
///   "max_feature_idx": 3,
///   "num_class": 1,
///   "num_tree_per_iteration": 1,
///   "tree_info": [
///     {
///       "tree_index": 0,
///       "tree_structure": { ... recursive ... }
///     }
///   ]
/// }
/// ```
pub fn parse_lightgbm(json_str: &str) -> Result<XgboostModel, String> {
    let raw: LgbJsonModel = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse LightGBM JSON: {}", e))?;

    if raw.tree_info.is_empty() {
        return Err("LightGBM model has no trees (tree_info is empty)".to_string());
    }

    // Determine number of features.
    // max_feature_idx is 0-indexed, so num_features = max_feature_idx + 1.
    // If not present, scan tree nodes to find the maximum split_feature.
    let num_features = if let Some(mfi) = raw.max_feature_idx {
        (mfi + 1) as usize
    } else {
        // Fallback: scan all trees for the maximum split_feature
        let mut max_feat: usize = 0;
        for ti in &raw.tree_info {
            if let Some(ts) = &ti.tree_structure {
                scan_max_feature(ts, &mut max_feat);
            }
        }
        max_feat + 1
    };

    // Determine number of classes.
    let num_tree_per_iter = raw.num_tree_per_iteration.unwrap_or(1) as usize;
    let num_classes = if num_tree_per_iter <= 1 {
        2
    } else {
        num_tree_per_iter
    };

    // Parse each tree.
    let mut trees = Vec::with_capacity(raw.tree_info.len());
    for (idx, ti) in raw.tree_info.iter().enumerate() {
        let ts = ti
            .tree_structure
            .as_ref()
            .ok_or_else(|| format!("Tree {} has no tree_structure", idx))?;
        let tree = flatten_lgb_tree(ts)?;
        trees.push(tree);
    }

    // LightGBM does not have a base_score in the same way as XGBoost.
    // The bias is baked into the first tree or handled externally.
    // We use 0.0 as the default.
    Ok(XgboostModel {
        num_features,
        num_classes,
        base_score: 0.0,
        trees,
    })
}

/// Recursively scan a LightGBM tree JSON node to find the maximum split_feature index.
fn scan_max_feature(node: &Value, max_feat: &mut usize) {
    if let Some(sf) = node.get("split_feature").and_then(|v| v.as_u64()) {
        let f = sf as usize;
        if f > *max_feat {
            *max_feat = f;
        }
    }
    if let Some(left) = node.get("left_child") {
        scan_max_feature(left, max_feat);
    }
    if let Some(right) = node.get("right_child") {
        scan_max_feature(right, max_feat);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load a model from a JSON file, using the specified format.
///
/// - `ModelFormat::Auto`: tries XGBoost first, then LightGBM.
/// - `ModelFormat::Xgboost`: only tries XGBoost JSON.
/// - `ModelFormat::Lightgbm`: only tries LightGBM JSON.
pub fn load_model_with_format(path: &str, format: ModelFormat) -> Result<XgboostModel, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read model file: {}", e))?;
    parse_model_with_format(&data, format)
}

/// Parse a model from a JSON string using the specified format.
pub fn parse_model_with_format(
    json_str: &str,
    format: ModelFormat,
) -> Result<XgboostModel, String> {
    match format {
        ModelFormat::Xgboost => parse_xgboost(json_str),
        ModelFormat::Lightgbm => parse_lightgbm(json_str),
        ModelFormat::Auto => {
            // Try XGBoost first, then LightGBM
            match parse_xgboost(json_str) {
                Ok(model) => Ok(model),
                Err(xgb_err) => match parse_lightgbm(json_str) {
                    Ok(model) => Ok(model),
                    Err(lgb_err) => Err(format!(
                        "Failed to parse model as either format. XGBoost: {}. LightGBM: {}",
                        xgb_err, lgb_err
                    )),
                },
            }
        }
    }
}

/// Load an XGBoost model from a JSON file (saved with `xgb.save_model("model.json")`).
///
/// This is a convenience wrapper that uses `ModelFormat::Auto`.
#[allow(dead_code)]
pub fn load_model(path: &str) -> Result<XgboostModel, String> {
    load_model_with_format(path, ModelFormat::Auto)
}

/// Parse an XGBoost model from a JSON string.
///
/// This is a convenience wrapper that only tries XGBoost format.
/// Kept for backward compatibility; prefer `parse_model_with_format` for new code.
#[allow(dead_code)]
pub fn parse_model(json_str: &str) -> Result<XgboostModel, String> {
    parse_xgboost(json_str)
}

/// Parse an XGBoost model from a JSON string.
fn parse_xgboost(json_str: &str) -> Result<XgboostModel, String> {
    let raw: XgbJsonModel = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse XGBoost JSON: {}", e))?;

    let num_features: usize = raw
        .learner
        .learner_model_param
        .num_feature
        .parse()
        .map_err(|e| format!("Invalid num_feature: {}", e))?;

    let num_class_str = &raw.learner.learner_model_param.num_class;
    let num_classes: usize = if num_class_str.is_empty() || num_class_str == "0" {
        // XGBoost uses 0 for binary classification / regression
        2
    } else {
        num_class_str
            .parse()
            .map_err(|e| format!("Invalid num_class: {}", e))?
    };

    let base_score: f64 = raw
        .learner
        .learner_model_param
        .base_score
        .parse()
        .map_err(|e| format!("Invalid base_score: {}", e))?;

    let xgb_trees = &raw.learner.gradient_booster.model.trees;
    let mut trees = Vec::with_capacity(xgb_trees.len());

    for (idx, xgb_tree) in xgb_trees.iter().enumerate() {
        let n = xgb_tree.left_children.len();
        if n == 0 {
            return Err(format!("Tree {} has no nodes", idx));
        }

        trees.push(Tree {
            left_children: xgb_tree.left_children.clone(),
            right_children: xgb_tree.right_children.clone(),
            split_indices: xgb_tree.split_indices.iter().map(|&x| x as usize).collect(),
            split_conditions: xgb_tree.split_conditions.clone(),
            base_weights: xgb_tree.base_weights.clone(),
        });
    }

    if trees.is_empty() {
        return Err("Model has no trees".to_string());
    }

    Ok(XgboostModel {
        num_features,
        num_classes,
        base_score,
        trees,
    })
}

/// Traverse a single tree given input features, returning the leaf value.
///
/// Starting at node 0 (root):
/// - If `left_children[i] == -1`, it is a leaf node; return `base_weights[i]`.
/// - Otherwise, compare `features[split_indices[i]] < split_conditions[i]`.
///   If true, go to `left_children[i]`; otherwise go to `right_children[i]`.
fn traverse_tree(tree: &Tree, features: &[f64]) -> f64 {
    let mut node: usize = 0;
    loop {
        if tree.left_children[node] == -1 {
            // Leaf node
            return tree.base_weights[node];
        }
        let feat_idx = tree.split_indices[node];
        let threshold = tree.split_conditions[node];
        if features[feat_idx] < threshold {
            node = tree.left_children[node] as usize;
        } else {
            node = tree.right_children[node] as usize;
        }
    }
}

/// Run inference on a single input, returning raw prediction scores.
///
/// - For binary classification (`num_classes <= 2`): all tree outputs are summed
///   together with `base_score`, returning a single-element vector.
/// - For multi-class: trees are grouped round-robin by class index, each class
///   gets its own sum plus `base_score`, returning a vector of length `num_classes`.
pub fn predict(model: &XgboostModel, features: &[f64]) -> Vec<f64> {
    if model.num_classes <= 2 {
        // Binary classification / regression: sum all trees + base_score
        let sum: f64 = model.trees.iter().map(|t| traverse_tree(t, features)).sum();
        vec![sum + model.base_score]
    } else {
        // Multi-class: trees assigned round-robin to classes
        let nc = model.num_classes;
        let mut scores = vec![model.base_score; nc];
        for (i, tree) in model.trees.iter().enumerate() {
            let class_idx = i % nc;
            scores[class_idx] += traverse_tree(tree, features);
        }
        scores
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a simple tree by hand:
    ///
    ///       [0] feat=0, thresh=5.0
    ///       /                \
    ///    [1] leaf=-0.3    [2] leaf=0.7
    fn make_simple_tree() -> Tree {
        Tree {
            left_children: vec![1, -1, -1],
            right_children: vec![2, -1, -1],
            split_indices: vec![0, 0, 0],
            split_conditions: vec![5.0, 0.0, 0.0],
            base_weights: vec![0.0, -0.3, 0.7],
        }
    }

    #[test]
    fn test_traverse_simple_tree() {
        let tree = make_simple_tree();

        // feature[0] = 3.0 < 5.0 => go left => leaf = -0.3
        assert_eq!(traverse_tree(&tree, &[3.0]), -0.3);

        // feature[0] = 5.0 >= 5.0 => go right => leaf = 0.7
        assert_eq!(traverse_tree(&tree, &[5.0]), 0.7);

        // feature[0] = 7.0 >= 5.0 => go right => leaf = 0.7
        assert_eq!(traverse_tree(&tree, &[7.0]), 0.7);
    }

    #[test]
    fn test_predict_binary() {
        // Two trees, binary classification, base_score = 0.5
        let model = XgboostModel {
            num_features: 1,
            num_classes: 2,
            base_score: 0.5,
            trees: vec![
                // Tree 0: feat[0] < 5.0 => -0.3, else 0.7
                make_simple_tree(),
                // Tree 1: feat[0] < 3.0 => 0.1, else 0.2
                Tree {
                    left_children: vec![1, -1, -1],
                    right_children: vec![2, -1, -1],
                    split_indices: vec![0, 0, 0],
                    split_conditions: vec![3.0, 0.0, 0.0],
                    base_weights: vec![0.0, 0.1, 0.2],
                },
            ],
        };

        // features = [1.0]: tree0 => -0.3 (1.0 < 5.0), tree1 => 0.1 (1.0 < 3.0)
        // sum = -0.3 + 0.1 + 0.5 = 0.3
        let result = predict(&model, &[1.0]);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 0.3).abs() < 1e-10);

        // features = [4.0]: tree0 => -0.3 (4.0 < 5.0), tree1 => 0.2 (4.0 >= 3.0)
        // sum = -0.3 + 0.2 + 0.5 = 0.4
        let result = predict(&model, &[4.0]);
        assert!((result[0] - 0.4).abs() < 1e-10);

        // features = [6.0]: tree0 => 0.7 (6.0 >= 5.0), tree1 => 0.2 (6.0 >= 3.0)
        // sum = 0.7 + 0.2 + 0.5 = 1.4
        let result = predict(&model, &[6.0]);
        assert!((result[0] - 1.4).abs() < 1e-10);
    }

    #[test]
    fn test_load_model() {
        // Find the test model relative to the crate root
        let model_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../test-model/model.json");
        let model = load_model(model_path.to_str().unwrap()).expect("failed to load model");

        assert_eq!(model.num_features, 4);
        assert_eq!(model.num_classes, 2); // num_class=0 => binary => 2
        assert!((model.base_score - 0.5).abs() < 1e-10);
        assert_eq!(model.trees.len(), 3);

        // All three trees have 7 nodes (depth-3 binary trees)
        assert_eq!(model.trees[0].left_children.len(), 7);
        assert_eq!(model.trees[1].left_children.len(), 7);
        assert_eq!(model.trees[2].left_children.len(), 7);

        // Run inference with known features and verify manually
        // features = [5.0, 3.5, 1.5, 0.3]
        //
        // Tree 0: root split feat[2]<2.45 => 1.5<2.45 => left(1)
        //         node 1: feat[0]<4.95 => 5.0>=4.95 => right(4) => leaf 0.36
        //
        // Tree 1: root split feat[3]<1.65 => 0.3<1.65 => left(1)
        //         node 1: feat[1]<2.85 => 3.5>=2.85 => right(4) => leaf 0.28
        //
        // Tree 2: root split feat[2]<4.75 => 1.5<4.75 => left(1)
        //         node 1: feat[3]<1.55 => 0.3<1.55 => left(3) => leaf -0.16
        //
        // sum = 0.36 + 0.28 + (-0.16) + 0.5(base) = 0.98
        let result = predict(&model, &[5.0, 3.5, 1.5, 0.3]);
        assert_eq!(result.len(), 1);
        assert!(
            (result[0] - 0.98).abs() < 1e-10,
            "Expected 0.98, got {}",
            result[0]
        );
    }

    #[test]
    fn test_predict_multiclass() {
        // 3 classes, 6 trees (2 per class, round-robin)
        let make_leaf = |val: f64| Tree {
            left_children: vec![-1],
            right_children: vec![-1],
            split_indices: vec![0],
            split_conditions: vec![0.0],
            base_weights: vec![val],
        };

        let model = XgboostModel {
            num_features: 1,
            num_classes: 3,
            base_score: 0.0,
            trees: vec![
                make_leaf(0.1), // class 0
                make_leaf(0.2), // class 1
                make_leaf(0.3), // class 2
                make_leaf(0.4), // class 0
                make_leaf(0.5), // class 1
                make_leaf(0.6), // class 2
            ],
        };

        let result = predict(&model, &[0.0]);
        assert_eq!(result.len(), 3);
        assert!((result[0] - 0.5).abs() < 1e-10); // 0.1 + 0.4
        assert!((result[1] - 0.7).abs() < 1e-10); // 0.2 + 0.5
        assert!((result[2] - 0.9).abs() < 1e-10); // 0.3 + 0.6
    }

    // -----------------------------------------------------------------------
    // LightGBM parsing tests
    // -----------------------------------------------------------------------

    /// Minimal LightGBM JSON with a single tree:
    ///
    ///       [root] feat=2, thresh=0.5
    ///       /                       \
    ///   leaf=0.1              [node] feat=0, thresh=1.5
    ///                         /                \
    ///                     leaf=-0.2         leaf=0.3
    const SAMPLE_LIGHTGBM_JSON: &str = r#"{
        "name": "tree",
        "max_feature_idx": 3,
        "num_class": 1,
        "num_tree_per_iteration": 1,
        "tree_info": [
            {
                "tree_index": 0,
                "tree_structure": {
                    "split_feature": 2,
                    "threshold": 0.5,
                    "decision_type": "<=",
                    "left_child": { "leaf_value": 0.1 },
                    "right_child": {
                        "split_feature": 0,
                        "threshold": 1.5,
                        "decision_type": "<=",
                        "left_child": { "leaf_value": -0.2 },
                        "right_child": { "leaf_value": 0.3 }
                    }
                }
            }
        ]
    }"#;

    #[test]
    fn test_parse_lightgbm_structure() {
        let model = parse_lightgbm(SAMPLE_LIGHTGBM_JSON).unwrap();

        // max_feature_idx=3 => num_features=4
        assert_eq!(model.num_features, 4);
        // num_tree_per_iteration=1 => binary => num_classes=2
        assert_eq!(model.num_classes, 2);
        assert_eq!(model.trees.len(), 1);
        // LightGBM has no base_score concept
        assert!((model.base_score - 0.0).abs() < 1e-10);

        let tree = &model.trees[0];
        // Root is internal (split_feature=2, threshold=0.5)
        assert_ne!(tree.left_children[0], -1);
        assert_eq!(tree.split_indices[0], 2);
        assert!((tree.split_conditions[0] - 0.5).abs() < 1e-10);

        // The tree should have 5 nodes total (BFS: root, left-leaf, right-internal, left-leaf-of-right, right-leaf-of-right)
        assert_eq!(tree.left_children.len(), 5);
    }

    #[test]
    fn test_lightgbm_inference() {
        let model = parse_lightgbm(SAMPLE_LIGHTGBM_JSON).unwrap();

        // features = [0.0, 0.0, 0.3, 0.0] => feat[2]=0.3 < 0.5 => left => leaf 0.1
        let result = predict(&model, &[0.0, 0.0, 0.3, 0.0]);
        assert_eq!(result.len(), 1);
        assert!(
            (result[0] - 0.1).abs() < 1e-10,
            "Expected 0.1, got {}",
            result[0]
        );

        // features = [1.0, 0.0, 0.8, 0.0] => feat[2]=0.8 >= 0.5 => right
        //   => feat[0]=1.0 < 1.5 => left => leaf -0.2
        let result = predict(&model, &[1.0, 0.0, 0.8, 0.0]);
        assert!(
            (result[0] - (-0.2)).abs() < 1e-10,
            "Expected -0.2, got {}",
            result[0]
        );

        // features = [2.0, 0.0, 0.8, 0.0] => feat[2]=0.8 >= 0.5 => right
        //   => feat[0]=2.0 >= 1.5 => right => leaf 0.3
        let result = predict(&model, &[2.0, 0.0, 0.8, 0.0]);
        assert!(
            (result[0] - 0.3).abs() < 1e-10,
            "Expected 0.3, got {}",
            result[0]
        );
    }

    #[test]
    fn test_lightgbm_multi_tree() {
        // Two trees, binary classification
        let json = r#"{
            "name": "tree",
            "max_feature_idx": 1,
            "num_class": 1,
            "num_tree_per_iteration": 1,
            "tree_info": [
                {
                    "tree_index": 0,
                    "tree_structure": {
                        "split_feature": 0,
                        "threshold": 5.0,
                        "left_child": { "leaf_value": -0.3 },
                        "right_child": { "leaf_value": 0.7 }
                    }
                },
                {
                    "tree_index": 1,
                    "tree_structure": {
                        "split_feature": 0,
                        "threshold": 3.0,
                        "left_child": { "leaf_value": 0.1 },
                        "right_child": { "leaf_value": 0.2 }
                    }
                }
            ]
        }"#;

        let model = parse_lightgbm(json).unwrap();
        assert_eq!(model.num_features, 2); // max_feature_idx=1 => 2 features
        assert_eq!(model.num_classes, 2);
        assert_eq!(model.trees.len(), 2);

        // features = [1.0, 0.0]: tree0 => -0.3 (1.0<5.0), tree1 => 0.1 (1.0<3.0)
        // sum = -0.3 + 0.1 = -0.2 (base_score=0.0)
        let result = predict(&model, &[1.0, 0.0]);
        assert!(
            (result[0] - (-0.2)).abs() < 1e-10,
            "Expected -0.2, got {}",
            result[0]
        );

        // features = [6.0, 0.0]: tree0 => 0.7 (6.0>=5.0), tree1 => 0.2 (6.0>=3.0)
        // sum = 0.7 + 0.2 = 0.9
        let result = predict(&model, &[6.0, 0.0]);
        assert!(
            (result[0] - 0.9).abs() < 1e-10,
            "Expected 0.9, got {}",
            result[0]
        );
    }

    #[test]
    fn test_lightgbm_multiclass() {
        // 3 classes, 3 trees (one per class per iteration)
        let json = r#"{
            "name": "tree",
            "max_feature_idx": 0,
            "num_class": 3,
            "num_tree_per_iteration": 3,
            "tree_info": [
                {
                    "tree_index": 0,
                    "tree_structure": { "leaf_value": 0.1 }
                },
                {
                    "tree_index": 1,
                    "tree_structure": { "leaf_value": 0.5 }
                },
                {
                    "tree_index": 2,
                    "tree_structure": { "leaf_value": 0.9 }
                }
            ]
        }"#;

        let model = parse_lightgbm(json).unwrap();
        assert_eq!(model.num_classes, 3);
        assert_eq!(model.trees.len(), 3);

        let result = predict(&model, &[0.0]);
        assert_eq!(result.len(), 3);
        assert!((result[0] - 0.1).abs() < 1e-10); // class 0
        assert!((result[1] - 0.5).abs() < 1e-10); // class 1
        assert!((result[2] - 0.9).abs() < 1e-10); // class 2
    }

    #[test]
    fn test_lightgbm_single_leaf_tree() {
        // Edge case: a tree that is just a single leaf (stump)
        let json = r#"{
            "name": "tree",
            "max_feature_idx": 2,
            "num_tree_per_iteration": 1,
            "tree_info": [
                {
                    "tree_index": 0,
                    "tree_structure": { "leaf_value": 0.42 }
                }
            ]
        }"#;

        let model = parse_lightgbm(json).unwrap();
        assert_eq!(model.num_features, 3);
        assert_eq!(model.trees.len(), 1);

        // Single leaf => always returns 0.42
        let result = predict(&model, &[1.0, 2.0, 3.0]);
        assert!(
            (result[0] - 0.42).abs() < 1e-10,
            "Expected 0.42, got {}",
            result[0]
        );
    }

    #[test]
    fn test_lightgbm_no_max_feature_idx() {
        // When max_feature_idx is absent, num_features is inferred from split_features.
        let json = r#"{
            "name": "tree",
            "num_tree_per_iteration": 1,
            "tree_info": [
                {
                    "tree_index": 0,
                    "tree_structure": {
                        "split_feature": 3,
                        "threshold": 1.0,
                        "left_child": { "leaf_value": 0.1 },
                        "right_child": { "leaf_value": 0.2 }
                    }
                }
            ]
        }"#;

        let model = parse_lightgbm(json).unwrap();
        // max split_feature = 3, so num_features = 4
        assert_eq!(model.num_features, 4);
    }

    #[test]
    fn test_lightgbm_deep_tree() {
        // Depth-3 tree to ensure recursive flattening works correctly.
        //
        //             [root] feat=0, thresh=5.0
        //            /                         \
        //     [n1] feat=1, thresh=2.0      [n2] feat=1, thresh=8.0
        //     /            \               /              \
        //   leaf=0.1    leaf=0.2        leaf=0.3        leaf=0.4
        let json = r#"{
            "name": "tree",
            "max_feature_idx": 1,
            "num_tree_per_iteration": 1,
            "tree_info": [
                {
                    "tree_index": 0,
                    "tree_structure": {
                        "split_feature": 0,
                        "threshold": 5.0,
                        "left_child": {
                            "split_feature": 1,
                            "threshold": 2.0,
                            "left_child": { "leaf_value": 0.1 },
                            "right_child": { "leaf_value": 0.2 }
                        },
                        "right_child": {
                            "split_feature": 1,
                            "threshold": 8.0,
                            "left_child": { "leaf_value": 0.3 },
                            "right_child": { "leaf_value": 0.4 }
                        }
                    }
                }
            ]
        }"#;

        let model = parse_lightgbm(json).unwrap();
        assert_eq!(model.num_features, 2);
        assert_eq!(model.trees.len(), 1);
        // 3 internal + 4 leaves = 7 nodes
        assert_eq!(model.trees[0].left_children.len(), 7);

        // features = [3.0, 1.0] => feat[0]=3.0<5.0 left => feat[1]=1.0<2.0 left => 0.1
        let r = predict(&model, &[3.0, 1.0]);
        assert!((r[0] - 0.1).abs() < 1e-10, "Expected 0.1, got {}", r[0]);

        // features = [3.0, 5.0] => feat[0]=3.0<5.0 left => feat[1]=5.0>=2.0 right => 0.2
        let r = predict(&model, &[3.0, 5.0]);
        assert!((r[0] - 0.2).abs() < 1e-10, "Expected 0.2, got {}", r[0]);

        // features = [6.0, 7.0] => feat[0]=6.0>=5.0 right => feat[1]=7.0<8.0 left => 0.3
        let r = predict(&model, &[6.0, 7.0]);
        assert!((r[0] - 0.3).abs() < 1e-10, "Expected 0.3, got {}", r[0]);

        // features = [6.0, 9.0] => feat[0]=6.0>=5.0 right => feat[1]=9.0>=8.0 right => 0.4
        let r = predict(&model, &[6.0, 9.0]);
        assert!((r[0] - 0.4).abs() < 1e-10, "Expected 0.4, got {}", r[0]);
    }

    // -----------------------------------------------------------------------
    // Auto-detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_auto_detect_xgboost() {
        // XGBoost JSON should be auto-detected
        let xgb_json = r#"{
            "learner": {
                "learner_model_param": {
                    "num_feature": "2",
                    "num_class": "0",
                    "base_score": "5E-1"
                },
                "gradient_booster": {
                    "model": {
                        "trees": [
                            {
                                "left_children": [1, -1, -1],
                                "right_children": [2, -1, -1],
                                "split_indices": [0, 0, 0],
                                "split_conditions": [0.5, 0.0, 0.0],
                                "base_weights": [0.0, -0.1, 0.1]
                            }
                        ]
                    }
                }
            }
        }"#;

        let model = parse_model_with_format(xgb_json, ModelFormat::Auto).unwrap();
        assert_eq!(model.num_features, 2);
        assert!((model.base_score - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_auto_detect_lightgbm() {
        // LightGBM JSON should be auto-detected (XGBoost parse will fail first)
        let model = parse_model_with_format(SAMPLE_LIGHTGBM_JSON, ModelFormat::Auto).unwrap();
        assert_eq!(model.num_features, 4);
        assert_eq!(model.trees.len(), 1);
    }

    #[test]
    fn test_auto_detect_invalid() {
        let result = parse_model_with_format("{}", ModelFormat::Auto);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("XGBoost") && err.contains("LightGBM"),
            "Error should mention both formats: {}",
            err
        );
    }

    #[test]
    fn test_model_format_from_str() {
        assert_eq!(ModelFormat::from_str("auto"), Some(ModelFormat::Auto));
        assert_eq!(ModelFormat::from_str("AUTO"), Some(ModelFormat::Auto));
        assert_eq!(ModelFormat::from_str("xgboost"), Some(ModelFormat::Xgboost));
        assert_eq!(ModelFormat::from_str("XGBOOST"), Some(ModelFormat::Xgboost));
        assert_eq!(
            ModelFormat::from_str("lightgbm"),
            Some(ModelFormat::Lightgbm)
        );
        assert_eq!(ModelFormat::from_str("lgbm"), Some(ModelFormat::Lightgbm));
        assert_eq!(ModelFormat::from_str("unknown"), None);
    }

    #[test]
    fn test_lightgbm_load_from_file() {
        // Write a temp LightGBM model file and load it with auto-detection
        let dir = std::env::temp_dir().join("tee_lgbm_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lgbm_model.json");
        std::fs::write(&path, SAMPLE_LIGHTGBM_JSON).unwrap();

        let model = load_model_with_format(path.to_str().unwrap(), ModelFormat::Auto).unwrap();
        assert_eq!(model.num_features, 4);
        assert_eq!(model.trees.len(), 1);

        // Also test explicit LightGBM format
        let model2 = load_model_with_format(path.to_str().unwrap(), ModelFormat::Lightgbm).unwrap();
        assert_eq!(model2.num_features, 4);

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
