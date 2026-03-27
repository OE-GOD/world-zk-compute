//! worldzk -- interactive CLI for World ZK Compute.
//!
//! Wraps the gateway API for terminal-based proof management. Supports model
//! upload, proof generation, verification, and service health monitoring.
//!
//! # Usage
//!
//! ```text
//! worldzk upload-model model.json --name my-model
//! worldzk models
//! worldzk prove --model <id> --features "1.0,2.5,3.0"
//! worldzk verify <proof-id-or-file>
//! worldzk receipt <proof-id>
//! worldzk proofs --model <hash> --limit 20
//! worldzk health
//! worldzk stats
//! ```

use clap::{Parser, Subcommand};
use colored::Colorize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "worldzk",
    about = "Verifiable AI Inference CLI",
    version,
    after_help = "Set WORLDZK_URL and WORLDZK_API_KEY environment variables for defaults."
)]
struct Cli {
    /// Gateway URL
    #[arg(long, env = "WORLDZK_URL", default_value = "http://localhost:8080")]
    url: String,

    /// API key for authenticated endpoints
    #[arg(long, env = "WORLDZK_API_KEY")]
    api_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload a model
    UploadModel {
        /// Path to model JSON file
        path: String,
        /// Model name
        #[arg(long)]
        name: Option<String>,
        /// Model format (auto-detected if omitted)
        #[arg(long)]
        format: Option<String>,
    },
    /// List registered models
    Models,
    /// Generate a proof
    Prove {
        /// Model ID
        #[arg(long)]
        model: String,
        /// Features as comma-separated values
        #[arg(long)]
        features: String,
    },
    /// Verify a proof (by proof ID or path to proof bundle JSON)
    Verify {
        /// Proof ID or path to proof bundle JSON file
        proof: String,
    },
    /// Get verification receipt for a proof
    Receipt {
        /// Proof ID
        proof_id: String,
    },
    /// List / search proofs
    Proofs {
        /// Filter by model hash
        #[arg(long)]
        model: Option<String>,
        /// Maximum number of results
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Check service health
    Health,
    /// Show service statistics
    Stats,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120));

    // Build default headers with API key if provided.
    if let Some(ref key) = cli.api_key {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        builder = builder.default_headers(headers);
    }

    let client = builder.build().expect("failed to build HTTP client");

    let result = match cli.command {
        Commands::UploadModel { path, name, format } => {
            cmd_upload_model(&client, &cli.url, &path, name, format).await
        }
        Commands::Models => cmd_models(&client, &cli.url).await,
        Commands::Prove { model, features } => {
            cmd_prove(&client, &cli.url, &model, &features).await
        }
        Commands::Verify { proof } => cmd_verify(&client, &cli.url, &proof).await,
        Commands::Receipt { proof_id } => cmd_receipt(&client, &cli.url, &proof_id).await,
        Commands::Proofs { model, limit } => cmd_proofs(&client, &cli.url, model, limit).await,
        Commands::Health => cmd_health(&client, &cli.url).await,
        Commands::Stats => cmd_stats(&client, &cli.url).await,
    };

    if let Err(e) = result {
        eprintln!("{} {e}", "Error:".red().bold());
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Print a labeled key-value line.
fn kv(key: &str, value: &str) {
    println!("  {} {}", format!("{key}:").dimmed(), value);
}

/// Extract an error message from a JSON response body if possible.
fn extract_error(body: &Value) -> String {
    body.get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error")
        .to_string()
}

/// Send a request and handle non-success status codes uniformly.
async fn check_response(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse response: {e}"))?;

    if !status.is_success() {
        return Err(format!(
            "HTTP {} -- {}",
            status.as_u16(),
            extract_error(&body)
        ));
    }
    Ok(body)
}

fn load_json(path: &str) -> Result<Value, String> {
    let data = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("parse {path}: {e}"))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn cmd_upload_model(
    client: &reqwest::Client,
    url: &str,
    path: &str,
    name: Option<String>,
    format: Option<String>,
) -> Result<(), String> {
    let model_json = load_json(path)?;

    // Derive a default name from the file stem if not provided.
    let model_name = name.unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string()
    });

    let mut body = serde_json::json!({
        "name": model_name,
        "model_json": model_json,
    });

    if let Some(fmt) = format {
        body["format"] = serde_json::Value::String(fmt);
    }

    let resp = client
        .post(format!("{url}/v1/models"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    println!("{}", "Model uploaded successfully.".green().bold());
    kv("model_id", result["model_id"].as_str().unwrap_or("?"));
    kv("model_hash", result["model_hash"].as_str().unwrap_or("?"));
    kv(
        "circuit_hash",
        result["circuit_hash"].as_str().unwrap_or("?"),
    );
    kv("format", result["format"].as_str().unwrap_or("?"));
    kv(
        "active",
        &result["active"].as_bool().unwrap_or(false).to_string(),
    );

    Ok(())
}

async fn cmd_models(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let resp = client
        .get(format!("{url}/v1/models"))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    let models = result["models"].as_array();
    let count = models.map(|m| m.len()).unwrap_or(0);

    println!(
        "{}",
        format!("Models ({count} registered):").cyan().bold()
    );

    if let Some(models) = models {
        if models.is_empty() {
            println!("  {}", "(none)".dimmed());
        }
        for m in models {
            let id = m["id"].as_str().unwrap_or("?");
            let name = m["name"].as_str().unwrap_or("?");
            let fmt = m["format"].as_str().unwrap_or("?");
            let active = m["active"].as_bool().unwrap_or(false);
            let hash = m["model_hash"].as_str().unwrap_or("?");

            let status = if active {
                "active".green().to_string()
            } else {
                "inactive".yellow().to_string()
            };

            println!("  {} {} [{}] ({})", id.bold(), name, fmt, status);
            println!("    hash: {hash}");
        }
    }

    Ok(())
}

async fn cmd_prove(
    client: &reqwest::Client,
    url: &str,
    model_id: &str,
    features_str: &str,
) -> Result<(), String> {
    let features: Vec<f64> = features_str
        .split(',')
        .map(|s| {
            s.trim()
                .parse()
                .map_err(|e| format!("invalid feature value '{s}': {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    println!(
        "{}",
        "Generating proof (this may take a while)...".dimmed()
    );

    let body = serde_json::json!({ "model_id": model_id, "features": features });
    let resp = client
        .post(format!("{url}/v1/prove"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    println!("{}", "Proof generated.".green().bold());
    kv("proof_id", result["proof_id"].as_str().unwrap_or("?"));
    kv("model_hash", result["model_hash"].as_str().unwrap_or("?"));
    kv(
        "circuit_hash",
        result["circuit_hash"].as_str().unwrap_or("?"),
    );
    kv("output", result["output"].as_str().unwrap_or("?"));
    kv(
        "mock",
        &result["mock"].as_bool().unwrap_or(false).to_string(),
    );
    kv(
        "prove_time_ms",
        &result["prove_time_ms"]
            .as_u64()
            .map(|v| format!("{v}ms"))
            .unwrap_or_else(|| "?".to_string()),
    );

    if let Some(reg_id) = result["registry_proof_id"].as_str() {
        kv("registry_proof_id", reg_id);
    }

    Ok(())
}

async fn cmd_verify(client: &reqwest::Client, url: &str, proof: &str) -> Result<(), String> {
    // Determine if the argument is a file path or a proof ID.
    let is_file = std::path::Path::new(proof).exists();

    if is_file {
        // File-based verification: POST the bundle to /v1/verify.
        let bundle = load_json(proof)?;
        let resp = client
            .post(format!("{url}/v1/verify"))
            .json(&bundle)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        let result = check_response(resp).await?;
        print_verify_result(&result);
    } else {
        // Proof-ID-based verification: POST to /v1/proofs/{id}/verify.
        let resp = client
            .post(format!("{url}/v1/proofs/{proof}/verify"))
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        let result = check_response(resp).await?;
        print_verify_result(&result);
    }

    Ok(())
}

fn print_verify_result(result: &Value) {
    let verified = result["verified"].as_bool().unwrap_or(false);

    if verified {
        println!("{}", "VERIFIED".green().bold());
    } else {
        println!("{}", "NOT VERIFIED".red().bold());
    }

    if let Some(receipt_id) = result["receipt_id"].as_str() {
        kv("receipt_id", receipt_id);
    }
    if let Some(err) = result["error"].as_str() {
        kv("error", err);
    }
}

async fn cmd_receipt(client: &reqwest::Client, url: &str, proof_id: &str) -> Result<(), String> {
    let resp = client
        .get(format!("{url}/v1/proofs/{proof_id}/receipt"))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    println!("{}", "Verification Receipt".cyan().bold());
    kv("proof_id", result["proof_id"].as_str().unwrap_or("?"));
    kv("verified", &result["verified"].to_string());
    kv(
        "circuit_hash",
        result["circuit_hash"].as_str().unwrap_or("?"),
    );
    kv(
        "model_hash",
        result["model_hash"].as_str().unwrap_or("?"),
    );
    kv("timestamp", result["timestamp"].as_str().unwrap_or("?"));
    kv("signature", result["signature"].as_str().unwrap_or("?"));

    if let Some(err) = result["error"].as_str() {
        kv("error", err);
    }

    Ok(())
}

async fn cmd_proofs(
    client: &reqwest::Client,
    url: &str,
    model: Option<String>,
    limit: usize,
) -> Result<(), String> {
    let mut query = format!("{url}/v1/proofs?limit={limit}");
    if let Some(ref m) = model {
        query.push_str(&format!("&model={m}"));
    }

    let resp = client
        .get(&query)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    let count = result["count"].as_u64().unwrap_or(0);
    println!("{}", format!("Proofs ({count} found):").cyan().bold());

    if let Some(proofs) = result["proofs"].as_array() {
        if proofs.is_empty() {
            println!("  {}", "(none)".dimmed());
        }
        for p in proofs {
            let id = p["id"].as_str().unwrap_or("?");
            let model_hash = p["model_hash"].as_str().unwrap_or("?");
            let verified = p["verified"].as_bool();
            let ts = p["submitted_at"].as_str().unwrap_or("?");

            let status = match verified {
                Some(true) => "verified".green().to_string(),
                Some(false) => "failed".red().to_string(),
                None => "pending".yellow().to_string(),
            };

            println!("  {} | model={} | {} | {}", id.bold(), model_hash, status, ts);
        }
    }

    Ok(())
}

async fn cmd_health(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let resp = client
        .get(format!("{url}/health"))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    let status_str = result["status"].as_str().unwrap_or("unknown");
    let status_colored = match status_str {
        "healthy" => status_str.green().bold().to_string(),
        "degraded" => status_str.yellow().bold().to_string(),
        _ => status_str.red().bold().to_string(),
    };

    println!("Gateway: {status_colored}");

    let healthy = result["healthy_count"].as_u64().unwrap_or(0);
    let total = result["total_count"].as_u64().unwrap_or(0);
    println!(
        "  {} of {} services healthy",
        healthy.to_string().bold(),
        total
    );

    if let Some(services) = result["services"].as_array() {
        for svc in services {
            let name = svc["name"].as_str().unwrap_or("?");
            let ok = svc["healthy"].as_bool().unwrap_or(false);
            let ms = svc["response_ms"]
                .as_u64()
                .map(|v| format!(" ({v}ms)"))
                .unwrap_or_default();

            let indicator = if ok {
                "OK".green().to_string()
            } else {
                "DOWN".red().to_string()
            };

            let error_msg = svc["error"]
                .as_str()
                .map(|e| format!(" -- {e}"))
                .unwrap_or_default();

            println!("  {name}: {indicator}{ms}{error_msg}");
        }
    }

    Ok(())
}

async fn cmd_stats(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let resp = client
        .get(format!("{url}/v1/stats"))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    println!("{}", "Service Statistics".cyan().bold());

    // Print all top-level fields in a readable format.
    if let Some(obj) = result.as_object() {
        for (key, value) in obj {
            let display = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => "null".to_string(),
                _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
            };
            kv(key, &display);
        }
    } else {
        // Fallback: pretty-print the whole response.
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
        );
    }

    Ok(())
}
