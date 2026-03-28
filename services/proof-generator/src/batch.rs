//! Batch proving — prove multiple inferences in one request.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct BatchProveRequest {
    pub model_id: String,
    pub inputs: Vec<Vec<f64>>,
}

#[derive(Debug, Serialize)]
pub struct BatchProveResponse {
    pub batch_id: String,
    pub results: Vec<BatchProveEntry>,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
}

#[derive(Debug, Serialize)]
pub struct BatchProveEntry {
    pub index: usize,
    pub proof_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Single result entry within a batch prove (alias for BatchProveEntry).
pub type BatchProveResult = BatchProveEntry;

/// Maximum batch size.
pub const MAX_BATCH_SIZE: usize = 100;

/// Default batch concurrency.
pub const DEFAULT_BATCH_CONCURRENCY: usize = 4;

impl BatchProveRequest {
    /// Return the effective concurrency for this batch.
    pub fn effective_concurrency(&self) -> usize {
        DEFAULT_BATCH_CONCURRENCY.min(self.inputs.len())
    }
}

/// Validate a batch request.
pub fn validate_batch_request(req: &BatchProveRequest) -> Result<(), String> {
    validate_batch(req)
}

/// Validate a batch request (original name).
pub fn validate_batch(req: &BatchProveRequest) -> Result<(), String> {
    if req.inputs.is_empty() {
        return Err("inputs array is empty".into());
    }
    if req.inputs.len() > MAX_BATCH_SIZE {
        return Err(format!("maximum {} inputs per batch", MAX_BATCH_SIZE));
    }
    if req.model_id.is_empty() {
        return Err("model_id is required".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ok() {
        let req = BatchProveRequest {
            model_id: "m1".into(),
            inputs: vec![vec![1.0, 2.0]],
        };
        assert!(validate_batch(&req).is_ok());
    }

    #[test]
    fn test_validate_empty() {
        let req = BatchProveRequest {
            model_id: "m1".into(),
            inputs: vec![],
        };
        assert!(validate_batch(&req).is_err());
    }

    #[test]
    fn test_validate_over_limit() {
        let req = BatchProveRequest {
            model_id: "m1".into(),
            inputs: vec![vec![1.0]; 101],
        };
        assert!(validate_batch(&req).is_err());
    }
}
