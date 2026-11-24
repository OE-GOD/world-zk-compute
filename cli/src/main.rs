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
//!
//! Proof IDs for `verify`, `receipt` and `proofs` are proof-registry IDs. The
//! `proof_id` printed by `prove` is the generator's own and is never known to
//! the registry: use the `registry_proof_id` printed next to it, when there is
//! one.
//!
//! `verify` exits with a non-zero status when the proof does not verify.

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

    // "http://host:8080/" and "http://host:8080" must hit the same routes.
    let url = cli.url.trim_end_matches('/');

    let result = match cli.command {
        Commands::UploadModel { path, name, format } => {
            cmd_upload_model(&client, url, &path, name, format).await
        }
        Commands::Models => cmd_models(&client, url).await,
        Commands::Prove { model, features } => cmd_prove(&client, url, &model, &features).await,
        Commands::Verify { proof } => match cmd_verify(&client, url, &proof).await {
            Ok(true) => Ok(()),
            // "NOT VERIFIED" is already printed; only the exit status is left.
            Ok(false) => std::process::exit(1),
            Err(e) => Err(e),
        },
        Commands::Receipt { proof_id } => cmd_receipt(&client, url, &proof_id).await,
        Commands::Proofs { model, limit } => cmd_proofs(&client, url, model, limit).await,
        Commands::Health => cmd_health(&client, url).await,
        Commands::Stats => cmd_stats(&client, url).await,
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
///
/// Error responses are not always JSON (a backend answers an unknown
/// sub-path with an empty 404, which the gateway forwards), so the status is
/// reported even when the body cannot be parsed.
async fn check_response(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("failed to read response: {e}"))?;
    interpret_response(status.as_u16(), &text)
}

/// Turn an HTTP status and body into the JSON payload or an error message.
fn interpret_response(status: u16, text: &str) -> Result<Value, String> {
    let body: Result<Value, _> = serde_json::from_str(text);

    if !(200..300).contains(&status) {
        let detail = match &body {
            Ok(json) => extract_error(json),
            Err(_) if text.trim().is_empty() => "empty response body".to_string(),
            Err(_) => text.trim().chars().take(200).collect(),
        };
        return Err(format!("HTTP {status} -- {detail}"));
    }
    body.map_err(|e| format!("HTTP {status} -- response is not JSON: {e}"))
}

fn load_json(path: &str) -> Result<Value, String> {
    let data = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("parse {path}: {e}"))
}

/// Build `{base}/v1/proofs/{proof_id}/{action}` with the ID percent-encoded
/// as one path segment.
fn proof_url(base: &str, proof_id: &str, action: &str) -> Result<reqwest::Url, String> {
    // `Url` drops these two segments instead of encoding them, which would
    // send the request to a different path.
    if proof_id == "." || proof_id == ".." {
        return Err(format!("invalid proof ID '{proof_id}'"));
    }
    let mut url =
        reqwest::Url::parse(base).map_err(|e| format!("invalid gateway URL '{base}': {e}"))?;
    url.path_segments_mut()
        .map_err(|()| format!("invalid gateway URL '{base}'"))?
        .pop_if_empty()
        .extend(["v1", "proofs", proof_id, action]);
    Ok(url)
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

    println!("{}", format!("Models ({count} registered):").cyan().bold());

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

    println!("{}", "Generating proof (this may take a while)...".dimmed());

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

    match result["registry_proof_id"].as_str() {
        Some(reg_id) => kv("registry_proof_id", reg_id),
        None => kv(
            "registry_proof_id",
            "(none: the proof was not stored in the registry, so `verify`/`receipt` cannot look it up)",
        ),
    }

    Ok(())
}

/// Returns whether the service reported the proof as verified.
async fn cmd_verify(client: &reqwest::Client, url: &str, proof: &str) -> Result<bool, String> {
    // Determine if the argument is a file path or a proof ID.
    let is_file = std::path::Path::new(proof).exists();

    let result = if is_file {
        // File-based verification: POST the bundle to /v1/verify.
        let bundle = load_json(proof)?;
        let resp = client
            .post(format!("{url}/v1/verify"))
            .json(&bundle)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        check_response(resp).await?
    } else {
        // Proof-ID-based verification: POST to /v1/proofs/{id}/verify.
        let resp = client
            .post(proof_url(url, proof, "verify")?)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        check_response(resp).await?
    };

    Ok(print_verify_result(&result))
}

/// Print a verification response and return its `verified` flag.
fn print_verify_result(result: &Value) -> bool {
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

    verified
}

async fn cmd_receipt(client: &reqwest::Client, url: &str, proof_id: &str) -> Result<(), String> {
    let resp = client
        .get(proof_url(url, proof_id, "receipt")?)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let result = check_response(resp).await?;

    println!("{}", "Verification Receipt".cyan().bold());
    for (key, value) in receipt_fields(&result) {
        kv(key, &value);
    }

    Ok(())
}

/// The fields of a registry verification receipt, in display order.
fn receipt_fields(receipt: &Value) -> Vec<(&'static str, String)> {
    let text = |key: &str| receipt[key].as_str().unwrap_or("?").to_string();
    let mut fields = vec![
        ("receipt_id", text("receipt_id")),
        ("proof_id", text("proof_id")),
        ("verified", receipt["verified"].to_string()),
        ("circuit_hash", text("circuit_hash")),
        ("model_hash", text("model_hash")),
        ("verified_at", text("verified_at")),
        ("verifier_version", text("verifier_version")),
        ("verifier_host", text("verifier_host")),
        ("signature", text("signature")),
    ];
    if let Some(err) = receipt["error"].as_str() {
        fields.push(("error", err.to_string()));
    }
    fields
}

async fn cmd_proofs(
    client: &reqwest::Client,
    url: &str,
    model: Option<String>,
    limit: usize,
) -> Result<(), String> {
    let mut query = vec![("limit", limit.to_string())];
    if let Some(m) = model {
        query.push(("model", m));
    }

    let resp = client
        .get(format!("{url}/v1/proofs"))
        .query(&query)
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
            let (id, model_hash, verified, created_at) = proof_row(p);

            let status = match verified {
                Some(true) => "verified".green().to_string(),
                Some(false) => "failed".red().to_string(),
                None => "pending".yellow().to_string(),
            };

            println!(
                "  {} | model={} | {} | {}",
                id.bold(),
                model_hash,
                status,
                created_at
            );
        }
    }

    Ok(())
}

/// One entry of the registry's proof listing: (id, model hash, verified, created_at).
fn proof_row(proof: &Value) -> (&str, &str, Option<bool>, &str) {
    (
        proof["id"].as_str().unwrap_or("?"),
        proof["model_hash"].as_str().unwrap_or("?"),
        proof["verified"].as_bool(),
        proof["created_at"].as_str().unwrap_or("?"),
    )
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Serve one canned HTTP response on a local port and return the base URL
    /// plus a handle that yields the raw request the client sent.
    fn serve_once(status_line: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            // Read until the end of the headers (the commands under test send no body).
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (url, handle)
    }

    // Response bodies below were captured from the gateway, proof-registry and
    // proof-generator built from this repository.

    #[test]
    fn proof_row_reads_registry_listing_fields() {
        let listing: Value = serde_json::from_str(
            r#"{"proofs":[{"id":"4d7b629e-41d5-420e-88a9-ede094a11f3b","model_hash":"","circuit_hash":"","created_at":"2026-11-22T21:08:09.248575+00:00","verified":true,"verified_at":"2026-11-22T21:08:41.495499+00:00","proof_size_bytes":348029,"deleted":false}],"count":1}"#,
        )
        .unwrap();
        let (id, model_hash, verified, created_at) = proof_row(&listing["proofs"][0]);
        assert_eq!(id, "4d7b629e-41d5-420e-88a9-ede094a11f3b");
        assert_eq!(model_hash, "");
        assert_eq!(verified, Some(true));
        assert_eq!(created_at, "2026-11-22T21:08:09.248575+00:00");

        // A proof that has not been verified yet has `verified: null`.
        let pending: Value =
            serde_json::from_str(r#"{"id":"x","verified":null,"created_at":"t"}"#).unwrap();
        assert_eq!(proof_row(&pending).2, None);
    }

    #[test]
    fn receipt_fields_read_registry_receipt() {
        let receipt: Value = serde_json::from_str(
            r#"{"receipt_id":"9a8d8981-3f13-443c-8124-994ce6ebf904","proof_id":"4d7b629e-41d5-420e-88a9-ede094a11f3b","circuit_hash":"0x8e7c338859ba0bcb6911e6b68794f5449c0bb36a0e2ce47cb5dc96e8eb56e909","model_hash":"","verified":true,"verified_at":"2026-11-22T21:08:41.494633+00:00","verifier_version":"0.1.0","verifier_host":"unknown","signature":"db4a"}"#,
        )
        .unwrap();
        let fields = receipt_fields(&receipt);
        let get = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("missing field {key}"))
        };
        assert_eq!(get("receipt_id"), "9a8d8981-3f13-443c-8124-994ce6ebf904");
        assert_eq!(get("verified"), "true");
        assert_eq!(get("verified_at"), "2026-11-22T21:08:41.494633+00:00");
        assert_eq!(get("verifier_version"), "0.1.0");
        assert_eq!(get("verifier_host"), "unknown");
        assert_eq!(get("signature"), "db4a");
        // Every field the registry sends is printed, and nothing falls back to "?".
        for key in receipt.as_object().unwrap().keys() {
            get(key);
        }
        assert!(fields.iter().all(|(_, v)| v != "?"), "{fields:?}");
    }

    #[test]
    fn proof_url_encodes_id_as_one_segment() {
        let url = |id: &str| proof_url("http://127.0.0.1:8080", id, "verify").map(String::from);
        assert_eq!(
            url("4d7b629e-41d5-420e-88a9-ede094a11f3b").unwrap(),
            "http://127.0.0.1:8080/v1/proofs/4d7b629e-41d5-420e-88a9-ede094a11f3b/verify"
        );
        assert_eq!(
            url("a/b?c#d").unwrap(),
            "http://127.0.0.1:8080/v1/proofs/a%2Fb%3Fc%23d/verify"
        );
        assert_eq!(
            url("%2e%2e").unwrap(),
            "http://127.0.0.1:8080/v1/proofs/%252e%252e/verify"
        );
        // These would be dropped from the path, not encoded.
        assert_eq!(url(".").unwrap_err(), "invalid proof ID '.'");
        assert_eq!(url("..").unwrap_err(), "invalid proof ID '..'");

        // A gateway behind a path prefix keeps the prefix.
        assert_eq!(
            proof_url("http://host/api", "x", "receipt")
                .unwrap()
                .as_str(),
            "http://host/api/v1/proofs/x/receipt"
        );
        assert!(proof_url("localhost:8080", "x", "receipt")
            .unwrap_err()
            .starts_with("invalid gateway URL"));
    }

    #[test]
    fn interpret_response_reports_status_for_any_body() {
        assert_eq!(
            interpret_response(200, r#"{"models":[]}"#).unwrap()["models"],
            serde_json::json!([])
        );
        assert_eq!(
            interpret_response(401, r#"{"error":"invalid or missing API key"}"#).unwrap_err(),
            "HTTP 401 -- invalid or missing API key"
        );
        // A backend's empty 404 for an unknown sub-path, forwarded by the gateway.
        assert_eq!(
            interpret_response(404, "").unwrap_err(),
            "HTTP 404 -- empty response body"
        );
        assert_eq!(
            interpret_response(502, "Bad Gateway").unwrap_err(),
            "HTTP 502 -- Bad Gateway"
        );
        assert!(interpret_response(200, "<html>")
            .unwrap_err()
            .starts_with("HTTP 200 -- response is not JSON"));
    }

    #[tokio::test]
    async fn proofs_sends_encoded_query_to_gateway_route() {
        let (url, server) = serve_once("200 OK", r#"{"proofs":[],"count":0}"#);
        let client = reqwest::Client::new();
        cmd_proofs(&client, &url, Some("0xab&limit=999 x".to_string()), 7)
            .await
            .unwrap();
        let request = server.join().unwrap();
        let request_line = request.lines().next().unwrap();
        assert_eq!(
            request_line,
            "GET /v1/proofs?limit=7&model=0xab%26limit%3D999+x HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn verify_and_receipt_send_encoded_id() {
        let client = reqwest::Client::new();

        let (url, server) = serve_once(
            "200 OK",
            r#"{"verified":true,"receipt_id":"f86d8359-39ea-4fd2-8299-cda5458a4f75"}"#,
        );
        cmd_verify(&client, &url, "x?y=1").await.unwrap();
        let request = server.join().unwrap();
        assert_eq!(
            request.lines().next().unwrap(),
            "POST /v1/proofs/x%3Fy=1/verify HTTP/1.1"
        );

        let (url, server) = serve_once("404 Not Found", r#"{"error":"proof 'a/b' not found"}"#);
        cmd_receipt(&client, &url, "a/b").await.unwrap_err();
        let request = server.join().unwrap();
        assert_eq!(
            request.lines().next().unwrap(),
            "GET /v1/proofs/a%2Fb/receipt HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn verify_by_id_reports_whether_the_proof_verified() {
        let client = reqwest::Client::new();

        let (url, server) = serve_once(
            "200 OK",
            r#"{"verified":true,"receipt_id":"f86d8359-39ea-4fd2-8299-cda5458a4f75"}"#,
        );
        assert_eq!(cmd_verify(&client, &url, "some-id").await, Ok(true));
        server.join().unwrap();

        // The registry answers 200 for a proof the verifier rejected.
        let (url, server) = serve_once(
            "200 OK",
            r#"{"verified":false,"receipt_id":"a84af8e4-46ba-4441-b86c-a683095c490d","error":"verification failed: sumcheck failed"}"#,
        );
        assert_eq!(cmd_verify(&client, &url, "some-id").await, Ok(false));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn error_status_becomes_err() {
        let (url, server) = serve_once(
            "401 Unauthorized",
            r#"{"error":"invalid or missing API key"}"#,
        );
        let client = reqwest::Client::new();
        let err = cmd_models(&client, &url).await.unwrap_err();
        assert_eq!(err, "HTTP 401 -- invalid or missing API key");
        assert!(server
            .join()
            .unwrap()
            .starts_with("GET /v1/models HTTP/1.1"));
    }
}
