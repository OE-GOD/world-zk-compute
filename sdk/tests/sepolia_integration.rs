//! Sepolia integration tests for the Rust SDK.
//!
//! These tests are `#[ignore]`d by default and only run when the
//! `SEPOLIA_RPC_URL` environment variable is set.
//!
//! Run with:
//! ```sh
//! SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
//!     cargo test --manifest-path sdk/Cargo.toml -- --ignored sepolia
//! ```

use std::env;

/// Get Sepolia RPC URL from environment. Panics if not set (tests are #[ignore]).
fn sepolia_rpc_url() -> String {
    env::var("SEPOLIA_RPC_URL").expect("SEPOLIA_RPC_URL must be set for Sepolia tests")
}

/// Try to load TEEMLVerifier address from deployment file or env.
fn tee_verifier_address() -> Option<String> {
    if let Ok(addr) = env::var("TEE_VERIFIER_ADDRESS") {
        return Some(addr);
    }

    // Try loading from deployments/11155111.json
    let deploy_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../deployments/11155111.json");
    if let Ok(contents) = std::fs::read_to_string(deploy_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
            if let Some(addr) = json
                .get("contracts")
                .and_then(|c| c.get("TEEMLVerifier"))
                .and_then(|t| t.get("address"))
                .and_then(|a| a.as_str())
            {
                return Some(addr.to_string());
            }
        }
    }

    None
}

#[tokio::test]
#[ignore]
async fn test_sepolia_connection() {
    let rpc_url = sepolia_rpc_url();

    let provider = alloy::providers::ProviderBuilder::new()
        .connect_http(rpc_url.parse().expect("invalid RPC URL"));

    let chain_id = provider
        .get_chain_id()
        .await
        .expect("failed to get chain ID");
    assert_eq!(chain_id, 11155111, "Expected Sepolia chain ID");
}

#[tokio::test]
#[ignore]
async fn test_sepolia_block_number() {
    let rpc_url = sepolia_rpc_url();

    let provider = alloy::providers::ProviderBuilder::new()
        .connect_http(rpc_url.parse().expect("invalid RPC URL"));

    let block = provider
        .get_block_number()
        .await
        .expect("failed to get block number");
    assert!(block > 0, "Block number should be positive");
}

#[tokio::test]
#[ignore]
async fn test_sepolia_contract_code() {
    let rpc_url = sepolia_rpc_url();
    let addr = match tee_verifier_address() {
        Some(a) => a,
        None => {
            eprintln!("SKIP: No TEEMLVerifier address found");
            return;
        }
    };

    let provider = alloy::providers::ProviderBuilder::new()
        .connect_http(rpc_url.parse().expect("invalid RPC URL"));

    let address: alloy::primitives::Address = addr.parse().expect("invalid address");
    let code = provider
        .get_code_at(address)
        .await
        .expect("failed to get code");
    assert!(!code.is_empty(), "No code at TEEMLVerifier address");
}

use alloy::providers::Provider;
