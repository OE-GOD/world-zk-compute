//! STARK to SNARK proof conversion
//!
//! Converts RISC Zero STARK proofs to Groth16 SNARKs for:

#![allow(dead_code)]
//! - **Smaller proofs**: ~256 bytes vs ~200KB for STARK
//! - **Faster verification**: ~200K gas vs ~2M gas on-chain
//! - **EVM compatibility**: Native Groth16 precompile support
//!
//! Use this for production deployments where gas costs matter.

use anyhow::{Context, Result};
use bonsai_sdk::blocking::{Client, SnarkId};
use risc0_zkvm::Receipt;
use std::time::Duration;
use tracing::{info, debug, warn};

/// SNARK proof configuration
#[derive(Clone, Debug)]
pub struct SnarkConfig {
    /// Maximum time to wait for SNARK conversion (seconds)
    pub timeout_secs: u64,
    /// Polling interval when waiting for SNARK (seconds)
    pub poll_interval_secs: u64,
}

impl Default for SnarkConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 600, // 10 minutes max for SNARK
            poll_interval_secs: 5,
        }
    }
}

/// SNARK converter using Bonsai
pub struct SnarkConverter {
    client: Client,
    config: SnarkConfig,
}

impl SnarkConverter {
    /// Create a new SNARK converter
    pub fn new(client: Client, config: SnarkConfig) -> Self {
        Self { client, config }
    }

    /// Convert a STARK proof to SNARK (Groth16)
    ///
    /// This takes the session ID from a completed Bonsai proving session
    /// and converts the STARK proof to a Groth16 SNARK.
    pub fn convert(&self, session_id: &str) -> Result<SnarkReceipt> {
        info!("Starting STARK to SNARK conversion for session: {}", session_id);
        let start = std::time::Instant::now();

        // Request SNARK conversion
        let snark_session = self.client.create_snark(session_id.to_string())?;
        info!("SNARK session created: {}", snark_session.uuid);

        // Wait for completion
        let snark_receipt = self.wait_for_snark(&snark_session)?;

        let elapsed = start.elapsed();
        info!("SNARK conversion completed in {:.2?}", elapsed);

        Ok(snark_receipt)
    }

    /// Wait for SNARK conversion to complete
    fn wait_for_snark(&self, snark_id: &SnarkId) -> Result<SnarkReceipt> {
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let poll_interval = Duration::from_secs(self.config.poll_interval_secs);
        let start = std::time::Instant::now();

        loop {
            // Check timeout
            if start.elapsed() > timeout {
                anyhow::bail!("SNARK conversion timed out after {:?}", timeout);
            }

            // Get status
            let status = snark_id.status(&self.client)?;

            match status.status.as_str() {
                "RUNNING" => {
                    debug!("SNARK conversion in progress...");
                }
                "SUCCEEDED" => {
                    info!("SNARK conversion succeeded!");
                    let output_url = status.output
                        .context("No output URL in successful SNARK session")?;
                    let receipt_bytes = self.client.download(&output_url)?;
                    let receipt: Receipt = bincode::deserialize(&receipt_bytes)?;

                    return Ok(SnarkReceipt {
                        receipt,
                        session_id: snark_id.uuid.clone(),
                    });
                }
                "FAILED" => {
                    let error = status.error_msg.unwrap_or_else(|| "Unknown error".to_string());
                    anyhow::bail!("SNARK conversion failed: {}", error);
                }
                "TIMED_OUT" => {
                    anyhow::bail!("SNARK session timed out");
                }
                "ABORTED" => {
                    anyhow::bail!("SNARK session was aborted");
                }
                other => {
                    warn!("Unknown SNARK status: {}", other);
                }
            }

            std::thread::sleep(poll_interval);
        }
    }
}

/// A SNARK (Groth16) receipt
#[derive(Debug)]
pub struct SnarkReceipt {
    /// The receipt with SNARK proof
    pub receipt: Receipt,
    /// The SNARK session ID
    pub session_id: String,
}

impl SnarkReceipt {
    /// Get the proof size in bytes
    pub fn proof_size(&self) -> usize {
        bincode::serialize(&self.receipt.inner)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Extract the seal (proof bytes)
    pub fn seal(&self) -> Result<Vec<u8>> {
        let seal = bincode::serialize(&self.receipt.inner)?;
        Ok(seal)
    }

    /// Get the journal (public outputs)
    pub fn journal(&self) -> &[u8] {
        &self.receipt.journal.bytes
    }
}

/// Proof type selection
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ProofType {
    /// STARK proof (default, faster to generate, larger)
    #[default]
    Stark,
    /// SNARK proof (slower to generate, smaller, cheaper on-chain)
    Snark,
}

impl ProofType {
    /// Parse from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "snark" | "groth16" => Self::Snark,
            _ => Self::Stark,
        }
    }

    /// Is this a SNARK proof?
    pub fn is_snark(&self) -> bool {
        matches!(self, Self::Snark)
    }
}

/// Compare STARK vs SNARK proof sizes
#[derive(Debug)]
pub struct ProofComparison {
    pub stark_size: usize,
    pub snark_size: usize,
    pub size_reduction: f64,
}

impl ProofComparison {
    pub fn new(stark_size: usize, snark_size: usize) -> Self {
        let size_reduction = if stark_size > 0 {
            1.0 - (snark_size as f64 / stark_size as f64)
        } else {
            0.0
        };

        Self {
            stark_size,
            snark_size,
            size_reduction,
        }
    }
}

impl std::fmt::Display for ProofComparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "STARK: {} bytes, SNARK: {} bytes ({:.1}% smaller)",
            self.stark_size,
            self.snark_size,
            self.size_reduction * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proof_type_parse() {
        assert_eq!(ProofType::from_str("stark"), ProofType::Stark);
        assert_eq!(ProofType::from_str("snark"), ProofType::Snark);
        assert_eq!(ProofType::from_str("groth16"), ProofType::Snark);
    }

    #[test]
    fn test_proof_comparison() {
        let comparison = ProofComparison::new(200_000, 256);
        assert!(comparison.size_reduction > 0.99);
    }
}
