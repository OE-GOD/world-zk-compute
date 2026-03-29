//! Model format auto-detection from JSON structure.
//!
//! Inspects top-level keys in a JSON document to determine whether it is an
//! XGBoost, LightGBM, random forest, or logistic regression model.  This lets
//! users omit the `--model-format` flag and have the CLI infer the format
//! automatically.

/// Supported model format identifiers returned by [`detect_model_format`].
pub const FORMAT_XGBOOST: &str = "xgboost";
pub const FORMAT_LIGHTGBM: &str = "lightgbm";
pub const FORMAT_RANDOM_FOREST: &str = "random_forest";
pub const FORMAT_LOGISTIC_REGRESSION: &str = "logistic_regression";
pub const FORMAT_MLP: &str = "mlp";

/// Detect the model format by inspecting top-level keys in the JSON string.
///
/// Returns one of `"xgboost"`, `"lightgbm"`, `"random_forest"`, or
/// `"logistic_regression"` on success.  Returns an error if the JSON is
/// invalid or the format cannot be determined.
///
/// Detection heuristics (checked in order):
///
/// | Format              | Distinguishing key(s)                       |
/// |---------------------|---------------------------------------------|
/// | XGBoost             | `"learner"`                                 |
/// | LightGBM            | `"tree_info"`                               |
/// | MLP                 | `"model_type"` == `"mlp"`                   |
/// | Random forest       | `"n_estimators"` or `"forest"`              |
/// | Logistic regression | `"weights"` (with or without `"bias"`)      |
pub fn detect_model_format(json_str: &str) -> Result<&'static str, String> {
    let v: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("Invalid JSON: {}", e))?;

    // XGBoost: has "learner" key (xgb.save_model format)
    if v.get("learner").is_some() {
        return Ok(FORMAT_XGBOOST);
    }

    // LightGBM: has "tree_info" key (model.dump_model() format)
    if v.get("tree_info").is_some() {
        return Ok(FORMAT_LIGHTGBM);
    }

    // MLP: has "model_type" == "mlp" and "layers" key
    if v.get("model_type").and_then(|t| t.as_str()) == Some("mlp") {
        return Ok(FORMAT_MLP);
    }

    // Random forest: has "n_estimators" or "forest" key (scikit-learn export)
    if v.get("n_estimators").is_some() || v.get("forest").is_some() {
        return Ok(FORMAT_RANDOM_FOREST);
    }

    // Logistic regression: has "weights" key
    if v.get("weights").is_some() {
        return Ok(FORMAT_LOGISTIC_REGRESSION);
    }

    Err("Could not detect model format. Use --model-format to specify.".into())
}

/// Detect format from a file path by reading and parsing its contents.
///
/// This is a convenience wrapper around [`detect_model_format`] for CLI use.
pub fn detect_model_format_from_file(path: &std::path::Path) -> Result<String, String> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    detect_model_format(&data).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== XGBoost detection =====

    #[test]
    fn test_detect_xgboost() {
        let json = r#"{"learner": {"gradient_booster": {"model": {"trees": []}}}}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_XGBOOST);
    }

    #[test]
    fn test_detect_xgboost_minimal() {
        // Just the presence of "learner" key is enough
        let json = r#"{"learner": {}}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_XGBOOST);
    }

    // ===== LightGBM detection =====

    #[test]
    fn test_detect_lightgbm() {
        let json = r#"{
            "name": "tree",
            "version": "v4",
            "num_class": 1,
            "max_feature_idx": 3,
            "tree_info": [{"tree_index": 0}]
        }"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_LIGHTGBM);
    }

    #[test]
    fn test_detect_lightgbm_minimal() {
        let json = r#"{"tree_info": []}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_LIGHTGBM);
    }

    // ===== Random forest detection =====

    #[test]
    fn test_detect_random_forest_n_estimators() {
        let json = r#"{
            "model_type": "random_forest",
            "n_estimators": 3,
            "n_features": 4,
            "n_classes": 2,
            "trees": []
        }"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_RANDOM_FOREST);
    }

    #[test]
    fn test_detect_random_forest_forest_key() {
        // Alternative key: "forest"
        let json = r#"{"forest": {"trees": []}}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_RANDOM_FOREST);
    }

    // ===== Logistic regression detection =====

    #[test]
    fn test_detect_logistic_regression_weights_and_bias() {
        let json = r#"{"weights": [0.5, -0.3, 0.8], "bias": -0.2}"#;
        assert_eq!(
            detect_model_format(json).unwrap(),
            FORMAT_LOGISTIC_REGRESSION
        );
    }

    #[test]
    fn test_detect_logistic_regression_weights_only() {
        // "bias" is optional -- "weights" alone is sufficient
        let json = r#"{"weights": [1.0, -1.0]}"#;
        assert_eq!(
            detect_model_format(json).unwrap(),
            FORMAT_LOGISTIC_REGRESSION
        );
    }

    #[test]
    fn test_detect_logistic_regression_full() {
        let json = r#"{
            "weights": [0.5, -0.3, 0.8, 0.1],
            "bias": -0.2,
            "threshold": 0.5,
            "feature_names": ["age", "income", "score", "history"]
        }"#;
        assert_eq!(
            detect_model_format(json).unwrap(),
            FORMAT_LOGISTIC_REGRESSION
        );
    }

    // ===== Unknown / error cases =====

    #[test]
    fn test_detect_unknown_format() {
        let json = r#"{"some_random_key": 42}"#;
        let err = detect_model_format(json).unwrap_err();
        assert!(
            err.contains("Could not detect model format"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_detect_empty_object() {
        let json = r#"{}"#;
        assert!(detect_model_format(json).is_err());
    }

    #[test]
    fn test_detect_invalid_json() {
        let err = detect_model_format("not json at all").unwrap_err();
        assert!(err.contains("Invalid JSON"), "unexpected error: {}", err);
    }

    #[test]
    fn test_detect_json_array_not_object() {
        // A JSON array has no named keys, so detection should fail
        let json = r#"[1, 2, 3]"#;
        assert!(detect_model_format(json).is_err());
    }

    // ===== Priority / ordering tests =====

    #[test]
    fn test_xgboost_takes_priority_over_weights() {
        // If JSON has both "learner" and "weights", XGBoost wins (checked first)
        let json = r#"{"learner": {}, "weights": [1.0]}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_XGBOOST);
    }

    #[test]
    fn test_lightgbm_takes_priority_over_random_forest() {
        // If JSON has both "tree_info" and "n_estimators", LightGBM wins
        let json = r#"{"tree_info": [], "n_estimators": 5}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_LIGHTGBM);
    }

    #[test]
    fn test_random_forest_takes_priority_over_logistic_regression() {
        // If JSON has both "n_estimators" and "weights", random forest wins
        let json = r#"{"n_estimators": 3, "weights": [1.0]}"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_RANDOM_FOREST);
    }

    // ===== Integration with real-ish model JSON =====

    #[test]
    fn test_detect_real_xgboost_structure() {
        let json = r#"{
            "learner": {
                "learner_model_param": {"num_class": "2", "num_feature": "4"},
                "gradient_booster": {
                    "model": {
                        "trees": [{
                            "tree_param": {"num_nodes": "5"},
                            "left_children": [1, -1, -1, -1, -1],
                            "right_children": [2, -1, -1, -1, -1],
                            "split_indices": [0, 0, 0, 0, 0],
                            "split_conditions": [2.45, 0, 0, 0, 0],
                            "base_weights": [0, -1.2, 0.8, 0, 0]
                        }]
                    }
                }
            }
        }"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_XGBOOST);
    }

    #[test]
    fn test_detect_real_lightgbm_structure() {
        let json = r#"{
            "name": "tree",
            "version": "v4",
            "num_class": 1,
            "num_tree_per_iteration": 1,
            "max_feature_idx": 3,
            "tree_info": [{
                "tree_index": 0,
                "num_leaves": 3,
                "shrinkage": 1,
                "tree_structure": {
                    "split_index": 0,
                    "split_feature": 2,
                    "threshold": 2.45,
                    "left_child": {"leaf_index": 0, "leaf_value": -1.0},
                    "right_child": {"leaf_index": 1, "leaf_value": 0.5}
                }
            }]
        }"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_LIGHTGBM);
    }

    #[test]
    fn test_detect_real_random_forest_structure() {
        let json = r#"{
            "model_type": "random_forest",
            "n_estimators": 2,
            "n_features": 4,
            "n_classes": 2,
            "trees": [{
                "children_left": [1, -1, -1],
                "children_right": [2, -1, -1],
                "feature": [0, -2, -2],
                "threshold": [5.0, -2.0, -2.0],
                "value": [[50, 50], [30, 0], [0, 20]]
            }]
        }"#;
        assert_eq!(detect_model_format(json).unwrap(), FORMAT_RANDOM_FOREST);
    }

    #[test]
    fn test_detect_real_logistic_regression_structure() {
        let json = r#"{
            "weights": [0.5, -0.3, 0.8, 0.1],
            "bias": -0.2,
            "threshold": 0.5,
            "feature_names": ["age", "income", "score", "history"]
        }"#;
        assert_eq!(
            detect_model_format(json).unwrap(),
            FORMAT_LOGISTIC_REGRESSION
        );
    }
}
