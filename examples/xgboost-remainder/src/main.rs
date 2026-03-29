//! XGBoost Decision Tree Inference with Remainder (GKR+Hyrax)
//!
//! This example demonstrates:
//! 1. Building a GKR circuit for XGBoost decision tree inference
//! 2. Generating a Hyrax polynomial commitment proof (zero-knowledge)
//! 3. Verifying the proof in Rust
//! 4. Serializing the proof to ABI format for on-chain verification
//!
//! Modes:
//! - One-shot: `--model model.json --input input.json` (build circuit + prove once)
//! - Warm server: `--model model.json --serve` (build circuit once, serve proofs via HTTP)

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::Level;

mod abi_encode;
mod circuit;
mod detect;
mod lightgbm;
mod logistic_regression;
mod mlp;
mod model;
mod proof_abi;
mod random_forest;
mod server;

/// XGBoost Remainder prover CLI
#[derive(Parser)]
#[command(name = "xgboost-remainder")]
struct Cli {
    /// Path to model JSON file
    #[arg(long)]
    model: PathBuf,

    /// Path to input features JSON (required for one-shot mode)
    #[arg(long)]
    input: Option<PathBuf>,

    /// Output path for ABI-encoded proof (one-shot mode)
    #[arg(long, default_value = "proof_output.json")]
    output: PathBuf,

    /// Only run inference without proving (one-shot mode)
    #[arg(long)]
    execute_only: bool,

    /// Model format: "auto" (detect from JSON), "xgboost", "lightgbm", "random_forest", or "logistic_regression"
    #[arg(long, default_value = "auto")]
    model_format: String,

    /// Start warm prover HTTP server instead of one-shot mode
    #[arg(long)]
    serve: bool,

    /// Host to bind the HTTP server to (default: 127.0.0.1)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port for the HTTP server (default: 3000)
    #[arg(long, default_value_t = 3000)]
    port: u16,

    /// Optional API key for authenticating /prove requests.
    /// When set, POST routes require `Authorization: Bearer <key>` header.
    /// GET /health remains unauthenticated.
    #[arg(long)]
    api_key: Option<String>,

    /// Max requests per minute per IP (0 = unlimited)
    #[arg(long, default_value_t = 60)]
    rate_limit: u32,

    /// Burst allowance above the per-minute rate
    #[arg(long, default_value_t = 10)]
    rate_limit_burst: u32,

    /// Request timeout in seconds (0 = no timeout). Default: 120.
    #[arg(long, default_value_t = 120)]
    request_timeout: u64,

    /// Enable CORS headers for browser clients.
    #[arg(long)]
    enable_cors: bool,

    /// Allowed CORS origins (comma-separated). Only used when --enable-cors is set.
    /// If not specified, allows all origins.
    #[arg(long, value_delimiter = ',')]
    cors_origins: Option<Vec<String>>,

    /// Output a self-contained ProofBundle JSON (proof + generators + circuit desc + metadata).
    /// When set, --output produces a bundle that can be verified by zkml-verifier.
    #[arg(long)]
    bundle: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let cli = Cli::parse();

    // Load model (supports XGBoost, LightGBM, Random Forest, and Logistic Regression JSON formats)
    let detected_format = if cli.model_format == "auto" {
        let fmt = detect::detect_model_format_from_file(&cli.model)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        println!("Auto-detected model format: {}", fmt);
        fmt
    } else {
        cli.model_format.clone()
    };
    let model = match detected_format.as_str() {
        "xgboost" => model::load_xgboost_json(&cli.model).map_err(|e| anyhow::anyhow!("{}", e))?,
        "lightgbm" => {
            lightgbm::load_lightgbm_json(&cli.model).map_err(|e| anyhow::anyhow!("{}", e))?
        }
        "random_forest" => {
            random_forest::load_random_forest_json(&cli.model)
                .map_err(|e| anyhow::anyhow!("{}", e))?
        }
        "logistic_regression" => {
            let lr = logistic_regression::LogisticRegressionModel::from_file(&cli.model)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            lr.to_xgboost_model()
        }
        "mlp" => mlp::load_mlp_json(&cli.model).map_err(|e| anyhow::anyhow!("{}", e))?,
        other => anyhow::bail!(
            "Unknown model format '{}'. Supported: auto, xgboost, lightgbm, random_forest, logistic_regression, mlp",
            other
        ),
    };
    println!(
        "Loaded {} model: {} trees, {} features, max_depth={}",
        detected_format,
        model.trees.len(),
        model.num_features,
        model.max_depth
    );

    if cli.serve {
        // Warm prover server mode
        let config = server::ServerConfig {
            host: cli.host,
            port: cli.port,
            api_key: cli.api_key,
            rate_limit: cli.rate_limit,
            rate_limit_burst: cli.rate_limit_burst,
            request_timeout_secs: cli.request_timeout,
            enable_cors: cli.enable_cors,
            cors_origins: cli.cors_origins,
        };
        server::run_server_with_config(model, config).await?;
        return Ok(());
    }

    // One-shot mode: require --input
    let input_path = cli.input.ok_or_else(|| {
        anyhow::anyhow!("--input is required in one-shot mode. Use --serve for server mode.")
    })?;

    // Load input
    let input_json = std::fs::read_to_string(&input_path)?;
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

    if cli.bundle {
        // Build self-contained ProofBundle JSON with generators and circuit description
        use sha2::Digest;
        let bundle_data =
            circuit::build_and_prove_bundle(&model, &input.features, predicted_class)?;
        let model_bytes = std::fs::read(&cli.model)?;

        let bundle = serde_json::json!({
            "proof_hex": format!("0x{}", hex::encode(&bundle_data.proof_bytes)),
            "gens_hex": format!("0x{}", hex::encode(&bundle_data.gens_bytes)),
            "public_inputs_hex": format!("0x{}", hex::encode(&bundle_data.public_inputs)),
            "dag_circuit_description": hex::encode(&bundle_data.circuit_desc_bytes),
            "model_hash": format!("0x{}", hex::encode(sha2::Sha256::digest(&model_bytes))),
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            "prover_version": env!("CARGO_PKG_VERSION"),
            "circuit_hash": format!("0x{}", hex::encode(&bundle_data.circuit_hash)),
        });

        let output_json = serde_json::to_string_pretty(&bundle)?;
        std::fs::write(&cli.output, &output_json)?;
        println!(
            "ProofBundle written to {} ({} bytes proof, {} bytes gens)",
            cli.output.display(),
            bundle_data.proof_bytes.len(),
            bundle_data.gens_bytes.len()
        );
    } else {
        // Legacy output format
        let output = ProofOutput {
            proof_hex: hex::encode(&proof_bytes),
            circuit_hash: hex::encode(&circuit_hash),
            public_inputs_hex: hex::encode(&public_inputs),
            predicted_class,
            proof_size_bytes: proof_bytes.len(),
            prove_time_ms: prove_time.as_millis() as u64,
        };

        let output_json = serde_json::to_string_pretty(&output)?;
        std::fs::write(&cli.output, &output_json)?;
        println!("Proof output written to {}", cli.output.display());
    }

    Ok(())
}
