//! Pre-configured network definitions for World ZK Compute.
//!
//! # Example
//!
//! ```rust,no_run
//! use world_zk_sdk::networks::SEPOLIA;
//!
//! println!("Chain ID: {}", SEPOLIA.chain_id);
//! println!("Verifier router: {}", SEPOLIA.verifier_router_address);
//! ```

/// Configuration for a known network.
pub struct NetworkConfig {
    pub name: &'static str,
    pub chain_id: u64,
    pub verifier_router_address: &'static str,
    pub tee_verifier_address: &'static str,
    pub execution_engine_address: &'static str,
    pub explorer_url: &'static str,
    pub tee_only: bool,
}

/// Ethereum Sepolia testnet.
pub const SEPOLIA: NetworkConfig = NetworkConfig {
    name: "Ethereum Sepolia",
    chain_id: 11155111,
    verifier_router_address: "0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187",
    tee_verifier_address: "0x0000000000000000000000000000000000000000",
    execution_engine_address: "0x0000000000000000000000000000000000000000",
    explorer_url: "https://sepolia.etherscan.io",
    tee_only: true,
};

/// World Chain Sepolia testnet (chainId 4801).
pub const WORLD_CHAIN_SEPOLIA: NetworkConfig = NetworkConfig {
    name: "World Chain Sepolia",
    chain_id: 4801,
    verifier_router_address: "0x0000000000000000000000000000000000000000",
    tee_verifier_address: "0x0000000000000000000000000000000000000000",
    execution_engine_address: "0x0000000000000000000000000000000000000000",
    explorer_url: "https://sepolia.worldscan.org",
    tee_only: false,
};

/// World Chain mainnet (chainId 480).
pub const WORLD_CHAIN_MAINNET: NetworkConfig = NetworkConfig {
    name: "World Chain",
    chain_id: 480,
    verifier_router_address: "0x0000000000000000000000000000000000000000",
    tee_verifier_address: "0x0000000000000000000000000000000000000000",
    execution_engine_address: "0x0000000000000000000000000000000000000000",
    explorer_url: "https://worldscan.org",
    tee_only: false,
};

/// Local Anvil development network.
pub const ANVIL: NetworkConfig = NetworkConfig {
    name: "Anvil (Local)",
    chain_id: 31337,
    verifier_router_address: "0x0000000000000000000000000000000000000000",
    tee_verifier_address: "0x0000000000000000000000000000000000000000",
    execution_engine_address: "0x0000000000000000000000000000000000000000",
    explorer_url: "",
    tee_only: false,
};

/// All known networks.
pub const ALL_NETWORKS: &[&NetworkConfig] =
    &[&SEPOLIA, &WORLD_CHAIN_SEPOLIA, &WORLD_CHAIN_MAINNET, &ANVIL];

/// Look up a network configuration by chain ID.
pub fn from_chain_id(chain_id: u64) -> Option<&'static NetworkConfig> {
    ALL_NETWORKS
        .iter()
        .copied()
        .find(|n| n.chain_id == chain_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_chain_id_known() {
        assert_eq!(from_chain_id(11155111).unwrap().name, "Ethereum Sepolia");
        assert_eq!(from_chain_id(4801).unwrap().name, "World Chain Sepolia");
        assert_eq!(from_chain_id(480).unwrap().name, "World Chain");
        assert_eq!(from_chain_id(31337).unwrap().name, "Anvil (Local)");
    }

    #[test]
    fn test_from_chain_id_unknown() {
        assert!(from_chain_id(999999).is_none());
    }

    #[test]
    fn test_all_networks_count() {
        assert_eq!(ALL_NETWORKS.len(), 4);
    }
}
