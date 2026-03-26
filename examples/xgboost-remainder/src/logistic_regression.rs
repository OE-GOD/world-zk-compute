//! Logistic regression model representation, inference, and GKR circuit.
//!
//! The simplest ML model for verifiable inference: a dot product of weights
//! and features, plus a bias term, followed by a sigmoid approximation
//! (sign-based binary classification).
//!
//! Model format (JSON):
//! ```json
//! {
//!   "weights": [0.5, -0.3, 0.8, 0.1],
//!   "bias": -0.2,
//!   "threshold": 0.5,
//!   "feature_names": ["age", "income", "score", "history"]
//! }
//! ```
//!
//! # GKR Circuit Structure
//!
//! The circuit proves that the dot product of committed (private) features
//! with public weights, plus a public bias, produces a logit whose sign
//! matches the claimed binary classification output.
//!
//! Layers:
//! 1. **Element-wise multiply**: `products[i] = weights[i] * features[i]`
//! 2. **Sum reduction**: pairwise tree reduction to compute `dot_product`
//! 3. **Bias addition**: `logit = dot_product + bias`
//! 4. **Bit decomposition check**: verifies `logit + offset = sum(2^i * decomp_bits[i])`
//! 5. **Sign consistency**: sign bit of decomposition must match expected class
//! 6. **Binary checks**: all bits (decomp_bits) must be 0 or 1
//!
//! All output positions must evaluate to zero for a valid proof.

use crate::model;
use anyhow::Result;
use frontend::abstract_expr::AbstractExpression;
use frontend::layouter::builder::{Circuit, CircuitBuilder, LayerVisibility, NodeRef};
use hyrax::gkr::verify_hyrax_proof;
use hyrax::utils::vandermonde::VandermondeInverse;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use shared_types::config::{GKRCircuitProverConfig, GKRCircuitVerifierConfig};
use shared_types::pedersen::PedersenCommitter;
use shared_types::transcript::ec_transcript::ECTranscript;
use shared_types::transcript::poseidon_sponge::PoseidonSponge;
use shared_types::{
    perform_function_under_prover_config, perform_function_under_verifier_config, Bn256Point, Fq,
    Fr,
};
use std::path::Path;

/// Fixed-point scale for quantizing weights and features (2^16).
pub const FIXED_POINT_SCALE: i64 = 1 << 16;

/// Number of bits for decomposition of the logit value.
/// Must be large enough to represent the range of possible logit values.
/// With 2^16 scale and reasonable feature magnitudes, 32 bits is sufficient.
pub const DECOMP_K: usize = 32;

/// A logistic regression model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogisticRegressionModel {
    /// Weight vector (one per feature).
    pub weights: Vec<f64>,
    /// Bias (intercept) term.
    #[serde(default)]
    pub bias: f64,
    /// Classification threshold (default 0.5).
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    /// Optional feature names for display.
    #[serde(default)]
    pub feature_names: Vec<String>,
}

fn default_threshold() -> f64 {
    0.5
}

/// Inference result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRegResult {
    /// Raw dot product + bias (before sigmoid).
    pub logit: f64,
    /// Probability (sigmoid of logit).
    pub probability: f64,
    /// Predicted class (0 or 1).
    pub predicted_class: u32,
}

impl LogisticRegressionModel {
    /// Load a model from a JSON file.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        Self::from_json(&data)
    }

    /// Parse a model from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let model: Self =
            serde_json::from_str(json).map_err(|e| format!("failed to parse model: {}", e))?;
        if model.weights.is_empty() {
            return Err("model has no weights".to_string());
        }
        Ok(model)
    }

    /// Number of features.
    pub fn num_features(&self) -> usize {
        self.weights.len()
    }

    /// Run inference on a feature vector.
    pub fn predict(&self, features: &[f64]) -> Result<LogRegResult, String> {
        if features.len() != self.weights.len() {
            return Err(format!(
                "expected {} features, got {}",
                self.weights.len(),
                features.len()
            ));
        }

        let logit: f64 = self
            .weights
            .iter()
            .zip(features.iter())
            .map(|(w, f)| w * f)
            .sum::<f64>()
            + self.bias;

        let probability = sigmoid(logit);
        let predicted_class = if probability >= self.threshold { 1 } else { 0 };

        Ok(LogRegResult {
            logit,
            probability,
            predicted_class,
        })
    }

    /// Convert to the common XgboostModel format for circuit compilation.
    ///
    /// Creates a trivial 1-tree model where the "tree" is just a single leaf
    /// with the precomputed dot product result. This allows reusing the
    /// existing verification pipeline.
    pub fn to_xgboost_model(&self) -> crate::model::XgboostModel {
        use crate::model::{DecisionTree, TreeNode, XgboostModel};

        // Single tree with one leaf = the logit value is computed externally
        let tree = DecisionTree {
            nodes: vec![TreeNode {
                feature_index: -1,
                threshold: 0.0,
                is_leaf: true,
                leaf_value: 0.0, // Placeholder -- actual value computed at inference
                left_child: 0,
                right_child: 0,
            }],
        };

        XgboostModel {
            num_features: self.weights.len(),
            num_classes: 2,
            max_depth: 0,
            trees: vec![tree],
            base_score: self.bias,
        }
    }
}

/// Sigmoid function.
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Quantize a float to fixed-point integer for circuit computation.
pub fn quantize_weights(weights: &[f64], scale: i64) -> Vec<i64> {
    weights
        .iter()
        .map(|w| (w * scale as f64).round() as i64)
        .collect()
}

// ========================================================================
// GKR Circuit for Logistic Regression
// ========================================================================

/// All prepared circuit inputs for the logistic regression circuit.
pub struct LogRegCircuitInputs {
    /// Quantized feature values (committed/private), padded to power of 2.
    pub features_quantized: Vec<i64>,
    /// Quantized weight values (public), padded to power of 2.
    pub weights_quantized: Vec<i64>,
    /// Quantized bias (public), scalar.
    pub bias_quantized: i64,
    /// Expected classification output: 1 if logit >= 0, else 0.
    pub expected_class: i64,
    /// Bit decomposition witness for the shifted logit (committed/private).
    pub decomp_bits: Vec<bool>,
    /// Number of features padded to next power of 2.
    pub n_padded: usize,
    /// Number of decomposition bits.
    pub decomp_k: usize,
}

/// Prepare circuit inputs from a logistic regression model and feature vector.
pub fn prepare_logreg_inputs(
    model: &LogisticRegressionModel,
    features: &[f64],
) -> LogRegCircuitInputs {
    assert_eq!(
        features.len(),
        model.weights.len(),
        "Feature count mismatch"
    );

    let n = model.weights.len();
    let n_padded = n.next_power_of_two();
    let decomp_k = DECOMP_K;

    // Quantize features and weights using fixed-point scale
    let mut features_q: Vec<i64> = features
        .iter()
        .map(|f| (f * FIXED_POINT_SCALE as f64).round() as i64)
        .collect();
    features_q.resize(n_padded, 0);

    let mut weights_q: Vec<i64> = model
        .weights
        .iter()
        .map(|w| (w * FIXED_POINT_SCALE as f64).round() as i64)
        .collect();
    weights_q.resize(n_padded, 0);

    // Bias is scaled by FIXED_POINT_SCALE^2 to match the product scale
    // (since products are weight_q * feature_q = value * SCALE^2)
    let bias_q = (model.bias * (FIXED_POINT_SCALE as f64) * (FIXED_POINT_SCALE as f64)).round()
        as i64;

    // Compute actual logit in quantized domain
    let dot_product: i64 = features_q
        .iter()
        .zip(weights_q.iter())
        .map(|(f, w)| f * w)
        .sum();
    let logit_q = dot_product + bias_q;

    // Classification: logit >= 0 => class 1, else class 0
    let expected_class = if logit_q >= 0 { 1i64 } else { 0i64 };

    // Bit decomposition of shifted logit: logit + offset
    // offset = 2^(K-1) ensures the value is non-negative for decomposition
    let offset = 1i64 << (decomp_k - 1);
    let shifted = logit_q + offset;
    assert!(
        shifted >= 0,
        "Shifted logit must be non-negative: logit_q={}, offset={}, shifted={}",
        logit_q,
        offset,
        shifted
    );

    let mut decomp_bits = vec![false; decomp_k];
    for i in 0..decomp_k {
        decomp_bits[i] = ((shifted >> i) & 1) != 0;
    }

    // Verify reconstruction
    let reconstructed: i64 = (0..decomp_k)
        .filter(|&i| decomp_bits[i])
        .map(|i| 1i64 << i)
        .sum();
    assert_eq!(
        reconstructed, shifted,
        "Bit decomposition reconstruction failed"
    );

    LogRegCircuitInputs {
        features_quantized: features_q,
        weights_quantized: weights_q,
        bias_quantized: bias_q,
        expected_class,
        decomp_bits,
        n_padded,
        decomp_k,
    }
}

/// Build a GKR circuit for logistic regression inference verification.
///
/// The circuit verifies:
/// 1. Element-wise multiply: products[i] = weights[i] * features[i]
/// 2. Sum reduction: dot_product = sum(products[i])
/// 3. Bias addition: logit = dot_product + bias
/// 4. Bit decomposition: logit + offset = sum(2^i * bits[i])
/// 5. Sign consistency: top bit (sign bit) matches expected class
/// 6. Binary checks: all decomp bits are 0 or 1
///
/// All output entries must be zero for valid inference.
pub fn build_logreg_circuit(n_padded: usize, decomp_k: usize) -> Circuit<Fr> {
    assert!(n_padded.is_power_of_two());
    assert!(n_padded >= 1);
    assert!(decomp_k > 0);

    let feat_nv = n_padded.trailing_zeros() as usize;
    let decomp_nv = model::next_log2(decomp_k);
    let decomp_padded = 1usize << decomp_nv;

    let mut builder = CircuitBuilder::<Fr>::new();

    // === Input layers ===
    let committed = builder.add_input_layer("committed", LayerVisibility::Committed);
    let public = builder.add_input_layer("public", LayerVisibility::Public);

    // Committed inputs: features (private), decomp_bits (private witness)
    let features = builder.add_input_shred("features", feat_nv, &committed);
    let decomp_bits_input = builder.add_input_shred("decomp_bits", decomp_nv, &committed);

    // Public inputs: weights (model), bias (scalar), expected_class (scalar)
    let weights = builder.add_input_shred("weights", feat_nv, &public);
    let bias = builder.add_input_shred("bias", 0, &public);
    let expected_class = builder.add_input_shred("expected_class", 0, &public);

    // === Layer 1: Element-wise multiply ===
    // products[i] = weights[i] * features[i]
    let products = builder.add_sector(weights.expr() * features.expr());

    // === Layer 2: Sum reduction (pairwise tree) ===
    // Reduces n_padded products down to a single scalar (dot product)
    let dot_product = aggregate_sum_logreg(&mut builder, &products, feat_nv);

    // === Layer 3: Add bias ===
    // logit = dot_product + bias
    let logit = builder.add_sector(dot_product.expr() + bias.expr());

    // === Layer 4: Bit decomposition verification ===
    // Check that logit + offset = sum(2^i * decomp_bits[i])
    // offset = 2^(K-1) to ensure non-negative representation

    // Binary check on decomp bits: bit^2 - bit must be zero
    let db_sq = builder.add_sector(decomp_bits_input.expr() * decomp_bits_input.expr());
    let decomp_bc = builder.add_sector(db_sq.expr() - decomp_bits_input.expr());

    // Weighted sum of decomp bits: sum(2^i * bit_i)
    let mut weighted_sum_expr: AbstractExpression<Fr> =
        AbstractExpression::constant(Fr::from(0u64));
    for bit_i in 0..decomp_k {
        // Route bit_i from the decomp input to a scalar
        let bit_routed =
            builder.add_identity_gate_node(&decomp_bits_input, vec![(0, bit_i as u32)], 0, None);
        let scale = if bit_i < 64 {
            Fr::from(1u64 << bit_i)
        } else {
            // For bit positions >= 64, we need to build the power manually
            let mut power = Fr::from(1u64 << 63);
            for _ in 63..bit_i {
                power = power + power; // double
            }
            power
        };
        weighted_sum_expr += AbstractExpression::scaled(bit_routed.expr(), scale);
    }
    let weighted_sum = builder.add_sector(weighted_sum_expr);

    // shifted_logit = logit + offset
    let offset = if decomp_k - 1 < 64 {
        Fr::from(1u64 << (decomp_k - 1))
    } else {
        let mut power = Fr::from(1u64 << 63);
        for _ in 63..(decomp_k - 1) {
            power = power + power;
        }
        power
    };
    let shifted_logit =
        builder.add_sector(logit.expr() + AbstractExpression::constant(offset));

    // Reconstruction residual: weighted_sum - shifted_logit must be zero
    let recon_residual = builder.add_sector(weighted_sum.expr() - shifted_logit.expr());

    // === Layer 5: Sign consistency ===
    // The sign bit is bit (K-1), the MSB of the decomposition.
    // If logit >= 0, then shifted = logit + 2^(K-1) >= 2^(K-1), so bit K-1 = 1 => class 1
    // If logit < 0, then shifted = logit + 2^(K-1) < 2^(K-1), so bit K-1 = 0 => class 0
    // So: sign_bit should equal expected_class
    let sign_bit = builder.add_identity_gate_node(
        &decomp_bits_input,
        vec![(0, (decomp_k - 1) as u32)],
        0,
        None,
    );
    let sign_residual = builder.add_sector(sign_bit.expr() - expected_class.expr());

    // === Output: combine all zero checks ===
    // Layout:
    //   [0]: reconstruction residual
    //   [1]: sign residual
    //   [2 .. 2+decomp_padded): decomp binary checks
    let total_checks = 2 + decomp_padded;
    let output_nv = model::next_log2(total_checks);

    let mut offset_pos = 0usize;

    // Reconstruction residual at position 0
    let recon_r = builder.add_identity_gate_node(
        &recon_residual,
        vec![(offset_pos as u32, 0)],
        output_nv,
        None,
    );
    offset_pos += 1;

    // Sign residual at position 1
    let sign_r = builder.add_identity_gate_node(
        &sign_residual,
        vec![(offset_pos as u32, 0)],
        output_nv,
        None,
    );
    offset_pos += 1;

    // Decomp binary check at positions 2..2+decomp_padded
    let mut dbc_gates = Vec::new();
    for i in 0..decomp_padded {
        dbc_gates.push(((offset_pos + i) as u32, i as u32));
    }
    let dbc_r = builder.add_identity_gate_node(&decomp_bc, dbc_gates, output_nv, None);

    // Combine all (disjoint positions, no cancellation)
    let combined = builder.add_sector(recon_r.expr() + sign_r.expr() + dbc_r.expr());

    builder.set_output(&combined);
    builder
        .build()
        .expect("Failed to build logistic regression circuit")
}

/// Sum n values down to 1 using pairwise tree reduction.
/// Same as `aggregate_sum` in circuit.rs but local to this module.
fn aggregate_sum_logreg(
    builder: &mut CircuitBuilder<Fr>,
    values: &NodeRef<Fr>,
    nv: usize,
) -> NodeRef<Fr> {
    let mut agg = values.clone();
    for level in (0..nv).rev() {
        let half_n = 1usize << level;
        let mut even_gates = Vec::new();
        let mut odd_gates = Vec::new();
        for i in 0..half_n {
            even_gates.push((i as u32, (2 * i) as u32));
            odd_gates.push((i as u32, (2 * i + 1) as u32));
        }
        let even = builder.add_identity_gate_node(&agg, even_gates, level, None);
        let odd = builder.add_identity_gate_node(&agg, odd_gates, level, None);
        agg = builder.add_sector(even.expr() + odd.expr());
    }
    agg
}

/// Build the circuit, set inputs, generate and verify a Hyrax proof.
/// Returns Ok on successful verification, Err on failure.
#[cfg(test)]
fn prove_and_verify_logreg(inputs: &LogRegCircuitInputs) -> Result<()> {
    let base_circuit = build_logreg_circuit(inputs.n_padded, inputs.decomp_k);

    let mut prover_circuit = base_circuit.clone();
    let verifier_circuit = base_circuit.clone();

    prover_circuit.set_input("features", inputs.features_quantized.clone().into());
    prover_circuit.set_input("decomp_bits", inputs.decomp_bits.clone().into());
    prover_circuit.set_input("weights", inputs.weights_quantized.clone().into());
    prover_circuit.set_input("bias", vec![inputs.bias_quantized].into());
    prover_circuit.set_input("expected_class", vec![inputs.expected_class].into());

    let hyrax_prover_config = GKRCircuitProverConfig::hyrax_compatible_runtime_optimized_default();
    let hyrax_verifier_config =
        GKRCircuitVerifierConfig::new_from_prover_config(&hyrax_prover_config, false);

    let mut hyrax_provable = prover_circuit
        .gen_hyrax_provable_circuit()
        .expect("Failed to generate Hyrax-provable circuit");

    let pedersen_committer =
        PedersenCommitter::new(512, "logistic-regression Pedersen committer", None);
    let mut blinding_rng = thread_rng();
    let mut vandermonde = VandermondeInverse::new();
    let mut prover_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("logistic-regression prover transcript");

    let (proof, proof_config) = perform_function_under_prover_config!(
        |w, x, y, z| hyrax_provable.prove(w, x, y, z),
        &hyrax_prover_config,
        &pedersen_committer,
        &mut blinding_rng,
        &mut vandermonde,
        &mut prover_transcript
    );

    let hyrax_verifiable = verifier_circuit
        .gen_hyrax_verifiable_circuit()
        .expect("Failed to generate Hyrax-verifiable circuit");

    let verifier_pedersen_committer =
        PedersenCommitter::new(512, "logistic-regression Pedersen committer", None);
    let mut verifier_transcript: ECTranscript<Bn256Point, PoseidonSponge<Fq>> =
        ECTranscript::new("logistic-regression prover transcript");

    perform_function_under_verifier_config!(
        verify_hyrax_proof,
        &hyrax_verifier_config,
        &proof,
        &hyrax_verifiable,
        &verifier_pedersen_committer,
        &mut verifier_transcript,
        &proof_config
    );

    Ok(())
}

/// Build a circuit description for hashing (deterministic encoding of model structure).
pub fn build_logreg_circuit_description(model: &LogisticRegressionModel) -> Vec<u8> {
    let mut desc = Vec::new();
    desc.extend_from_slice(b"REMAINDER_LOGREG_V1");
    desc.extend_from_slice(&(model.weights.len() as u32).to_be_bytes());

    // Encode quantized weights
    for w in &model.weights {
        let w_q = (w * FIXED_POINT_SCALE as f64).round() as i64;
        desc.extend_from_slice(&w_q.to_be_bytes());
    }

    // Encode quantized bias
    let bias_q = (model.bias * (FIXED_POINT_SCALE as f64) * (FIXED_POINT_SCALE as f64)).round()
        as i64;
    desc.extend_from_slice(&bias_q.to_be_bytes());

    desc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_model() -> LogisticRegressionModel {
        LogisticRegressionModel {
            weights: vec![0.5, -0.3, 0.8, 0.1],
            bias: -0.2,
            threshold: 0.5,
            feature_names: vec![
                "age".into(),
                "income".into(),
                "score".into(),
                "history".into(),
            ],
        }
    }

    // ===== Model parsing and inference tests =====

    #[test]
    fn test_predict_positive() {
        let model = sample_model();
        // 0.5*10 + -0.3*5 + 0.8*8 + 0.1*3 - 0.2 = 5 - 1.5 + 6.4 + 0.3 - 0.2 = 10.0
        let result = model.predict(&[10.0, 5.0, 8.0, 3.0]).unwrap();
        assert!(result.probability > 0.99);
        assert_eq!(result.predicted_class, 1);
    }

    #[test]
    fn test_predict_negative() {
        let model = sample_model();
        // 0.5*(-5) + -0.3*10 + 0.8*(-3) + 0.1*(-2) - 0.2 = -2.5 - 3 - 2.4 - 0.2 - 0.2 = -8.3
        let result = model.predict(&[-5.0, 10.0, -3.0, -2.0]).unwrap();
        assert!(result.probability < 0.01);
        assert_eq!(result.predicted_class, 0);
    }

    #[test]
    fn test_predict_wrong_features() {
        let model = sample_model();
        assert!(model.predict(&[1.0, 2.0]).is_err());
    }

    #[test]
    fn test_from_json() {
        let json = r#"{"weights": [1.0, -1.0], "bias": 0.0}"#;
        let model = LogisticRegressionModel::from_json(json).unwrap();
        assert_eq!(model.num_features(), 2);
        assert_eq!(model.threshold, 0.5); // default
    }

    #[test]
    fn test_empty_weights_rejected() {
        let json = r#"{"weights": [], "bias": 0.0}"#;
        assert!(LogisticRegressionModel::from_json(json).is_err());
    }

    #[test]
    fn test_sigmoid() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-10);
        assert!(sigmoid(100.0) > 0.999);
        assert!(sigmoid(-100.0) < 0.001);
    }

    #[test]
    fn test_quantize_weights() {
        let weights = vec![0.5, -0.25, 1.0];
        let scale = 1000;
        let quantized = quantize_weights(&weights, scale);
        assert_eq!(quantized, vec![500, -250, 1000]);
    }

    #[test]
    fn test_to_xgboost_model() {
        let model = sample_model();
        let xgb = model.to_xgboost_model();
        assert_eq!(xgb.num_features, 4);
        assert_eq!(xgb.trees.len(), 1);
    }

    #[test]
    fn test_serde_roundtrip() {
        let model = sample_model();
        let json = serde_json::to_string(&model).unwrap();
        let decoded = LogisticRegressionModel::from_json(&json).unwrap();
        assert_eq!(decoded.weights, model.weights);
        assert_eq!(decoded.bias, model.bias);
    }

    // ===== Circuit tests =====

    #[test]
    fn test_prepare_logreg_inputs_positive() {
        let model = sample_model();
        let features = vec![10.0, 5.0, 8.0, 3.0];
        let inputs = prepare_logreg_inputs(&model, &features);
        assert_eq!(inputs.n_padded, 4);
        assert_eq!(inputs.expected_class, 1); // logit = 10.0 > 0
        assert_eq!(inputs.decomp_k, DECOMP_K);
    }

    #[test]
    fn test_prepare_logreg_inputs_negative() {
        let model = sample_model();
        let features = vec![-5.0, 10.0, -3.0, -2.0];
        let inputs = prepare_logreg_inputs(&model, &features);
        assert_eq!(inputs.expected_class, 0); // logit = -8.3 < 0
    }

    #[test]
    fn test_build_logreg_circuit() {
        // Just check circuit builds without panicking
        let _circuit = build_logreg_circuit(4, DECOMP_K);
    }

    #[test]
    fn test_build_logreg_circuit_single_feature() {
        let _circuit = build_logreg_circuit(1, DECOMP_K);
    }

    #[test]
    fn test_build_logreg_circuit_8_features() {
        let _circuit = build_logreg_circuit(8, DECOMP_K);
    }

    #[test]
    fn test_logreg_circuit_description() {
        let model = sample_model();
        let desc = build_logreg_circuit_description(&model);
        assert!(!desc.is_empty());
        assert!(desc.starts_with(b"REMAINDER_LOGREG_V1"));
    }

    #[test]
    fn test_logreg_circuit_description_deterministic() {
        let model = sample_model();
        let desc1 = build_logreg_circuit_description(&model);
        let desc2 = build_logreg_circuit_description(&model);
        assert_eq!(desc1, desc2);
    }

    #[test]
    fn test_from_json_file_sample() {
        let json = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("test-model/logistic_regression_sample.json"),
        )
        .unwrap();
        let model = LogisticRegressionModel::from_json(&json).unwrap();
        assert_eq!(model.num_features(), 4);
        assert_eq!(model.weights.len(), 4);
    }

    // ===== Prove-and-verify tests =====

    #[test]
    fn test_prove_and_verify_positive() {
        let model = sample_model();
        let features = vec![10.0, 5.0, 8.0, 3.0]; // logit ~ 10.0
        let inputs = prepare_logreg_inputs(&model, &features);
        let result = prove_and_verify_logreg(&inputs);
        assert!(
            result.is_ok(),
            "prove_and_verify failed for positive case: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_prove_and_verify_negative() {
        let model = sample_model();
        let features = vec![-5.0, 10.0, -3.0, -2.0]; // logit ~ -8.3
        let inputs = prepare_logreg_inputs(&model, &features);
        let result = prove_and_verify_logreg(&inputs);
        assert!(
            result.is_ok(),
            "prove_and_verify failed for negative case: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_prove_and_verify_2_features() {
        let model = LogisticRegressionModel {
            weights: vec![1.0, -1.0],
            bias: 0.5,
            threshold: 0.5,
            feature_names: vec![],
        };
        let features = vec![2.0, 0.5]; // logit = 1*2 + -1*0.5 + 0.5 = 2.0
        let inputs = prepare_logreg_inputs(&model, &features);
        assert_eq!(inputs.expected_class, 1);
        let result = prove_and_verify_logreg(&inputs);
        assert!(
            result.is_ok(),
            "prove_and_verify failed for 2-feature case: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_prove_and_verify_single_feature() {
        let model = LogisticRegressionModel {
            weights: vec![1.0],
            bias: -0.5,
            threshold: 0.5,
            feature_names: vec![],
        };
        // logit = 1.0 * 1.0 - 0.5 = 0.5 > 0 => class 1
        let features = vec![1.0];
        let inputs = prepare_logreg_inputs(&model, &features);
        assert_eq!(inputs.expected_class, 1);
        let result = prove_and_verify_logreg(&inputs);
        assert!(
            result.is_ok(),
            "prove_and_verify failed for single-feature case: {:?}",
            result.err()
        );
    }

    // ===== Negative tests =====

    /// Helper: prepare inputs, apply tampering, and try to prove.
    fn try_prove_tampered(
        model: &LogisticRegressionModel,
        features: &[f64],
        tamper: impl FnOnce(&mut LogRegCircuitInputs) + std::panic::UnwindSafe,
    ) -> bool {
        let mut inputs = prepare_logreg_inputs(model, features);
        tamper(&mut inputs);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prove_and_verify_logreg(&inputs).is_ok()
        }))
        .unwrap_or(false)
    }

    #[test]
    fn test_reject_wrong_class() {
        let model = sample_model();
        let features = vec![10.0, 5.0, 8.0, 3.0]; // class 1
        let result = try_prove_tampered(&model, &features, |inputs| {
            inputs.expected_class = 0; // tamper: claim class 0
        });
        assert!(!result, "Should reject wrong class");
    }

    #[test]
    fn test_reject_flipped_decomp_bit() {
        let model = sample_model();
        let features = vec![10.0, 5.0, 8.0, 3.0];
        let result = try_prove_tampered(&model, &features, |inputs| {
            inputs.decomp_bits[5] = !inputs.decomp_bits[5]; // flip a middle bit
        });
        assert!(!result, "Should reject flipped decomp bit");
    }

    #[test]
    fn test_reject_tampered_feature() {
        let model = sample_model();
        let features = vec![10.0, 5.0, 8.0, 3.0];
        let result = try_prove_tampered(&model, &features, |inputs| {
            inputs.features_quantized[0] = 0; // tamper: zero out first feature
        });
        assert!(!result, "Should reject tampered feature");
    }
}
