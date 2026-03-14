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
