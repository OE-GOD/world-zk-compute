//! CLI entry point for zkml-verifier.
//!
//! Usage:
//!   zkml-verifier verify <proof_bundle.json>
//!   zkml-verifier verify <proof_bundle.json> --hybrid
//!   zkml-verifier bundle --proof <hex_file> --gens <hex_file> --desc <json_file> -o <output.json>

use std::process;
use zkml_verifier::{verify, verify_hybrid, ProofBundle};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "verify" => cmd_verify(&args[2..]),
        "bundle" => cmd_bundle(&args[2..]),
        "--help" | "-h" => print_usage(),
        // Legacy: treat first arg as a file path for backward compat
        path if path.ends_with(".json") || path.ends_with(".json.gz") => cmd_verify(&args[1..]),
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  zkml-verifier verify <proof_bundle.json|.json.gz> [--hybrid] [--json]");
    eprintln!(
        "  zkml-verifier bundle --proof <file> --gens <file> --desc <file> [-o <output.json>] [--compress]"
    );
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  verify   Verify a proof bundle (auto-detects gzip-compressed files)");
    eprintln!("  bundle   Assemble proof components into a ProofBundle JSON file");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --compress   Output gzip-compressed bundle (adds .gz suffix if needed)");
    eprintln!("               Compressed bundles are auto-detected on verify");
}

fn cmd_verify(args: &[String]) {
    if args.is_empty() {
        eprintln!("Error: verify requires a proof bundle path");
        process::exit(1);
    }

    let path = &args[0];
    let hybrid = args.iter().any(|a| a == "--hybrid");
    let json_output = args.iter().any(|a| a == "--json");

    let bundle = match ProofBundle::from_file(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error loading bundle: {e}");
            process::exit(1);
        }
    };

    if hybrid {
        match verify_hybrid(&bundle) {
            Ok(result) => {
                if json_output {
                    let out = serde_json::json!({
                        "circuit_hash": format!("0x{}", hex::encode(result.circuit_hash)),
                        "transcript_digest": format!("0x{}", hex::encode(result.transcript_digest)),
                        "num_compute_layers": result.compute_fr.rlc_betas.len(),
                        "num_l_tensor": result.input_fr.l_tensor_flat.len(),
                        "num_z_dot_rs": result.input_fr.z_dot_rs.len(),
                        "num_mle_evals": result.input_fr.mle_evals.len(),
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap());
                } else {
                    println!("circuit_hash: 0x{}", hex::encode(result.circuit_hash));
                    println!(
                        "transcript_digest: 0x{}",
                        hex::encode(result.transcript_digest)
                    );
                    println!(
                        "compute_layers: {} rlc_betas, {} z_dot_j_stars",
                        result.compute_fr.rlc_betas.len(),
                        result.compute_fr.z_dot_j_stars.len()
                    );
                    println!(
                        "input_layers: {} l_tensor entries, {} z_dot_rs, {} mle_evals",
                        result.input_fr.l_tensor_flat.len(),
                        result.input_fr.z_dot_rs.len(),
                        result.input_fr.mle_evals.len()
                    );
                }
            }
            Err(e) => {
                eprintln!("Hybrid verification error: {e}");
                process::exit(1);
            }
        }
    } else {
        match verify(&bundle) {
            Ok(result) => {
                if json_output {
                    let out = serde_json::json!({
                        "verified": result.verified,
                        "circuit_hash": format!("0x{}", hex::encode(result.circuit_hash)),
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap());
                } else {
                    println!(
                        "verified: {}, circuit_hash: 0x{}",
                        result.verified,
                        hex::encode(result.circuit_hash)
                    );
                }
                if !result.verified {
                    process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("Verification error: {e}");
                process::exit(1);
            }
        }
    }
}

fn cmd_bundle(args: &[String]) {
    let mut proof_path: Option<&str> = None;
    let mut gens_path: Option<&str> = None;
    let mut desc_path: Option<&str> = None;
    let mut output_path: Option<&str> = None;
    let mut pub_inputs_path: Option<&str> = None;
    let mut compress = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--proof" | "-p" => {
                i += 1;
                proof_path = args.get(i).map(|s| s.as_str());
            }
            "--gens" | "-g" => {
                i += 1;
                gens_path = args.get(i).map(|s| s.as_str());
            }
            "--desc" | "-d" => {
                i += 1;
                desc_path = args.get(i).map(|s| s.as_str());
            }
            "--pub-inputs" => {
                i += 1;
                pub_inputs_path = args.get(i).map(|s| s.as_str());
            }
            "-o" | "--output" => {
                i += 1;
                output_path = args.get(i).map(|s| s.as_str());
            }
            "--compress" => {
                compress = true;
            }
            _ => {
                eprintln!("Unknown bundle option: {}", args[i]);
                process::exit(1);
            }
        }
        i += 1;
    }

    let proof_path = proof_path.unwrap_or_else(|| {
        eprintln!("Error: --proof <file> is required");
        process::exit(1);
    });
    let gens_path = gens_path.unwrap_or_else(|| {
        eprintln!("Error: --gens <file> is required");
        process::exit(1);
    });
    let desc_path = desc_path.unwrap_or_else(|| {
        eprintln!("Error: --desc <file> is required");
        process::exit(1);
    });

    // Read proof (raw bytes -> hex)
    let proof_hex = read_hex_file(proof_path);
    let gens_hex = read_hex_file(gens_path);
    let pub_inputs_hex = pub_inputs_path.map(read_hex_file).unwrap_or_default();

    // Read circuit description JSON
    let desc_str = std::fs::read_to_string(desc_path).unwrap_or_else(|e| {
        eprintln!("Error reading {desc_path}: {e}");
        process::exit(1);
    });
    let desc: serde_json::Value = serde_json::from_str(&desc_str).unwrap_or_else(|e| {
        eprintln!("Error parsing {desc_path}: {e}");
        process::exit(1);
    });

    let bundle = ProofBundle {
        proof_hex,
        public_inputs_hex: pub_inputs_hex,
        gens_hex,
        dag_circuit_description: desc,
        model_hash: None,
        timestamp: None,
        prover_version: None,
        circuit_hash: None,
    };

    match output_path {
        Some(path) => {
            // Determine effective path: add .gz suffix if --compress and not already .gz
            let effective_path = if compress && !path.ends_with(".gz") {
                format!("{}.gz", path)
            } else {
                path.to_string()
            };

            if let Err(e) = bundle.save(&effective_path) {
                eprintln!("Error writing {effective_path}: {e}");
                process::exit(1);
            }

            if compress {
                let original_size = serde_json::to_string(&bundle).unwrap().len();
                let compressed_size = std::fs::metadata(&effective_path).unwrap().len() as usize;
                let ratio = if original_size > 0 {
                    100.0 - (compressed_size as f64 / original_size as f64 * 100.0)
                } else {
                    0.0
                };
                eprintln!(
                    "Bundle written to {effective_path} (compressed: {compressed_size} bytes, {ratio:.1}% reduction from {original_size} bytes)"
                );
            } else {
                eprintln!("Bundle written to {effective_path}");
            }
        }
        None => {
            if compress {
                // Write compressed binary to stdout
                let compressed = bundle.to_compressed_json().unwrap_or_else(|e| {
                    eprintln!("Error compressing bundle: {e}");
                    process::exit(1);
                });
                use std::io::Write;
                std::io::stdout()
                    .write_all(&compressed)
                    .unwrap_or_else(|e| {
                        eprintln!("Error writing to stdout: {e}");
                        process::exit(1);
                    });
            } else {
                let json = serde_json::to_string_pretty(&bundle).unwrap();
                println!("{json}");
            }
        }
    }
}

/// Read a file and return its contents as a "0x"-prefixed hex string.
/// If the file already contains hex text (starts with "0x"), return as-is.
fn read_hex_file(path: &str) -> String {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Error reading {path}: {e}");
        process::exit(1);
    });

    // Check if file is already hex-encoded text
    if let Ok(text) = std::str::from_utf8(&data) {
        let trimmed = text.trim();
        if trimmed.starts_with("0x") && trimmed[2..].chars().all(|c| c.is_ascii_hexdigit()) {
            return trimmed.to_string();
        }
    }

    // Raw binary -> hex encode
    format!("0x{}", hex::encode(&data))
}
