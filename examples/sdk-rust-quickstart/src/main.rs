//! World ZK Compute — Rust SDK Quickstart
//!
//! Prerequisites:
//!   1. Start Anvil: `anvil --block-time 1`
//!   2. Deploy TEEMLVerifier and set CONTRACT_ADDRESS below
//!
//! Usage:
//!   cargo run

use alloy::primitives::{Address, Bytes, B256, U256, keccak256};
use alloy::providers::ProviderBuilder;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use anyhow::{Context, Result};
use std::str::FromStr;

sol! {
    #[sol(rpc)]
    contract TEEMLVerifier {
        function submitResult(bytes32 modelHash, bytes32 inputHash, bytes32 result, bytes32 imageHash, bytes attestation) external payable;
        function finalizeResult(bytes32 resultId) external;
        function isResultValid(bytes32 resultId) external view returns (bool);
        function proverStake() external view returns (uint256);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".into());
    let contract_addr = std::env::var("CONTRACT_ADDRESS")
        .unwrap_or_else(|_| "0x5FbDB2315678afecb367f032d93F642f64180aa3".into());
    let private_key = std::env::var("PRIVATE_KEY").unwrap_or_else(|_| {
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".into()
    });

    let contract_addr =
        Address::from_str(&contract_addr).context("invalid contract address")?;
    let signer: PrivateKeySigner = private_key.parse().context("invalid private key")?;
    let sender = signer.address();

    let provider = ProviderBuilder::new()
        .wallet(alloy::network::EthereumWallet::from(signer))
        .connect_http(rpc_url.parse()?);

    println!("Connected to {rpc_url}");
    println!("Sender: {sender}");

    let contract = TEEMLVerifier::new(contract_addr, &provider);

    // --- Query stake ---
    let stake: U256 = contract.proverStake().call().await.context("proverStake() failed")?;
    println!("Required stake: {stake} wei");

    // --- Submit result ---
    let mut model_bytes = [0u8; 32];
    model_bytes[31] = 1;
    let model_hash = B256::from(model_bytes);
    let mut input_bytes = [0u8; 32];
    input_bytes[31] = 2;
    let input_hash = B256::from(input_bytes);
    let mut result_bytes = [0u8; 32];
    result_bytes[31] = 3;
    let result = B256::from(result_bytes);
    let mut image_bytes = [0u8; 32];
    image_bytes[31] = 4;
    let image_hash = B256::from(image_bytes);
    let attestation = Bytes::from(vec![0u8; 65]);

    let receipt = contract
        .submitResult(model_hash, input_hash, result, image_hash, attestation)
        .value(stake)
        .send()
        .await?
        .get_receipt()
        .await?;
    println!("submitResult tx: {} (status={})", receipt.transaction_hash, receipt.status());

    // Compute result ID
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(model_hash.as_slice());
    buf[32..].copy_from_slice(input_hash.as_slice());
    let result_id = keccak256(buf);
    println!("Result ID: {result_id}");

    // --- Query result ---
    let is_valid: bool = contract.isResultValid(result_id).call().await?;
    println!("Is valid (before finalize): {is_valid}");

    println!("\nResult submitted successfully!");
    println!("To finalize, wait for the challenge window to pass, then call finalizeResult().");

    Ok(())
}
