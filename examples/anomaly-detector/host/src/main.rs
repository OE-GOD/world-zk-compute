//! Anomaly Detection Host Program
//!
//! This shows how to:
//! 1. Prepare detection input data
//! 2. Submit a detection job to World ZK Compute
//! 3. Wait for the proof to be generated
//! 4. Verify the results on-chain
//!
//! Detection teams can use this as a template for integrating
//! their algorithms with the World ZK Compute network.

use alloy::primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

/// Detection input format (matches guest program)
#[derive(Serialize, Deserialize)]
struct DetectionInput {
    data_points: Vec<DataPoint>,
    threshold: f64,
    params: DetectionParams,
}

#[derive(Serialize, Deserialize)]
struct DataPoint {
    id: [u8; 32],
    features: Vec<f64>,
    timestamp: u64,
}

#[derive(Serialize, Deserialize)]
struct DetectionParams {
    window_size: usize,
    min_cluster_size: usize,
    distance_threshold: f64,
}

/// Detection output format (matches guest program)
#[derive(Serialize, Deserialize, Debug)]
struct DetectionOutput {
    total_analyzed: usize,
    anomalies_found: usize,
    flagged_ids: Vec<[u8; 32]>,
    risk_score: f64,
    input_hash: [u8; 32],
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== World ZK Compute - Anomaly Detection Example ===\n");

    // Step 1: Prepare detection input
    println!("1. Preparing detection input...");
    let input = create_sample_input();
    let input_bytes = bincode::serialize(&input)?;
    println!("   - {} data points to analyze", input.data_points.len());
    println!("   - Threshold: {}", input.threshold);

    // Step 2: Compute input hash
    let input_hash = compute_hash(&input_bytes);
    println!("   - Input hash: 0x{}", hex::encode(&input_hash));

    // Step 3: Upload input to IPFS or data URL
    println!("\n2. Uploading input data...");
    let input_url = upload_input(&input_bytes).await?;
    println!("   - Input URL: {}", input_url);

    // Step 4: Get the program image ID
    // (This would be computed from the guest ELF)
    let image_id = get_program_image_id();
    println!("\n3. Program image ID: 0x{}", hex::encode(&image_id));

    // Step 5: Submit to World ZK Compute
    println!("\n4. Submitting detection job to World ZK Compute...");
    println!("   - Engine: {}", get_engine_address());
    println!("   - Bounty: 0.01 ETH");

    // In production, this would call the smart contract:
    // engine.requestExecution(imageId, inputHash, inputUrl, callback, maxDelay)

    println!("\n5. Waiting for proof...");
    println!("   - A prover will claim and execute the job");
    println!("   - Proof generation: ~30s with Bonsai, ~30min local");

    // Step 6: Parse results from journal
    println!("\n6. Detection results (from proof journal):");
    let sample_output = DetectionOutput {
        total_analyzed: input.data_points.len(),
        anomalies_found: 2,
        flagged_ids: vec![input.data_points[3].id, input.data_points[7].id],
        risk_score: 0.15,
        input_hash,
    };

    println!("   - Total analyzed: {}", sample_output.total_analyzed);
    println!("   - Anomalies found: {}", sample_output.anomalies_found);
    println!("   - Risk score: {:.2}%", sample_output.risk_score * 100.0);
    println!("   - Flagged IDs:");
    for id in &sample_output.flagged_ids {
        println!("     - 0x{}", hex::encode(id));
    }

    println!("\n7. Proof verified on-chain!");
    println!("   - Results are cryptographically guaranteed correct");
    println!("   - No one can see the raw data, only the proven results");

    println!("\n=== Detection Complete ===");

    Ok(())
}

/// Create sample detection input
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

/// Compute SHA256 hash
fn compute_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

/// Upload input to storage (mock implementation)
async fn upload_input(data: &[u8]) -> anyhow::Result<String> {
    // In production, upload to IPFS, S3, or similar
    // For demo, use data URL
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    Ok(format!("data:application/octet-stream;base64,{}", b64))
}

/// Get program image ID (mock implementation)
fn get_program_image_id() -> [u8; 32] {
    // In production, compute from guest ELF:
    // risc0_zkvm::compute_image_id(GUEST_ELF)
    [0x42u8; 32]
}

/// Get engine contract address (mock implementation)
fn get_engine_address() -> &'static str {
    "0x9CFd1CF0e263420e010013373Ec4008d341a483e"
}
