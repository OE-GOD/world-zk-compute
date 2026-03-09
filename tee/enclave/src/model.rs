//! XGBoost model loading and inference.
//!
//! Parses the native XGBoost JSON format produced by `xgb.save_model("model.json")`
//! and runs tree-based inference.

use serde::Deserialize;

/// A complete XGBoost model (ensemble of decision trees).
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
pub struct Tree {
    pub left_children: Vec<i32>,
    pub right_children: Vec<i32>,
    pub split_indices: Vec<usize>,
    pub split_conditions: Vec<f64>,
    pub base_weights: Vec<f64>,
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
// Public API
// ---------------------------------------------------------------------------

/// Load an XGBoost model from a JSON file (saved with `xgb.save_model("model.json")`).
pub fn load_model(path: &str) -> Result<XgboostModel, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read model file: {}", e))?;
    parse_model(&data)
}

/// Parse an XGBoost model from a JSON string.
pub fn parse_model(json_str: &str) -> Result<XgboostModel, String> {
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
}
