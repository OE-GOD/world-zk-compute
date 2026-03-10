//! E2E integration test for TEE verification flow.
//!
//! Requires Anvil running with TEEMLVerifier deployed.
//! Set `RPC_URL` and `TEE_CONTRACT_ADDRESS` env vars before running.
//!
//! Run via: `scripts/tee-sdk-e2e.sh`
//! Or manually:
//!   RPC_URL=http://127.0.0.1:8549 TEE_CONTRACT_ADDRESS=0x... \
//!     cargo test --test e2e_tee_verify -- --ignored --nocapture

use alloy::primitives::{keccak256, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use world_zk_sdk::{Client, TEEVerifier};

/// Anvil pre-funded accounts.
const ADMIN_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ENCLAVE_KEY_1: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const ENCLAVE_KEY_2: &str = "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";
const CHALLENGER_KEY: &str = "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";

/// Build an ECDSA attestation (Ethereum signed message) that the contract expects.
async fn build_attestation(
    enclave_key: &str,
    model_hash: B256,
    input_hash: B256,
    result: &[u8],
) -> Vec<u8> {
    let signer: PrivateKeySigner = enclave_key.parse().unwrap();
    let result_hash = keccak256(result);
    let message = keccak256(
        [
            model_hash.as_slice(),
            input_hash.as_slice(),
            result_hash.as_slice(),
        ]
        .concat(),
    );
    let sig = signer.sign_message(message.as_slice()).await.unwrap();
    sig.as_bytes().to_vec()
}

/// Fast-forward Anvil time by the given number of seconds.
async fn advance_time(rpc_url: &str, seconds: u64) {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse().unwrap());
    // evm_increaseTime returns a number
    let _: serde_json::Value = provider
        .raw_request("evm_increaseTime".into(), [U256::from(seconds)])
        .await
        .unwrap();
    // Mine a block to apply the time change
    let _: serde_json::Value = provider
        .raw_request("evm_mine".into(), ())
        .await
        .unwrap();
}

fn get_env(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("{var} env var required"))
}

#[tokio::test]
#[ignore] // Requires Anvil + deployed contract
async fn test_tee_happy_path_submit_and_finalize() {
    let rpc_url = get_env("RPC_URL");
    let contract_addr = get_env("TEE_CONTRACT_ADDRESS");

    // Admin client — used for registration
    let admin_client =
        Client::new(&rpc_url, ADMIN_KEY, &contract_addr).expect("Failed to create admin client");
    let admin_verifier = TEEVerifier::new(admin_client);

    // Verify owner
    let owner = admin_verifier.owner().await.expect("Failed to get owner");
    println!("Contract owner: {owner}");

    // Register enclave (use ENCLAVE_KEY_1 — unique to this test)
    let enclave_signer: PrivateKeySigner = ENCLAVE_KEY_1.parse().unwrap();
    let enclave_addr = enclave_signer.address();
    let image_hash = B256::from([0xABu8; 32]);

    let tx = admin_verifier
        .register_enclave(enclave_addr, image_hash)
        .await
        .expect("Failed to register enclave");
    println!("Enclave registered: tx={tx}");

    // Submit result with valid attestation
    let model_hash = B256::from([0x01u8; 32]);
    let input_hash = B256::from([0x02u8; 32]);
    let result_data = b"prediction:class_0";
    let attestation = build_attestation(ENCLAVE_KEY_1, model_hash, input_hash, result_data).await;

    let submitter_client =
        Client::new(&rpc_url, ADMIN_KEY, &contract_addr).expect("Failed to create submitter");
    let submitter = TEEVerifier::new(submitter_client);

    let stake = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
    let tx = submitter
        .submit_result(model_hash, input_hash, result_data, &attestation, stake)
        .await
        .expect("Failed to submit result");
    println!("Result submitted: tx={tx}");

    // Compute resultId the same way the contract does:
    // keccak256(abi.encodePacked(msg.sender, modelHash, inputHash, block.number))
    // We need to query the contract for the result to get the actual ID.
    // Use block number from the tx receipt.
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse().unwrap());
    let receipt = provider
        .get_transaction_receipt(tx)
        .await
        .expect("Failed to get receipt")
        .expect("Receipt not found");

    // Extract resultId from ResultSubmitted event log
    let result_id = receipt.inner.logs()[0].topics()[1];
    println!("Result ID: {result_id}");

    // Fast-forward past the challenge window (1 hour = 3600s)
    advance_time(&rpc_url, 3601).await;

    // Finalize
    let tx = submitter
        .finalize(result_id)
        .await
        .expect("Failed to finalize");
    println!("Result finalized: tx={tx}");

    // Verify result is valid
    let valid = submitter
        .is_result_valid(result_id)
        .await
        .expect("Failed to check validity");
    assert!(valid, "Result should be valid after finalization");
    println!("Result is valid: {valid}");
}

#[tokio::test]
#[ignore] // Requires Anvil + deployed contract
async fn test_tee_challenge_flow() {
    let rpc_url = get_env("RPC_URL");
    let contract_addr = get_env("TEE_CONTRACT_ADDRESS");

    // Admin — register enclave
    let admin_client =
        Client::new(&rpc_url, ADMIN_KEY, &contract_addr).expect("Failed to create admin client");
    let admin_verifier = TEEVerifier::new(admin_client);

    // Use ENCLAVE_KEY_2 — unique to this test to avoid conflicts
    let enclave_signer: PrivateKeySigner = ENCLAVE_KEY_2.parse().unwrap();
    let enclave_addr = enclave_signer.address();

    let tx = admin_verifier
        .register_enclave(enclave_addr, B256::from([0xCDu8; 32]))
        .await
        .expect("Failed to register enclave");
    println!("Enclave registered: tx={tx}");

    // Submit result
    let model_hash = B256::from([0x11u8; 32]);
    let input_hash = B256::from([0x22u8; 32]);
    let result_data = b"prediction:class_1";
    let attestation = build_attestation(ENCLAVE_KEY_2, model_hash, input_hash, result_data).await;

    let submitter_client =
        Client::new(&rpc_url, ADMIN_KEY, &contract_addr).expect("Failed to create submitter");
    let submitter = TEEVerifier::new(submitter_client);

    let stake = U256::from(100_000_000_000_000_000u128);
    let tx = submitter
        .submit_result(model_hash, input_hash, result_data, &attestation, stake)
        .await
        .expect("Failed to submit result");
    println!("Result submitted: tx={tx}");

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse().unwrap());
    let receipt = provider
        .get_transaction_receipt(tx)
        .await
        .expect("Failed to get receipt")
        .expect("Receipt not found");
    let result_id = receipt.inner.logs()[0].topics()[1];
    println!("Result ID: {result_id}");

    // Challenge from a different account
    let challenger_client = Client::new(&rpc_url, CHALLENGER_KEY, &contract_addr)
        .expect("Failed to create challenger");
    let challenger = TEEVerifier::new(challenger_client);

    let bond = U256::from(100_000_000_000_000_000u128);
    let tx = challenger
        .challenge(result_id, bond)
        .await
        .expect("Failed to challenge");
    println!("Result challenged: tx={tx}");

    // Verify result is now challenged
    let result = submitter
        .get_result(result_id)
        .await
        .expect("Failed to get result");
    assert!(result.challenged, "Result should be challenged");
    println!("Result challenged status: {}", result.challenged);
    println!(
        "Challenger: {}",
        result.challenger
    );

    // Verify challenger address matches
    let challenger_signer: PrivateKeySigner = CHALLENGER_KEY.parse().unwrap();
    assert_eq!(
        result.challenger,
        challenger_signer.address(),
        "Challenger address mismatch"
    );

    // Fast-forward past dispute window (24 hours)
    advance_time(&rpc_url, 86401).await;

    // Resolve by timeout (challenger wins since prover didn't submit proof)
    let tx = challenger
        .resolve_dispute_by_timeout(result_id)
        .await
        .expect("Failed to resolve by timeout");
    println!("Dispute resolved by timeout: tx={tx}");

    // Result should not be valid (challenger won)
    let valid = submitter
        .is_result_valid(result_id)
        .await
        .expect("Failed to check validity");
    assert!(!valid, "Result should NOT be valid after challenger wins");
    println!("Result valid (should be false): {valid}");
}

#[tokio::test]
#[ignore] // Requires Anvil + deployed contract
async fn test_tee_pause_unpause() {
    let rpc_url = get_env("RPC_URL");
    let contract_addr = get_env("TEE_CONTRACT_ADDRESS");

    let admin_client =
        Client::new(&rpc_url, ADMIN_KEY, &contract_addr).expect("Failed to create client");
    let admin_verifier = TEEVerifier::new(admin_client);

    // Check initial state
    let paused = admin_verifier
        .paused()
        .await
        .expect("Failed to check paused");
    assert!(!paused, "Contract should not be paused initially");

    // Pause
    admin_verifier.pause().await.expect("Failed to pause");
    let paused = admin_verifier
        .paused()
        .await
        .expect("Failed to check paused");
    assert!(paused, "Contract should be paused");
    println!("Contract paused: {paused}");

    // Unpause
    admin_verifier.unpause().await.expect("Failed to unpause");
    let paused = admin_verifier
        .paused()
        .await
        .expect("Failed to check paused");
    assert!(!paused, "Contract should be unpaused");
    println!("Contract unpaused: {paused}");
}
