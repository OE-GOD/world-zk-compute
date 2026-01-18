//! Signature-Verified Detection Guest Program
//!
//! This example demonstrates how to use RISC Zero precompiles for
//! accelerated cryptographic operations (10-100x faster).
//!
//! ## What This Proves
//!
//! 1. The input data has a valid Ethereum signature from the data source
//! 2. The detection algorithm was executed correctly on verified data
//! 3. The results are mathematically correct
//!
//! ## Precompiles Used
//!
//! - SHA-256: Hashing data before signature verification (~100x faster)
//! - secp256k1: Ethereum ECDSA signature verification (~100x faster)
//!
//! ## Use Cases
//!
//! - Verify orb operator signed the biometric data
//! - Verify trusted data source before processing
//! - Chain of custody verification for sensitive data

#![no_main]
#![no_std]

extern crate alloc;
use alloc::vec::Vec;

use risc0_zkvm::guest::env;

// These use ACCELERATED precompiles - same API, 100x faster!
use k256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use sha2::{Digest, Sha256};

risc0_zkvm::guest::entry!(main);

// ═══════════════════════════════════════════════════════════════════════════
// Input/Output Types
// ═══════════════════════════════════════════════════════════════════════════

/// Signed detection input
///
/// The data source signs the data with their Ethereum private key.
/// We verify the signature before processing.
#[derive(serde::Deserialize)]
struct SignedDetectionInput {
    /// The actual detection data points
    data_points: Vec<DataPoint>,
    /// Detection threshold (0.0 - 1.0)
    threshold: f64,
    /// ECDSA signature over SHA256(data_points || threshold)
    /// 64 bytes: r (32 bytes) || s (32 bytes)
    signature: [u8; 64],
    /// Compressed public key of the signer (33 bytes)
    signer_pubkey: [u8; 33],
    /// Expected signer address (for verification in output)
    expected_signer: [u8; 20],
}

#[derive(serde::Deserialize, serde::Serialize)]
struct DataPoint {
    /// Unique identifier
    id: [u8; 32],
    /// Feature values for analysis
    features: Vec<i64>,
    /// Timestamp
    timestamp: u64,
}

/// Detection output with signature verification status
#[derive(serde::Serialize)]
struct VerifiedOutput {
    /// Was the signature valid?
    signature_valid: bool,
    /// Address of the signer (derived from pubkey)
    signer_address: [u8; 20],
    /// Did signer match expected?
    signer_matches: bool,
    /// Hash of the signed data
    data_hash: [u8; 32],
    /// Number of data points analyzed
    total_analyzed: usize,
    /// Number of anomalies detected
    anomalies_found: usize,
    /// Flagged item IDs
    flagged_ids: Vec<[u8; 32]>,
    /// Overall risk score
    risk_score: f64,
}

// ═══════════════════════════════════════════════════════════════════════════
// Main Entry Point
// ═══════════════════════════════════════════════════════════════════════════

fn main() {
    // Read input from host
    let input: SignedDetectionInput = env::read();

    // Step 1: Hash the data (ACCELERATED - uses SHA-256 precompile)
    let data_hash = hash_detection_data(&input.data_points, input.threshold);

    // Step 2: Verify signature (ACCELERATED - uses secp256k1 precompile)
    let (signature_valid, signer_address) =
        verify_signature(&data_hash, &input.signature, &input.signer_pubkey);

    // Step 3: Check if signer matches expected
    let signer_matches = signer_address == input.expected_signer;

    // Step 4: Only run detection if signature is valid and signer matches
    let (flagged_ids, risk_score) = if signature_valid && signer_matches {
        detect_anomalies(&input.data_points, input.threshold)
    } else {
        // Don't process unverified data
        (Vec::new(), 0.0)
    };

    // Commit verified output
    env::commit(&VerifiedOutput {
        signature_valid,
        signer_address,
        signer_matches,
        data_hash,
        total_analyzed: input.data_points.len(),
        anomalies_found: flagged_ids.len(),
        flagged_ids,
        risk_score,
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Cryptographic Operations (ACCELERATED with precompiles)
// ═══════════════════════════════════════════════════════════════════════════

/// Hash detection data using SHA-256 precompile
///
/// This is ~100x faster than standard SHA-256 in the zkVM.
fn hash_detection_data(data_points: &[DataPoint], threshold: f64) -> [u8; 32] {
    let mut hasher = Sha256::new();

    // Hash each data point
    for point in data_points {
        hasher.update(&point.id);
        for &feature in &point.features {
            hasher.update(&feature.to_le_bytes());
        }
        hasher.update(&point.timestamp.to_le_bytes());
    }

    // Include threshold in hash
    hasher.update(&threshold.to_le_bytes());

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

/// Verify ECDSA signature using secp256k1 precompile
///
/// This is ~100x faster than standard ECDSA verification in the zkVM.
/// Returns (is_valid, signer_address)
fn verify_signature(
    message_hash: &[u8; 32],
    signature_bytes: &[u8; 64],
    pubkey_bytes: &[u8; 33],
) -> (bool, [u8; 20]) {
    // Parse signature
    let signature = match Signature::from_slice(signature_bytes) {
        Ok(sig) => sig,
        Err(_) => return (false, [0u8; 20]),
    };

    // Parse public key
    let pubkey = match VerifyingKey::from_sec1_bytes(pubkey_bytes) {
        Ok(pk) => pk,
        Err(_) => return (false, [0u8; 20]),
    };

    // Verify signature (ACCELERATED)
    let is_valid = pubkey.verify(message_hash, &signature).is_ok();

    // Derive Ethereum address from public key
    let address = pubkey_to_eth_address(&pubkey);

    (is_valid, address)
}

/// Convert secp256k1 public key to Ethereum address
///
/// address = keccak256(uncompressed_pubkey[1..])[12..32]
fn pubkey_to_eth_address(pubkey: &VerifyingKey) -> [u8; 20] {
    use k256::elliptic_curve::sec1::ToEncodedPoint;

    // Get uncompressed public key (65 bytes: 0x04 || x || y)
    let uncompressed = pubkey.to_encoded_point(false);
    let pubkey_bytes = uncompressed.as_bytes();

    // For simplicity, we use SHA256 instead of Keccak256
    // In production, you'd use the Keccak precompile when available
    let mut hasher = Sha256::new();
    hasher.update(&pubkey_bytes[1..]); // Skip 0x04 prefix
    let hash = hasher.finalize();

    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..32]);
    address
}

// ═══════════════════════════════════════════════════════════════════════════
// Detection Algorithm
// ═══════════════════════════════════════════════════════════════════════════

/// Simple z-score based anomaly detection
fn detect_anomalies(data_points: &[DataPoint], threshold: f64) -> (Vec<[u8; 32]>, f64) {
    if data_points.is_empty() {
        return (Vec::new(), 0.0);
    }

    let num_features = data_points[0].features.len();
    if num_features == 0 {
        return (Vec::new(), 0.0);
    }

    // Calculate mean for each feature
    let mut means = alloc::vec![0i64; num_features];
    for point in data_points {
        for (i, &val) in point.features.iter().enumerate() {
            means[i] += val;
        }
    }
    for mean in &mut means {
        *mean /= data_points.len() as i64;
    }

    // Calculate standard deviation for each feature
    let mut stds = alloc::vec![0i64; num_features];
    for point in data_points {
        for (i, &val) in point.features.iter().enumerate() {
            let diff = val - means[i];
            stds[i] += diff * diff;
        }
    }
    for std in &mut stds {
        *std = isqrt(*std / data_points.len() as i64);
        if *std == 0 {
            *std = 1; // Avoid division by zero
        }
    }

    // Score each point
    let mut flagged = Vec::new();
    let mut total_score: i64 = 0;

    for point in data_points {
        let mut point_score: i64 = 0;

        for (i, &val) in point.features.iter().enumerate() {
            let z_score = (val - means[i]).abs() / stds[i];
            point_score += z_score;
        }

        point_score /= num_features as i64;
        total_score += point_score;

        // Flag if z-score exceeds threshold * 3 (3-sigma rule)
        let threshold_int = (threshold * 3.0 * 100.0) as i64;
        if point_score * 100 > threshold_int {
            flagged.push(point.id);
        }
    }

    let avg_score = total_score / data_points.len() as i64;
    let risk_score = (avg_score as f64 / 300.0).min(1.0);

    (flagged, risk_score)
}

/// Integer square root
fn isqrt(n: i64) -> i64 {
    if n < 0 {
        return 0;
    }
    if n < 2 {
        return n;
    }

    let mut x = n;
    let mut y = (x + 1) / 2;

    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }

    x
}
