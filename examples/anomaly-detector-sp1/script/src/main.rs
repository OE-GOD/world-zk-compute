//! Anomaly Detection Host Script (SP1 version)
//!
//! Benchmarks the anomaly detection algorithm on the SP1 zkVM.
//!
//! Usage:
//!   cargo run --release -- --execute     # Execute only (no proof, fast)
//!   cargo run --release -- --prove       # Generate and verify proof

use sp1_sdk::{include_elf, ProverClient, SP1Stdin};
use std::time::Instant;

/// The ELF binary for the anomaly-detector-sp1-program.
const ELF: &[u8] = include_elf!("anomaly-detector-sp1-program");

/// Detection input format (matches guest program)
#[derive(serde::Serialize, serde::Deserialize)]
struct DetectionInput {
    data_points: Vec<DataPoint>,
    threshold: f64,
    params: DetectionParams,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DataPoint {
    id: [u8; 32],
    features: Vec<f64>,
    timestamp: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DetectionParams {
    window_size: usize,
    min_cluster_size: usize,
    distance_threshold: f64,
}

/// Detection output format (matches guest program)
#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct DetectionOutput {
    total_analyzed: usize,
    anomalies_found: usize,
    flagged_ids: Vec<[u8; 32]>,
    risk_score: f64,
    input_hash: [u8; 32],
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let execute = args.iter().any(|a| a == "--execute");
    let prove = args.iter().any(|a| a == "--prove");

    if !execute && !prove {
        eprintln!("Usage: anomaly-detector-sp1-script [--execute] [--prove]");
        eprintln!("  --execute   Execute only (no proof, reports cycle count)");
        eprintln!("  --prove     Generate and verify a proof");
        std::process::exit(1);
    }

    println!("=== SP1 Anomaly Detector Benchmark ===\n");

    // Create the same test input as the RISC Zero version
    let input = create_sample_input();
    println!("Input: {} data points, threshold={}", input.data_points.len(), input.threshold);

    let client = ProverClient::from_env();
    let mut stdin = SP1Stdin::new();
    stdin.write(&input);

    if execute {
        println!("\n--- Execute Mode (no proof) ---");
        let start = Instant::now();
        let (mut output, report) = client.execute(ELF, &stdin).run()?;
        let exec_time = start.elapsed();

        let result = output.read::<DetectionOutput>();

        println!("SP1 Execution time: {:?}", exec_time);
        println!("SP1 Cycles: {}", report.total_instruction_count());
        println!("\nResults:");
        println!("  Total analyzed: {}", result.total_analyzed);
        println!("  Anomalies found: {}", result.anomalies_found);
        println!("  Risk score: {:.4}", result.risk_score);
        println!("  Input hash: 0x{}", hex::encode(result.input_hash));
        println!("  Flagged IDs:");
        for id in &result.flagged_ids {
            println!("    0x{}", hex::encode(id));
        }
    }

    if prove {
        println!("\n--- Prove Mode ---");
        let (pk, vk) = client.setup(ELF);

        let start = Instant::now();
        let proof = client.prove(&pk, &stdin).compressed().run()?;
        let prove_time = start.elapsed();

        let proof_size = bincode::serialize(&proof).map(|b| b.len()).unwrap_or(0);
        println!("SP1 Proof time: {:?}", prove_time);
        println!("SP1 Proof size: {} bytes", proof_size);

        let start = Instant::now();
        client.verify(&proof, &vk)?;
        let verify_time = start.elapsed();
        println!("SP1 Verify time: {:?}", verify_time);
        println!("SP1 Proof verified successfully!");
    }

    Ok(())
}

/// Create sample detection input (identical to RISC Zero version)
fn create_sample_input() -> DetectionInput {
    let mut data_points = Vec::new();

    // Normal data points
    for i in 0..8 {
        data_points.push(DataPoint {
            id: {
                let mut id = [0u8; 32];
                id[0] = i as u8;
                id
            },
            features: vec![
                100.0 + (i as f64 * 2.0),  // Normal range: 100-116
                50.0 + (i as f64 * 0.5),   // Normal range: 50-54
                1.0,                        // Normal: 1.0
            ],
            timestamp: 1700000000 + (i as u64 * 3600),
        });
    }

    // Anomalous data points
    data_points.push(DataPoint {
        id: {
            let mut id = [0u8; 32];
            id[0] = 8;
            id
        },
        features: vec![500.0, 200.0, 10.0], // Way outside normal range
        timestamp: 1700000000 + 28800,
    });

    data_points.push(DataPoint {
        id: {
            let mut id = [0u8; 32];
            id[0] = 9;
            id
        },
        features: vec![-50.0, -10.0, 0.0], // Negative values (suspicious)
        timestamp: 1700000000 + 32400,
    });

    DetectionInput {
        data_points,
        threshold: 0.8,
        params: DetectionParams {
            window_size: 100,
            min_cluster_size: 3,
            distance_threshold: 0.5,
        },
    }
}
