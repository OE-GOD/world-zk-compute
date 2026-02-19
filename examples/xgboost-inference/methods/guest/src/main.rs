//! XGBoost Inference Guest Program — Optimized
//!
//! Manual wire format parsing replaces serde for reduced cycle count.
//! The wire format (risc0 serde u32 LE words) is unchanged.
//!
//! Produces a proof that:
//! 1. The model was applied correctly to every sample
//! 2. Tree traversal logic is deterministic and correct
//! 3. Flagging decisions match the threshold exactly
//! 4. The input data (model + samples) was not tampered with

#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec::Vec;
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

// ═══════════════════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════════════════

struct XGBoostModel {
    num_features: u32,
    num_classes: u32,
    base_score: f64,
    trees: Vec<Tree>,
}

struct Tree {
    nodes: Vec<TreeNode>,
}

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

struct Sample {
    id: [u8; 32],
    features: Vec<f64>,
}

struct Prediction {
    id: [u8; 32],
    score: f64,
    /// 0 = not flagged, 1 = flagged
    flagged: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Manual wire format reader — bypasses serde visitor pattern
//
// Wire format (risc0 serde u32 LE words):
//   u32       → 1 word
//   f64       → 2 words (reinterpreted as u64: low, high)
//   u8        → 1 word (zero-extended)
//   [u8; 32]  → 32 words (each byte as u32)
//   Vec<T>    → 1 word (length) + length × T
// ═══════════════════════════════════════════════════════════════════════════════

#[inline(always)]
fn read_word() -> u32 {
    env::read::<u32>()
}

#[inline(always)]
fn read_f64_val() -> f64 {
    let low = read_word() as u64;
    let high = read_word() as u64;
    f64::from_bits((high << 32) | low)
}

fn read_byte_array_32() -> [u8; 32] {
    let mut arr = [0u8; 32];
    for b in arr.iter_mut() {
        *b = read_word() as u8;
    }
    arr
}

fn read_vec_f64() -> Vec<f64> {
    let len = read_word() as usize;
    let mut v = Vec::with_capacity(len);
    for _ in 0..len {
        v.push(read_f64_val());
    }
    v
}

fn read_input() -> (XGBoostModel, Vec<Sample>, f64) {
    // Model
    let num_features = read_word();
    let num_classes = read_word();
    let base_score = read_f64_val();

    // Trees
    let num_trees = read_word() as usize;
    let mut trees = Vec::with_capacity(num_trees);
    for _ in 0..num_trees {
        let num_nodes = read_word() as usize;
        let mut nodes = Vec::with_capacity(num_nodes);
        for _ in 0..num_nodes {
            nodes.push(TreeNode {
                is_leaf: read_word(),
                feature_idx: read_word(),
                threshold: read_f64_val(),
                left_child: read_word(),
                right_child: read_word(),
                value: read_f64_val(),
            });
        }
        trees.push(Tree { nodes });
    }

    let model = XGBoostModel { num_features, num_classes, base_score, trees };

    // Samples
    let num_samples = read_word() as usize;
    let mut samples = Vec::with_capacity(num_samples);
    for _ in 0..num_samples {
        samples.push(Sample {
            id: read_byte_array_32(),
            features: read_vec_f64(),
        });
    }

    // Threshold
    let threshold = read_f64_val();

    (model, samples, threshold)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Manual output serialization — writes u32 words matching risc0 serde format
// ═══════════════════════════════════════════════════════════════════════════════

fn push_f64(words: &mut Vec<u32>, val: f64) {
    let bits = val.to_bits();
    words.push(bits as u32);        // low
    words.push((bits >> 32) as u32); // high
}

fn push_byte_array_32(words: &mut Vec<u32>, arr: &[u8; 32]) {
    for &byte in arr {
        words.push(byte as u32);
    }
}

fn commit_output(
    total_samples: u32,
    predictions: &[Prediction],
    flagged_count: u32,
    flagged_ids: &[[u8; 32]],
    input_hash: &[u8; 32],
) {
    let mut words = Vec::with_capacity(
        1 + 1 + predictions.len() * 35 + 1 + 1 + flagged_ids.len() * 32 + 32,
    );

    // total_samples: u32
    words.push(total_samples);

    // predictions: Vec<Prediction>
    words.push(predictions.len() as u32);
    for pred in predictions {
        push_byte_array_32(&mut words, &pred.id);   // id: [u8; 32]
        push_f64(&mut words, pred.score);            // score: f64
        words.push(pred.flagged);                     // flagged: u32
    }

    // flagged_count: u32
    words.push(flagged_count);

    // flagged_ids: Vec<[u8; 32]>
    words.push(flagged_ids.len() as u32);
    for id in flagged_ids {
        push_byte_array_32(&mut words, id);
    }

    // input_hash: [u8; 32]
    push_byte_array_32(&mut words, input_hash);

    env::commit_slice(&words);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main entry point
// ═══════════════════════════════════════════════════════════════════════════════

fn main() {
    let (model, samples, threshold) = read_input();

    let input_hash = compute_input_hash(&model, &samples, threshold);

    let mut predictions = Vec::with_capacity(samples.len());
    let mut flagged_count = 0u32;
    let mut flagged_ids = Vec::new();

    for sample in &samples {
        let score = predict_sample(&model, sample);
        let flagged = if score >= threshold { 1u32 } else { 0u32 };

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

    commit_output(
        samples.len() as u32,
        &predictions,
        flagged_count,
        &flagged_ids,
        &input_hash,
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tree traversal & prediction
// ═══════════════════════════════════════════════════════════════════════════════

/// Traverse a single tree iteratively, returning the leaf value.
fn traverse_tree(tree: &Tree, features: &[f64]) -> f64 {
    let num_nodes = tree.nodes.len();
    if num_nodes == 0 {
        return 0.0;
    }

    let mut node_idx = 0u32;

    for _ in 0..num_nodes {
        if node_idx as usize >= num_nodes {
            return 0.0;
        }

        let node = &tree.nodes[node_idx as usize];

        if node.is_leaf == 1 {
            return node.value;
        }

        let feat_idx = node.feature_idx as usize;
        let feat_val = if feat_idx < features.len() {
            features[feat_idx]
        } else {
            0.0
        };

        if feat_val < node.threshold {
            node_idx = node.left_child;
        } else {
            node_idx = node.right_child;
        }
    }

    0.0
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

fn compute_input_hash(model: &XGBoostModel, samples: &[Sample], threshold: f64) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    // Hash model structure
    hasher.update(&model.num_features.to_le_bytes());
    hasher.update(&model.num_classes.to_le_bytes());
    hasher.update(&model.base_score.to_le_bytes());

    for tree in &model.trees {
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
    for sample in samples {
        hasher.update(&sample.id);
        for &f in &sample.features {
            hasher.update(&f.to_le_bytes());
        }
    }

    hasher.update(&threshold.to_le_bytes());

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
