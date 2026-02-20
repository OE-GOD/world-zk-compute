//! XGBoost Decomposition Strategy
//!
//! Splits XGBoost inference inputs into independent sub-jobs by partitioning
//! the sample set. Each sub-job gets the full model + a chunk of samples.
//!
//! Input format (risc0 serde, u32 word-aligned):
//! ```text
//! XGBoostInput { model: XGBoostModel, samples: Vec<Sample>, threshold: f64 }
//! ```
//!
//! The guest program deserializes via `env::read()` (risc0 serde), so we use
//! `risc0_zkvm::serde` here to match.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::input_decomposer::DecompositionStrategy;

// ═══════════════════════════════════════════════════════════════════════════════
// XGBoost Types (mirror guest program definitions)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XGBoostInput {
    pub model: XGBoostModel,
    pub samples: Vec<Sample>,
    pub threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XGBoostModel {
    pub num_features: u32,
    pub num_classes: u32,
    pub base_score: f64,
    pub trees: Vec<Tree>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    pub nodes: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub is_leaf: u32,
    pub feature_idx: u32,
    pub threshold: f64,
    pub left_child: u32,
    pub right_child: u32,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub id: [u8; 32],
    pub features: Vec<f64>,
}

// Output types (for journal merging)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XGBoostOutput {
    pub total_samples: u32,
    pub predictions: Vec<Prediction>,
    pub flagged_count: u32,
    pub flagged_ids: Vec<[u8; 32]>,
    pub input_hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    pub id: [u8; 32],
    pub score: f64,
    pub flagged: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// risc0 serde helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Serialize a value using risc0 serde format, returning bytes.
fn risc0_serialize<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let words = risc0_zkvm::serde::to_vec(value).map_err(|e| anyhow!("risc0 serialize: {}", e))?;
    Ok(words.iter().flat_map(|w| w.to_le_bytes()).collect())
}

/// Deserialize bytes using risc0 serde format.
fn risc0_deserialize<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    // risc0 serde requires u32-aligned input; reject unaligned data early
    if !bytes.len().is_multiple_of(4) {
        return Err(anyhow!(
            "risc0 deserialize: input length {} not u32-aligned",
            bytes.len()
        ));
    }
    risc0_zkvm::serde::from_slice::<T, u8>(bytes).map_err(|e| anyhow!("risc0 deserialize: {}", e))
}

// ═══════════════════════════════════════════════════════════════════════════════
// XGBoost Decomposition Strategy
// ═══════════════════════════════════════════════════════════════════════════════

/// Decomposes XGBoost inference by splitting the sample set.
///
/// Each sub-job gets the full model + a chunk of samples.
/// Results are merged by concatenating predictions and recomputing totals.
pub struct XGBoostDecompStrategy;

impl XGBoostDecompStrategy {
    pub fn new() -> Self {
        Self
    }

    /// Try to parse the input as an XGBoostInput (risc0 serde format).
    fn try_parse(input: &[u8]) -> Result<XGBoostInput> {
        risc0_deserialize(input)
    }
}

impl DecompositionStrategy for XGBoostDecompStrategy {
    fn name(&self) -> &str {
        "xgboost"
    }

    fn can_decompose(&self, input: &[u8]) -> bool {
        Self::try_parse(input)
            .map(|parsed| parsed.samples.len() > 1)
            .unwrap_or(false)
    }

    fn item_count(&self, input: &[u8]) -> Result<usize> {
        let parsed = Self::try_parse(input)?;
        Ok(parsed.samples.len())
    }

    fn split(&self, input: &[u8], n: usize) -> Result<Vec<Vec<u8>>> {
        let parsed = Self::try_parse(input)?;
        let total = parsed.samples.len();

        if total == 0 {
            return Err(anyhow!("No samples to split"));
        }

        let chunk_count = n.min(total);
        let base_size = total / chunk_count;
        let remainder = total % chunk_count;

        let mut sub_inputs = Vec::with_capacity(chunk_count);
        let mut offset = 0;

        for i in 0..chunk_count {
            let this_chunk = base_size + if i < remainder { 1 } else { 0 };
            let chunk_samples = parsed.samples[offset..offset + this_chunk].to_vec();

            let sub_input = XGBoostInput {
                model: parsed.model.clone(),
                samples: chunk_samples,
                threshold: parsed.threshold,
            };

            let serialized = risc0_serialize(&sub_input)?;
            sub_inputs.push(serialized);
            offset += this_chunk;
        }

        info!(
            "XGBoost split: {} samples → {} chunks ({} trees per chunk)",
            total,
            chunk_count,
            parsed.model.trees.len()
        );

        Ok(sub_inputs)
    }

    fn merge_journals(&self, journals: &[Vec<u8>]) -> Result<Vec<u8>> {
        let mut all_predictions = Vec::new();
        let mut all_flagged_ids = Vec::new();
        let mut total_samples = 0u32;
        let mut input_hash = [0u8; 32];

        for journal in journals {
            let output: XGBoostOutput = risc0_deserialize(journal)?;

            total_samples += output.total_samples;
            all_predictions.extend(output.predictions);
            all_flagged_ids.extend(output.flagged_ids);

            // Use the first sub-journal's input_hash (they share the same model)
            if input_hash == [0u8; 32] {
                input_hash = output.input_hash;
            }
        }

        let flagged_count = all_flagged_ids.len() as u32;

        let merged = XGBoostOutput {
            total_samples,
            predictions: all_predictions,
            flagged_count,
            flagged_ids: all_flagged_ids,
            input_hash,
        };

        risc0_serialize(&merged)
    }

    fn estimate_cycles(&self, input: &[u8]) -> Option<u64> {
        let parsed = Self::try_parse(input).ok()?;
        let samples = parsed.samples.len() as u64;
        let trees = parsed.model.trees.len() as u64;
        // Heuristic: each sample traverses all trees, ~1000 cycles per tree traversal
        // Plus 5M cycles base overhead for model loading and setup
        Some(samples * trees * 1000 + 5_000_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_model() -> XGBoostModel {
        XGBoostModel {
            num_features: 3,
            num_classes: 2,
            base_score: 0.5,
            trees: vec![Tree {
                nodes: vec![TreeNode {
                    is_leaf: 1,
                    feature_idx: 0,
                    threshold: 0.5,
                    left_child: 0,
                    right_child: 0,
                    value: 1.0,
                }],
            }],
        }
    }

    fn make_test_sample(id_byte: u8) -> Sample {
        let mut id = [0u8; 32];
        id[0] = id_byte;
        Sample {
            id,
            features: vec![1.0, 2.0, 3.0],
        }
    }

    fn make_test_input(num_samples: usize) -> Vec<u8> {
        let input = XGBoostInput {
            model: make_test_model(),
            samples: (0..num_samples)
                .map(|i| make_test_sample(i as u8))
                .collect(),
            threshold: 0.7,
        };
        risc0_serialize(&input).unwrap()
    }

    fn make_test_output(samples: &[u8]) -> Vec<u8> {
        let output = XGBoostOutput {
            total_samples: samples.len() as u32,
            predictions: samples
                .iter()
                .map(|&id_byte| {
                    let mut id = [0u8; 32];
                    id[0] = id_byte;
                    Prediction {
                        id,
                        score: 0.8,
                        flagged: if id_byte > 5 { 1 } else { 0 },
                    }
                })
                .collect(),
            flagged_count: samples.iter().filter(|&&b| b > 5).count() as u32,
            flagged_ids: samples
                .iter()
                .filter(|&&b| b > 5)
                .map(|&b| {
                    let mut id = [0u8; 32];
                    id[0] = b;
                    id
                })
                .collect(),
            input_hash: [0xAA; 32],
        };
        risc0_serialize(&output).unwrap()
    }

    #[test]
    fn test_can_decompose() {
        let strategy = XGBoostDecompStrategy::new();

        // Single sample — not decomposable
        let single = make_test_input(1);
        assert!(!strategy.can_decompose(&single));

        // Multiple samples — decomposable
        let multi = make_test_input(10);
        assert!(strategy.can_decompose(&multi));

        // Random bytes — not decomposable
        assert!(!strategy.can_decompose(&[1, 2, 3]));
    }

    #[test]
    fn test_item_count() {
        let strategy = XGBoostDecompStrategy::new();
        let input = make_test_input(42);
        assert_eq!(strategy.item_count(&input).unwrap(), 42);
    }

    #[test]
    fn test_split() {
        let strategy = XGBoostDecompStrategy::new();
        let input = make_test_input(10);

        let chunks = strategy.split(&input, 3).unwrap();
        assert_eq!(chunks.len(), 3);

        // Verify each chunk is valid XGBoostInput
        let mut total_samples = 0;
        for chunk in &chunks {
            let parsed: XGBoostInput = risc0_deserialize(chunk).unwrap();
            assert!(!parsed.samples.is_empty());
            assert_eq!(parsed.model.trees.len(), 1);
            assert_eq!(parsed.threshold, 0.7);
            total_samples += parsed.samples.len();
        }

        // All samples accounted for
        assert_eq!(total_samples, 10);
    }

    #[test]
    fn test_split_preserves_data() {
        let strategy = XGBoostDecompStrategy::new();
        let input = make_test_input(6);

        let chunks = strategy.split(&input, 2).unwrap();
        assert_eq!(chunks.len(), 2);

        let chunk0: XGBoostInput = risc0_deserialize(&chunks[0]).unwrap();
        let chunk1: XGBoostInput = risc0_deserialize(&chunks[1]).unwrap();

        // Verify samples are split correctly
        assert_eq!(chunk0.samples.len(), 3);
        assert_eq!(chunk1.samples.len(), 3);

        // Verify sample IDs
        assert_eq!(chunk0.samples[0].id[0], 0);
        assert_eq!(chunk0.samples[1].id[0], 1);
        assert_eq!(chunk0.samples[2].id[0], 2);
        assert_eq!(chunk1.samples[0].id[0], 3);
        assert_eq!(chunk1.samples[1].id[0], 4);
        assert_eq!(chunk1.samples[2].id[0], 5);
    }

    #[test]
    fn test_merge_journals() {
        let strategy = XGBoostDecompStrategy::new();

        let journal0 = make_test_output(&[0, 1, 2]);
        let journal1 = make_test_output(&[3, 4, 10]); // 10 > 5, so flagged

        let merged = strategy.merge_journals(&[journal0, journal1]).unwrap();
        let output: XGBoostOutput = risc0_deserialize(&merged).unwrap();

        assert_eq!(output.total_samples, 6);
        assert_eq!(output.predictions.len(), 6);
        assert_eq!(output.flagged_count, 1); // only sample 10
        assert_eq!(output.flagged_ids.len(), 1);
    }

    #[test]
    fn test_estimate_cycles() {
        let strategy = XGBoostDecompStrategy::new();
        let input = make_test_input(100);

        let cycles = strategy.estimate_cycles(&input).unwrap();
        // 100 samples * 1 tree * 1000 + 5M
        assert_eq!(cycles, 100 * 1 * 1000 + 5_000_000);
    }

    #[test]
    fn test_split_more_chunks_than_samples() {
        let strategy = XGBoostDecompStrategy::new();
        let input = make_test_input(3);

        // Request 10 chunks but only 3 samples
        let chunks = strategy.split(&input, 10).unwrap();
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn test_roundtrip_with_risc0_serde() {
        // Verify that risc0 serde roundtrip works correctly
        let input = XGBoostInput {
            model: make_test_model(),
            samples: vec![make_test_sample(42)],
            threshold: 0.7,
        };

        let bytes = risc0_serialize(&input).unwrap();
        let decoded: XGBoostInput = risc0_deserialize(&bytes).unwrap();

        assert_eq!(decoded.model.num_features, 3);
        assert_eq!(decoded.model.base_score, 0.5);
        assert_eq!(decoded.samples.len(), 1);
        assert_eq!(decoded.samples[0].id[0], 42);
        assert_eq!(decoded.samples[0].features, vec![1.0, 2.0, 3.0]);
        assert_eq!(decoded.threshold, 0.7);
    }
}
