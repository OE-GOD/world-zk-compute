//! World ZK Compute — Rust SDK Quickstart
//!
//! Prerequisites:
//!   1. Start Anvil: `anvil --block-time 1`
//!   2. Deploy TEEMLVerifier and set CONTRACT_ADDRESS below
//!
//! Usage:
//!   cargo run

use alloy::primitives::{keccak256, B256, U256};
use anyhow::Result;
use world_zk_sdk::{Client, tee::TEEVerifier};

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".into());
    let contract_addr = std::env::var("CONTRACT_ADDRESS")
        .unwrap_or_else(|_| "0x5FbDB2315678afecb367f032d93F642f64180aa3".into());
    let private_key = std::env::var("PRIVATE_KEY").unwrap_or_else(|_| {
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".into()
    });

    // --- Create SDK client and TEE verifier ---
    let client = Client::new(&rpc_url, &private_key, &contract_addr)?;
    println!("Connected to {rpc_url}");
    println!("Sender: {}", client.signer_address());

    let tee = TEEVerifier::new(client);

    // --- Submit a TEE result ---
    let model_hash = B256::left_padding_from(&[1]);
    let input_hash = B256::left_padding_from(&[2]);
    let result_data = vec![0u8; 32]; // mock result bytes
    let attestation = vec![0u8; 65]; // mock attestation

    let tx_hash = tee
        .submit_result(model_hash, input_hash, &result_data, &attestation, U256::ZERO)
        .await?;
    println!("submitResult tx: {tx_hash}");

    // --- Query result ---
    // result_id = keccak256(model_hash ++ input_hash)
    let mut preimage = [0u8; 64];
    preimage[..32].copy_from_slice(model_hash.as_ref());
    preimage[32..].copy_from_slice(input_hash.as_ref());
    let result_id = keccak256(preimage);

    let is_valid = tee.is_result_valid(result_id).await?;
    println!("Is valid (before finalize): {is_valid}");

    println!("\nResult submitted successfully!");
    println!("To finalize, wait for the challenge window to pass, then call tee.finalize().");

    Ok(())
}
