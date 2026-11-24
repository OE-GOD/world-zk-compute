//! CLI entry point for zkml-verifier.
//!
//! Usage:
//!   zkml-verifier verify <proof_bundle.json> [--json]
//!   zkml-verifier verify <proof_bundle.json> --hybrid [--json]
//!   zkml-verifier bundle --proof <hex_file> --gens <hex_file> --desc <json_file> -o <output.json>
//!
//! Exit status: 0 on success, 1 on any failure (bad arguments, unreadable or
//! malformed bundle, proof rejected, output that cannot be written). With
//! `--json`, a bundle that cannot be loaded and a proof that is rejected are
//! also reported on stdout, as `{"verified": false, "error": "..."}`
//! (`{"error": "..."}` with `--hybrid`). Argument errors are reported on
//! stderr only.

use std::io::Write;
use std::process;
use zkml_verifier::{verify, verify_hybrid, ProofBundle};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprint!("{USAGE}");
        process::exit(1);
    }

    match args[1].as_str() {
        "verify" => cmd_verify(&args[2..]),
        "bundle" => cmd_bundle(&args[2..]),
        "--help" | "-h" | "help" => print!("{USAGE}"),
        "--version" | "-V" => println!("zkml-verifier {}", env!("CARGO_PKG_VERSION")),
        // Legacy: treat first arg as a file path for backward compat
        path if path.ends_with(".json") || path.ends_with(".json.gz") => cmd_verify(&args[1..]),
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            eprint!("{USAGE}");
            process::exit(1);
        }
    }
}

const USAGE: &str = "\
Usage:
  zkml-verifier verify <proof_bundle.json|.json.gz> [--hybrid] [--json]
  zkml-verifier bundle --proof <file> --gens <file> --desc <file>
                       [--pub-inputs <file>] [-o <output.json>] [--compress]

Commands:
  verify   Verify a proof bundle (auto-detects gzip-compressed files)
  bundle   Assemble proof components into a ProofBundle JSON file

Options for verify:
  --json       Print the result as JSON on stdout (also on failure)
  --hybrid     Replay the transcript and print the field outputs only.
               This skips every elliptic-curve check, so it does NOT verify
               the proof and exits 0 for proofs that `verify` rejects.

Options for bundle:
  -p, --proof <file>     Proof bytes (raw binary, or 0x-prefixed hex text)
  -g, --gens <file>      Pedersen generators (raw binary or hex text)
  -d, --desc <file>      DAG circuit description (JSON)
  --pub-inputs <file>    Public inputs (raw binary or hex text, optional)
  -o, --output <file>    Output path (default: stdout)
  --compress             Output gzip-compressed bundle (adds .gz suffix if needed)
                         Compressed bundles are auto-detected on verify

Other:
  -h, --help       Show this help
  -V, --version    Show the version

Exit status: 0 on success, 1 on any error or rejected proof.
";

/// Report a failure and exit with status 1. With `--json` the error also goes
/// to stdout as a JSON object so scripts can parse both outcomes.
fn fail(json_output: bool, hybrid: bool, context: &str, err: &dyn std::fmt::Display) -> ! {
    eprintln!("{context}: {err}");
    if json_output {
        let out = if hybrid {
            serde_json::json!({ "error": err.to_string() })
        } else {
            serde_json::json!({ "verified": false, "error": err.to_string() })
        };
        print_line(&serde_json::to_string_pretty(&out).unwrap());
    }
    process::exit(1);
}

/// Write one line to stdout. If the write fails (closed pipe, full disk),
/// report it on stderr and exit with status 1.
fn print_line(line: &str) {
    if let Err(e) = writeln!(std::io::stdout(), "{line}") {
        eprintln!("Error writing to stdout: {e}");
        process::exit(1);
    }
}

/// Run a call into the library with panic messages silenced.
///
/// The library reports a failed check by catching an internal panic and
/// returning Err. With the default hook a rejected proof would print a panic
/// message and a backtrace hint in addition to the error line. The previous
/// hook is put back afterwards, so a panic anywhere else is still reported.
fn with_silent_panic_hook<T>(call: impl FnOnce() -> T) -> T {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = call();
    std::panic::set_hook(previous);
    result
}

fn cmd_verify(args: &[String]) {
    let mut path: Option<&str> = None;
    let mut hybrid = false;
    let mut json_output = false;
    for arg in args {
        match arg.as_str() {
            "--hybrid" => hybrid = true,
            "--json" => json_output = true,
            "--help" | "-h" => {
                print!("{USAGE}");
                return;
            }
            opt if opt.starts_with('-') => {
                eprintln!("Unknown verify option: {opt}");
                process::exit(1);
            }
            p if path.is_none() => path = Some(p),
            extra => {
                eprintln!("Error: unexpected argument: {extra}");
                process::exit(1);
            }
        }
    }
    let path = path.unwrap_or_else(|| {
        eprintln!("Error: verify requires a proof bundle path");
        process::exit(1);
    });

    let bundle = match ProofBundle::from_file(path) {
        Ok(b) => b,
        Err(e) => fail(json_output, hybrid, "Error loading bundle", &e),
    };

    if hybrid {
        match with_silent_panic_hook(|| verify_hybrid(&bundle)) {
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
                    print_line(&serde_json::to_string_pretty(&out).unwrap());
                } else {
                    print_line(&format!(
                        "circuit_hash: 0x{}",
                        hex::encode(result.circuit_hash)
                    ));
                    print_line(&format!(
                        "transcript_digest: 0x{}",
                        hex::encode(result.transcript_digest)
                    ));
                    print_line(&format!(
                        "compute_layers: {} rlc_betas, {} z_dot_j_stars",
                        result.compute_fr.rlc_betas.len(),
                        result.compute_fr.z_dot_j_stars.len()
                    ));
                    print_line(&format!(
                        "input_layers: {} l_tensor entries, {} z_dot_rs, {} mle_evals",
                        result.input_fr.l_tensor_flat.len(),
                        result.input_fr.z_dot_rs.len(),
                        result.input_fr.mle_evals.len()
                    ));
                }
            }
            Err(e) => fail(json_output, hybrid, "Hybrid verification error", &e),
        }
    } else {
        match with_silent_panic_hook(|| verify(&bundle)) {
            Ok(result) => {
                if json_output {
                    let out = serde_json::json!({
                        "verified": result.verified,
                        "circuit_hash": format!("0x{}", hex::encode(result.circuit_hash)),
                    });
                    print_line(&serde_json::to_string_pretty(&out).unwrap());
                } else {
                    print_line(&format!(
                        "verified: {}, circuit_hash: 0x{}",
                        result.verified,
                        hex::encode(result.circuit_hash)
                    ));
                }
                if !result.verified {
                    process::exit(1);
                }
            }
            Err(e) => fail(json_output, hybrid, "Verification error", &e),
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
            "--help" | "-h" => {
                print!("{USAGE}");
                return;
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
                std::io::stdout()
                    .write_all(&compressed)
                    .unwrap_or_else(|e| {
                        eprintln!("Error writing to stdout: {e}");
                        process::exit(1);
                    });
            } else {
                print_line(&serde_json::to_string_pretty(&bundle).unwrap());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// The panic hook is process-wide, so this is the only test in this file.
    #[test]
    fn silent_panic_hook_is_scoped_to_the_call() {
        let reported = Arc::new(AtomicUsize::new(0));
        let counter = reported.clone();
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        let caught = with_silent_panic_hook(|| std::panic::catch_unwind(|| panic!("inside")));
        assert!(caught.is_err());
        let after_call = std::panic::catch_unwind(|| panic!("outside"));
        std::panic::set_hook(original);

        assert!(after_call.is_err());
        assert_eq!(
            reported.load(Ordering::SeqCst),
            1,
            "only the panic after the call reaches the hook that was installed before it"
        );
    }
}
