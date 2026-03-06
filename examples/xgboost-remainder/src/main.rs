//! XGBoost Decision Tree Inference with Remainder (GKR+Hyrax)
//!
//! This example demonstrates:
//! 1. Building a GKR circuit for XGBoost decision tree inference
//! 2. Generating a Hyrax polynomial commitment proof (zero-knowledge)
//! 3. Verifying the proof in Rust
//! 4. Serializing the proof to ABI format for on-chain verification

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tracing::Level;

mod abi_encode;
mod circuit;
mod model;
mod proof_abi;

/// XGBoost Remainder prover CLI
#[derive(Parser)]
#[command(name = "xgboost-remainder")]
struct Cli {
    /// Path to XGBoost model JSON
    #[arg(long)]
    model: PathBuf,

    /// Path to input features JSON
    #[arg(long)]
    input: PathBuf,

    /// Output path for ABI-encoded proof
    #[arg(long, default_value = "proof_output.json")]
    output: PathBuf,

    /// Only run inference without proving
    #[arg(long)]
    execute_only: bool,
}

/// Standard input format for the XGBoost circuit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XgboostInput {
    /// Feature vector (private input)
    pub features: Vec<f64>,
    /// Expected prediction class (public, for verification)
    pub expected_class: u32,
}

/// Proof output containing all data needed for on-chain verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOutput {
    /// Serialized proof bytes (hex)
    pub proof_hex: String,
    /// Circuit description hash
    pub circuit_hash: String,
    /// Public inputs (hex)
    pub public_inputs_hex: String,
    /// Predicted class
    pub predicted_class: u32,
    /// Proof size in bytes
    pub proof_size_bytes: usize,
    /// Proving time in milliseconds
    pub prove_time_ms: u64,
}

fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let cli = Cli::parse();

    // Load model
    let model_json = std::fs::read_to_string(&cli.model)?;
    let model: model::XgboostModel = serde_json::from_str(&model_json)?;
    println!(
        "Loaded XGBoost model: {} trees, {} features, max_depth={}",
        model.trees.len(),
        model.num_features,
        model.max_depth
    );

    // Load input
    let input_json = std::fs::read_to_string(&cli.input)?;
    let input: XgboostInput = serde_json::from_str(&input_json)?;
    println!("Input features: {:?}", input.features);

    // Run inference (no proof)
    let predicted_class = model::predict(&model, &input.features);
    println!("Predicted class: {}", predicted_class);

    if cli.execute_only {
        println!("Execute-only mode, skipping proof generation");
        return Ok(());
    }

    // Build circuit and generate Hyrax proof
    println!("Building GKR circuit and generating Hyrax proof...");
    let start = std::time::Instant::now();
    let (proof_bytes, circuit_hash, public_inputs) =
        circuit::build_and_prove(&model, &input.features, predicted_class)?;
    let prove_time = start.elapsed();

    println!(
        "Proof generated and verified in {:.2}s",
        prove_time.as_secs_f64()
    );
    println!("Proof size: {} bytes", proof_bytes.len());

    // proof_bytes is already ABI-encoded by abi_encode::encode_hyrax_proof
    let output = ProofOutput {
        proof_hex: hex::encode(&proof_bytes),
        circuit_hash: hex::encode(&circuit_hash),
        public_inputs_hex: hex::encode(&public_inputs),
        predicted_class,
        proof_size_bytes: proof_bytes.len(),
        prove_time_ms: prove_time.as_millis() as u64,
    };

    // Write output
    let output_json = serde_json::to_string_pretty(&output)?;
    std::fs::write(&cli.output, &output_json)?;
    println!("Proof output written to {}", cli.output.display());

    Ok(())
}
