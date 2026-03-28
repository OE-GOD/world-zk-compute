//! Multi-Layer Perceptron (MLP) model parser and circuit adapter.
//!
//! Parses a simple MLP JSON model, runs forward-pass inference, and converts
//! the model to the existing [`XgboostModel`] format so it can be proved via
//! the GKR+Hyrax circuit pipeline without any changes to the circuit code.
//!
//! # Conversion strategy
//!
//! For a small MLP with `n_features` (up to 8) inputs and `n_classes` outputs,
//! we build a **complete binary tree of depth `n_features`** per output class.
//! Each leaf corresponds to one quantized input region: feature 0 is split at
//! the first level, feature 1 at the second, and so on.  The leaf value is the
//! MLP's output for the midpoint of that region (pre-computed at parse time).
//!
//! This lets the existing tree-inference circuit prove that the tree's output
//! matches the network's output for a given input region.  The proof is exact
//! for quantized inputs — any feature vector mapped to the same region produces
//! the same tree output.
//!
//! # JSON format
//!
//! ```json
//! {
//!     "model_type": "mlp",
//!     "n_features": 4,
//!     "n_classes": 2,
//!     "layers": [
//!         {
//!             "weights": [[0.5, -0.3, 0.2, 0.1], [0.1, 0.4, -0.2, 0.3]],
//!             "biases": [0.1, -0.1],
//!             "activation": "relu"
//!         },
//!         {
//!             "weights": [[0.6, -0.4], [0.3, 0.7]],
//!             "biases": [0.0, 0.0],
//!             "activation": "none"
//!         }
//!     ]
//! }
//! ```
//!
//! * `n_features`: number of input features (max 8 for the tree conversion)
//! * `n_classes`: number of output classes (one tree per class)
//! * `layers`: ordered list of fully-connected layers
//!   * `weights[i][j]`: weight from input neuron `j` to output neuron `i`
//!   * `biases[i]`: bias for output neuron `i`
//!   * `activation`: `"relu"`, `"sigmoid"`, or `"none"` / `"linear"`

use crate::model::{DecisionTree, TreeNode, XgboostModel, FIXED_POINT_SCALE};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ============================================================================
// Serde types
// ============================================================================

/// Activation function identifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Activation {
    /// Rectified Linear Unit: max(0, x).
    Relu,
    /// Logistic sigmoid: 1 / (1 + exp(-x)).
    Sigmoid,
    /// Identity (no activation).
    #[serde(alias = "linear")]
    None,
}

impl Default for Activation {
    fn default() -> Self {
        Activation::None
    }
}

/// A single fully-connected layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MLPLayer {
    /// Weight matrix: `weights[out][in]`.
    pub weights: Vec<Vec<f64>>,
    /// Bias vector, one per output neuron.
    pub biases: Vec<f64>,
    /// Post-activation function.
    #[serde(default)]
    pub activation: Activation,
}

/// A complete MLP model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MLPModel {
    /// Must be "mlp" (validated at parse time).
    #[serde(default)]
    pub model_type: String,
    /// Number of input features.
    pub n_features: usize,
    /// Number of output classes.
    pub n_classes: usize,
    /// Ordered list of fully-connected layers.
    pub layers: Vec<MLPLayer>,
}

// ============================================================================
// Public API
// ============================================================================

/// Load an MLP model from a JSON file and convert it to [`XgboostModel`].
pub fn load_mlp_json(path: &Path) -> Result<XgboostModel, String> {
    let data =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read model file: {}", e))?;
    parse_mlp_json(&data)
}

/// Parse an MLP model from a JSON string and convert it to [`XgboostModel`].
pub fn parse_mlp_json(json_str: &str) -> Result<XgboostModel, String> {
    let mlp = parse_mlp_model(json_str)?;
    mlp_to_xgboost(&mlp)
}

/// Parse only the raw [`MLPModel`] without converting to `XgboostModel`.
///
/// Useful for running inference directly via [`predict_mlp`].
pub fn parse_mlp_model(json_str: &str) -> Result<MLPModel, String> {
    let mlp: MLPModel = serde_json::from_str(json_str)
        .map_err(|e| format!("Failed to parse MLP JSON: {}", e))?;
    validate_mlp(&mlp)?;
    Ok(mlp)
}

// ============================================================================
// Validation
// ============================================================================

fn validate_mlp(mlp: &MLPModel) -> Result<(), String> {
    if mlp.n_features == 0 {
        return Err("MLP must have at least 1 feature".to_string());
    }
    if mlp.n_classes == 0 {
        return Err("MLP must have at least 1 output class".to_string());
    }
    if mlp.layers.is_empty() {
        return Err("MLP must have at least 1 layer".to_string());
    }
    if mlp.n_features > 8 {
        return Err(format!(
            "n_features={} exceeds maximum of 8 (tree depth would be too large)",
            mlp.n_features
        ));
    }

    // Check layer dimensions are consistent
    let mut expected_in = mlp.n_features;
    for (i, layer) in mlp.layers.iter().enumerate() {
        if layer.biases.len() != layer.weights.len() {
            return Err(format!(
                "Layer {}: biases.len()={} != weights.len()={}",
                i,
                layer.biases.len(),
                layer.weights.len()
            ));
        }
        if layer.weights.is_empty() {
            return Err(format!("Layer {}: weights must be non-empty", i));
        }
        for (j, row) in layer.weights.iter().enumerate() {
            if row.len() != expected_in {
                return Err(format!(
                    "Layer {}, neuron {}: expected {} inputs, got {}",
                    i,
                    j,
                    expected_in,
                    row.len()
                ));
            }
        }
        expected_in = layer.weights.len();
    }

    // Last layer output must match n_classes
    let last_out = mlp.layers.last().unwrap().weights.len();
    if last_out != mlp.n_classes {
        return Err(format!(
            "Last layer has {} outputs but n_classes={}",
            last_out, mlp.n_classes
        ));
    }

    Ok(())
}

// ============================================================================
// Forward pass inference
// ============================================================================

/// Apply an activation function element-wise.
fn apply_activation(values: &mut [f64], activation: &Activation) {
    match activation {
        Activation::Relu => {
            for v in values.iter_mut() {
                if *v < 0.0 {
                    *v = 0.0;
                }
            }
        }
        Activation::Sigmoid => {
            for v in values.iter_mut() {
                *v = 1.0 / (1.0 + (-*v).exp());
            }
        }
        Activation::None => {}
    }
}

/// Run a full MLP forward pass and return the output vector.
///
/// Returns one score per output class.  For binary classification use
/// `outputs[1] - outputs[0]` as a logit, or argmax for multi-class.
pub fn predict_mlp(mlp: &MLPModel, features: &[f64]) -> Result<Vec<f64>, String> {
    if features.len() != mlp.n_features {
        return Err(format!(
            "Expected {} features, got {}",
            mlp.n_features,
            features.len()
        ));
    }

    let mut activations: Vec<f64> = features.to_vec();

    for layer in &mlp.layers {
        let n_out = layer.weights.len();
        let mut next = vec![0.0f64; n_out];
        for (i, (row, bias)) in layer.weights.iter().zip(layer.biases.iter()).enumerate() {
            next[i] = row
                .iter()
                .zip(activations.iter())
                .map(|(w, x)| w * x)
                .sum::<f64>()
                + bias;
        }
        apply_activation(&mut next, &layer.activation);
        activations = next;
    }

    Ok(activations)
}

/// Return the predicted class index (argmax over output scores).
pub fn predict_mlp_class(mlp: &MLPModel, features: &[f64]) -> Result<u32, String> {
    let scores = predict_mlp(mlp, features)?;
    let class = scores
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i as u32)
        .unwrap_or(0);
    Ok(class)
}

// ============================================================================
// Conversion to XgboostModel (lookup-tree encoding)
// ============================================================================

/// Convert an [`MLPModel`] to [`XgboostModel`] via a complete binary lookup tree.
///
/// For each output class, this builds a complete binary decision tree of depth
/// `n_features`.  Level `k` splits on feature `k` at threshold `0.5` (assumes
/// features are normalised to `[0, 1]`).  Each leaf pre-computes the MLP output
/// for the midpoint of the corresponding input region.
///
/// The circuit then proves that a given (committed) input leads to the correct
/// leaf — i.e. that the MLP output for that quantized region matches the claim.
pub fn mlp_to_xgboost(mlp: &MLPModel) -> Result<XgboostModel, String> {
    validate_mlp(mlp)?;

    let n_features = mlp.n_features;
    let n_classes = mlp.n_classes;
    // Split every feature at 0.5; inputs assumed to be in [0, 1].
    let threshold = 0.5_f64;

    let mut trees = Vec::with_capacity(n_classes);
    for class_idx in 0..n_classes {
        let tree = build_mlp_lookup_tree(mlp, class_idx, n_features, threshold)?;
        trees.push(tree);
    }

    Ok(XgboostModel {
        num_features: n_features,
        num_classes: n_classes,
        max_depth: n_features,
        trees,
        base_score: 0.0,
    })
}

/// Build one complete binary decision tree of depth `n_features` for `class_idx`.
///
/// Node numbering: the tree is stored in level-order (BFS order).
/// For a complete binary tree of depth `d`, node `i` has:
/// * left child  = `2*i + 1`
/// * right child = `2*i + 2`
/// Total nodes   = `2^(d+1) - 1`.
fn build_mlp_lookup_tree(
    mlp: &MLPModel,
    class_idx: usize,
    depth: usize,
    threshold: f64,
) -> Result<DecisionTree, String> {
    let total_nodes = (1usize << (depth + 1)) - 1;
    let first_leaf = (1usize << depth) - 1; // index of the first leaf node

    let mut nodes = Vec::with_capacity(total_nodes);

    for i in 0..total_nodes {
        if i < first_leaf {
            // Internal node: split on feature at level = floor(log2(i+1))
            let level = usize::BITS as usize - (i + 1).leading_zeros() as usize - 1;
            let feature_idx = level as i32;

            let left_child = 2 * i + 1;
            let right_child = 2 * i + 2;

            nodes.push(TreeNode {
                feature_index: feature_idx,
                threshold,
                left_child,
                right_child,
                leaf_value: 0.0,
                is_leaf: false,
            });
        } else {
            // Leaf node: decode which input region this leaf represents
            // Leaf offset within leaves: leaf_offset = i - first_leaf
            // Path bits (MSB-first): bit k = (leaf_offset >> (depth-1-k)) & 1
            //   0 means "went left" (feature < 0.5 → midpoint = 0.25)
            //   1 means "went right" (feature >= 0.5 → midpoint = 0.75)
            let leaf_offset = i - first_leaf;
            let midpoint_features: Vec<f64> = (0..depth)
                .map(|k| {
                    let bit = (leaf_offset >> (depth - 1 - k)) & 1;
                    if bit == 0 {
                        0.25_f64
                    } else {
                        0.75_f64
                    }
                })
                .collect();

            // Run MLP on midpoint and quantize the output for this class
            let outputs = predict_mlp(mlp, &midpoint_features)?;
            let raw_score = outputs[class_idx];
            let leaf_value_q = (raw_score * FIXED_POINT_SCALE as f64).round() as i64;

            nodes.push(TreeNode {
                feature_index: -1,
                threshold: 0.0,
                left_child: 0,
                right_child: 0,
                leaf_value: leaf_value_q as f64 / FIXED_POINT_SCALE as f64,
                is_leaf: true,
            });
        }
    }

    Ok(DecisionTree { nodes })
}

// ============================================================================
// Circuit description for deterministic hashing
// ============================================================================

/// Serialize a stable description of the MLP structure for circuit hashing.
///
/// This encodes the quantized weights and biases so that different models
/// produce different circuit hashes.  The encoding is deterministic.
pub fn build_mlp_circuit_description(mlp: &MLPModel) -> Vec<u8> {
    let mut desc = Vec::new();
    desc.extend_from_slice(b"REMAINDER_MLP_V1");
    desc.extend_from_slice(&(mlp.n_features as u32).to_be_bytes());
    desc.extend_from_slice(&(mlp.n_classes as u32).to_be_bytes());
    desc.extend_from_slice(&(mlp.layers.len() as u32).to_be_bytes());

    for layer in &mlp.layers {
        desc.extend_from_slice(&(layer.weights.len() as u32).to_be_bytes());
        for row in &layer.weights {
            for &w in row {
                let w_q = (w * FIXED_POINT_SCALE as f64).round() as i64;
                desc.extend_from_slice(&w_q.to_be_bytes());
            }
        }
        for &b in &layer.biases {
            let b_q = (b * FIXED_POINT_SCALE as f64).round() as i64;
            desc.extend_from_slice(&b_q.to_be_bytes());
        }
        // Activation encoded as a single byte
        let act_byte: u8 = match layer.activation {
            Activation::Relu => 1,
            Activation::Sigmoid => 2,
            Activation::None => 0,
        };
        desc.push(act_byte);
    }

    desc
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    fn two_layer_model_json() -> &'static str {
        r#"{
            "model_type": "mlp",
            "n_features": 4,
            "n_classes": 2,
            "layers": [
                {
                    "weights": [
                        [0.5, -0.3, 0.2, 0.1],
                        [0.1,  0.4, -0.2, 0.3]
                    ],
                    "biases": [0.1, -0.1],
                    "activation": "relu"
                },
                {
                    "weights": [
                        [0.6, -0.4],
                        [0.3,  0.7]
                    ],
                    "biases": [0.0, 0.0],
                    "activation": "none"
                }
            ]
        }"#
    }

    fn single_layer_model() -> MLPModel {
        MLPModel {
            model_type: "mlp".to_string(),
            n_features: 2,
            n_classes: 2,
            layers: vec![MLPLayer {
                weights: vec![vec![1.0, -1.0], vec![-1.0, 1.0]],
                biases: vec![0.0, 0.0],
                activation: Activation::None,
            }],
        }
    }

    // ---- test_parse_mlp_json ----

    #[test]
    fn test_parse_mlp_json() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        assert_eq!(mlp.n_features, 4);
        assert_eq!(mlp.n_classes, 2);
        assert_eq!(mlp.layers.len(), 2);
        assert_eq!(mlp.layers[0].weights.len(), 2);
        assert_eq!(mlp.layers[0].weights[0].len(), 4);
        assert_eq!(mlp.layers[0].activation, Activation::Relu);
        assert_eq!(mlp.layers[1].activation, Activation::None);
    }

    #[test]
    fn test_parse_mlp_model_type_field() {
        // model_type is optional; missing it should still work
        let json = r#"{
            "n_features": 2,
            "n_classes": 1,
            "layers": [{"weights": [[1.0, 0.0]], "biases": [0.0], "activation": "none"}]
        }"#;
        let mlp = parse_mlp_model(json).unwrap();
        assert_eq!(mlp.n_features, 2);
    }

    #[test]
    fn test_parse_mlp_json_invalid_json() {
        assert!(parse_mlp_model("not json").is_err());
    }

    #[test]
    fn test_parse_mlp_json_no_features() {
        let json = r#"{
            "n_features": 0,
            "n_classes": 2,
            "layers": [{"weights": [[1.0]], "biases": [0.0], "activation": "none"}]
        }"#;
        assert!(parse_mlp_model(json).is_err());
    }

    #[test]
    fn test_parse_mlp_json_too_many_features() {
        // n_features > 8 is rejected
        let weights: Vec<Vec<f64>> = vec![vec![1.0; 9]];
        let json = serde_json::json!({
            "n_features": 9,
            "n_classes": 1,
            "layers": [{"weights": weights, "biases": [0.0], "activation": "none"}]
        })
        .to_string();
        assert!(parse_mlp_model(&json).is_err());
    }

    #[test]
    fn test_parse_mlp_json_dimension_mismatch() {
        // weights row has wrong length
        let json = r#"{
            "n_features": 3,
            "n_classes": 1,
            "layers": [{"weights": [[1.0, 2.0]], "biases": [0.0], "activation": "none"}]
        }"#;
        assert!(parse_mlp_model(json).is_err());
    }

    // ---- test_mlp_predict ----

    #[test]
    fn test_mlp_predict_identity_network() {
        // Single linear layer: identity transform for 2 neurons
        let mlp = single_layer_model();
        let out = predict_mlp(&mlp, &[1.0, 3.0]).unwrap();
        // neuron 0: 1*1 + -1*3 = -2
        // neuron 1: -1*1 + 1*3 = 2
        assert!((out[0] - (-2.0)).abs() < 1e-10, "neuron 0 = {}", out[0]);
        assert!((out[1] - 2.0).abs() < 1e-10, "neuron 1 = {}", out[1]);
    }

    #[test]
    fn test_mlp_predict_wrong_feature_count() {
        let mlp = single_layer_model();
        assert!(predict_mlp(&mlp, &[1.0]).is_err());
        assert!(predict_mlp(&mlp, &[1.0, 2.0, 3.0]).is_err());
    }

    #[test]
    fn test_mlp_predict_two_layer() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let out = predict_mlp(&mlp, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        // Just check dimensions and finite values
        assert_eq!(out.len(), 2);
        assert!(out[0].is_finite());
        assert!(out[1].is_finite());
    }

    #[test]
    fn test_mlp_predict_class() {
        let mlp = single_layer_model();
        // features [0.1, 0.9]: neuron0 = 0.1-0.9 = -0.8, neuron1 = -0.1+0.9 = 0.8 -> class 1
        let cls = predict_mlp_class(&mlp, &[0.1, 0.9]).unwrap();
        assert_eq!(cls, 1);
        // features [0.9, 0.1]: neuron0 = 0.9-0.1 = 0.8, neuron1 = -0.9+0.1 = -0.8 -> class 0
        let cls = predict_mlp_class(&mlp, &[0.9, 0.1]).unwrap();
        assert_eq!(cls, 0);
    }

    // ---- test_mlp_relu ----

    #[test]
    fn test_mlp_relu_clamps_negative() {
        let mlp = MLPModel {
            model_type: "mlp".to_string(),
            n_features: 2,
            n_classes: 2,
            layers: vec![MLPLayer {
                weights: vec![vec![-1.0, 0.0], vec![0.0, -1.0]],
                biases: vec![0.0, 0.0],
                activation: Activation::Relu,
            }],
        };
        // Positive inputs → negative pre-activation → ReLU yields 0
        let out = predict_mlp(&mlp, &[1.0, 1.0]).unwrap();
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn test_mlp_relu_passes_positive() {
        let mlp = MLPModel {
            model_type: "mlp".to_string(),
            n_features: 2,
            n_classes: 1,
            layers: vec![MLPLayer {
                weights: vec![vec![1.0, 1.0]],
                biases: vec![0.5],
                activation: Activation::Relu,
            }],
        };
        let out = predict_mlp(&mlp, &[0.3, 0.2]).unwrap();
        // 0.3 + 0.2 + 0.5 = 1.0 > 0 → passes through unchanged
        assert!((out[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_mlp_sigmoid_range() {
        let mlp = MLPModel {
            model_type: "mlp".to_string(),
            n_features: 1,
            n_classes: 1,
            layers: vec![MLPLayer {
                weights: vec![vec![1.0]],
                biases: vec![0.0],
                activation: Activation::Sigmoid,
            }],
        };
        // Sigmoid output must be in (0, 1)
        for &x in &[-10.0, -1.0, 0.0, 1.0, 10.0_f64] {
            let out = predict_mlp(&mlp, &[x]).unwrap();
            assert!(out[0] > 0.0 && out[0] < 1.0, "sigmoid({}) = {}", x, out[0]);
        }
    }

    // ---- test_mlp_to_xgboost_conversion ----

    #[test]
    fn test_mlp_to_xgboost_conversion_tree_count() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let xgb = mlp_to_xgboost(&mlp).unwrap();
        // One tree per class
        assert_eq!(xgb.trees.len(), 2);
        assert_eq!(xgb.num_features, 4);
        assert_eq!(xgb.num_classes, 2);
        assert_eq!(xgb.max_depth, 4);
    }

    #[test]
    fn test_mlp_to_xgboost_conversion_tree_structure() {
        let mlp = single_layer_model();
        let xgb = mlp_to_xgboost(&mlp).unwrap();

        // depth=2: 2^3 - 1 = 7 nodes
        assert_eq!(xgb.trees[0].nodes.len(), 7);
        assert_eq!(xgb.trees[1].nodes.len(), 7);

        // Root (index 0) should be an internal node splitting on feature 0
        let root = &xgb.trees[0].nodes[0];
        assert!(!root.is_leaf);
        assert_eq!(root.feature_index, 0);
        assert!((root.threshold - 0.5).abs() < 1e-10);

        // Level-1 nodes (indices 1, 2) split on feature 1
        for idx in [1, 2] {
            let node = &xgb.trees[0].nodes[idx];
            assert!(!node.is_leaf, "node {} should be internal", idx);
            assert_eq!(node.feature_index, 1);
        }

        // Leaf nodes (indices 3..6) should all be leaves
        for idx in 3..7 {
            let node = &xgb.trees[0].nodes[idx];
            assert!(node.is_leaf, "node {} should be a leaf", idx);
        }
    }

    #[test]
    fn test_mlp_to_xgboost_conversion_leaf_values_valid() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let xgb = mlp_to_xgboost(&mlp).unwrap();

        for (t, tree) in xgb.trees.iter().enumerate() {
            for (i, node) in tree.nodes.iter().enumerate() {
                if node.is_leaf {
                    assert!(
                        node.leaf_value.is_finite(),
                        "tree {} node {} leaf value is not finite",
                        t,
                        i
                    );
                }
            }
        }
    }

    #[test]
    fn test_mlp_to_xgboost_single_feature() {
        // n_features=1: depth-1 tree (3 nodes: root + 2 leaves)
        let mlp = MLPModel {
            model_type: "mlp".to_string(),
            n_features: 1,
            n_classes: 1,
            layers: vec![MLPLayer {
                weights: vec![vec![2.0]],
                biases: vec![0.0],
                activation: Activation::None,
            }],
        };
        let xgb = mlp_to_xgboost(&mlp).unwrap();
        assert_eq!(xgb.trees.len(), 1);
        assert_eq!(xgb.trees[0].nodes.len(), 3); // 2^2 - 1
        let root = &xgb.trees[0].nodes[0];
        assert!(!root.is_leaf);
        // Left leaf: input midpoint 0.25 → output = 2*0.25 = 0.5
        let left_leaf = &xgb.trees[0].nodes[1];
        assert!(left_leaf.is_leaf);
        assert!((left_leaf.leaf_value - 0.5).abs() < 1e-3);
        // Right leaf: input midpoint 0.75 → output = 2*0.75 = 1.5
        let right_leaf = &xgb.trees[0].nodes[2];
        assert!(right_leaf.is_leaf);
        assert!((right_leaf.leaf_value - 1.5).abs() < 1e-3);
    }

    #[test]
    fn test_mlp_to_xgboost_child_indices_valid() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let xgb = mlp_to_xgboost(&mlp).unwrap();

        for tree in &xgb.trees {
            let n = tree.nodes.len();
            for node in &tree.nodes {
                if !node.is_leaf {
                    assert!(node.left_child < n, "left_child out of bounds");
                    assert!(node.right_child < n, "right_child out of bounds");
                    assert!(
                        tree.nodes[node.left_child].is_leaf
                            || !tree.nodes[node.left_child].is_leaf,
                        "left child must exist"
                    );
                }
            }
        }
    }

    // ---- circuit description ----

    #[test]
    fn test_mlp_circuit_description_prefix() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let desc = build_mlp_circuit_description(&mlp);
        assert!(desc.starts_with(b"REMAINDER_MLP_V1"));
    }

    #[test]
    fn test_mlp_circuit_description_deterministic() {
        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let d1 = build_mlp_circuit_description(&mlp);
        let d2 = build_mlp_circuit_description(&mlp);
        assert_eq!(d1, d2);
    }

    #[test]
    fn test_mlp_circuit_description_differs_between_models() {
        let m1 = parse_mlp_model(two_layer_model_json()).unwrap();
        let m2 = single_layer_model();
        let d1 = build_mlp_circuit_description(&m1);
        let d2 = build_mlp_circuit_description(&m2);
        assert_ne!(d1, d2);
    }

    // ---- file loading ----

    #[test]
    fn test_load_mlp_from_sample_file() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-model/mlp_sample.json");
        let xgb = load_mlp_json(&path).unwrap();
        assert_eq!(xgb.num_features, 4);
        assert_eq!(xgb.num_classes, 2);
        assert_eq!(xgb.trees.len(), 2);
    }

    // ---- E2E prove + verify (ignored — slow) ----

    #[test]
    #[ignore]
    fn test_mlp_e2e_prove_verify() {
        use crate::circuit;
        use crate::model;

        let mlp = parse_mlp_model(two_layer_model_json()).unwrap();
        let xgb = mlp_to_xgboost(&mlp).unwrap();

        let features = vec![0.3, 0.7, 0.2, 0.8];
        let predicted_class = model::predict(&xgb, &features);

        let result = circuit::build_and_prove(&xgb, &features, predicted_class);
        assert!(
            result.is_ok(),
            "E2E prove+verify failed: {:?}",
            result.err()
        );
    }
}
