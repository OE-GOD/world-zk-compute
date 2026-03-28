//! Batch proof generation: prove multiple inferences in one request.
//!
//! The `POST /v1/prove/batch` endpoint accepts an array of feature vectors and
//! proves them all with configurable concurrency. Each individual failure does
//! not block the rest of the batch.

use serde::{Deserialize, Serialize};

/// Default concurrency limit for batch proving.
const DEFAULT_MAX_CONCURRENCY: usize = 4;

/// Maximum allowed concurrency to prevent resource exhaustion.
const MAX_CONCURRENCY_LIMIT: usize = 64;

/// Maximum number of inputs in a single batch request.
const MAX_BATCH_SIZE: usize = 10_000;

/// Request body for `POST /v1/prove/batch`.
#[derive(Debug, Deserialize)]
pub struct BatchProveRequest {
    /// ID of a previously uploaded model.
    pub model_id: String,
    /// Array of feature vectors, one per inference.
    pub inputs: Vec<Vec<f64>>,
    /// Maximum number of concurrent proof tasks. Defaults to 4.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
}

/// Response body for `POST /v1/prove/batch`.
#[derive(Debug, Serialize, Deserialize)]
pub struct BatchProveResponse {
    /// Unique identifier for this batch.
    pub batch_id: String,
    /// Total number of inputs in the batch.
    pub total: usize,
    /// Number of successfully completed proofs.
    pub completed: usize,
    /// Number of failed proofs.
    pub failed: usize,
    /// Per-input results, in the same order as the request inputs.
    pub results: Vec<BatchProveResult>,
}

/// Result for a single input within a batch.
#[derive(Debug, Serialize, Deserialize)]
pub struct BatchProveResult {
    /// Zero-based index of this input within the batch.
    pub index: usize,
    /// Proof ID, if proving succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_id: Option<String>,
    /// Predicted output label (e.g. "approved" or "denied"), if proving succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Error message, if proving failed for this input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BatchProveRequest {
    /// Return the effective concurrency limit, clamped to sane bounds.
    pub fn effective_concurrency(&self) -> usize {
        self.max_concurrency
            .unwrap_or(DEFAULT_MAX_CONCURRENCY)
            .clamp(1, MAX_CONCURRENCY_LIMIT)
    }
}

/// Validate a batch prove request, returning an error message if invalid.
pub fn validate_batch_request(req: &BatchProveRequest) -> Result<(), String> {
    if req.inputs.is_empty() {
        return Err("inputs array must not be empty".to_string());
    }
    if req.inputs.len() > MAX_BATCH_SIZE {
        return Err(format!(
            "batch size {} exceeds maximum of {}",
            req.inputs.len(),
            MAX_BATCH_SIZE
        ));
    }
    for (i, input) in req.inputs.iter().enumerate() {
        if input.is_empty() {
            return Err(format!("input[{}] feature vector must not be empty", i));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effective_concurrency_default() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![vec![1.0]],
            max_concurrency: None,
        };
        assert_eq!(req.effective_concurrency(), DEFAULT_MAX_CONCURRENCY);
    }

    #[test]
    fn test_effective_concurrency_custom() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![vec![1.0]],
            max_concurrency: Some(8),
        };
        assert_eq!(req.effective_concurrency(), 8);
    }

    #[test]
    fn test_effective_concurrency_clamped_low() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![vec![1.0]],
            max_concurrency: Some(0),
        };
        assert_eq!(req.effective_concurrency(), 1);
    }

    #[test]
    fn test_effective_concurrency_clamped_high() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![vec![1.0]],
            max_concurrency: Some(1000),
        };
        assert_eq!(req.effective_concurrency(), MAX_CONCURRENCY_LIMIT);
    }

    #[test]
    fn test_validate_empty_inputs() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![],
            max_concurrency: None,
        };
        let err = validate_batch_request(&req).unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn test_validate_empty_feature_vector() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![vec![1.0], vec![], vec![2.0]],
            max_concurrency: None,
        };
        let err = validate_batch_request(&req).unwrap_err();
        assert!(err.contains("input[1]"));
    }

    #[test]
    fn test_validate_ok() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: vec![vec![1.0, 2.0], vec![3.0, 4.0]],
            max_concurrency: Some(2),
        };
        assert!(validate_batch_request(&req).is_ok());
    }

    #[test]
    fn test_batch_prove_response_serialization() {
        let resp = BatchProveResponse {
            batch_id: "batch-123".to_string(),
            total: 2,
            completed: 1,
            failed: 1,
            results: vec![
                BatchProveResult {
                    index: 0,
                    proof_id: Some("proof-1".to_string()),
                    output: Some("approved".to_string()),
                    error: None,
                },
                BatchProveResult {
                    index: 1,
                    proof_id: None,
                    output: None,
                    error: Some("Model not found".to_string()),
                },
            ],
        };

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("batch-123"));
        assert!(json.contains("proof-1"));
        assert!(json.contains("Model not found"));

        // Verify skip_serializing_if works: success result has no "error" field
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["results"][0].get("error").is_none());
        assert!(parsed["results"][1].get("proof_id").is_none());
    }

    #[test]
    fn test_batch_prove_request_deserialization() {
        let json = r#"{
            "model_id": "abc-123",
            "inputs": [[1.0, 2.0], [3.0, 4.0, 5.0]],
            "max_concurrency": 8
        }"#;
        let req: BatchProveRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model_id, "abc-123");
        assert_eq!(req.inputs.len(), 2);
        assert_eq!(req.inputs[0], vec![1.0, 2.0]);
        assert_eq!(req.inputs[1], vec![3.0, 4.0, 5.0]);
        assert_eq!(req.max_concurrency, Some(8));
    }

    #[test]
    fn test_batch_prove_request_deserialization_no_concurrency() {
        let json = r#"{
            "model_id": "abc-123",
            "inputs": [[1.0]]
        }"#;
        let req: BatchProveRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.max_concurrency, None);
        assert_eq!(req.effective_concurrency(), DEFAULT_MAX_CONCURRENCY);
    }

    #[test]
    fn test_validate_too_many_inputs() {
        let req = BatchProveRequest {
            model_id: "m1".to_string(),
            inputs: (0..10_001).map(|_| vec![1.0]).collect(),
            max_concurrency: None,
        };
        let err = validate_batch_request(&req).unwrap_err();
        assert!(err.contains("exceeds maximum"));
    }
}
