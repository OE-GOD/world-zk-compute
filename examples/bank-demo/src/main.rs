//! Bank credit scoring demo.
//!
//! Demonstrates the end-to-end ZKML workflow:
//! 1. Load an XGBoost credit scoring model
//! 2. Run inference on applicant features
//! 3. Hash model and inputs (simulating what the TEE enclave does)
//! 4. Display the result with cryptographic commitments

use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Bank credit scoring demo CLI.
#[derive(Parser)]
#[command(name = "bank-demo", about = "ZKML credit scoring demo")]
struct Cli {
    /// Path to the XGBoost model JSON file.
    #[arg(short, long, default_value = "examples/bank-demo/models/credit_scoring.json")]
    model: PathBuf,

    /// Credit score (300-850).
    #[arg(long, default_value_t = 720.0)]
    credit_score: f64,

    /// Annual income in USD.
    #[arg(long, default_value_t = 65000.0)]
    income: f64,

    /// Number of years of credit history.
    #[arg(long, default_value_t = 8.0)]
    credit_history_years: f64,

    /// Debt-to-income ratio (0.0-1.0).
    #[arg(long, default_value_t = 0.25)]
    dti_ratio: f64,

    /// Number of existing credit accounts.
    #[arg(long, default_value_t = 4.0)]
    num_accounts: f64,

    /// Number of recent hard inquiries.
    #[arg(long, default_value_t = 1.0)]
    recent_inquiries: f64,

    /// Output as JSON.
    #[arg(long)]
    json: bool,
}

/// XGBoost model JSON structure (subset).
#[derive(Deserialize)]
struct XgbModel {
    learner: Learner,
}

#[derive(Deserialize)]
struct Learner {
    learner_model_param: LearnerParam,
    gradient_booster: GradientBooster,
}

#[derive(Deserialize)]
struct LearnerParam {
    num_feature: String,
}

#[derive(Deserialize)]
struct GradientBooster {
    model: GbTreeModel,
}

#[derive(Deserialize)]
struct GbTreeModel {
    trees: Vec<Tree>,
}

#[derive(Deserialize)]
struct Tree {
    left_children: Vec<i32>,
    right_children: Vec<i32>,
    split_indices: Vec<usize>,
    split_conditions: Vec<f64>,
    base_weights: Vec<f64>,
}

/// Inference result with cryptographic commitments.
#[derive(Serialize)]
struct InferenceResult {
    /// Raw score from the model (sum of tree leaf values).
    raw_score: f64,
    /// Probability (sigmoid of raw score).
    approval_probability: f64,
    /// Decision based on 0.5 threshold.
    decision: String,
    /// SHA-256 hash of the model file.
    model_hash: String,
    /// SHA-256 hash of the input features.
    input_hash: String,
    /// SHA-256 hash of the result.
    result_hash: String,
    /// Number of trees in the model.
    num_trees: usize,
    /// Number of features expected.
    num_features: usize,
    /// Input features used.
    features: Vec<f64>,
    /// Feature names.
    feature_names: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    // 1. Load model
    let model_bytes = std::fs::read(&cli.model).unwrap_or_else(|e| {
        eprintln!("Error loading model {}: {}", cli.model.display(), e);
        std::process::exit(1);
    });
    let model: XgbModel = serde_json::from_slice(&model_bytes).unwrap_or_else(|e| {
        eprintln!("Error parsing model: {}", e);
        std::process::exit(1);
    });

    let num_features: usize = model.learner.learner_model_param.num_feature.parse().unwrap_or(6);
    let trees = &model.learner.gradient_booster.model.trees;

    // 2. Prepare features
    let features = vec![
        cli.credit_score,
        cli.income,
        cli.credit_history_years,
        cli.dti_ratio,
        cli.num_accounts,
        cli.recent_inquiries,
    ];

    let feature_names = vec![
        "credit_score".to_string(),
        "annual_income".to_string(),
        "credit_history_years".to_string(),
        "debt_to_income_ratio".to_string(),
        "num_accounts".to_string(),
        "recent_inquiries".to_string(),
    ];

    // 3. Run inference
    let mut raw_score = 0.0_f64;
    for tree in trees {
        raw_score += traverse_tree(tree, &features);
    }
    let approval_prob = sigmoid(raw_score);
    let decision = if approval_prob >= 0.5 { "APPROVED" } else { "DENIED" };

    // 4. Compute hashes
    let model_hash = sha256_hex(&model_bytes);
    let input_json = serde_json::to_string(&features).unwrap();
    let input_hash = sha256_hex(input_json.as_bytes());
    let result_bytes = format!("{:.6}", raw_score);
    let result_hash = sha256_hex(result_bytes.as_bytes());

    let result = InferenceResult {
        raw_score,
        approval_probability: approval_prob,
        decision: decision.to_string(),
        model_hash,
        input_hash,
        result_hash,
        num_trees: trees.len(),
        num_features,
        features,
        feature_names,
    };

    // 5. Output
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    } else {
        println!("=== Bank Credit Scoring Demo ===");
        println!();
        println!("Model: {} ({} trees, {} features)", cli.model.display(), result.num_trees, result.num_features);
        println!("Model hash: 0x{}", &result.model_hash[..16]);
        println!();
        println!("--- Applicant ---");
        for (name, val) in result.feature_names.iter().zip(result.features.iter()) {
            println!("  {:<25} {:.2}", name, val);
        }
        println!();
        println!("--- Result ---");
        println!("  Raw score:            {:.6}", result.raw_score);
        println!("  Approval probability: {:.2}%", result.approval_probability * 100.0);
        println!("  Decision:             {}", result.decision);
        println!();
        println!("--- Cryptographic Commitments ---");
        println!("  Model hash:  0x{}", result.model_hash);
        println!("  Input hash:  0x{}", result.input_hash);
        println!("  Result hash: 0x{}", result.result_hash);
        println!();
        println!("These hashes would be signed by the TEE enclave and submitted on-chain.");
        println!("A ZK proof can verify the inference without revealing the model or inputs.");
    }
}

/// Traverse an XGBoost tree to find the leaf value.
fn traverse_tree(tree: &Tree, features: &[f64]) -> f64 {
    let mut node = 0usize;
    loop {
        let left = tree.left_children[node];
        if left == -1 {
            return tree.base_weights[node];
        }
        let feature_idx = tree.split_indices[node];
        let threshold = tree.split_conditions[node];
        let feature_val = if feature_idx < features.len() {
            features[feature_idx]
        } else {
            0.0
        };
        node = if feature_val < threshold {
            left as usize
        } else {
            tree.right_children[node] as usize
        };
    }
}

/// Sigmoid function.
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// SHA-256 hash as hex string.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigmoid() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-10);
        assert!(sigmoid(10.0) > 0.99);
        assert!(sigmoid(-10.0) < 0.01);
    }

    #[test]
    fn test_traverse_simple_tree() {
        let tree = Tree {
            left_children: vec![1, -1, -1],
            right_children: vec![2, -1, -1],
            split_indices: vec![0, 0, 0],
            split_conditions: vec![5.0, 0.0, 0.0],
            base_weights: vec![0.0, -0.5, 0.5],
        };
        // feature[0] = 3.0 < 5.0 → go left → leaf value -0.5
        assert_eq!(traverse_tree(&tree, &[3.0]), -0.5);
        // feature[0] = 7.0 >= 5.0 → go right → leaf value 0.5
        assert_eq!(traverse_tree(&tree, &[7.0]), 0.5);
    }

    #[test]
    fn test_sha256_deterministic() {
        let h1 = sha256_hex(b"hello");
        let h2 = sha256_hex(b"hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_load_credit_scoring_model() {
        let model_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/models/credit_scoring.json"
        );
        let data = std::fs::read(model_path).expect("model file not found");
        let model: XgbModel = serde_json::from_slice(&data).expect("failed to parse model");
        let trees = &model.learner.gradient_booster.model.trees;
        assert_eq!(trees.len(), 3);
        let num_features: usize = model.learner.learner_model_param.num_feature.parse().unwrap();
        assert_eq!(num_features, 6);
    }

    #[test]
    fn test_inference_credit_scoring() {
        let model_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/models/credit_scoring.json"
        );
        let data = std::fs::read(model_path).unwrap();
        let model: XgbModel = serde_json::from_slice(&data).unwrap();
        let trees = &model.learner.gradient_booster.model.trees;

        // Good applicant: high credit score, good income
        let good = vec![750.0, 100000.0, 15.0, 0.15, 6.0, 0.0];
        let mut score = 0.0;
        for tree in trees {
            score += traverse_tree(tree, &good);
        }
        let prob = sigmoid(score);
        assert!(prob > 0.5, "good applicant should be approved, got {:.3}", prob);

        // Risky applicant: low credit score, low income
        let risky = vec![550.0, 25000.0, 1.0, 0.8, 1.0, 5.0];
        let mut score2 = 0.0;
        for tree in trees {
            score2 += traverse_tree(tree, &risky);
        }
        let prob2 = sigmoid(score2);
        assert!(prob2 < 0.5, "risky applicant should be denied, got {:.3}", prob2);
    }
}
