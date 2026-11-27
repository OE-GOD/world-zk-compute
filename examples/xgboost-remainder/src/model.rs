//! XGBoost model representation and inference
//!
//! Supports a simplified XGBoost model format with binary decision trees.
//! Each tree is a collection of nodes that perform threshold comparisons
//! on input features.

use serde::{Deserialize, Serialize};
use std::path::Path;

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

// ========================================================================
// XGBoost JSON model import (from xgb.save_model("model.json"))
// ========================================================================

/// Raw XGBoost JSON model format (top-level)
#[derive(Debug, Deserialize)]
struct XgbJsonModel {
    learner: XgbLearner,
}

#[derive(Debug, Deserialize)]
struct XgbLearner {
    learner_model_param: XgbModelParam,
    gradient_booster: XgbGradientBooster,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
struct XgbGradientBooster {
    model: XgbGbtreeModel,
}

#[derive(Debug, Deserialize)]
struct XgbGbtreeModel {
    trees: Vec<XgbTree>,
    #[serde(default)]
    #[allow(dead_code)]
    tree_info: Vec<i32>,
}

#[derive(Debug, Deserialize)]
struct XgbTree {
    left_children: Vec<i32>,
    right_children: Vec<i32>,
    split_indices: Vec<i32>,
    split_conditions: Vec<f64>,
    base_weights: Vec<f64>,
}

/// Load an XGBoost model from a JSON file: either the file written by
/// `xgb.save_model("model.json")` or this crate's own serialized
/// [`XgboostModel`] layout (the one `sample_model.json` uses).
pub fn load_xgboost_json(path: &Path) -> Result<XgboostModel, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read model file: {}", e))?;
    if let Ok(model) = serde_json::from_str::<XgboostModel>(&data) {
        return Ok(model);
    }
    parse_xgboost_json(&data)
}

/// Parse an XGBoost model from a JSON string.
pub fn parse_xgboost_json(json_str: &str) -> Result<XgboostModel, String> {
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
    let mut max_depth = 0usize;

    for (tree_idx, xgb_tree) in xgb_trees.iter().enumerate() {
        let num_nodes = xgb_tree.left_children.len();
        if num_nodes == 0 {
            return Err(format!("Tree {} has no nodes", tree_idx));
        }

        // Every per-node array is indexed by node, so a short one would panic below.
        if xgb_tree.right_children.len() != num_nodes
            || xgb_tree.split_indices.len() != num_nodes
            || xgb_tree.split_conditions.len() != num_nodes
            || xgb_tree.base_weights.len() != num_nodes
        {
            return Err(format!(
                "Tree {}: array length mismatch (left_children={}, right_children={}, \
                 split_indices={}, split_conditions={}, base_weights={})",
                tree_idx,
                num_nodes,
                xgb_tree.right_children.len(),
                xgb_tree.split_indices.len(),
                xgb_tree.split_conditions.len(),
                xgb_tree.base_weights.len()
            ));
        }

        let mut nodes = Vec::with_capacity(num_nodes);
        for i in 0..num_nodes {
            let is_leaf = xgb_tree.left_children[i] == -1;
            nodes.push(TreeNode {
                feature_index: if is_leaf {
                    -1
                } else {
                    xgb_tree.split_indices[i]
                },
                threshold: if is_leaf {
                    0.0
                } else {
                    xgb_tree.split_conditions[i]
                },
                left_child: if is_leaf {
                    0
                } else {
                    xgb_tree.left_children[i] as usize
                },
                right_child: if is_leaf {
                    0
                } else {
                    xgb_tree.right_children[i] as usize
                },
                leaf_value: if is_leaf {
                    xgb_tree.base_weights[i]
                } else {
                    0.0
                },
                is_leaf,
            });
        }

        let dt = DecisionTree { nodes };
        let d = check_tree_structure(&dt).map_err(|e| format!("Tree {}: {}", tree_idx, e))?;
        if d > max_depth {
            max_depth = d;
        }
        trees.push(dt);
    }

    Ok(XgboostModel {
        num_features,
        num_classes,
        max_depth,
        trees,
        base_score,
    })
}

// ========================================================================
// Model validation
// ========================================================================

/// Deepest tree [`check_tree_structure`] accepts.
///
/// This limit and the two below keep loading and checking a model, and building its
/// leaf table, from panicking on a shift, aborting or exhausting memory. They are not
/// a statement of what is practical to prove: the prover's other tables are many times
/// the leaf table (see [`compute_comparison_tables`]), so a model at the limits can
/// still need more memory than the machine has.
pub const MAX_TREE_DEPTH: usize = 24;

/// Most entries [`validate_model`] accepts in the leaf table: the number of trees
/// rounded up to a power of two, times 2^depth of the deepest tree. See [`MAX_TREE_DEPTH`].
pub const MAX_LEAF_SLOTS: usize = 1 << 24;

/// Most features [`validate_model`] accepts. See [`MAX_TREE_DEPTH`].
pub const MAX_FEATURES: usize = 1 << 20;

/// Check that `model` is well formed and within [`MAX_TREE_DEPTH`], [`MAX_LEAF_SLOTS`]
/// and [`MAX_FEATURES`], and that a model with more than 2 classes has at least one tree
/// per class; the error names the problem.
///
/// Well formed means every tree passes [`check_tree_structure`], every split's feature
/// is one of the model's features, some tree has a split, and the `max_depth` field is
/// the depth of the deepest tree. Every node is checked, including nodes no path from
/// the root reaches. An accepted model can still be too large to prove on a given
/// machine (see [`MAX_TREE_DEPTH`]).
pub fn validate_model(model: &XgboostModel) -> Result<(), String> {
    if model.trees.is_empty() {
        return Err("Model has no trees".to_string());
    }
    if model.num_features > MAX_FEATURES {
        return Err(format!(
            "Model has {} features, over the limit of {}",
            model.num_features, MAX_FEATURES
        ));
    }
    // predict adds tree i to class i % num_classes and allocates one score per class, so
    // with fewer trees than classes some class has no tree, and nothing else bounds the
    // allocation.
    if model.num_classes > 2 && model.num_classes > model.trees.len() {
        return Err(format!(
            "Model has {} classes but {} trees: a model with more than 2 classes needs \
             at least one tree per class",
            model.num_classes,
            model.trees.len()
        ));
    }

    let mut max_depth = 0usize;
    for (tree_idx, tree) in model.trees.iter().enumerate() {
        let depth = check_tree_structure(tree).map_err(|e| format!("Tree {}: {}", tree_idx, e))?;
        max_depth = max_depth.max(depth);

        for (node_idx, node) in tree.nodes.iter().enumerate() {
            let in_range = usize::try_from(node.feature_index)
                .is_ok_and(|feature| feature < model.num_features);
            if !node.is_leaf && !in_range {
                return Err(format!(
                    "Tree {}: node {} splits on feature {}, but the model has {} features",
                    tree_idx, node_idx, node.feature_index, model.num_features
                ));
            }
        }
    }

    if max_depth == 0 {
        return Err("Model has depth 0: every tree is a single leaf".to_string());
    }
    // The field comes from the file in this crate's own layout, and the model hash
    // includes it, so a wrong value would name a different model than the trees prove.
    if model.max_depth != max_depth {
        return Err(format!(
            "Model's max_depth field is {}, but its deepest tree is {} levels deep",
            model.max_depth, max_depth
        ));
    }

    // In u128 so that no tree count overflows: at most 2^64 trees times 2^MAX_TREE_DEPTH.
    let trees_padded = (model.trees.len() as u128).next_power_of_two();
    let leaf_slots = trees_padded << max_depth;
    if leaf_slots > MAX_LEAF_SLOTS as u128 {
        return Err(format!(
            "Model's leaf table would have {} entries ({} trees, padded to {}, times 2^{}), \
             over the limit of {}",
            leaf_slots,
            model.trees.len(),
            trees_padded,
            max_depth,
            MAX_LEAF_SLOTS
        ));
    }

    Ok(())
}

/// Check that a tree has a root, that its child indices are in range, that no node has
/// two parents and that no node is its own descendant, so that every path ends in a leaf
/// and a walk from the root visits each node once; return the tree's depth.
///
/// Every node is checked, including nodes no path from the root reaches. A tree deeper
/// than [`MAX_TREE_DEPTH`] is an error. A leaf's child fields are not children.
pub fn check_tree_structure(tree: &DecisionTree) -> Result<usize, String> {
    let depth = walk_tree(tree)?;
    if depth > MAX_TREE_DEPTH {
        return Err(format!(
            "tree is {} levels deep, over the limit of {}",
            depth, MAX_TREE_DEPTH
        ));
    }
    Ok(depth)
}

/// The checks of [`check_tree_structure`] except the depth limit; returns the depth.
///
/// It does not recurse and visits each node once, so it takes time linear in the
/// number of nodes on any input.
fn walk_tree(tree: &DecisionTree) -> Result<usize, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Visit {
        New,
        OnPath,
        Done,
    }

    let nodes = &tree.nodes;
    if nodes.is_empty() {
        return Err("tree has no nodes".to_string());
    }

    let mut parents: Vec<Option<usize>> = vec![None; nodes.len()];
    for (node_idx, node) in nodes.iter().enumerate() {
        if node.is_leaf {
            continue;
        }
        for child in [node.left_child, node.right_child] {
            if child >= nodes.len() {
                // The loaders cast signed indices, so -1 arrives as usize::MAX.
                return Err(format!(
                    "node {} has child index {}, but the tree has {} nodes",
                    node_idx,
                    child as isize,
                    nodes.len()
                ));
            }
            // A shared child is walked once per path to it, and n nodes that share
            // their children have 2^n paths.
            if let Some(parent) = parents[child] {
                return Err(format!(
                    "node {} has child {}, which is already a child of node {}: \
                     a tree node has one parent",
                    node_idx, child, parent
                ));
            }
            parents[child] = Some(node_idx);
        }
    }

    // Depth-first search with an explicit stack of (node, children already visited).
    let mut depth = 0usize;
    let mut visits = vec![Visit::New; nodes.len()];
    for start in 0..nodes.len() {
        if visits[start] != Visit::New {
            continue;
        }
        visits[start] = Visit::OnPath;
        let mut stack = vec![(start, 0usize)];
        while let Some((node_idx, visited_children)) = stack.pop() {
            let node = &nodes[node_idx];
            if node.is_leaf || visited_children == 2 {
                visits[node_idx] = Visit::Done;
                continue;
            }
            stack.push((node_idx, visited_children + 1));
            let child = [node.left_child, node.right_child][visited_children];
            match visits[child] {
                Visit::New => {
                    visits[child] = Visit::OnPath;
                    stack.push((child, 0));
                    // The stack holds the path from `start`, so the child's depth is
                    // one less than its length.
                    if start == 0 {
                        depth = depth.max(stack.len() - 1);
                    }
                }
                Visit::OnPath => {
                    return Err(format!(
                        "node {} has child {}, which leads back to it: the path never reaches a leaf",
                        node_idx, child
                    ));
                }
                Visit::Done => {}
            }
        }
    }

    Ok(depth)
}

/// Check that a feature vector has one value per model feature.
pub fn check_feature_count(model: &XgboostModel, features: &[f64]) -> Result<(), String> {
    if features.len() != model.num_features {
        return Err(format!(
            "Expected {} features, got {}",
            model.num_features,
            features.len()
        ));
    }
    Ok(())
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
        let _trees_per_class = model.trees.len() / model.num_classes;
        let mut scores = vec![model.base_score; model.num_classes];

        for (i, tree) in model.trees.iter().enumerate() {
            let class_idx = i % model.num_classes;
            scores[class_idx] += traverse_tree(tree, features);
        }

        // Argmax
        scores
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
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
#[allow(dead_code)]
pub fn trace_inference(model: &XgboostModel, features: &[f64]) -> Vec<Vec<(usize, bool)>> {
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

#[allow(dead_code)]
pub fn dequantize(val: i64) -> f64 {
    val as f64 / FIXED_POINT_SCALE as f64
}

// ========================================================================
// Tree inference circuit utilities
// ========================================================================

/// Compute the actual depth of a single tree (max path length from root to any leaf).
///
/// The depth is that of a tree [`check_tree_structure`] accepts, or refuses only for
/// its depth; for any other tree this returns 0. It walks each node once without
/// recursing, so a malformed tree cannot make it loop, panic or overflow the stack.
pub fn tree_depth(tree: &DecisionTree) -> usize {
    walk_tree(tree).unwrap_or(0)
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
        fill_leaves(
            nodes,
            node.left_child,
            depth + 1,
            target_depth,
            path_index,
            leaves,
        );
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
    let max_d = model.trees.iter().map(tree_depth).max().unwrap_or(0);
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
    let max_d = model.trees.iter().map(tree_depth).max().unwrap_or(0);
    let mut all_bits = Vec::new();

    for tree in &model.trees {
        let mut bits = vec![false; max_d];
        let mut node_idx = 0;
        for bit in bits.iter_mut() {
            let node = &tree.nodes[node_idx];
            if node.is_leaf {
                break; // remaining bits stay false (left)
            }
            let feature_val = features[node.feature_index as usize];
            let goes_right = feature_val >= node.threshold;
            *bit = goes_right;
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

// ========================================================================
// Phase 1b: Comparison constraint utilities
// ========================================================================

/// Default number of bits for decomposition. In fixed point (2^16 scale) it
/// requires -2.0 <= quantize(feature) - quantize(threshold) < 2.0.
pub const DEFAULT_DECOMP_K: usize = 18;

/// Compute comparison tables for Phase 1b circuit verification.
///
/// For each (depth k, tree t, leaf position j), walks the tree from root
/// following the k-bit prefix of j (big-endian) and records the node's properties.
///
/// Returns three flat arrays of size max_depth * num_trees * 2^max_depth:
/// - thresholds: quantized threshold (0 for leaf nodes)
/// - feature_indices: index into features array (0 for leaves)
/// - is_real: 1 for real comparisons, 0 for leaves
///
/// Layout index: k * num_trees * 2^d + t * 2^d + j
pub fn compute_comparison_tables(
    model: &XgboostModel,
    max_depth: usize,
) -> (Vec<i64>, Vec<usize>, Vec<i64>) {
    let num_trees = model.trees.len();
    let num_positions = 1usize << max_depth;
    let total = max_depth * num_trees * num_positions;

    let mut thresholds = vec![0i64; total];
    let mut feature_indices = vec![0usize; total];
    let mut is_real = vec![0i64; total];

    for k in 0..max_depth {
        for (t, tree) in model.trees.iter().enumerate() {
            for j in 0..num_positions {
                let idx = k * num_trees * num_positions + t * num_positions + j;

                // Walk tree from root following k-bit prefix (big-endian)
                let mut node_idx = 0;
                let mut reached_leaf = false;
                for b in 0..k {
                    let node = &tree.nodes[node_idx];
                    if node.is_leaf {
                        reached_leaf = true;
                        break;
                    }
                    let bit = (j >> (max_depth - 1 - b)) & 1 == 1;
                    node_idx = if bit {
                        node.right_child
                    } else {
                        node.left_child
                    };
                }

                if !reached_leaf {
                    let node = &tree.nodes[node_idx];
                    if !node.is_leaf {
                        thresholds[idx] = quantize(node.threshold);
                        feature_indices[idx] = node.feature_index as usize;
                        is_real[idx] = 1;
                    }
                }
            }
        }
    }

    (thresholds, feature_indices, is_real)
}

/// Compute the bit decomposition witness for comparison verification.
///
/// For each (tree t, depth k) along the actual inference path:
///   diff = quantize(feature) - quantize(threshold)
///   shifted = diff + 2^(K-1)
///   decompose shifted into K bits (little-endian)
///
/// For leaf positions (no real comparison), all bits are set to false
/// (masked by is_real in the circuit, so correctness is not required).
///
/// Returns decomp_bits of size num_trees * max_depth * decomp_k.
/// Layout: bits[(t * max_depth + k) * decomp_k + bit_i]
pub fn compute_comparison_witness(
    model: &XgboostModel,
    features: &[f64],
    max_depth: usize,
    decomp_k: usize,
) -> Vec<bool> {
    let num_trees = model.trees.len();
    let offset = 1i64 << (decomp_k - 1);
    let mut decomp_bits = vec![false; num_trees * max_depth * decomp_k];

    let (path_bits_2d, _) = compute_path_bits(model, features);

    for (t, tree) in model.trees.iter().enumerate() {
        let mut node_idx = 0;
        for (k, &path_bit) in path_bits_2d[t].iter().enumerate().take(max_depth) {
            let node = &tree.nodes[node_idx];
            let pos = t * max_depth + k;

            if node.is_leaf {
                break; // remaining positions stay false (masked by is_real)
            }

            let feat_val = quantize(features[node.feature_index as usize]);
            let thresh_val = quantize(node.threshold);
            let diff = feat_val - thresh_val;
            let shifted = diff + offset;

            debug_assert!(
                shifted >= 0,
                "shifted underflow: diff={}, offset={}",
                diff,
                offset
            );
            debug_assert!(
                shifted < (1 << decomp_k),
                "shifted overflow: shifted={}, K={}",
                shifted,
                decomp_k
            );

            for bit_i in 0..decomp_k {
                decomp_bits[pos * decomp_k + bit_i] = (shifted >> bit_i) & 1 == 1;
            }

            // Advance along actual path
            if path_bit {
                node_idx = node.right_child;
            } else {
                node_idx = node.left_child;
            }
        }
    }

    decomp_bits
}

/// Generate a perfect binary tree of given depth with deterministic thresholds and leaf values.
/// Uses `num_features` features with thresholds spread across [0.1, 0.9] and
/// leaf values in [-1.0, 1.0].
#[allow(dead_code)]
pub fn generate_perfect_tree(depth: usize, num_features: usize, seed: u64) -> DecisionTree {
    let num_internal = (1usize << depth) - 1;
    let num_leaves = 1usize << depth;
    let total_nodes = num_internal + num_leaves;
    let mut nodes = Vec::with_capacity(total_nodes);

    // Internal nodes (indices 0..num_internal)
    for i in 0..num_internal {
        let hash = seed.wrapping_mul(31).wrapping_add(i as u64);
        let feature_index = (hash % num_features as u64) as i32;
        let threshold = 0.1 + 0.8 * ((hash % 100) as f64 / 100.0);
        nodes.push(TreeNode {
            feature_index,
            threshold,
            left_child: 2 * i + 1,
            right_child: 2 * i + 2,
            leaf_value: 0.0,
            is_leaf: false,
        });
    }

    // Leaf nodes (indices num_internal..total_nodes)
    for i in 0..num_leaves {
        let hash = seed
            .wrapping_mul(17)
            .wrapping_add(i as u64)
            .wrapping_add(1000);
        let leaf_value = -1.0 + 2.0 * ((hash % 200) as f64 / 200.0);
        nodes.push(TreeNode {
            feature_index: -1,
            threshold: 0.0,
            left_child: 0,
            right_child: 0,
            leaf_value,
            is_leaf: true,
        });
    }

    DecisionTree { nodes }
}

/// Check the sizes of a model that [`generate_model`] would build, against the
/// limits of [`validate_model`]. `generate_model` allocates and fills every tree
/// first, so it panics or runs out of memory on sizes that `validate_model`
/// would refuse; callers that take the sizes from outside check them here first.
pub fn check_generated_model_size(
    num_trees: usize,
    depth: usize,
    num_features: usize,
) -> Result<(), String> {
    if num_trees == 0 {
        return Err("A model needs at least one tree".to_string());
    }
    if depth == 0 || depth > MAX_TREE_DEPTH {
        return Err(format!(
            "Tree depth {} is outside 1 to {}",
            depth, MAX_TREE_DEPTH
        ));
    }
    if num_features == 0 || num_features > MAX_FEATURES {
        return Err(format!(
            "Feature count {} is outside 1 to {}",
            num_features, MAX_FEATURES
        ));
    }
    let padded = (num_trees as u128).next_power_of_two();
    let slots = padded << depth;
    if slots > MAX_LEAF_SLOTS as u128 {
        return Err(format!(
            "Model's leaf table would have {} entries ({} trees, padded to {}, times 2^{}), over the limit of {}",
            slots, num_trees, padded, depth, MAX_LEAF_SLOTS
        ));
    }
    Ok(())
}

/// Generate a complete XGBoost model with `num_trees` perfect trees of given depth.
#[allow(dead_code)]
pub fn generate_model(num_trees: usize, depth: usize, num_features: usize) -> XgboostModel {
    let trees: Vec<DecisionTree> = (0..num_trees)
        .map(|t| generate_perfect_tree(depth, num_features, (t + 1) as u64 * 137))
        .collect();

    XgboostModel {
        num_features,
        num_classes: 2,
        max_depth: depth,
        base_score: 0.0,
        trees,
    }
}

/// Generate deterministic feature values for testing (spread across [0.0, 1.0]).
#[allow(dead_code)]
pub fn generate_features(num_features: usize, seed: u64) -> Vec<f64> {
    (0..num_features)
        .map(|i| {
            let hash = seed.wrapping_mul(53).wrapping_add(i as u64);
            (hash % 100) as f64 / 100.0
        })
        .collect()
}

/// Create a sample XGBoost model for testing
#[allow(dead_code)]
pub fn sample_model() -> XgboostModel {
    // Simple 2-tree binary classifier on 5 features
    XgboostModel {
        num_features: 5,
        num_classes: 2,
        max_depth: 2,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comparison_tables_sample_model() {
        let model = sample_model();
        let (_, max_depth) = collect_all_leaf_values(&model);
        let (thresholds, feature_indices, is_real) = compute_comparison_tables(&model, max_depth);

        let num_trees = model.trees.len(); // 2
        let num_positions = 1usize << max_depth; // 4 for depth=2
        let total = max_depth * num_trees * num_positions;
        assert_eq!(thresholds.len(), total);
        assert_eq!(feature_indices.len(), total);
        assert_eq!(is_real.len(), total);

        // Depth 0: both trees have real comparisons at root
        // Tree 0 root: feature_index=0, threshold=0.5
        let idx_d0_t0 = 0 * num_trees * num_positions + 0 * num_positions;
        for j in 0..num_positions {
            assert_eq!(is_real[idx_d0_t0 + j], 1, "depth 0, tree 0, pos {}", j);
            assert_eq!(thresholds[idx_d0_t0 + j], quantize(0.5));
            assert_eq!(feature_indices[idx_d0_t0 + j], 0);
        }

        // Tree 1 root: feature_index=3, threshold=0.4
        let idx_d0_t1 = 0 * num_trees * num_positions + 1 * num_positions;
        for j in 0..num_positions {
            assert_eq!(is_real[idx_d0_t1 + j], 1);
            assert_eq!(thresholds[idx_d0_t1 + j], quantize(0.4));
            assert_eq!(feature_indices[idx_d0_t1 + j], 3);
        }

        // Depth 1, tree 0: all positions have real comparisons
        // Prefix 0 (j=0,1) → left child node 1: feature_index=1, threshold=0.3
        // Prefix 1 (j=2,3) → right child node 2: feature_index=2, threshold=0.7
        let idx_d1_t0 = 1 * num_trees * num_positions + 0 * num_positions;
        assert_eq!(thresholds[idx_d1_t0 + 0], quantize(0.3));
        assert_eq!(thresholds[idx_d1_t0 + 1], quantize(0.3));
        assert_eq!(feature_indices[idx_d1_t0 + 0], 1);
        assert_eq!(thresholds[idx_d1_t0 + 2], quantize(0.7));
        assert_eq!(thresholds[idx_d1_t0 + 3], quantize(0.7));
        assert_eq!(feature_indices[idx_d1_t0 + 2], 2);
        assert_eq!(is_real[idx_d1_t0 + 0], 1);
        assert_eq!(is_real[idx_d1_t0 + 2], 1);

        // Depth 1, tree 1:
        // Prefix 0 (j=0,1) → node 1 (leaf) → is_real=0
        // Prefix 1 (j=2,3) → node 2: feature_index=4, threshold=0.6
        let idx_d1_t1 = 1 * num_trees * num_positions + 1 * num_positions;
        assert_eq!(is_real[idx_d1_t1 + 0], 0, "tree 1 left child is leaf");
        assert_eq!(is_real[idx_d1_t1 + 1], 0);
        assert_eq!(is_real[idx_d1_t1 + 2], 1, "tree 1 right child is internal");
        assert_eq!(is_real[idx_d1_t1 + 3], 1);
        assert_eq!(thresholds[idx_d1_t1 + 2], quantize(0.6));
        assert_eq!(feature_indices[idx_d1_t1 + 2], 4);
    }

    #[test]
    fn test_comparison_witness_sample_model() {
        let model = sample_model();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];
        let (_, max_depth) = collect_all_leaf_values(&model);
        let decomp_k = DEFAULT_DECOMP_K;
        let offset = 1i64 << (decomp_k - 1);

        let decomp_bits = compute_comparison_witness(&model, &features, max_depth, decomp_k);
        assert_eq!(decomp_bits.len(), model.trees.len() * max_depth * decomp_k);

        // Tree 0, depth 0: feature[0]=0.6, threshold=0.5, diff=quantize(0.6)-quantize(0.5)
        let feat0 = quantize(0.6);
        let thresh0 = quantize(0.5);
        let diff0 = feat0 - thresh0;
        let shifted0 = diff0 + offset;
        let pos0 = (0 * max_depth + 0) * decomp_k;
        let mut reconstructed = 0i64;
        for bit_i in 0..decomp_k {
            if decomp_bits[pos0 + bit_i] {
                reconstructed += 1 << bit_i;
            }
        }
        assert_eq!(
            reconstructed, shifted0,
            "reconstruction check for tree 0, depth 0"
        );

        // Verify sign bit matches path direction
        // feature[0]=0.6 >= threshold=0.5, so goes right, path_bit=true, top_bit should be 1
        let top_bit = decomp_bits[pos0 + decomp_k - 1];
        assert!(top_bit, "sign bit should be 1 for right path");
    }

    #[test]
    fn test_comparison_witness_left_path() {
        let model = sample_model();
        let features = vec![0.1, 0.1, 0.1, 0.1, 0.1]; // all left
        let (_, max_depth) = collect_all_leaf_values(&model);
        let decomp_k = DEFAULT_DECOMP_K;
        let offset = 1i64 << (decomp_k - 1);

        let decomp_bits = compute_comparison_witness(&model, &features, max_depth, decomp_k);

        // Tree 0, depth 0: feature[0]=0.1 < threshold=0.5, goes left
        let pos = 0;
        let feat = quantize(0.1);
        let thresh = quantize(0.5);
        let shifted = (feat - thresh) + offset;
        let mut reconstructed = 0i64;
        for bit_i in 0..decomp_k {
            if decomp_bits[pos * decomp_k + bit_i] {
                reconstructed += 1 << bit_i;
            }
        }
        assert_eq!(reconstructed, shifted);
        // Top bit should be 0 (went left, diff < 0)
        assert!(
            !decomp_bits[pos * decomp_k + decomp_k - 1],
            "sign bit should be 0 for left"
        );
    }

    #[test]
    fn test_comparison_tables_early_leaf() {
        // Tree with depth 1 but max_depth forced to 2
        let model = XgboostModel {
            num_features: 2,
            num_classes: 2,
            max_depth: 2,
            base_score: 0.0,
            trees: vec![DecisionTree {
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
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: -1.0,
                        is_leaf: true,
                    },
                    TreeNode {
                        feature_index: -1,
                        threshold: 0.0,
                        left_child: 0,
                        right_child: 0,
                        leaf_value: 1.0,
                        is_leaf: true,
                    },
                ],
            }],
        };

        // Actual tree depth = 1, but we use max_depth = 2
        let (_, _, is_real_table) = compute_comparison_tables(&model, 2);

        // Depth 0: root is internal → all is_real
        let num_pos = 4;
        for j in 0..num_pos {
            assert_eq!(is_real_table[j], 1, "depth 0 should be real");
        }

        // Depth 1: all children are leaves → no real comparisons
        for j in 0..num_pos {
            assert_eq!(
                is_real_table[num_pos + j],
                0,
                "depth 1 should not be real (all leaves)"
            );
        }
    }

    // ========================================================================
    // XGBoost JSON import tests
    // ========================================================================

    /// Minimal XGBoost JSON matching real `xgb.save_model("model.json")` output.
    /// This is a binary classifier with 2 trees, 4 features, max depth 2.
    const SAMPLE_XGBOOST_JSON: &str = r#"{
        "version": [2, 0, 0],
        "learner": {
            "learner_model_param": {
                "num_feature": "4",
                "num_class": "0",
                "base_score": "5E-1"
            },
            "gradient_booster": {
                "name": "gbtree",
                "model": {
                    "gbtree_model_param": {
                        "num_trees": "2"
                    },
                    "trees": [
                        {
                            "tree_param": { "num_nodes": "7" },
                            "id": 0,
                            "left_children": [1, 3, 5, -1, -1, -1, -1],
                            "right_children": [2, 4, 6, -1, -1, -1, -1],
                            "split_indices": [2, 0, 3, 0, 0, 0, 0],
                            "split_conditions": [2.45, 1.5, 1.75, 0.0, 0.0, 0.0, 0.0],
                            "base_weights": [0.0, 0.0, 0.0, -0.2, 0.5, -0.3, 0.8]
                        },
                        {
                            "tree_param": { "num_nodes": "3" },
                            "id": 1,
                            "left_children": [1, -1, -1],
                            "right_children": [2, -1, -1],
                            "split_indices": [1, 0, 0],
                            "split_conditions": [3.0, 0.0, 0.0],
                            "base_weights": [0.0, -0.4, 0.6]
                        }
                    ],
                    "tree_info": [0, 0]
                }
            },
            "objective": {
                "name": "binary:logistic"
            }
        }
    }"#;

    #[test]
    fn test_parse_xgboost_json_structure() {
        let model = parse_xgboost_json(SAMPLE_XGBOOST_JSON).unwrap();

        assert_eq!(model.num_features, 4);
        assert_eq!(model.num_classes, 2);
        assert!((model.base_score - 0.5).abs() < 1e-9);
        assert_eq!(model.trees.len(), 2);

        // Tree 0: 7 nodes, depth 2
        assert_eq!(model.trees[0].nodes.len(), 7);
        assert_eq!(tree_depth(&model.trees[0]), 2);

        // Root of tree 0: splits on feature 2 at threshold 2.45
        let root = &model.trees[0].nodes[0];
        assert!(!root.is_leaf);
        assert_eq!(root.feature_index, 2);
        assert!((root.threshold - 2.45).abs() < 1e-9);
        assert_eq!(root.left_child, 1);
        assert_eq!(root.right_child, 2);

        // Node 3 (leaf): value -0.2
        let leaf3 = &model.trees[0].nodes[3];
        assert!(leaf3.is_leaf);
        assert!((leaf3.leaf_value - (-0.2)).abs() < 1e-9);

        // Node 6 (leaf): value 0.8
        let leaf6 = &model.trees[0].nodes[6];
        assert!(leaf6.is_leaf);
        assert!((leaf6.leaf_value - 0.8).abs() < 1e-9);

        // Tree 1: 3 nodes, depth 1
        assert_eq!(model.trees[1].nodes.len(), 3);
        assert_eq!(tree_depth(&model.trees[1]), 1);

        // Root of tree 1: splits on feature 1 at threshold 3.0
        let root1 = &model.trees[1].nodes[0];
        assert!(!root1.is_leaf);
        assert_eq!(root1.feature_index, 1);
        assert!((root1.threshold - 3.0).abs() < 1e-9);

        // max_depth should be 2 (max of tree depths)
        assert_eq!(model.max_depth, 2);
    }

    #[test]
    fn test_parse_xgboost_json_inference() {
        let model = parse_xgboost_json(SAMPLE_XGBOOST_JSON).unwrap();

        // features: [1.0, 2.0, 3.0, 1.0]
        // Tree 0: root splits on feature[2]=3.0 >= 2.45 → right (node 2)
        //   node 2: splits on feature[3]=1.0 < 1.75 → left (node 5)
        //   node 5 (leaf): -0.3
        // Tree 1: root splits on feature[1]=2.0 < 3.0 → left (node 1)
        //   node 1 (leaf): -0.4
        let features = vec![1.0, 2.0, 3.0, 1.0];
        let tree0_val = traverse_tree(&model.trees[0], &features);
        let tree1_val = traverse_tree(&model.trees[1], &features);

        assert!((tree0_val - (-0.3)).abs() < 1e-9);
        assert!((tree1_val - (-0.4)).abs() < 1e-9);

        // Total score = base_score + tree0 + tree1 = 0.5 + (-0.3) + (-0.4) = -0.2
        // Sigmoid(-0.2) < 0.5, so predict class 0
        assert_eq!(predict(&model, &features), 0);
    }

    #[test]
    fn test_parse_xgboost_json_circuit_compatible() {
        // Verify that parsed model works with circuit utilities
        let model = parse_xgboost_json(SAMPLE_XGBOOST_JSON).unwrap();
        let features = vec![1.0, 2.0, 3.0, 1.0];

        // collect_all_leaf_values should work
        let (leaf_vals, max_d) = collect_all_leaf_values(&model);
        assert_eq!(max_d, 2);
        assert_eq!(leaf_vals.len(), 2 * 4); // 2 trees * 2^2 leaves

        // compute_path_bits should work
        let (path_bits, _) = compute_path_bits(&model, &features);
        assert_eq!(path_bits.len(), 2); // 2 trees
        assert_eq!(path_bits[0].len(), 2); // max_depth=2
                                           // Tree 0: feature[2]=3.0 >= 2.45 → right (true), then feature[3]=1.0 < 1.75 → left (false)
        assert!(path_bits[0][0]); // right at root
        assert!(!path_bits[0][1]); // left at depth 1
                                   // Tree 1: feature[1]=2.0 < 3.0 → left (false), then leaf (padded false)
        assert!(!path_bits[1][0]); // left at root
        assert!(!path_bits[1][1]); // padded

        // compute_leaf_sum should work
        let leaf_sum = compute_leaf_sum(&model, &features);
        let expected_sum = quantize(0.5) + quantize(-0.3) + quantize(-0.4);
        assert_eq!(leaf_sum, expected_sum);

        // compute_comparison_tables should work
        let (thresholds, fi, is_real) = compute_comparison_tables(&model, max_d);
        assert_eq!(thresholds.len(), max_d * 2 * 4);

        // compute_comparison_witness should work
        let decomp_bits = compute_comparison_witness(&model, &features, max_d, DEFAULT_DECOMP_K);
        assert_eq!(decomp_bits.len(), 2 * max_d * DEFAULT_DECOMP_K);

        // Suppress unused variable warnings
        let _ = (leaf_vals, fi, is_real, decomp_bits);
    }

    #[test]
    fn test_parse_xgboost_json_multiclass() {
        // 3-class model with 6 trees (2 per class)
        let json = r#"{
            "version": [2, 0, 0],
            "learner": {
                "learner_model_param": {
                    "num_feature": "2",
                    "num_class": "3",
                    "base_score": "5E-1"
                },
                "gradient_booster": {
                    "name": "gbtree",
                    "model": {
                        "trees": [
                            {
                                "left_children": [1, -1, -1],
                                "right_children": [2, -1, -1],
                                "split_indices": [0, 0, 0],
                                "split_conditions": [0.5, 0.0, 0.0],
                                "base_weights": [0.0, 0.9, -0.1]
                            },
                            {
                                "left_children": [-1],
                                "right_children": [-1],
                                "split_indices": [0],
                                "split_conditions": [0.0],
                                "base_weights": [0.1]
                            },
                            {
                                "left_children": [-1],
                                "right_children": [-1],
                                "split_indices": [0],
                                "split_conditions": [0.0],
                                "base_weights": [-0.5]
                            },
                            {
                                "left_children": [-1],
                                "right_children": [-1],
                                "split_indices": [0],
                                "split_conditions": [0.0],
                                "base_weights": [0.3]
                            },
                            {
                                "left_children": [-1],
                                "right_children": [-1],
                                "split_indices": [0],
                                "split_conditions": [0.0],
                                "base_weights": [0.2]
                            },
                            {
                                "left_children": [-1],
                                "right_children": [-1],
                                "split_indices": [0],
                                "split_conditions": [0.0],
                                "base_weights": [-0.1]
                            }
                        ],
                        "tree_info": [0, 1, 2, 0, 1, 2]
                    }
                },
                "objective": { "name": "multi:softmax" }
            }
        }"#;

        let model = parse_xgboost_json(json).unwrap();
        assert_eq!(model.num_classes, 3);
        assert_eq!(model.trees.len(), 6);
        assert_eq!(model.num_features, 2);
    }

    // ========================================================================
    // Model validation tests
    // ========================================================================

    /// A model of one tree with the given nodes, on two features, whose `max_depth` field
    /// is the tree's depth.
    fn one_tree_model(nodes: Vec<TreeNode>) -> XgboostModel {
        let tree = DecisionTree { nodes };
        XgboostModel {
            num_features: 2,
            num_classes: 2,
            max_depth: tree_depth(&tree),
            base_score: 0.0,
            trees: vec![tree],
        }
    }

    fn split(feature_index: i32, left_child: usize, right_child: usize) -> TreeNode {
        TreeNode {
            feature_index,
            threshold: 0.5,
            left_child,
            right_child,
            leaf_value: 0.0,
            is_leaf: false,
        }
    }

    fn leaf() -> TreeNode {
        TreeNode {
            feature_index: -1,
            threshold: 0.0,
            left_child: 0,
            right_child: 0,
            leaf_value: 1.0,
            is_leaf: true,
        }
    }

    /// `splits` split nodes in a chain: split i has a leaf on the left and the next
    /// split on the right, so the tree is `splits` levels deep.
    fn chain(splits: usize) -> Vec<TreeNode> {
        let mut nodes = Vec::with_capacity(2 * splits + 1);
        for i in 0..splits {
            nodes.push(split(0, 2 * i + 1, 2 * i + 2));
            nodes.push(leaf());
        }
        nodes.push(leaf());
        nodes
    }

    /// Run `f` on another thread and fail if it takes longer than ten seconds, so that a
    /// check that is exponential in the tree's depth fails the test instead of hanging it.
    fn finishes_quickly<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(f()));
        match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(value) => value,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("took longer than ten seconds")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the thread panicked"),
        }
    }

    /// An `xgb.save_model` file with one tree made of the given arrays, on two features.
    fn xgboost_json(
        left_children: &[i32],
        right_children: &[i32],
        split_indices: &[i32],
        split_conditions: &[f64],
        base_weights: &[f64],
    ) -> String {
        serde_json::json!({
            "learner": {
                "learner_model_param": { "num_feature": "2" },
                "gradient_booster": {
                    "model": {
                        "trees": [{
                            "left_children": left_children,
                            "right_children": right_children,
                            "split_indices": split_indices,
                            "split_conditions": split_conditions,
                            "base_weights": base_weights,
                        }]
                    }
                }
            }
        })
        .to_string()
    }

    /// The arrays of [`xgboost_json`] for the nodes of a tree.
    fn xgboost_json_of(nodes: &[TreeNode]) -> String {
        let child = |node: &TreeNode, index: usize| if node.is_leaf { -1 } else { index as i32 };
        let left: Vec<i32> = nodes.iter().map(|n| child(n, n.left_child)).collect();
        let right: Vec<i32> = nodes.iter().map(|n| child(n, n.right_child)).collect();
        let features: Vec<i32> = nodes.iter().map(|n| n.feature_index.max(0)).collect();
        let thresholds: Vec<f64> = nodes.iter().map(|n| n.threshold).collect();
        let weights: Vec<f64> = nodes.iter().map(|n| n.leaf_value).collect();
        xgboost_json(&left, &right, &features, &thresholds, &weights)
    }

    /// Deep chains are refused for their depth by both loaders, without recursing (a
    /// 200,000-level recursion overflows a test thread's stack), without shifting by the
    /// depth (1 << 64 panics) and without allocating 2^depth leaves.
    #[test]
    fn test_deep_chains_are_refused_quickly() {
        for splits in [200_000, 64, 30] {
            let expected = format!(
                "Tree 0: tree is {} levels deep, over the limit of 24",
                splits
            );
            let nodes = chain(splits);
            let json = xgboost_json_of(&nodes);
            let model = one_tree_model(nodes);
            let (validated, parsed, depth) = finishes_quickly(move || {
                (
                    validate_model(&model),
                    parse_xgboost_json(&json),
                    tree_depth(&model.trees[0]),
                )
            });
            assert_eq!(validated.unwrap_err(), expected);
            assert_eq!(parsed.unwrap_err(), expected);
            assert_eq!(depth, splits);
        }
    }

    /// 40 splits that each have one child on both sides: 2^40 paths through 41 nodes.
    #[test]
    fn test_shared_children_are_refused_quickly() {
        let mut nodes: Vec<TreeNode> = (0..40).map(|i| split(0, i + 1, i + 1)).collect();
        nodes.push(leaf());
        let expected = "Tree 0: node 0 has child 1, which is already a child of node 0: \
                        a tree node has one parent";
        let json = xgboost_json_of(&nodes);
        let model = one_tree_model(nodes);
        let (validated, parsed, depth) = finishes_quickly(move || {
            (
                validate_model(&model),
                parse_xgboost_json(&json),
                tree_depth(&model.trees[0]),
            )
        });
        assert_eq!(validated.unwrap_err(), expected);
        assert_eq!(parsed.unwrap_err(), expected);
        // tree_depth gives 0 for a tree it cannot measure instead of walking every path.
        assert_eq!(depth, 0);
    }

    /// Per-node arrays of different lengths are refused before any is indexed.
    #[test]
    fn test_parse_xgboost_json_short_arrays() {
        let left = [1, -1, -1];
        let right = [2, -1, -1];
        let indices = [0, 0, 0];
        let conditions = [0.5, 0.0, 0.0];
        let weights = [0.0, 0.1, 0.2];
        for (json, lengths) in [
            (
                xgboost_json(&left, &right, &[], &conditions, &weights),
                [3, 3, 0, 3, 3],
            ),
            (
                xgboost_json(&left, &right, &indices, &[0.5], &weights),
                [3, 3, 3, 1, 3],
            ),
            (
                xgboost_json(&left, &right, &indices, &conditions, &[0.0, 0.1]),
                [3, 3, 3, 3, 2],
            ),
            (
                xgboost_json(&left, &[2, -1], &indices, &conditions, &weights),
                [3, 2, 3, 3, 3],
            ),
            (
                xgboost_json(&left, &[2, -1, -1, -1], &indices, &conditions, &weights),
                [3, 4, 3, 3, 3],
            ),
        ] {
            let expected = format!(
                "Tree 0: array length mismatch (left_children={}, right_children={}, \
                 split_indices={}, split_conditions={}, base_weights={})",
                lengths[0], lengths[1], lengths[2], lengths[3], lengths[4]
            );
            assert_eq!(parse_xgboost_json(&json).unwrap_err(), expected);
        }
        parse_xgboost_json(&xgboost_json(
            &left,
            &right,
            &indices,
            &conditions,
            &weights,
        ))
        .unwrap();
    }

    #[test]
    fn test_generated_model_sizes_are_checked_before_building() {
        assert!(check_generated_model_size(1, MAX_TREE_DEPTH, MAX_FEATURES).is_ok());
        assert!(check_generated_model_size(128, 6, 5).is_ok());
        for (trees, depth, features, error) in [
            (0, 2, 5, "A model needs at least one tree"),
            (1, 0, 5, "Tree depth 0 is outside 1 to 24"),
            (1, 25, 5, "Tree depth 25 is outside 1 to 24"),
            (1, 64, 5, "Tree depth 64 is outside 1 to 24"),
            (1, 2, 0, "Feature count 0 is outside 1 to 1048576"),
            (1, 2, MAX_FEATURES + 1, "Feature count 1048577 is outside 1 to 1048576"),
            (
                2,
                MAX_TREE_DEPTH,
                5,
                "Model's leaf table would have 33554432 entries (2 trees, padded to 2, times 2^24), over the limit of 16777216",
            ),
        ] {
            assert_eq!(
                check_generated_model_size(trees, depth, features).unwrap_err(),
                error,
                "{trees} trees, depth {depth}, {features} features"
            );
        }
        // Far more trees than memory could hold still gives an error, not a panic.
        assert!(check_generated_model_size(usize::MAX, 1, 5).is_err());
    }

    #[test]
    fn test_validate_model_accepts() {
        validate_model(&sample_model()).unwrap();
        validate_model(&parse_xgboost_json(SAMPLE_XGBOOST_JSON).unwrap()).unwrap();
        for depth in 1..=4 {
            validate_model(&generate_model(3, depth, 5)).unwrap();
        }

        // The last feature
        validate_model(&one_tree_model(vec![split(1, 1, 2), leaf(), leaf()])).unwrap();
        // One split is enough: the other trees may be single leaves
        let mut model = one_tree_model(vec![split(0, 1, 2), leaf(), leaf()]);
        model.trees.insert(
            0,
            DecisionTree {
                nodes: vec![leaf()],
            },
        );
        validate_model(&model).unwrap();
        // The limits themselves
        validate_model(&one_tree_model(chain(MAX_TREE_DEPTH))).unwrap();
        let mut many_features = one_tree_model(vec![split(0, 1, 2), leaf(), leaf()]);
        many_features.num_features = MAX_FEATURES;
        validate_model(&many_features).unwrap();
        // One tree per class
        let mut three_classes = sample_model();
        three_classes.trees.push(three_classes.trees[1].clone());
        three_classes.num_classes = 3;
        validate_model(&three_classes).unwrap();
        // Binary models need no tree per class
        let mut one_tree = sample_model();
        one_tree.trees.pop();
        validate_model(&one_tree).unwrap();
    }

    #[test]
    fn test_validate_model_rejects() {
        let mut no_trees = sample_model();
        no_trees.trees.clear();

        let mut second_tree = sample_model();
        second_tree.trees[1].nodes[2].feature_index = 5;

        let mut too_many_features = one_tree_model(vec![split(0, 1, 2), leaf(), leaf()]);
        too_many_features.num_features = MAX_FEATURES + 1;

        // Each tree is within the depth limit, but two of them need twice the leaf slots.
        let mut too_many_leaves = one_tree_model(chain(MAX_TREE_DEPTH));
        too_many_leaves.trees.push(DecisionTree {
            nodes: vec![leaf()],
        });

        let mut too_few_trees = sample_model();
        too_few_trees.num_classes = 3;
        // predict would allocate 2^62 class scores.
        let mut huge_class_count = sample_model();
        huge_class_count.num_classes = 1 << 62;

        let mut depth_field_too_large = sample_model();
        depth_field_too_large.max_depth = 3;
        let mut depth_field_too_small = sample_model();
        depth_field_too_small.max_depth = 1;
        let mut depth_field_huge = sample_model();
        depth_field_huge.max_depth = 4_000_000_000;

        for (model, expected) in [
            (no_trees, "Model has no trees"),
            (one_tree_model(vec![]), "Tree 0: tree has no nodes"),
            (
                one_tree_model(vec![leaf()]),
                "Model has depth 0: every tree is a single leaf",
            ),
            (
                one_tree_model(vec![split(-1, 1, 2), leaf(), leaf()]),
                "Tree 0: node 0 splits on feature -1, but the model has 2 features",
            ),
            (
                one_tree_model(vec![split(2, 1, 2), leaf(), leaf()]),
                "Tree 0: node 0 splits on feature 2, but the model has 2 features",
            ),
            (
                second_tree,
                "Tree 1: node 2 splits on feature 5, but the model has 5 features",
            ),
            (
                one_tree_model(vec![split(0, 3, 2), leaf(), leaf()]),
                "Tree 0: node 0 has child index 3, but the tree has 3 nodes",
            ),
            (
                one_tree_model(vec![split(0, 1, 3), leaf(), leaf()]),
                "Tree 0: node 0 has child index 3, but the tree has 3 nodes",
            ),
            // A node that is its own child
            (
                one_tree_model(vec![split(0, 0, 1), leaf()]),
                "Tree 0: node 0 has child 0, which leads back to it: the path never reaches a leaf",
            ),
            // A cycle through the right child and more than one node
            (
                one_tree_model(vec![split(0, 1, 2), leaf(), split(1, 3, 0), leaf()]),
                "Tree 0: node 2 has child 0, which leads back to it: the path never reaches a leaf",
            ),
            // A child with two parents
            (
                one_tree_model(vec![split(0, 1, 2), split(1, 2, 3), leaf(), leaf()]),
                "Tree 0: node 1 has child 2, which is already a child of node 0: \
                 a tree node has one parent",
            ),
            (
                one_tree_model(vec![split(0, 1, 1), leaf()]),
                "Tree 0: node 0 has child 1, which is already a child of node 0: \
                 a tree node has one parent",
            ),
            // Nodes that no path from the root reaches
            (
                one_tree_model(vec![split(0, 1, 2), leaf(), leaf(), split(0, 3, 4), leaf()]),
                "Tree 0: node 3 has child 3, which leads back to it: the path never reaches a leaf",
            ),
            (
                one_tree_model(vec![split(0, 1, 2), leaf(), leaf(), split(0, 4, 1), leaf()]),
                "Tree 0: node 3 has child 1, which is already a child of node 0: \
                 a tree node has one parent",
            ),
            (
                one_tree_model(vec![
                    split(0, 1, 2),
                    leaf(),
                    leaf(),
                    split(7, 4, 5),
                    leaf(),
                    leaf(),
                ]),
                "Tree 0: node 3 splits on feature 7, but the model has 2 features",
            ),
            // Over the limits
            (
                one_tree_model(chain(MAX_TREE_DEPTH + 1)),
                "Tree 0: tree is 25 levels deep, over the limit of 24",
            ),
            (
                too_many_features,
                "Model has 1048577 features, over the limit of 1048576",
            ),
            (
                too_many_leaves,
                "Model's leaf table would have 33554432 entries (2 trees, padded to 2, \
                 times 2^24), over the limit of 16777216",
            ),
            // More classes than trees
            (
                too_few_trees,
                "Model has 3 classes but 2 trees: a model with more than 2 classes needs \
                 at least one tree per class",
            ),
            (
                huge_class_count,
                "Model has 4611686018427387904 classes but 2 trees: a model with more than \
                 2 classes needs at least one tree per class",
            ),
            // A max_depth field that is not the trees' depth
            (
                depth_field_too_large,
                "Model's max_depth field is 3, but its deepest tree is 2 levels deep",
            ),
            (
                depth_field_too_small,
                "Model's max_depth field is 1, but its deepest tree is 2 levels deep",
            ),
            (
                depth_field_huge,
                "Model's max_depth field is 4000000000, but its deepest tree is 2 levels deep",
            ),
        ] {
            assert_eq!(validate_model(&model).unwrap_err(), expected);
        }
    }

    /// The depth is that of the walk from the root: a deeper chain of nodes the root does
    /// not reach is checked, but does not count.
    #[test]
    fn test_unreachable_nodes_do_not_count_towards_the_depth() {
        let mut nodes = vec![split(0, 1, 2), leaf(), leaf()];
        // 30 splits from node 3, which has no parent: over the depth limit if counted.
        for i in 0..30 {
            nodes.push(split(0, 3 + 2 * i + 1, 3 + 2 * i + 2));
            nodes.push(leaf());
        }
        nodes.push(leaf());
        let tree = DecisionTree { nodes };
        assert_eq!(tree_depth(&tree), 1);
        assert_eq!(check_tree_structure(&tree), Ok(1));
        let model = one_tree_model(tree.nodes);
        assert_eq!(model.max_depth, 1);
        validate_model(&model).unwrap();
    }

    /// The sample model, in code and in sample_model.json, has the max_depth field of its
    /// trees, so both pass validate_model.
    #[test]
    fn test_sample_models_are_valid() {
        validate_model(&sample_model()).unwrap();
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sample_model.json");
        let loaded = load_xgboost_json(&path).unwrap();
        assert_eq!(loaded.max_depth, 2);
        validate_model(&loaded).unwrap();
    }

    #[test]
    fn test_check_feature_count() {
        let model = sample_model();
        check_feature_count(&model, &[0.0; 5]).unwrap();
        for len in [0, 4, 6, 8] {
            assert_eq!(
                check_feature_count(&model, &vec![0.0; len]).unwrap_err(),
                format!("Expected 5 features, got {}", len)
            );
        }
    }

    /// Trees the parser cannot compute a depth for are errors.
    #[test]
    fn test_parse_xgboost_json_bad_tree() {
        for (left_children, right_children, expected) in [
            (
                "[0, -1, -1]",
                "[1, -1, -1]",
                "Tree 0: node 0 has child 0, which leads back to it: the path never reaches a leaf",
            ),
            (
                "[1, 0, -1]",
                "[2, 2, -1]",
                "Tree 0: node 1 has child 2, which is already a child of node 0: \
                 a tree node has one parent",
            ),
            (
                "[1, -1, -1]",
                "[3, -1, -1]",
                "Tree 0: node 0 has child index 3, but the tree has 3 nodes",
            ),
            (
                "[1, -1, -1]",
                "[-1, -1, -1]",
                "Tree 0: node 0 has child index -1, but the tree has 3 nodes",
            ),
        ] {
            let json = format!(
                r#"{{
                    "learner": {{
                        "learner_model_param": {{ "num_feature": "2" }},
                        "gradient_booster": {{
                            "model": {{
                                "trees": [
                                    {{
                                        "left_children": {left_children},
                                        "right_children": {right_children},
                                        "split_indices": [0, 0, 0],
                                        "split_conditions": [0.5, 0.5, 0.0],
                                        "base_weights": [0.0, 0.1, 0.2]
                                    }}
                                ]
                            }}
                        }}
                    }}
                }}"#
            );
            assert_eq!(parse_xgboost_json(&json).unwrap_err(), expected);
        }
    }
}
