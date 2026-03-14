//! Sepolia smoke test — verifies contract connectivity on live testnet.
//!
//! Requires env vars:
//!   ALCHEMY_SEPOLIA_RPC_URL
//!   TEE_VERIFIER_ADDRESS
//!
//! Run with: cargo test -p e2e-tests --test sepolia_smoke -- --ignored

use std::env;

#[tokio::test]
#[ignore = "requires Sepolia RPC and deployed contracts"]
async fn test_sepolia_rpc_connectivity() {
    let rpc_url = env::var("ALCHEMY_SEPOLIA_RPC_URL").expect("ALCHEMY_SEPOLIA_RPC_URL not set");

    let client = reqwest::Client::new();
    let resp = client
        .post(&rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        }))
        .send()
        .await
        .expect("Failed to connect to Sepolia RPC");

    assert!(resp.status().is_success(), "RPC returned non-200");

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON response");
    let block_hex = body["result"].as_str().expect("Missing result field");
    let block = u64::from_str_radix(block_hex.trim_start_matches("0x"), 16)
        .expect("Invalid block number");
    assert!(block > 0, "Block number should be positive");

    println!("Sepolia block number: {block}");
}

#[tokio::test]
#[ignore = "requires Sepolia RPC and deployed contracts"]
async fn test_sepolia_contract_has_code() {
    let rpc_url = env::var("ALCHEMY_SEPOLIA_RPC_URL").expect("ALCHEMY_SEPOLIA_RPC_URL not set");
    let contract = env::var("TEE_VERIFIER_ADDRESS").expect("TEE_VERIFIER_ADDRESS not set");

    let client = reqwest::Client::new();
    let resp = client
        .post(&rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getCode",
            "params": [contract, "latest"],
            "id": 1
        }))
        .send()
        .await
        .expect("Failed to connect to Sepolia RPC");

    let body: serde_json::Value = resp.json().await.expect("Invalid JSON response");
    let code = body["result"].as_str().expect("Missing result field");
    assert!(
        code.len() > 2,
        "Contract has no code (is it deployed?): {contract}"
    );

    println!("Contract {contract} has {} bytes of code", (code.len() - 2) / 2);
}
