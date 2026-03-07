//! E2E integration test for DAG batch verification.
//!
//! Requires Anvil running with RemainderVerifier deployed.
//! Set `RPC_URL` and `CONTRACT_ADDRESS` env vars before running.
//!
//! Run via: `scripts/rust-sdk-e2e.sh`
//! Or manually:
//!   RPC_URL=http://127.0.0.1:8549 CONTRACT_ADDRESS=0x... \
//!     cargo test --test e2e_batch_verify -- --ignored --nocapture

use std::path::PathBuf;

use world_zk_sdk::{BatchProgress, Client, DAGFixture, DAGVerifier};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/test/fixtures/phase1a_dag_fixture.json")
}

#[tokio::test]
#[ignore] // Requires Anvil + deployed contract
async fn test_batch_verify_e2e() {
    let rpc_url = std::env::var("RPC_URL").expect("RPC_URL env var required");
    let contract_address =
        std::env::var("CONTRACT_ADDRESS").expect("CONTRACT_ADDRESS env var required");
    let private_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    // Load fixture
    let fixture_path = fixture_path();
    assert!(
        fixture_path.exists(),
        "Fixture not found at {:?}",
        fixture_path
    );
    let fixture = DAGFixture::load(&fixture_path).expect("Failed to load fixture");
    let proof = fixture.to_proof_data().expect("Failed to convert proof data");

    // Create client + verifier
    let client =
        Client::new(&rpc_url, private_key, &contract_address).expect("Failed to create client");
    let verifier = DAGVerifier::new(client);

    // Verify circuit is registered
    let active = verifier
        .is_circuit_active(proof.circuit_hash)
        .await
        .expect("Failed to query circuit status");
    assert!(active, "Circuit should be active after deployment");

    // Run batch verification
    println!("Starting batch verification...");
    let session_id = verifier
        .verify_batch(&proof, |progress| match &progress {
            BatchProgress::Started {
                session_id,
                total_batches,
            } => {
                println!(
                    "[Start] Session: {}, Total batches: {}",
                    session_id, total_batches
                );
            }
            BatchProgress::Computing { batch, total } => {
                println!("[Continue] Batch {}/{}", batch, total);
            }
            BatchProgress::Finalizing { step } => {
                println!("[Finalize] Step {}", step);
            }
            BatchProgress::Complete => {
                println!("[Complete] Batch verification finished");
            }
        })
        .await
        .expect("Batch verification failed");

    assert!(!session_id.is_zero(), "Session ID should be non-zero");
    println!("Batch verification succeeded. Session ID: {}", session_id);
}
