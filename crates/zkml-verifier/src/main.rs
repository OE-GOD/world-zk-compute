//! CLI entry point for zkml-verifier.
//!
//! Usage:
//!   zkml-verifier <proof_bundle.json>
//!   zkml-verifier <proof_bundle.json> --hybrid

use zkml_verifier::{verify, verify_hybrid, ProofBundle};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!("Usage: zkml-verifier <proof_bundle.json> [--hybrid]");
        std::process::exit(1);
    }

    let path = &args[1];
    let hybrid = args.len() == 3 && args[2] == "--hybrid";

    let bundle = match ProofBundle::from_file(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error loading bundle: {e}");
            std::process::exit(1);
        }
    };

    if hybrid {
        match verify_hybrid(&bundle) {
            Ok(result) => {
                println!(
                    "circuit_hash: 0x{}",
                    hex::encode(result.circuit_hash)
                );
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
            Err(e) => {
                eprintln!("Hybrid verification error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        match verify(&bundle) {
            Ok(result) => {
                println!(
                    "verified: {}, circuit_hash: 0x{}",
                    result.verified,
                    hex::encode(result.circuit_hash)
                );
                if !result.verified {
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("Verification error: {e}");
                std::process::exit(1);
            }
        }
    }
}
