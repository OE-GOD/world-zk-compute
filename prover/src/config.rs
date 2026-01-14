//! Prover configuration

use alloy::primitives::{Address, B256, U256};
use crate::bonsai::{BonsaiConfig, ProvingMode};

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

    /// Proving mode (local, bonsai, bonsai-fallback)
    pub proving_mode: ProvingMode,

    /// Bonsai configuration (if using Bonsai)
    #[allow(dead_code)]
    pub bonsai_config: Option<BonsaiConfig>,
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

    /// Check if Bonsai is enabled and configured
    #[allow(dead_code)]
    pub fn is_bonsai_enabled(&self) -> bool {
        match self.proving_mode {
            ProvingMode::Bonsai | ProvingMode::BonsaiWithFallback => {
                self.bonsai_config.as_ref().map_or(false, |c| c.is_configured())
            }
            ProvingMode::Local => false,
        }
    }
}

impl Default for ProverConfig {
    fn default() -> Self {
        Self {
            engine_address: Address::ZERO,
            min_tip_wei: U256::from(100_000_000_000_000u64), // 0.0001 ETH
            allowed_image_ids: vec![],
            poll_interval_secs: 5,
            proving_mode: ProvingMode::Local,
            bonsai_config: None,
        }
    }
}
