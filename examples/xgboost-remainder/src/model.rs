//! XGBoost model representation and inference
//!
//! Supports a simplified XGBoost model format with binary decision trees.
//! Each tree is a collection of nodes that perform threshold comparisons
//! on input features.

use serde::{Deserialize, Serialize};

/// A complete XGBoost model (ensemble of decision trees)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XgboostModel {
    /// Number of input features
    pub num_features: usize,
    /// Number of output classes (1 for regression, N for classification)
    pub num_classes: usize,
    /// Maximum tree depth
    pub max_depth: usize,
    /// Decision trees
    pub trees: Vec<DecisionTree>,
    /// Base score (bias)
    pub base_score: f64,
}

/// A single decision tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionTree {
    /// Tree nodes (index 0 = root)
    pub nodes: Vec<TreeNode>,
}

/// A node in a decision tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Feature index to split on (-1 for leaf)
    pub feature_index: i32,
    /// Threshold value for comparison
    pub threshold: f64,
    /// Left child index (taken when feature < threshold)
    pub left_child: usize,
    /// Right child index (taken when feature >= threshold)
    pub right_child: usize,
    /// Leaf value (only valid for leaf nodes)
    pub leaf_value: f64,
    /// Whether this is a leaf node
    pub is_leaf: bool,
}

/// Run inference on a single input through the entire model
pub fn predict(model: &XgboostModel, features: &[f64]) -> u32 {
    assert_eq!(
        features.len(),
        model.num_features,
        "Feature count mismatch: expected {}, got {}",
        model.num_features,
        features.len()
    );

    if model.num_classes <= 2 {
        // Binary classification or regression
        let mut score = model.base_score;
        for tree in &model.trees {
            score += traverse_tree(tree, features);
        }
        // Sigmoid for binary classification
        if score > 0.0 {
            1
        } else {
            0
        }
    } else {
        // Multi-class: trees are grouped by class
        let trees_per_class = model.trees.len() / model.num_classes;
        let mut scores = vec![model.base_score; model.num_classes];

        for (i, tree) in model.trees.iter().enumerate() {
            let class_idx = i % model.num_classes;
            scores[class_idx] += traverse_tree(tree, features);
        }

        // Argmax
        scores
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(idx, _)| idx as u32)
            .unwrap_or(0)
    }
}

/// Traverse a single decision tree to get its leaf value
pub fn traverse_tree(tree: &DecisionTree, features: &[f64]) -> f64 {
    let mut node_idx = 0;
    loop {
        let node = &tree.nodes[node_idx];
        if node.is_leaf {
            return node.leaf_value;
        }

        let feature_val = features[node.feature_index as usize];
        if feature_val < node.threshold {
            node_idx = node.left_child;
        } else {
            node_idx = node.right_child;
        }
    }
}

/// Collect all decision path information for circuit building.
/// Returns (node_indices, comparisons) for each tree.
pub fn trace_inference(
    model: &XgboostModel,
    features: &[f64],
) -> Vec<Vec<(usize, bool)>> {
    let mut paths = Vec::new();

    for tree in &model.trees {
        let mut path = Vec::new();
        let mut node_idx = 0;

        loop {
            let node = &tree.nodes[node_idx];
            if node.is_leaf {
                break;
            }

            let feature_val = features[node.feature_index as usize];
            let goes_left = feature_val < node.threshold;
            path.push((node_idx, goes_left));

            if goes_left {
                node_idx = node.left_child;
            } else {
                node_idx = node.right_child;
            }
        }

        paths.push(path);
    }

    paths
}

/// Quantize a floating-point value to fixed-point for circuit arithmetic.
/// Uses 2^16 scaling factor.
pub const FIXED_POINT_SCALE: u64 = 1 << 16;

pub fn quantize(val: f64) -> i64 {
    (val * FIXED_POINT_SCALE as f64).round() as i64
}

pub fn dequantize(val: i64) -> f64 {
    val as f64 / FIXED_POINT_SCALE as f64
}

/// Create a sample XGBoost model for testing
pub fn sample_model() -> XgboostModel {
    // Simple 2-tree binary classifier on 5 features
    XgboostModel {
        num_features: 5,
        num_classes: 2,
        max_depth: 3,
        base_score: 0.0,
        trees: vec![
            // Tree 0
            DecisionTree {
                nodes: vec![
                    TreeNode {
                        feature_index: 0,
                        threshold: 0.5,
                        left_child: 1,
                        right_child: 2,
                        leaf_value: 0.0,
                        is_leaf: false,
                    },
                    TreeNode {
                        feature_index: 1,
                        threshold: 0.3,
                        left_child: 3,
                        right_child: 4,
                        leaf_value: 0.0,
                        is_leaf: false,
                    },
                    TreeNode {
                        feature_index: 2,
                        threshold: 0.7,
                        left_child: 5,
                        right_child: 6,
                        leaf_value: 0.0,
                        is_leaf: false,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: -0.8,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 0.2,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 0.3,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 0.9,
                        is_leaf: true,
                    },
                ],
            },
            // Tree 1
            DecisionTree {
                nodes: vec![
                    TreeNode {
                        feature_index: 3,
                        threshold: 0.4,
                        left_child: 1,
                        right_child: 2,
                        leaf_value: 0.0,
                        is_leaf: false,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: -0.5,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: 4,
                        threshold: 0.6,
                        left_child: 3,
                        right_child: 4,
                        leaf_value: 0.0,
                        is_leaf: false,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 0.4,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 0.7,
                        is_leaf: true,
                    },
                ],
            },
        ],
    }
}
