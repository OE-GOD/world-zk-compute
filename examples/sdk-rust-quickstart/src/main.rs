//! World ZK Compute — Rust SDK Quickstart
//!
//! Submits one TEE-attested result to a TEEMLVerifier deployed on a local Anvil
//! chain and reads it back. See README.md for the setup steps (start Anvil,
//! deploy the contract).
//!
//! Usage:
//!   cargo run

use alloy::primitives::{keccak256, Address, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};
use anyhow::Result;
use world_zk_sdk::{
    compute_input_hash, compute_model_hash, compute_result_hash_from_bytes, Client, TEEVerifier,
};

sol! {
    /// The struct an enclave signs; `TEEMLVerifier.submitResult` recovers the
    /// enclave key from this EIP-712 signature.
    struct TEEMLResult {
        bytes32 modelHash;
        bytes32 inputHash;
        bytes32 resultHash;
    }
}

/// Sign a result the way `TEEMLVerifier.submitResult` checks it (EIP-712).
///
/// In a real deployment the enclave produces this signature; here a local key
/// stands in for it.
async fn sign_attestation(
    enclave: &PrivateKeySigner,
    model_hash: B256,
    input_hash: B256,
    result: &[u8],
    chain_id: u64,
    verifying_contract: Address,
) -> Result<Vec<u8>> {
    let domain = eip712_domain! {
        name: "TEEMLVerifier",
        version: "1",
        chain_id: chain_id,
        verifying_contract: verifying_contract,
    };
    let message = TEEMLResult {
        modelHash: model_hash,
        inputHash: input_hash,
        resultHash: compute_result_hash_from_bytes(result),
    };
    // digest = keccak256("\x19\x01" || domainSeparator || hashStruct(message))
    let digest = message.eip712_signing_hash(&domain);
    let signature = enclave.sign_hash(&digest).await?;
    Ok(signature.as_bytes().to_vec())
}

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".into());
    // Proxy address printed by DeployTEEMLVerifier.s.sol on a fresh Anvil chain
    // (0x5FbDB... is the implementation behind it and must not be used directly).
    let contract_addr = std::env::var("CONTRACT_ADDRESS")
        .unwrap_or_else(|_| "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512".into());
    // Anvil default account #0: the deployer, and therefore the contract admin.
    let private_key = std::env::var("PRIVATE_KEY").unwrap_or_else(|_| {
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".into()
    });

    // --- Create SDK client and TEE verifier ---
    let client = Client::new(&rpc_url, &private_key, &contract_addr)?;
    let contract_address = client.contract_address();
    println!("Connected to {rpc_url}");
    println!("Sender: {}", client.signer_address());

    let tee = TEEVerifier::new(client);
    println!("Contract admin: {}", tee.admin().await?);

    // --- Register an enclave signing key (admin only) ---
    // A fresh local key stands in for the enclave, so the example can be re-run.
    let enclave = PrivateKeySigner::random();
    tee.register_enclave(enclave.address(), keccak256("enclave-image-v1"))
        .await?;
    println!("Registered enclave key: {}", enclave.address());

    // --- Compute hashes using SDK utilities ---
    let model_hash = compute_model_hash(b"my-model-v1");
    let input_hash = compute_input_hash(&[1.0, 2.5, 3.7]);
    println!("Model hash: {model_hash}");
    println!("Input hash: {input_hash}");

    // --- Submit a TEE result ---
    let result_data = b"[0.85]"; // the enclave returns the scores as JSON bytes
    let chain_id = ProviderBuilder::new()
        .connect_http(rpc_url.parse()?)
        .get_chain_id()
        .await?;
    let attestation = sign_attestation(
        &enclave,
        model_hash,
        input_hash,
        result_data,
        chain_id,
        contract_address,
    )
    .await?;
    let stake = tee.prover_stake().await?; // 0.1 ETH by default

    let (tx_hash, result_id) = tee
        .submit_result_with_id(model_hash, input_hash, result_data, &attestation, stake)
        .await?;
    println!("submitResult tx: {tx_hash}");
    println!("Result ID: {result_id}");

    // --- Query result ---
    let ml_result = tee.get_result(result_id).await?;
    println!("Submitter: {}", ml_result.submitter);
    println!("Enclave: {}", ml_result.enclave);
    println!("Finalized: {}", ml_result.finalized);
    println!("Challenged: {}", ml_result.challenged);

    let is_valid = tee.is_result_valid(result_id).await?;
    println!("Is valid (before finalize): {is_valid}");

    println!("\nQuickstart complete!");
    println!(
        "To finalize, wait for the challenge window (1 hour), then call tee.finalize(result_id)."
    );

    Ok(())
}
