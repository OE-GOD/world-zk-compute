//! Prover configuration

use crate::bonsai::{BonsaiConfig, ProvingMode};
use alloy::primitives::{Address, B256, U256};

/// Configuration for the prover node
#[derive(Clone, Debug)]
pub struct ProverConfig {
    /// Address of the ExecutionEngine contract
    pub engine_address: Address,

    /// Address of the ProgramRegistry contract (optional, for on-chain program lookup)
    pub registry_address: Option<Address>,

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

    /// Boundless configuration (if using Boundless)
    #[cfg(feature = "boundless")]
    #[allow(dead_code)]
    pub boundless_config: Option<crate::boundless::BoundlessConfig>,

    /// Minimum profit margin (0.0 - 1.0, e.g., 0.2 = 20% profit required)
    pub min_profit_margin: f64,

    /// Skip profitability check (for testing)
    pub skip_profitability_check: bool,

    /// Enable SNARK (Groth16) proof generation for on-chain verification
    pub use_snark: bool,

    /// Maximum concurrent GPU proving jobs (0 = auto-detect from GPU count)
    #[allow(dead_code)]
    pub max_gpu_concurrent: usize,

    /// Maximum concurrent CPU proving jobs (0 = auto-detect from CPU count - 1)
    #[allow(dead_code)]
    pub max_cpu_concurrent: usize,
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

impl Default for ProverConfig {
    fn default() -> Self {
        Self {
            engine_address: Address::ZERO,
            registry_address: None,
            min_tip_wei: U256::from(100_000_000_000_000u64), // 0.0001 ETH
            allowed_image_ids: vec![],
            poll_interval_secs: 5,
            proving_mode: ProvingMode::GpuWithCpuFallback, // Try GPU first, fall back to CPU
            bonsai_config: None,
            #[cfg(feature = "boundless")]
            boundless_config: None,
            min_profit_margin: 0.2, // 20% minimum profit
            skip_profitability_check: false,
            use_snark: false,
            max_gpu_concurrent: 0, // Auto-detect
            max_cpu_concurrent: 0, // Auto-detect
        }
    }
}
