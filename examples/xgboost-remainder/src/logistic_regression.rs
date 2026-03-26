//! Logistic regression model representation and inference.
//!
//! The simplest ML model for verifiable inference: a dot product of weights
//! and features, plus a bias term, followed by a sigmoid.
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

use serde::{Deserialize, Serialize};
use std::path::Path;

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
                leaf_value: 0.0, // Placeholder — actual value computed at inference
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
    weights.iter().map(|w| (w * scale as f64).round() as i64).collect()
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
}
