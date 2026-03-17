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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a B256 from a byte value (fills first byte, rest zeros).
    fn b256_from_byte(b: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[0] = b;
        B256::from(bytes)
    }

    // ========== is_image_allowed ==========

    #[test]
    fn test_is_image_allowed_empty_allowlist_accepts_all() {
        let config = ProverConfig::default();
        assert!(config.allowed_image_ids.is_empty());

        // Any image ID should be accepted
        assert!(config.is_image_allowed(&b256_from_byte(0xAA)));
        assert!(config.is_image_allowed(&b256_from_byte(0xBB)));
        assert!(config.is_image_allowed(&B256::ZERO));
    }

    #[test]
    fn test_is_image_allowed_specific_allowlist_accepts_listed() {
        let allowed = b256_from_byte(0x42);
        let config = ProverConfig {
            allowed_image_ids: vec![allowed],
            ..Default::default()
        };

        assert!(config.is_image_allowed(&allowed));
    }

    #[test]
    fn test_is_image_allowed_specific_allowlist_rejects_unlisted() {
        let allowed = b256_from_byte(0x42);
        let not_allowed = b256_from_byte(0x99);
        let config = ProverConfig {
            allowed_image_ids: vec![allowed],
            ..Default::default()
        };

        assert!(!config.is_image_allowed(&not_allowed));
    }

    #[test]
    fn test_is_image_allowed_multiple_ids() {
        let id1 = b256_from_byte(0x01);
        let id2 = b256_from_byte(0x02);
        let id3 = b256_from_byte(0x03);
        let config = ProverConfig {
            allowed_image_ids: vec![id1, id2],
            ..Default::default()
        };

        assert!(config.is_image_allowed(&id1));
        assert!(config.is_image_allowed(&id2));
        assert!(!config.is_image_allowed(&id3));
    }

    // ========== is_tip_acceptable ==========

    #[test]
    fn test_is_tip_acceptable_at_minimum() {
        let config = ProverConfig::default();
        // Default min_tip_wei is 100_000_000_000_000 (0.0001 ETH)
        assert!(config.is_tip_acceptable(U256::from(100_000_000_000_000u64)));
    }

    #[test]
    fn test_is_tip_acceptable_above_minimum() {
        let config = ProverConfig::default();
        assert!(config.is_tip_acceptable(U256::from(200_000_000_000_000u64)));
    }

    #[test]
    fn test_is_tip_acceptable_below_minimum() {
        let config = ProverConfig::default();
        assert!(!config.is_tip_acceptable(U256::from(99_999_999_999_999u64)));
    }

    #[test]
    fn test_is_tip_acceptable_zero() {
        let config = ProverConfig::default();
        assert!(!config.is_tip_acceptable(U256::ZERO));
    }

    #[test]
    fn test_is_tip_acceptable_custom_minimum() {
        let config = ProverConfig {
            min_tip_wei: U256::from(1_000_000_000_000_000_000u64), // 1 ETH
            ..Default::default()
        };

        assert!(!config.is_tip_acceptable(U256::from(999_999_999_999_999_999u64)));
        assert!(config.is_tip_acceptable(U256::from(1_000_000_000_000_000_000u64)));
        assert!(config.is_tip_acceptable(U256::from(2_000_000_000_000_000_000u64)));
    }

    #[test]
    fn test_is_tip_acceptable_zero_minimum_accepts_all() {
        let config = ProverConfig {
            min_tip_wei: U256::ZERO,
            ..Default::default()
        };

        assert!(config.is_tip_acceptable(U256::ZERO));
        assert!(config.is_tip_acceptable(U256::from(1u64)));
    }

    // ========== Default values ==========

    #[test]
    fn test_default_config_values() {
        let config = ProverConfig::default();

        assert_eq!(config.engine_address, Address::ZERO);
        assert!(config.registry_address.is_none());
        assert_eq!(config.min_tip_wei, U256::from(100_000_000_000_000u64));
        assert!(config.allowed_image_ids.is_empty());
        assert_eq!(config.poll_interval_secs, 5);
        assert_eq!(config.min_profit_margin, 0.2);
        assert!(!config.skip_profitability_check);
        assert!(!config.use_snark);
        assert_eq!(config.max_gpu_concurrent, 0);
        assert_eq!(config.max_cpu_concurrent, 0);
    }
}
