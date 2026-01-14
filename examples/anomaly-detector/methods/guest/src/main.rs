//! Anomaly Detection Guest Program
//!
//! This runs inside the RISC Zero zkVM and produces a proof that:
//! 1. The detection algorithm was executed correctly
//! 2. The input data was processed without tampering
//! 3. The output (anomaly scores) is mathematically correct
//!
//! World's detection team can replace this with ANY algorithm:
//! - Sybil detection (clustering, graph analysis)
//! - Presentation attack detection (liveness checks)
//! - Operator fraud detection (pattern matching)
//! - Geographic anomaly detection (location clustering)

#![no_main]
#![no_std]

use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

/// Detection input format
#[derive(serde::Deserialize)]
struct DetectionInput {
    /// Data points to analyze (e.g., registration timestamps, locations, etc.)
    data_points: Vec<DataPoint>,
    /// Detection threshold (0.0 - 1.0)
    threshold: f64,
    /// Algorithm parameters
    params: DetectionParams,
}

#[derive(serde::Deserialize)]
struct DataPoint {
    /// Unique identifier (e.g., nullifier hash)
    id: [u8; 32],
    /// Feature vector for analysis
    features: Vec<f64>,
    /// Timestamp
    timestamp: u64,
}

#[derive(serde::Deserialize)]
struct DetectionParams {
    /// Window size for temporal analysis
    window_size: usize,
    /// Minimum cluster size
    min_cluster_size: usize,
    /// Distance threshold for clustering
    distance_threshold: f64,
}

/// Detection output format
#[derive(serde::Serialize)]
struct DetectionOutput {
    /// Number of data points analyzed
    total_analyzed: usize,
    /// Number of anomalies detected
    anomalies_found: usize,
    /// IDs flagged as anomalous
    flagged_ids: Vec<[u8; 32]>,
    /// Overall risk score (0.0 - 1.0)
    risk_score: f64,
    /// Hash of the input data (for verification)
    input_hash: [u8; 32],
}

fn main() {
    // Read input from the host
    let input: DetectionInput = env::read();

    // Compute hash of input for integrity verification
    let input_hash = compute_input_hash(&input);

    // Run anomaly detection algorithm
    let (flagged_ids, risk_score) = detect_anomalies(&input);

    // Create output
    let output = DetectionOutput {
        total_analyzed: input.data_points.len(),
        anomalies_found: flagged_ids.len(),
        flagged_ids,
        risk_score,
        input_hash,
    };

    // Commit output to the journal (public output)
    env::commit(&output);
}

/// Simple statistical anomaly detection
///
/// This is a basic example - World can replace with sophisticated ML models:
/// - Isolation Forest
/// - Local Outlier Factor
/// - DBSCAN clustering
/// - Neural network inference (small models)
fn detect_anomalies(input: &DetectionInput) -> (Vec<[u8; 32]>, f64) {
    let mut flagged = Vec::new();
    let mut total_score = 0.0;

    if input.data_points.is_empty() {
        return (flagged, 0.0);
    }

    // Calculate mean and std for each feature
    let num_features = input.data_points[0].features.len();
    let mut means = vec![0.0; num_features];
    let mut variances = vec![0.0; num_features];

    // Calculate means
    for point in &input.data_points {
        for (i, &val) in point.features.iter().enumerate() {
            means[i] += val;
        }
    }
    for mean in &mut means {
        *mean /= input.data_points.len() as f64;
    }

    // Calculate variances
    for point in &input.data_points {
        for (i, &val) in point.features.iter().enumerate() {
            let diff = val - means[i];
            variances[i] += diff * diff;
        }
    }
    for var in &mut variances {
        *var = (*var / input.data_points.len() as f64).sqrt();
        if *var == 0.0 {
            *var = 1.0; // Avoid division by zero
        }
    }

    // Score each point using z-score method
    for point in &input.data_points {
        let mut anomaly_score = 0.0;

        for (i, &val) in point.features.iter().enumerate() {
            let z_score = (val - means[i]).abs() / variances[i];
            anomaly_score += z_score;
        }

        anomaly_score /= num_features as f64;
        total_score += anomaly_score;

        // Flag if above threshold
        if anomaly_score > input.threshold * 3.0 {
            // Using 3-sigma rule
            flagged.push(point.id);
        }
    }

    let avg_score = total_score / input.data_points.len() as f64;
    let risk_score = (avg_score / 3.0).min(1.0); // Normalize to 0-1

    (flagged, risk_score)
}

/// Compute SHA256 hash of input for verification
fn compute_input_hash(input: &DetectionInput) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();

    // Hash all data points
    for point in &input.data_points {
        hasher.update(&point.id);
        for &f in &point.features {
            hasher.update(&f.to_le_bytes());
        }
        hasher.update(&point.timestamp.to_le_bytes());
    }

    hasher.update(&input.threshold.to_le_bytes());

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
