//! XGBoost Inference Guest Program
//!
//! Runs XGBoost decision tree ensemble inference inside the zkVM.
//! Covers capability 5 (model inference) from World's security analysis stack.
//!
//! Produces a proof that:
//! 1. The model was applied correctly to every sample
//! 2. Tree traversal logic is deterministic and correct
//! 3. Flagging decisions match the threshold exactly
//! 4. The input data (model + samples) was not tampered with
//!
//! Design notes:
//! - Uses f64 floats (proven to work in existing anomaly-detector guest)
//! - Tree traversal is iterative (no recursion — friendlier to zkVM stack)
//! - Scores are raw logits (base_score + sum of leaf values); no sigmoid
//! - Threshold comparison is in raw logit space

#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec::Vec;
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

// ═══════════════════════════════════════════════════════════════════════════════
// Input types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Deserialize)]
struct XGBoostInput {
    model: XGBoostModel,
    samples: Vec<Sample>,
    threshold: f64,
}

#[derive(serde::Deserialize)]
struct XGBoostModel {
    num_features: u32,
    num_classes: u32,
    base_score: f64,
    trees: Vec<Tree>,
}

#[derive(serde::Deserialize)]
struct Tree {
    nodes: Vec<TreeNode>,
}

#[derive(serde::Deserialize)]
struct TreeNode {
    /// 0 = internal node, 1 = leaf
    is_leaf: u32,
    feature_idx: u32,
    threshold: f64,
    left_child: u32,
    right_child: u32,
    /// Leaf value (only meaningful when is_leaf == 1)
    value: f64,
}

#[derive(serde::Deserialize)]
struct Sample {
    id: [u8; 32],
    features: Vec<f64>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Output types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(serde::Serialize)]
struct XGBoostOutput {
    total_samples: u32,
    predictions: Vec<Prediction>,
    flagged_count: u32,
    flagged_ids: Vec<[u8; 32]>,
    input_hash: [u8; 32],
}

#[derive(serde::Serialize)]
struct Prediction {
    id: [u8; 32],
    score: f64,
    /// 0 = not flagged, 1 = flagged
    flagged: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════════

fn main() {
    let input: XGBoostInput = env::read();

    let input_hash = compute_input_hash(&input);

    let mut predictions = Vec::with_capacity(input.samples.len());
    let mut flagged_count = 0u32;
    let mut flagged_ids = Vec::new();

    for sample in &input.samples {
        let score = predict_sample(&input.model, sample);
        let flagged = if score >= input.threshold { 1u32 } else { 0u32 };

        if flagged == 1 {
            flagged_count += 1;
            flagged_ids.push(sample.id);
        }

        predictions.push(Prediction {
            id: sample.id,
            score,
            flagged,
        });
    }

    let output = XGBoostOutput {
        total_samples: input.samples.len() as u32,
        predictions,
        flagged_count,
        flagged_ids,
        input_hash,
    };

    env::commit(&output);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tree traversal & prediction
// ═══════════════════════════════════════════════════════════════════════════════

/// Traverse a single tree iteratively, returning the leaf value.
fn traverse_tree(tree: &Tree, features: &[f64]) -> f64 {
    let mut node_idx = 0u32;

    loop {
        let node = &tree.nodes[node_idx as usize];

        if node.is_leaf == 1 {
            return node.value;
        }

        let feat_idx = node.feature_idx as usize;
        let feat_val = if feat_idx < features.len() {
            features[feat_idx]
        } else {
            0.0 // Missing feature defaults to 0.0
        };

        if feat_val < node.threshold {
            node_idx = node.left_child;
        } else {
            node_idx = node.right_child;
        }
    }
}

/// Predict a single sample: base_score + sum of all tree leaf values.
fn predict_sample(model: &XGBoostModel, sample: &Sample) -> f64 {
    let mut score = model.base_score;

    for tree in &model.trees {
        score += traverse_tree(tree, &sample.features);
    }

    score
}

// ═══════════════════════════════════════════════════════════════════════════════
// Input hashing
// ═══════════════════════════════════════════════════════════════════════════════

fn compute_input_hash(input: &XGBoostInput) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    // Hash model structure
    hasher.update(&input.model.num_features.to_le_bytes());
    hasher.update(&input.model.num_classes.to_le_bytes());
    hasher.update(&input.model.base_score.to_le_bytes());

    for tree in &input.model.trees {
        hasher.update(&(tree.nodes.len() as u32).to_le_bytes());
        for node in &tree.nodes {
            hasher.update(&node.is_leaf.to_le_bytes());
            hasher.update(&node.feature_idx.to_le_bytes());
            hasher.update(&node.threshold.to_le_bytes());
            hasher.update(&node.left_child.to_le_bytes());
            hasher.update(&node.right_child.to_le_bytes());
            hasher.update(&node.value.to_le_bytes());
        }
    }

    // Hash samples
    for sample in &input.samples {
        hasher.update(&sample.id);
        for &f in &sample.features {
            hasher.update(&f.to_le_bytes());
        }
    }

    hasher.update(&input.threshold.to_le_bytes());

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
