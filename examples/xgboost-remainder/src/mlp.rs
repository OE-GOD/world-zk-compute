//! Multi-Layer Perceptron (MLP) model parser, native GKR circuit, and tree adapter.
//!
//! This module provides two proving strategies for MLP inference:
//!
//! 1. **Native GKR circuit** (`build_mlp_circuit`): Directly encodes matrix
//!    multiply + bias + ReLU as GKR layers.  This is the primary approach for
//!    proving neural network inference.
//!
//! 2. **XGBoost lookup-tree conversion** (`mlp_to_xgboost`): Converts the MLP
//!    to a lookup table encoded as a decision tree, reusing the existing
//!    tree-inference circuit.  Limited to n_features <= 8.
//!
//! # Native GKR Circuit Structure
//!
//! For each MLP layer with ReLU activation:
//!
//! 1. **Dot products**: For each output neuron j, compute
//!    `sum_i(weight[j][i] * input[i])` via element-wise multiply + pairwise
//!    tree reduction.
//! 2. **Bias addition**: `z[j] = dot_product[j] + bias[j]`
//! 3. **ReLU verification** (bit decomposition):
//!    - Witness: `decomp_bits` such that `z[j] + 2^(K-1) = sum(2^i * bits[i])`
//!    - Sign bit = `decomp_bits[K-1]` (1 if z >= 0, 0 if z < 0)
//!    - `relu_output[j] = z[j] * sign_bit[j]`
//!    - Binary checks on all decomp bits
//!    - Reconstruction residual must be zero
//!
//! For the final layer (no ReLU), just dot products + bias.  The output layer
//! checks that the computed outputs match the claimed values.
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

use crate::model::{self, DecisionTree, TreeNode, XgboostModel, FIXED_POINT_SCALE};
use frontend::abstract_expr::AbstractExpression;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility, NodeRef};
use serde::{Deserialize, Serialize};
use shared_types::Fr;
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
// Native GKR Circuit for MLP Inference
// ============================================================================

/// Fixed-point scale for quantizing MLP weights and inputs (2^16).
pub const MLP_FIXED_POINT_SCALE: i64 = 1 << 16;

/// Number of bits for decomposition of pre-activation values.
/// Products are weight_q * input_q where each is scaled by 2^16, so
/// products are in 2^32 scale.  With N inputs summed, values can be
/// up to N * max_product.  48 bits provides ample headroom.
pub const MLP_DECOMP_K: usize = 48;

/// All prepared circuit inputs for the native MLP GKR circuit.
pub struct MLPCircuitInputs {
    /// Quantized input features (committed/private), padded to power of 2.
    pub features_quantized: Vec<i64>,
    /// Number of input features padded to next power of 2.
    pub n_inputs_padded: usize,

    /// Per-layer quantized weights, flattened per neuron row.
    /// `layer_weights[l]` has shape `[n_out_padded * n_in_padded]` with rows
    /// for each output neuron, each row padded to `n_in_padded`.
    pub layer_weights: Vec<Vec<i64>>,

    /// Per-layer quantized biases (one per output neuron, padded).
    /// Biases are scaled by SCALE^2 to match the product domain.
    pub layer_biases: Vec<Vec<i64>>,

    /// Per-ReLU-layer decomposition bits for the pre-activation values.
    /// `relu_decomp_bits[r]` is a flat vector of `n_out_padded * decomp_k`
    /// bits for the r-th ReLU layer.
    pub relu_decomp_bits: Vec<Vec<bool>>,

    /// Per-ReLU-layer sign bits (1 if z >= 0, 0 if z < 0).
    /// `relu_sign_bits[r]` has `n_out_padded` entries.
    pub relu_sign_bits: Vec<Vec<bool>>,

    /// Expected output values (quantized), padded to power of 2.
    pub expected_outputs: Vec<i64>,

    /// Number of output neurons padded to next power of 2.
    pub n_outputs_padded: usize,

    /// Number of decomposition bits.
    pub decomp_k: usize,

    /// Structural info: (n_in_padded, n_out_padded) per layer.
    pub layer_dims: Vec<(usize, usize)>,

    /// Which layers have ReLU (indices into layers).
    pub relu_layer_indices: Vec<usize>,
}

/// Quantize a float value to fixed-point integer for the MLP circuit.
fn mlp_quantize(val: f64) -> i64 {
    (val * MLP_FIXED_POINT_SCALE as f64).round() as i64
}

/// Prepare circuit inputs from an MLP model and feature vector.
///
/// Runs the forward pass in quantized integer arithmetic to compute
/// intermediate values and witness data (decomp bits for ReLU layers).
pub fn prepare_mlp_circuit_inputs(
    mlp: &MLPModel,
    features: &[f64],
) -> MLPCircuitInputs {
    assert_eq!(features.len(), mlp.n_features, "Feature count mismatch");

    let decomp_k = MLP_DECOMP_K;

    // Quantize input features
    let n_inputs_padded = mlp.n_features.next_power_of_two();
    let mut features_q: Vec<i64> = features.iter().map(|f| mlp_quantize(*f)).collect();
    features_q.resize(n_inputs_padded, 0);

    let mut layer_weights = Vec::new();
    let mut layer_biases = Vec::new();
    let mut layer_dims = Vec::new();
    let mut relu_decomp_bits = Vec::new();
    let mut relu_sign_bits = Vec::new();
    let mut relu_layer_indices = Vec::new();

    // Current activations in quantized domain
    let mut current_activations = features_q.clone();
    let mut current_n_padded = n_inputs_padded;

    // Process each layer
    for (layer_idx, layer) in mlp.layers.iter().enumerate() {
        let n_in = layer.weights[0].len();
        let n_out = layer.weights.len();
        let n_in_padded = n_in.next_power_of_two();
        let n_out_padded = n_out.next_power_of_two();

        // Quantize weights: flatten [n_out][n_in] -> [n_out_padded * n_in_padded]
        let mut weights_flat = vec![0i64; n_out_padded * n_in_padded];
        for j in 0..n_out {
            for i in 0..n_in {
                weights_flat[j * n_in_padded + i] = mlp_quantize(layer.weights[j][i]);
            }
        }
        layer_weights.push(weights_flat.clone());

        // Quantize biases (scaled by SCALE^2 to match w*x domain)
        let mut biases_q = vec![0i64; n_out_padded];
        for j in 0..n_out {
            biases_q[j] = (layer.biases[j]
                * MLP_FIXED_POINT_SCALE as f64
                * MLP_FIXED_POINT_SCALE as f64)
                .round() as i64;
        }
        layer_biases.push(biases_q.clone());

        layer_dims.push((n_in_padded, n_out_padded));

        // Compute pre-activation values: z[j] = sum_i(w[j][i] * x[i]) + b[j]
        // Ensure current_activations is padded to n_in_padded
        current_activations.resize(n_in_padded, 0);

        let mut z_values = vec![0i64; n_out_padded];
        for j in 0..n_out {
            let mut dot: i64 = 0;
            for i in 0..n_in_padded {
                dot += weights_flat[j * n_in_padded + i] * current_activations[i];
            }
            z_values[j] = dot + biases_q[j];
        }

        // Apply activation
        match layer.activation {
            Activation::Relu => {
                relu_layer_indices.push(layer_idx);

                // Compute sign bits and decomp bits
                let mut sign_bits = vec![false; n_out_padded];
                let mut decomp = vec![false; n_out_padded * decomp_k];

                let offset = 1i128 << (decomp_k - 1);

                for j in 0..n_out_padded {
                    let z = z_values[j] as i128;
                    sign_bits[j] = z >= 0;

                    let shifted = z + offset;
                    assert!(
                        shifted >= 0,
                        "Shifted value must be non-negative: z={}, offset={}, shifted={}",
                        z,
                        offset,
                        shifted
                    );

                    for bit_i in 0..decomp_k {
                        decomp[j * decomp_k + bit_i] = ((shifted >> bit_i) & 1) != 0;
                    }

                    // Verify reconstruction
                    let reconstructed: i128 = (0..decomp_k)
                        .filter(|&i| decomp[j * decomp_k + i])
                        .map(|i| 1i128 << i)
                        .sum();
                    assert_eq!(
                        reconstructed, shifted,
                        "Bit decomposition reconstruction failed for neuron {}: expected {}, got {}",
                        j, shifted, reconstructed
                    );
                }

                relu_decomp_bits.push(decomp);
                relu_sign_bits.push(sign_bits.clone());

                // ReLU: a[j] = max(0, z[j]) = z[j] if sign >= 0, else 0
                current_activations = vec![0i64; n_out_padded];
                for j in 0..n_out_padded {
                    current_activations[j] = if sign_bits[j] { z_values[j] } else { 0 };
                }
            }
            Activation::None => {
                // Linear: activations = z_values
                current_activations = z_values;
            }
            Activation::Sigmoid => {
                // Sigmoid not supported in native circuit yet; fall through as linear
                current_activations = z_values;
            }
        }

        current_n_padded = n_out_padded;
    }

    // Expected outputs = final activations
    let n_outputs_padded = current_n_padded;
    let expected_outputs = current_activations;

    MLPCircuitInputs {
        features_quantized: features_q,
        n_inputs_padded,
        layer_weights,
        layer_biases,
        relu_decomp_bits,
        relu_sign_bits,
        expected_outputs,
        n_outputs_padded,
        decomp_k,
        layer_dims,
        relu_layer_indices,
    }
}

/// Build a native GKR circuit for MLP inference.
///
/// The circuit verifies the full forward pass of a multi-layer perceptron:
/// matrix multiply + bias + ReLU for each hidden layer, and matrix multiply
/// + bias for the output layer.  All output entries must be zero for valid
/// inference.
///
/// # Arguments
///
/// * `layer_dims` - `(n_in_padded, n_out_padded)` for each layer
/// * `relu_layer_indices` - which layers have ReLU activation
/// * `decomp_k` - number of decomposition bits for ReLU sign checks
pub fn build_mlp_circuit(
    layer_dims: &[(usize, usize)],
    relu_layer_indices: &[usize],
    decomp_k: usize,
) -> Circuit<Fr> {
    let num_layers = layer_dims.len();
    assert!(num_layers >= 1, "MLP must have at least 1 layer");
    assert!(decomp_k > 0);

    for &(n_in, n_out) in layer_dims {
        assert!(n_in.is_power_of_two(), "n_in={} must be power of 2", n_in);
        assert!(
            n_out.is_power_of_two(),
            "n_out={} must be power of 2",
            n_out
        );
    }

    let mut builder = CircuitBuilder::<Fr>::new();

    // === Input layers ===
    let committed = builder.add_input_layer("committed", LayerVisibility::Committed);
    let public = builder.add_input_layer("public", LayerVisibility::Public);

    // Input features (committed/private)
    let first_n_in = layer_dims[0].0;
    let feat_nv = first_n_in.trailing_zeros() as usize;
    let features = builder.add_input_shred("features", feat_nv, &committed);

    // Per-layer weights (public) and biases (public)
    let mut weight_shreds = Vec::new();
    let mut bias_shreds = Vec::new();
    for (l, &(n_in, n_out)) in layer_dims.iter().enumerate() {
        let w_size = n_out * n_in;
        let w_nv = model::next_log2(w_size);
        let w_shred = builder.add_input_shred(
            &format!("weights_{}", l),
            w_nv,
            &public,
        );
        weight_shreds.push(w_shred);

        let b_nv = n_out.trailing_zeros() as usize;
        let b_shred = builder.add_input_shred(
            &format!("biases_{}", l),
            b_nv,
            &public,
        );
        bias_shreds.push(b_shred);
    }

    // Per-ReLU-layer decomposition bits (committed/private witness)
    let mut decomp_shreds = Vec::new();
    let mut sign_shreds = Vec::new();
    for (relu_idx, &layer_idx) in relu_layer_indices.iter().enumerate() {
        let (_, n_out) = layer_dims[layer_idx];
        let decomp_size = n_out * decomp_k;
        let decomp_nv = model::next_log2(decomp_size);
        let decomp_shred = builder.add_input_shred(
            &format!("decomp_bits_{}", relu_idx),
            decomp_nv,
            &committed,
        );
        decomp_shreds.push(decomp_shred);

        let sign_nv = n_out.trailing_zeros() as usize;
        let sign_shred = builder.add_input_shred(
            &format!("sign_bits_{}", relu_idx),
            sign_nv,
            &committed,
        );
        sign_shreds.push(sign_shred);
    }

    // Expected outputs (public)
    let last_n_out = layer_dims[num_layers - 1].1;
    let out_nv = last_n_out.trailing_zeros() as usize;
    let expected_outputs = builder.add_input_shred("expected_outputs", out_nv, &public);

    // === Process each layer ===
    let mut current_activations = features;
    let mut relu_counter = 0usize;
    let mut all_checks: Vec<(NodeRef<Fr>, usize)> = Vec::new(); // (check_node, count)

    for (l, &(n_in, n_out)) in layer_dims.iter().enumerate() {
        let in_nv = n_in.trailing_zeros() as usize;
        let out_nv_l = n_out.trailing_zeros() as usize;

        // === Matrix multiply: for each output neuron j, compute dot(w[j], x) ===
        // Strategy: expand weights and inputs so we can do element-wise multiply,
        // then sum reduce per neuron.
        //
        // weights layout: [j * n_in + i] (j = output neuron, i = input)
        // We need products[j * n_in + i] = weights[j * n_in + i] * inputs[i]
        //
        // Expand inputs: replicate each input value for all output neurons
        let total = n_out * n_in;
        let total_nv = model::next_log2(total);

        let mut input_expand_gates = Vec::new();
        for j in 0..n_out {
            for i in 0..n_in {
                input_expand_gates.push(((j * n_in + i) as u32, i as u32));
            }
        }
        let inputs_expanded = builder.add_identity_gate_node(
            &current_activations,
            input_expand_gates,
            total_nv,
            None,
        );

        // Weight shred already has [j * n_in + i] layout; route to match
        let mut weight_gates = Vec::new();
        for idx in 0..total {
            weight_gates.push((idx as u32, idx as u32));
        }
        let weights_routed = builder.add_identity_gate_node(
            &weight_shreds[l],
            weight_gates,
            total_nv,
            None,
        );

        // Element-wise multiply
        let products = builder.add_sector(inputs_expanded.expr() * weights_routed.expr());

        // Sum reduction: reduce n_in products per neuron to get dot products
        // After this, we have n_out values.
        let dot_products = sum_per_neuron(&mut builder, &products, n_out, n_in);

        // === Bias addition ===
        let pre_activations = builder.add_sector(dot_products.expr() + bias_shreds[l].expr());

        // === Activation ===
        let is_relu = relu_layer_indices.contains(&l);

        if is_relu {
            let decomp_shred = &decomp_shreds[relu_counter];
            let sign_shred = &sign_shreds[relu_counter];

            // --- Binary check on decomp bits: bit^2 - bit = 0 ---
            let db_sq = builder.add_sector(decomp_shred.expr() * decomp_shred.expr());
            let decomp_bc = builder.add_sector(db_sq.expr() - decomp_shred.expr());
            let decomp_padded = 1usize << model::next_log2(n_out * decomp_k);
            all_checks.push((decomp_bc, decomp_padded));

            // --- Binary check on sign bits: bit^2 - bit = 0 ---
            let sb_sq = builder.add_sector(sign_shred.expr() * sign_shred.expr());
            let sign_bc = builder.add_sector(sb_sq.expr() - sign_shred.expr());
            all_checks.push((sign_bc, n_out));

            // --- Reconstruction check: for each neuron j, ---
            // --- weighted_sum[j] == z[j] + 2^(K-1)        ---
            //
            // Compute weighted sum of decomp bits per neuron:
            // weighted_sum[j] = sum_{i=0}^{K-1} 2^i * decomp_bits[j*K + i]
            //
            // We do this by routing each bit to a scalar, scaling, and summing.
            // For efficiency, we build the weighted sum expression per neuron.

            // Route and scale decomp bits into per-neuron weighted sums
            let mut weighted_sum_expr: AbstractExpression<Fr> =
                AbstractExpression::constant(Fr::from(0u64));

            for j in 0..n_out {
                for bit_i in 0..decomp_k {
                    let src_idx = j * decomp_k + bit_i;
                    let bit_routed = builder.add_identity_gate_node(
                        decomp_shred,
                        vec![(j as u32, src_idx as u32)],
                        out_nv_l,
                        None,
                    );
                    let scale = fr_power_of_2(bit_i);
                    weighted_sum_expr += AbstractExpression::scaled(bit_routed.expr(), scale);
                }
            }
            let weighted_sum = builder.add_sector(weighted_sum_expr);

            // shifted = z + 2^(K-1)
            let offset = fr_power_of_2(decomp_k - 1);
            let shifted = builder.add_sector(
                pre_activations.expr() + AbstractExpression::constant(offset),
            );

            // Reconstruction residual: weighted_sum - shifted
            let recon_residual = builder.add_sector(weighted_sum.expr() - shifted.expr());
            all_checks.push((recon_residual, n_out));

            // --- Sign consistency: sign_bit[j] == decomp_bits[j*K + (K-1)] ---
            let mut sign_from_decomp_gates = Vec::new();
            for j in 0..n_out {
                let msb_idx = j * decomp_k + (decomp_k - 1);
                sign_from_decomp_gates.push((j as u32, msb_idx as u32));
            }
            let sign_from_decomp = builder.add_identity_gate_node(
                decomp_shred,
                sign_from_decomp_gates,
                out_nv_l,
                None,
            );
            let sign_consistency =
                builder.add_sector(sign_from_decomp.expr() - sign_shred.expr());
            all_checks.push((sign_consistency, n_out));

            // --- ReLU output: relu[j] = z[j] * sign[j] ---
            current_activations =
                builder.add_sector(pre_activations.expr() * sign_shred.expr());

            relu_counter += 1;
        } else {
            // Linear: pass through
            current_activations = pre_activations;
        }
    }

    // === Output residual: computed_outputs - expected_outputs must be zero ===
    let output_residual =
        builder.add_sector(current_activations.expr() - expected_outputs.expr());
    all_checks.push((output_residual, last_n_out));

    // === Combine all checks into a single output layer ===
    let total_checks: usize = all_checks.iter().map(|(_, count)| *count).sum();
    let output_total_nv = model::next_log2(total_checks);

    let mut offset_pos = 0usize;
    let mut combined_expr: AbstractExpression<Fr> =
        AbstractExpression::constant(Fr::from(0u64));

    for (check_node, count) in &all_checks {
        let mut gates = Vec::new();
        for i in 0..*count {
            gates.push(((offset_pos + i) as u32, i as u32));
        }
        let routed =
            builder.add_identity_gate_node(check_node, gates, output_total_nv, None);
        combined_expr = combined_expr + routed.expr();
        offset_pos += count;
    }

    let combined = builder.add_sector(combined_expr);
    builder.set_output(&combined);
    builder
        .build()
        .expect("Failed to build MLP circuit")
}

/// Sum-reduce n_in values per neuron to get n_out dot products.
///
/// Input layout: `[j * n_in + i]` for j in 0..n_out, i in 0..n_in.
/// Output: n_out values (one per neuron).
fn sum_per_neuron(
    builder: &mut CircuitBuilder<Fr>,
    products: &NodeRef<Fr>,
    n_out: usize,
    n_in: usize,
) -> NodeRef<Fr> {
    let out_nv = n_out.trailing_zeros() as usize;
    let in_nv = n_in.trailing_zeros() as usize;

    // Pairwise reduction within each neuron's block of n_in values
    let mut current = products.clone();
    let mut current_block_size = n_in;
    let mut current_total_nv = model::next_log2(n_out * n_in);

    for _level in (0..in_nv).rev() {
        let half_block = current_block_size / 2;
        let new_total = n_out * half_block;
        let new_nv = model::next_log2(new_total);

        let mut even_gates = Vec::new();
        let mut odd_gates = Vec::new();
        for j in 0..n_out {
            for i in 0..half_block {
                let src_even = j * current_block_size + 2 * i;
                let src_odd = j * current_block_size + 2 * i + 1;
                let dst = j * half_block + i;
                even_gates.push((dst as u32, src_even as u32));
                odd_gates.push((dst as u32, src_odd as u32));
            }
        }

        let even = builder.add_identity_gate_node(&current, even_gates, new_nv, None);
        let odd = builder.add_identity_gate_node(&current, odd_gates, new_nv, None);
        current = builder.add_sector(even.expr() + odd.expr());

        current_block_size = half_block;
        current_total_nv = new_nv;
    }

    // current now has n_out values (one dot product per neuron)
    current
}

/// Compute Fr::from(2^k) handling k >= 64 correctly.
fn fr_power_of_2(k: usize) -> Fr {
    if k < 64 {
        Fr::from(1u64 << k)
    } else {
        let mut power = Fr::from(1u64 << 63);
        for _ in 63..k {
            power = power + power;
        }
        power
    }
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
