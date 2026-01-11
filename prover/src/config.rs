//! Prover configuration

use alloy::primitives::{Address, B256, U256};

/// Configuration for the prover node
#[derive(Clone, Debug)]
pub struct ProverConfig {
    /// Address of the ExecutionEngine contract
    pub engine_address: Address,

    /// Minimum tip to accept (in wei)
    pub min_tip_wei: U256,

    /// List of image IDs to accept (empty = all)
    pub allowed_image_ids: Vec<B256>,

    /// How often to poll for new requests (seconds)
    pub poll_interval_secs: u64,
}

impl ProverConfig {
    /// Check if an image ID is allowed
    pub fn is_image_allowed(&self, image_id: &B256) -> bool {
        if self.allowed_image_ids.is_empty() {
            return true; // Accept all
        }
        self.allowed_image_ids.contains(image_id)
    }

    /// Check if a tip is acceptable
    pub fn is_tip_acceptable(&self, tip: U256) -> bool {
        tip >= self.min_tip_wei
    }
}
