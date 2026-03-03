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

// ========================================================================
// Tree inference circuit utilities
// ========================================================================

/// Compute the actual depth of a single tree (max path length from root to any leaf).
pub fn tree_depth(tree: &DecisionTree) -> usize {
    fn depth_of(nodes: &[TreeNode], idx: usize) -> usize {
        let node = &nodes[idx];
        if node.is_leaf {
            return 0;
        }
        1 + depth_of(nodes, node.left_child).max(depth_of(nodes, node.right_child))
    }
    depth_of(&tree.nodes, 0)
}

/// Extract all leaf values from a tree as a flat array of length 2^depth.
/// Trees that are not perfectly balanced are padded: missing leaves get value 0.
/// Ordering: leaves are indexed by path bits (b_0, b_1, ..., b_{d-1}) in big-endian,
/// where b_i=0 means left, b_i=1 means right.
pub fn tree_leaf_values(tree: &DecisionTree, target_depth: usize) -> Vec<i64> {
    let num_leaves = 1usize << target_depth;
    let mut leaves = vec![0i64; num_leaves];

    fn fill_leaves(
        nodes: &[TreeNode],
        idx: usize,
        depth: usize,
        target_depth: usize,
        path_index: usize,
        leaves: &mut Vec<i64>,
    ) {
        let node = &nodes[idx];
        if node.is_leaf || depth == target_depth {
            // Fill all descendants at target_depth with this leaf's value
            let val = if node.is_leaf {
                quantize(node.leaf_value)
            } else {
                0
            };
            let span = 1usize << (target_depth - depth);
            for i in 0..span {
                leaves[path_index + i] = val;
            }
            return;
        }
        let half = 1usize << (target_depth - depth - 1);
        // Left child: path bit = 0 (lower half of indices)
        fill_leaves(nodes, node.left_child, depth + 1, target_depth, path_index, leaves);
        // Right child: path bit = 1 (upper half of indices)
        fill_leaves(
            nodes,
            node.right_child,
            depth + 1,
            target_depth,
            path_index + half,
            leaves,
        );
    }

    fill_leaves(&tree.nodes, 0, 0, target_depth, 0, &mut leaves);
    leaves
}

/// Collect all leaf values from all trees, padded to uniform max depth.
/// Returns (leaf_values_flat, max_depth).
/// leaf_values_flat has T * 2^max_depth entries.
pub fn collect_all_leaf_values(model: &XgboostModel) -> (Vec<i64>, usize) {
    let max_d = model.trees.iter().map(|t| tree_depth(t)).max().unwrap_or(0);
    let mut all_leaves = Vec::new();
    for tree in &model.trees {
        all_leaves.extend(tree_leaf_values(tree, max_d));
    }
    (all_leaves, max_d)
}

/// Compute path bits for all trees given features.
/// Returns path_bits[t][k] = false (left) or true (right) for tree t at depth k.
/// Paths are padded to max_depth (extra levels get false/left).
pub fn compute_path_bits(model: &XgboostModel, features: &[f64]) -> (Vec<Vec<bool>>, usize) {
    let max_d = model.trees.iter().map(|t| tree_depth(t)).max().unwrap_or(0);
    let mut all_bits = Vec::new();

    for tree in &model.trees {
        let mut bits = vec![false; max_d];
        let mut node_idx = 0;
        for depth in 0..max_d {
            let node = &tree.nodes[node_idx];
            if node.is_leaf {
                break; // remaining bits stay false (left)
            }
            let feature_val = features[node.feature_index as usize];
            let goes_right = feature_val >= node.threshold;
            bits[depth] = goes_right;
            if goes_right {
                node_idx = node.right_child;
            } else {
                node_idx = node.left_child;
            }
        }
        all_bits.push(bits);
    }

    (all_bits, max_d)
}

/// Compute the expected sum of selected leaf values across all trees (quantized).
pub fn compute_leaf_sum(model: &XgboostModel, features: &[f64]) -> i64 {
    let mut total = quantize(model.base_score);
    for tree in &model.trees {
        total += quantize(traverse_tree(tree, features));
    }
    total
}

/// Compute ceil(log2(n)), minimum 1
pub fn next_log2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    let mut v = n - 1;
    let mut log = 0;
    while v > 0 {
        v >>= 1;
        log += 1;
    }
    log
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
