//! LightGBM model parser
//!
//! Converts LightGBM JSON format (from `model.dump_model()` in Python)
//! to the existing XgboostModel/DecisionTree/TreeNode types used by the
//! circuit builder and prover.

use crate::model::{tree_depth, DecisionTree, TreeNode, XgboostModel};
use serde::Deserialize;
use std::collections::VecDeque;
use std::path::Path;

// ========================================================================
// LightGBM JSON deserialization types
// ========================================================================

/// Top-level LightGBM model JSON structure.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct LightGBMModel {
    #[serde(default = "default_num_class")]
    num_class: usize,
    #[serde(default)]
    num_tree_per_iteration: usize,
    #[serde(default)]
    max_feature_idx: usize,
    tree_info: Vec<LightGBMTreeInfo>,
    #[serde(default)]
    feature_names: Vec<String>,
}

fn default_num_class() -> usize {
    1
}

/// Per-tree metadata and structure.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct LightGBMTreeInfo {
    #[serde(default)]
    tree_index: usize,
    #[serde(default)]
    num_leaves: usize,
    #[serde(default)]
    shrinkage: f64,
    tree_structure: LightGBMNode,
}

/// A node in the LightGBM recursive tree structure.
/// Uses `serde(untagged)` to distinguish leaf vs internal nodes by their fields.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LightGBMNode {
    /// Leaf node: has `leaf_index` and `leaf_value`.
    Leaf {
        #[allow(dead_code)]
        leaf_index: usize,
        leaf_value: f64,
    },
    /// Internal (split) node: has split info and two children.
    Internal {
        #[allow(dead_code)]
        split_index: usize,
        split_feature: usize,
        #[allow(dead_code)]
        #[serde(default)]
        split_gain: f64,
        threshold: f64,
        #[allow(dead_code)]
        #[serde(default)]
        decision_type: String,
        left_child: Box<LightGBMNode>,
        right_child: Box<LightGBMNode>,
    },
}

// ========================================================================
// Conversion to XgboostModel
// ========================================================================

/// Load a LightGBM model from a JSON file (saved with `model.dump_model()` in Python).
pub fn load_lightgbm_json(path: &Path) -> Result<XgboostModel, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read model file: {}", e))?;
    parse_lightgbm_json(&data)
}

/// Parse a LightGBM model from a JSON string and convert to XgboostModel.
///
/// The conversion flattens the recursive tree structure into flat arrays
/// (the same format as XGBoost's DecisionTree/TreeNode), so the existing
/// circuit builder, prover, and all inference utilities work unchanged.
pub fn parse_lightgbm_json(json_str: &str) -> Result<XgboostModel, String> {
    let lgbm: LightGBMModel = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse LightGBM JSON: {}", e))?;

    // num_classes: LightGBM uses num_class=1 for binary classification/regression,
    // which maps to 2 classes in our format (matching XGBoost's convention).
    let num_classes = if lgbm.num_class <= 1 {
        2
    } else {
        lgbm.num_class
    };

    let num_features = lgbm.max_feature_idx + 1;

    let mut trees = Vec::with_capacity(lgbm.tree_info.len());
    let mut max_depth = 0usize;

    for (tree_idx, tree_info) in lgbm.tree_info.iter().enumerate() {
        let dt = flatten_lgbm_tree(&tree_info.tree_structure)
            .map_err(|e| format!("Tree {}: {}", tree_idx, e))?;

        if dt.nodes.is_empty() {
            return Err(format!("Tree {} has no nodes", tree_idx));
        }

        let d = tree_depth(&dt);
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
        base_score: 0.0, // LightGBM handles bias internally, not as a separate base_score
    })
}

/// Count the total number of nodes in a recursive LightGBM tree.
fn count_nodes(node: &LightGBMNode) -> usize {
    match node {
        LightGBMNode::Leaf { .. } => 1,
        LightGBMNode::Internal {
            left_child,
            right_child,
            ..
        } => 1 + count_nodes(left_child) + count_nodes(right_child),
    }
}

/// Flatten a recursive LightGBM tree node into a flat DecisionTree using BFS.
///
/// BFS guarantees that node indices are assigned in level-order, producing
/// the same layout that XGBoost uses (root=0, then left/right children follow).
fn flatten_lgbm_tree(root: &LightGBMNode) -> Result<DecisionTree, String> {
    struct QueueEntry<'a> {
        node: &'a LightGBMNode,
        self_index: usize,
    }

    // First, count total nodes via a quick BFS so we can pre-allocate.
    let total_nodes = count_nodes(root);
    let mut nodes: Vec<TreeNode> = vec![
        TreeNode {
            feature_index: -1,
            threshold: 0.0,
            left_child: 0,
            right_child: 0,
            leaf_value: 0.0,
            is_leaf: true,
        };
        total_nodes
    ];

    let mut bfs_queue: VecDeque<QueueEntry> = VecDeque::new();
    bfs_queue.push_back(QueueEntry {
        node: root,
        self_index: 0,
    });

    let mut next_idx = 1usize;

    while let Some(entry) = bfs_queue.pop_front() {
        match entry.node {
            LightGBMNode::Internal {
                split_feature,
                threshold,
                left_child,
                right_child,
                ..
            } => {
                let left_idx = next_idx;
                next_idx += 1;
                let right_idx = next_idx;
                next_idx += 1;

                nodes[entry.self_index] = TreeNode {
                    feature_index: *split_feature as i32,
                    threshold: *threshold,
                    left_child: left_idx,
                    right_child: right_idx,
                    leaf_value: 0.0,
                    is_leaf: false,
                };

                bfs_queue.push_back(QueueEntry {
                    node: left_child,
                    self_index: left_idx,
                });
                bfs_queue.push_back(QueueEntry {
                    node: right_child,
                    self_index: right_idx,
                });
            }
            LightGBMNode::Leaf { leaf_value, .. } => {
                nodes[entry.self_index] = TreeNode {
                    feature_index: -1,
                    threshold: 0.0,
                    left_child: 0,
                    right_child: 0,
                    leaf_value: *leaf_value,
                    is_leaf: true,
                };
            }
        }
    }

    Ok(DecisionTree { nodes })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        collect_all_leaf_values, compute_comparison_tables, compute_comparison_witness,
        compute_leaf_sum, compute_path_bits, predict, quantize, traverse_tree, tree_depth,
        DEFAULT_DECOMP_K,
    };

    /// A single-tree LightGBM binary classifier with 3 internal nodes and 4 leaves.
    /// Tree structure:
    ///        [split_feature=2, threshold=2.45]
    ///       /                                 \
    ///  leaf(-1.234)                 [split_feature=3, threshold=1.75]
    ///                              /                                \
    ///                        leaf(0.567)                      leaf(-0.890)
    const SINGLE_TREE_LGBM: &str = r#"{
        "name": "tree",
        "version": "v4",
        "num_class": 1,
        "num_tree_per_iteration": 1,
        "label_index": 0,
        "max_feature_idx": 3,
        "objective": "binary sigmoid:1",
        "average_output": false,
        "feature_names": ["f0", "f1", "f2", "f3"],
        "tree_info": [
            {
                "tree_index": 0,
                "num_leaves": 3,
                "num_cat": 0,
                "shrinkage": 1,
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 2,
                    "split_gain": 100.0,
                    "threshold": 2.45,
                    "decision_type": "<=",
                    "default_left": true,
                    "internal_value": 0,
                    "left_child": {
                        "leaf_index": 0,
                        "leaf_value": -1.234
                    },
                    "right_child": {
                        "split_index": 1,
                        "split_feature": 3,
                        "split_gain": 50.0,
                        "threshold": 1.75,
                        "decision_type": "<=",
                        "default_left": true,
                        "internal_value": 0,
                        "left_child": {
                            "leaf_index": 1,
                            "leaf_value": 0.567
                        },
                        "right_child": {
                            "leaf_index": 2,
                            "leaf_value": -0.890
                        }
                    }
                }
            }
        ]
    }"#;

    /// A two-tree LightGBM model for multi-tree tests.
    const MULTI_TREE_LGBM: &str = r#"{
        "name": "tree",
        "version": "v4",
        "num_class": 1,
        "num_tree_per_iteration": 1,
        "max_feature_idx": 4,
        "tree_info": [
            {
                "tree_index": 0,
                "num_leaves": 4,
                "shrinkage": 1,
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 0,
                    "split_gain": 80.0,
                    "threshold": 0.5,
                    "decision_type": "<=",
                    "left_child": {
                        "split_index": 1,
                        "split_feature": 1,
                        "split_gain": 40.0,
                        "threshold": 0.3,
                        "decision_type": "<=",
                        "left_child": {
                            "leaf_index": 0,
                            "leaf_value": -0.8
                        },
                        "right_child": {
                            "leaf_index": 1,
                            "leaf_value": 0.2
                        }
                    },
                    "right_child": {
                        "split_index": 2,
                        "split_feature": 2,
                        "split_gain": 60.0,
                        "threshold": 0.7,
                        "decision_type": "<=",
                        "left_child": {
                            "leaf_index": 2,
                            "leaf_value": 0.3
                        },
                        "right_child": {
                            "leaf_index": 3,
                            "leaf_value": 0.9
                        }
                    }
                }
            },
            {
                "tree_index": 1,
                "num_leaves": 3,
                "shrinkage": 1,
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 3,
                    "split_gain": 70.0,
                    "threshold": 0.4,
                    "decision_type": "<=",
                    "left_child": {
                        "leaf_index": 0,
                        "leaf_value": -0.5
                    },
                    "right_child": {
                        "split_index": 1,
                        "split_feature": 4,
                        "split_gain": 30.0,
                        "threshold": 0.6,
                        "decision_type": "<=",
                        "left_child": {
                            "leaf_index": 1,
                            "leaf_value": 0.4
                        },
                        "right_child": {
                            "leaf_index": 2,
                            "leaf_value": 0.7
                        }
                    }
                }
            }
        ]
    }"#;

    #[test]
    fn test_parse_lightgbm_single_tree() {
        let model = parse_lightgbm_json(SINGLE_TREE_LGBM).unwrap();

        assert_eq!(model.trees.len(), 1);
        assert_eq!(model.num_features, 4); // max_feature_idx=3 => 4 features

        let tree = &model.trees[0];
        // 3 nodes total: 1 internal (root) + 1 internal + 3 leaves? No:
        // Root is internal, left child is leaf, right child is internal with 2 leaves
        // Total = 1 root + 1 leaf + 1 internal + 2 leaves = 5 nodes
        assert_eq!(tree.nodes.len(), 5);

        // Root node: splits on feature 2 at threshold 2.45
        let root = &tree.nodes[0];
        assert!(!root.is_leaf);
        assert_eq!(root.feature_index, 2);
        assert!((root.threshold - 2.45).abs() < 1e-9);

        // BFS order: root(0), left_child(1), right_child(2), right.left(3), right.right(4)
        // Node 1: leaf with value -1.234
        let node1 = &tree.nodes[1];
        assert!(node1.is_leaf);
        assert!((node1.leaf_value - (-1.234)).abs() < 1e-9);

        // Node 2: internal, splits on feature 3 at threshold 1.75
        let node2 = &tree.nodes[2];
        assert!(!node2.is_leaf);
        assert_eq!(node2.feature_index, 3);
        assert!((node2.threshold - 1.75).abs() < 1e-9);

        // Node 3: leaf with value 0.567
        let node3 = &tree.nodes[3];
        assert!(node3.is_leaf);
        assert!((node3.leaf_value - 0.567).abs() < 1e-9);

        // Node 4: leaf with value -0.890
        let node4 = &tree.nodes[4];
        assert!(node4.is_leaf);
        assert!((node4.leaf_value - (-0.890)).abs() < 1e-9);
    }

    #[test]
    fn test_parse_lightgbm_multi_tree() {
        let model = parse_lightgbm_json(MULTI_TREE_LGBM).unwrap();

        assert_eq!(model.trees.len(), 2);
        assert_eq!(model.num_features, 5); // max_feature_idx=4

        // Tree 0: 3 internal + 4 leaves = 7 nodes
        assert_eq!(model.trees[0].nodes.len(), 7);

        // Tree 1: 2 internal + 3 leaves = 5 nodes
        assert_eq!(model.trees[1].nodes.len(), 5);

        // Verify tree 0 root
        let root0 = &model.trees[0].nodes[0];
        assert!(!root0.is_leaf);
        assert_eq!(root0.feature_index, 0);
        assert!((root0.threshold - 0.5).abs() < 1e-9);

        // Verify tree 1 root
        let root1 = &model.trees[1].nodes[0];
        assert!(!root1.is_leaf);
        assert_eq!(root1.feature_index, 3);
        assert!((root1.threshold - 0.4).abs() < 1e-9);

        // Verify leaf values in tree 0 (BFS order)
        // BFS: root(0) -> left_internal(1), right_internal(2) -> leaves(3,4,5,6)
        assert!(model.trees[0].nodes[3].is_leaf);
        assert!((model.trees[0].nodes[3].leaf_value - (-0.8)).abs() < 1e-9);
        assert!(model.trees[0].nodes[4].is_leaf);
        assert!((model.trees[0].nodes[4].leaf_value - 0.2).abs() < 1e-9);
        assert!(model.trees[0].nodes[5].is_leaf);
        assert!((model.trees[0].nodes[5].leaf_value - 0.3).abs() < 1e-9);
        assert!(model.trees[0].nodes[6].is_leaf);
        assert!((model.trees[0].nodes[6].leaf_value - 0.9).abs() < 1e-9);
    }

    #[test]
    fn test_lightgbm_tree_depth() {
        let model = parse_lightgbm_json(SINGLE_TREE_LGBM).unwrap();

        // Tree has depth 2:
        //   root -> right_child -> leaf (depth 2)
        //   root -> left_child (leaf, depth 1)
        // max depth = 2
        assert_eq!(tree_depth(&model.trees[0]), 2);
        assert_eq!(model.max_depth, 2);

        // Multi-tree: tree 0 depth 2, tree 1 depth 2
        let model2 = parse_lightgbm_json(MULTI_TREE_LGBM).unwrap();
        assert_eq!(tree_depth(&model2.trees[0]), 2);
        assert_eq!(tree_depth(&model2.trees[1]), 2);
        assert_eq!(model2.max_depth, 2);
    }

    #[test]
    fn test_lightgbm_num_features() {
        // Single tree: max_feature_idx=3 => num_features=4
        let model = parse_lightgbm_json(SINGLE_TREE_LGBM).unwrap();
        assert_eq!(model.num_features, 4);

        // Multi tree: max_feature_idx=4 => num_features=5
        let model2 = parse_lightgbm_json(MULTI_TREE_LGBM).unwrap();
        assert_eq!(model2.num_features, 5);
    }

    #[test]
    fn test_lightgbm_binary_classification() {
        // num_class=1 in LightGBM => binary classification => num_classes=2 in our format
        let model = parse_lightgbm_json(SINGLE_TREE_LGBM).unwrap();
        assert_eq!(model.num_classes, 2);
    }

    #[test]
    fn test_lightgbm_multiclass() {
        // A 3-class model should preserve num_class=3
        let json = r#"{
            "num_class": 3,
            "num_tree_per_iteration": 3,
            "max_feature_idx": 2,
            "tree_info": [
                {
                    "tree_index": 0,
                    "num_leaves": 1,
                    "shrinkage": 1,
                    "tree_structure": { "leaf_index": 0, "leaf_value": 0.1 }
                },
                {
                    "tree_index": 1,
                    "num_leaves": 1,
                    "shrinkage": 1,
                    "tree_structure": { "leaf_index": 0, "leaf_value": 0.2 }
                },
                {
                    "tree_index": 2,
                    "num_leaves": 1,
                    "shrinkage": 1,
                    "tree_structure": { "leaf_index": 0, "leaf_value": -0.1 }
                }
            ]
        }"#;

        let model = parse_lightgbm_json(json).unwrap();
        assert_eq!(model.num_classes, 3);
        assert_eq!(model.trees.len(), 3);
    }

    #[test]
    fn test_lightgbm_leaf_ordering() {
        // Verify that the flat tree structure produces correct leaf ordering
        // for the circuit (big-endian path bits).
        //
        // The multi-tree model's tree 0 is a perfect depth-2 tree:
        //        [feature 0, threshold 0.5]
        //       /                          \
        //  [feature 1, th 0.3]       [feature 2, th 0.7]
        //   /       \                 /       \
        // -0.8     0.2             0.3       0.9
        //
        // Path bits (big-endian): 00->-0.8, 01->0.2, 10->0.3, 11->0.9

        let model = parse_lightgbm_json(MULTI_TREE_LGBM).unwrap();

        // Use tree_leaf_values to extract leaves in circuit ordering
        let leaves = crate::model::tree_leaf_values(&model.trees[0], 2);
        assert_eq!(leaves.len(), 4);
        assert_eq!(leaves[0], quantize(-0.8)); // path 00
        assert_eq!(leaves[1], quantize(0.2)); // path 01
        assert_eq!(leaves[2], quantize(0.3)); // path 10
        assert_eq!(leaves[3], quantize(0.9)); // path 11
    }

    #[test]
    fn test_lightgbm_inference() {
        // The multi-tree LGBM model should produce the same inference results
        // as equivalent XGBoost model.
        let model = parse_lightgbm_json(MULTI_TREE_LGBM).unwrap();

        // features: [0.6, 0.2, 0.8, 0.5, 0.3]
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];

        // Tree 0: root splits on feature[0]=0.6 >= 0.5 => right (node 2)
        //   node 2: splits on feature[2]=0.8 >= 0.7 => right (node 6, leaf)
        //   leaf value = 0.9
        let t0_val = traverse_tree(&model.trees[0], &features);
        assert!((t0_val - 0.9).abs() < 1e-9);

        // Tree 1: root splits on feature[3]=0.5 >= 0.4 => right (node 2)
        //   node 2: splits on feature[4]=0.3 < 0.6 => left (node 3, leaf)
        //   leaf value = 0.4
        let t1_val = traverse_tree(&model.trees[1], &features);
        assert!((t1_val - 0.4).abs() < 1e-9);

        // Total = 0.0 (base_score) + 0.9 + 0.4 = 1.3 > 0 => class 1
        assert_eq!(predict(&model, &features), 1);
    }

    #[test]
    fn test_lightgbm_circuit_compatible() {
        // Verify that parsed LightGBM model works with all circuit utilities
        let model = parse_lightgbm_json(MULTI_TREE_LGBM).unwrap();
        let features = vec![0.6, 0.2, 0.8, 0.5, 0.3];

        // collect_all_leaf_values
        let (leaf_vals, max_d) = collect_all_leaf_values(&model);
        assert_eq!(max_d, 2);
        assert_eq!(leaf_vals.len(), 2 * 4); // 2 trees * 2^2 leaves

        // compute_path_bits
        let (path_bits, _) = compute_path_bits(&model, &features);
        assert_eq!(path_bits.len(), 2);
        assert_eq!(path_bits[0].len(), 2);
        // Tree 0: feature[0]=0.6 >= 0.5 => right(true), feature[2]=0.8 >= 0.7 => right(true)
        assert!(path_bits[0][0]);
        assert!(path_bits[0][1]);
        // Tree 1: feature[3]=0.5 >= 0.4 => right(true), feature[4]=0.3 < 0.6 => left(false)
        assert!(path_bits[1][0]);
        assert!(!path_bits[1][1]);

        // compute_leaf_sum
        let leaf_sum = compute_leaf_sum(&model, &features);
        let expected = quantize(0.0) + quantize(0.9) + quantize(0.4);
        assert_eq!(leaf_sum, expected);

        // compute_comparison_tables
        let (thresholds, fi, is_real) = compute_comparison_tables(&model, max_d);
        assert_eq!(thresholds.len(), max_d * 2 * 4);

        // compute_comparison_witness
        let decomp_bits = compute_comparison_witness(&model, &features, max_d, DEFAULT_DECOMP_K);
        assert_eq!(decomp_bits.len(), 2 * max_d * DEFAULT_DECOMP_K);

        // Suppress unused variable warnings
        let _ = (leaf_vals, fi, is_real, decomp_bits);
    }

    #[test]
    fn test_lightgbm_single_leaf_tree() {
        // Edge case: a tree that is just a single leaf (no splits)
        let json = r#"{
            "num_class": 1,
            "max_feature_idx": 1,
            "tree_info": [
                {
                    "tree_index": 0,
                    "num_leaves": 1,
                    "shrinkage": 1,
                    "tree_structure": {
                        "leaf_index": 0,
                        "leaf_value": 0.42
                    }
                }
            ]
        }"#;

        let model = parse_lightgbm_json(json).unwrap();
        assert_eq!(model.trees.len(), 1);
        assert_eq!(model.trees[0].nodes.len(), 1);
        assert!(model.trees[0].nodes[0].is_leaf);
        assert!((model.trees[0].nodes[0].leaf_value - 0.42).abs() < 1e-9);
        assert_eq!(tree_depth(&model.trees[0]), 0);
        assert_eq!(model.max_depth, 0);
    }

    #[test]
    fn test_lightgbm_deep_tree() {
        // A depth-3 tree to verify deeper structures
        let json = r#"{
            "num_class": 1,
            "max_feature_idx": 2,
            "tree_info": [
                {
                    "tree_index": 0,
                    "num_leaves": 5,
                    "shrinkage": 1,
                    "tree_structure": {
                        "split_index": 0,
                        "split_feature": 0,
                        "split_gain": 100.0,
                        "threshold": 1.0,
                        "decision_type": "<=",
                        "left_child": {
                            "split_index": 1,
                            "split_feature": 1,
                            "split_gain": 50.0,
                            "threshold": 2.0,
                            "decision_type": "<=",
                            "left_child": {
                                "split_index": 3,
                                "split_feature": 2,
                                "split_gain": 25.0,
                                "threshold": 3.0,
                                "decision_type": "<=",
                                "left_child": {
                                    "leaf_index": 0,
                                    "leaf_value": 0.1
                                },
                                "right_child": {
                                    "leaf_index": 1,
                                    "leaf_value": 0.2
                                }
                            },
                            "right_child": {
                                "leaf_index": 2,
                                "leaf_value": 0.3
                            }
                        },
                        "right_child": {
                            "leaf_index": 3,
                            "leaf_value": 0.4
                        }
                    }
                }
            ]
        }"#;

        let model = parse_lightgbm_json(json).unwrap();
        assert_eq!(model.trees[0].nodes.len(), 7); // 3 internal + 4 leaves (BFS)
                                                   // Wait -- 3 internal nodes + 4 leaves = 7? Let me count:
                                                   // root(internal), left(internal), right(leaf), left.left(internal), left.right(leaf),
                                                   // left.left.left(leaf), left.left.right(leaf)
                                                   // = 3 internal + 4 leaves = 7. Yes.
        assert_eq!(tree_depth(&model.trees[0]), 3);
        assert_eq!(model.max_depth, 3);

        // Verify inference
        let features = vec![0.5, 1.5, 2.5];
        // root: feature[0]=0.5 < 1.0 => left (node 1)
        // node 1: feature[1]=1.5 < 2.0 => left (node 3)
        // node 3: feature[2]=2.5 < 3.0 => left (leaf: 0.1)
        let val = traverse_tree(&model.trees[0], &features);
        assert!((val - 0.1).abs() < 1e-9);

        let features2 = vec![0.5, 1.5, 3.5];
        // root: left, node 1: left, node 3: feature[2]=3.5 >= 3.0 => right (leaf: 0.2)
        let val2 = traverse_tree(&model.trees[0], &features2);
        assert!((val2 - 0.2).abs() < 1e-9);

        let features3 = vec![2.0, 0.0, 0.0];
        // root: feature[0]=2.0 >= 1.0 => right (leaf: 0.4)
        let val3 = traverse_tree(&model.trees[0], &features3);
        assert!((val3 - 0.4).abs() < 1e-9);
    }

    #[test]
    fn test_lightgbm_base_score_zero() {
        // LightGBM base_score should always be 0.0
        let model = parse_lightgbm_json(SINGLE_TREE_LGBM).unwrap();
        assert!((model.base_score - 0.0).abs() < 1e-15);
    }

    /// E2E test: parse LightGBM model → build circuit → prove → verify in Rust.
    /// Uses the sample LightGBM credit scoring model in test-model/.
    #[test]
    #[ignore] // ~2s, run with: cargo test lightgbm_e2e_prove_verify -- --ignored --nocapture
    fn test_lightgbm_e2e_prove_verify() {
        use crate::circuit::build_and_prove;
        use crate::model::predict;

        let model_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-model/lightgbm_sample.json");
        let model = load_lightgbm_json(&model_path).unwrap();

        eprintln!(
            "LightGBM model: {} trees, {} features, depth {}",
            model.trees.len(),
            model.num_features,
            model.max_depth
        );

        // Run inference
        let features = vec![4.5, 2.5, 1.0, 0.3];
        let predicted_class = predict(&model, &features);
        eprintln!("Predicted class: {}", predicted_class);

        // Build circuit and prove
        let (proof_bytes, circuit_hash, public_inputs) =
            build_and_prove(&model, &features, predicted_class).unwrap();

        eprintln!("Proof size: {} bytes", proof_bytes.len());
        eprintln!("Circuit hash: 0x{}", hex::encode(&circuit_hash));
        eprintln!("Public inputs: 0x{}", hex::encode(&public_inputs));

        // Proof bytes should have "REM1" selector
        assert_eq!(&proof_bytes[0..4], b"REM1", "proof should start with REM1 selector");
        assert!(!circuit_hash.is_empty(), "circuit hash should be non-empty");
    }
}
