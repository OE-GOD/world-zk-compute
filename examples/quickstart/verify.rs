//! Quickstart: Verify a ZKML proof bundle in Rust.
//!
//! This file demonstrates the main verification APIs. It is not a standalone
//! binary -- copy the code into your own project, or use the built-in CLI:
//!
//!   cargo run -p zkml-verifier -- verify proof_bundle.json
//!
//! To use this as a standalone binary, create a new crate:
//!
//!   cargo new my-verifier
//!   cd my-verifier
//!
//! Add to Cargo.toml:
//!
//!   [dependencies]
//!   zkml-verifier = "0.1"
//!   hex = "0.4"
//!
//! Then copy the main() function below into src/main.rs.

use zkml_verifier::{verify, verify_hybrid, verify_raw, ProofBundle, VerifyError};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "proof_bundle.json".to_string());

    println!("Loading proof bundle: {path}");

    // --- Method 1: High-level API (recommended) ---
    //
    // ProofBundle::from_file() reads and parses the JSON file.
    // verify() runs full GKR + Hyrax verification including EC checks.

    let bundle = ProofBundle::from_file(&path)?;

    // Show bundle metadata
    let meta = bundle.metadata();
    if !meta.model_hash.is_empty() {
        println!("  Model hash: {}", meta.model_hash);
    }
    if meta.timestamp > 0 {
        println!("  Timestamp:  {}", meta.timestamp);
    }
    if !meta.prover_version.is_empty() {
        println!("  Prover:     {}", meta.prover_version);
    }

    let result = verify(&bundle)?;
    println!("Verified: {}", result.verified);
    println!("Circuit hash: 0x{}", hex::encode(result.circuit_hash));

    // --- Method 2: From a JSON string ---
    //
    // Useful when you receive proof data over the network.

    // let json_str = std::fs::read_to_string(&path)?;
    // let bundle = ProofBundle::from_json(&json_str)?;
    // let result = verify(&bundle)?;

    // --- Method 3: Hybrid verification ---
    //
    // Replays the Fiat-Shamir transcript without EC checks.
    // Returns intermediate Fr values for off-chain Groth16 wrapping.

    // let hybrid = verify_hybrid(&bundle)?;
    // println!("Transcript digest: 0x{}", hex::encode(hybrid.transcript_digest));
    // println!("Compute layers: {}", hybrid.compute_fr.rlc_betas.len());

    // --- Method 4: Low-level raw bytes API ---
    //
    // Skip the JSON layer entirely when you have raw byte slices.

    // let proof_data = bundle.proof_data()?;
    // let gens_data = bundle.gens_data()?;
    // let circuit_desc = encode_circuit_desc(&bundle.dag_circuit_description);
    // let result = verify_raw(&proof_data, &gens_data, &circuit_desc)?;

    // --- Error handling ---
    //
    // All verification functions return Result<_, VerifyError>.
    // VerifyError variants:
    //   InvalidProof   -- proof data is invalid
    //   InvalidFormat  -- wrong selector, too short, etc.
    //   BundleParse    -- JSON parsing failed
    //   VerificationFailed -- cryptographic check failed
    //   DecodeError    -- proof binary decoding failed

    if !result.verified {
        std::process::exit(1);
    }
    Ok(())
}
