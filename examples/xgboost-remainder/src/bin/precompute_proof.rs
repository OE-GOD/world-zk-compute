//! Pre-compute a GKR+Hyrax proof for XGBoost inference.
//!
//! Generates a ZK proof that can be used for on-chain dispute resolution
//! via TEEMLVerifier.resolveDispute().
//!
//! Usage:
//!   # With direct-format model (XgboostModel JSON):
//!   cargo run --release --bin precompute_proof -- \
//!     --model examples/xgboost-remainder/sample_model.json \
//!     --features '[0.6, 0.2, 0.8, 0.5, 0.3]' \
//!     --output /tmp/proof.json
//!
//!   # With built-in sample model:
//!   cargo run --release --bin precompute_proof -- \
//!     --sample \
//!     --features '[0.6, 0.2, 0.8, 0.5, 0.3]' \
//!     --output /tmp/proof.json

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use serde_json::json;
use shared_types::pedersen::PedersenCommitter;

use xgboost_remainder::abi_encode::encode_pedersen_gens;
use xgboost_remainder::circuit::build_and_prove;
use xgboost_remainder::lightgbm;
use xgboost_remainder::model::{self, predict, XgboostModel};

#[derive(Parser)]
#[command(name = "precompute_proof")]
#[command(about = "Generate GKR+Hyrax proof for XGBoost inference")]
struct Args {
    /// Path to model JSON file (XgboostModel format or XGBoost native format)
    #[arg(long)]
    model: Option<PathBuf>,

    /// Use the built-in sample model (2 trees, 5 features, depth 2)
    #[arg(long)]
    sample: bool,

    /// Model format: "xgboost" (default) or "lightgbm"
    #[arg(long, default_value = "xgboost")]
    model_format: String,

    /// Feature vector as JSON array string, e.g. '[0.6, 0.2, 0.8, 0.5, 0.3]'
    #[arg(long)]
    features: String,

    /// Output file path (if omitted, prints to stdout)
    #[arg(long)]
    output: Option<PathBuf>,
}

/// Load a model from a JSON file using the specified format.
fn load_model(path: &std::path::Path, model_format: &str) -> Result<XgboostModel> {
    match model_format {
        "lightgbm" => {
            eprintln!("  (loading as LightGBM format)");
            lightgbm::load_lightgbm_json(path).map_err(|e| anyhow::anyhow!("{}", e))
        }
        "xgboost" => {
            let data = std::fs::read_to_string(path)?;

            // Try direct XgboostModel deserialization first
            if let Ok(m) = serde_json::from_str::<XgboostModel>(&data) {
                eprintln!("  (loaded as XgboostModel format)");
                return Ok(m);
            }

            // Fall back to XGBoost native format
            model::load_xgboost_json(path).map_err(|e| anyhow::anyhow!("Failed to load model: {e}"))
        }
        other => anyhow::bail!(
            "Unknown model format '{}'. Supported: xgboost, lightgbm",
            other
        ),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.model.is_none() && !args.sample {
        anyhow::bail!("Either --model <path> or --sample is required");
    }

    // Parse features
    let features: Vec<f64> = serde_json::from_str(&args.features)?;

    // Load model
    let m = if args.sample {
        eprintln!("Using built-in sample model...");
        model::sample_model()
    } else {
        let path = args
            .model
            .as_ref()
            .expect("--model is required when --sample is not set (checked above)");
        eprintln!("Loading {} model from {:?}...", args.model_format, path);
        load_model(path, &args.model_format)?
    };
    eprintln!(
        "  Model: {} trees, {} features, {} classes",
        m.trees.len(),
        m.num_features,
        m.num_classes
    );

    // Predict class
    let predicted_class = predict(&m, &features);
    eprintln!("  Predicted class: {}", predicted_class);

    // Generate proof
    eprintln!("Generating proof...");
    let start = Instant::now();
    let (proof_bytes, circuit_hash, public_inputs) =
        build_and_prove(&m, &features, predicted_class)?;
    let prove_time = start.elapsed();
    eprintln!("  Proof generated in {:.1}s", prove_time.as_secs_f64());

    // Generate Pedersen gens data
    let committer = PedersenCommitter::new(512, "xgboost-remainder Pedersen committer", None);
    let gens_bytes = encode_pedersen_gens(&committer)?;
    eprintln!("  Gens data: {} bytes", gens_bytes.len());

    // Build output
    let output = json!({
        "proof_hex": format!("0x{}", hex::encode(&proof_bytes)),
        "circuit_hash": format!("0x{}", hex::encode(&circuit_hash)),
        "public_inputs_hex": format!("0x{}", hex::encode(&public_inputs)),
        "gens_hex": format!("0x{}", hex::encode(&gens_bytes)),
        "predicted_class": predicted_class,
        "prove_time_ms": prove_time.as_millis() as u64,
    });

    let json_str = serde_json::to_string_pretty(&output)?;

    if let Some(path) = args.output {
        std::fs::write(&path, &json_str)?;
        eprintln!("Output written to {:?}", path);
    } else {
        println!("{}", json_str);
    }

    Ok(())
}
