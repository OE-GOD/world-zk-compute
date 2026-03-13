//! Integration tests for the EventWatcher against a local Anvil instance.
//!
//! Each test spawns its own Anvil node, deploys a minimal EventEmitter
//! contract that emits the same events as TEEMLVerifier, and verifies that
//! `EventWatcher` correctly detects and parses each event type.
//!
//! All tests are `#[ignore]` — run with:
//!   cd services/operator && cargo test watcher_integration -- --ignored --nocapture
//!
//! Prerequisites: `anvil` and `solc` must be installed.

use std::process::{Command, Stdio};
use std::time::Duration;

use alloy::primitives::{keccak256, Address, Bytes, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;

use tee_operator::watcher::{EventWatcher, TEEEvent};

// ---------------------------------------------------------------------------
// Anvil helper (manual spawn on random port)
// ---------------------------------------------------------------------------

struct AnvilProc {
    child: std::process::Child,
    port: u16,
}

impl AnvilProc {
    fn spawn() -> Self {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };

        let child = Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--accounts",
                "1",
                "--balance",
                "10000",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil not found — install foundry");

        Self { child, port }
    }

    fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn wait_ready(&self) {
        for _ in 0..50 {
            if let Ok(parsed) = self.rpc_url().parse() {
                let provider = ProviderBuilder::new().connect_http(parsed);
                if provider.get_block_number().await.is_ok() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("Anvil not ready on port {}", self.port);
    }
}

impl Drop for AnvilProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Deploy a minimal EventEmitter contract via solc
// ---------------------------------------------------------------------------

const EVENT_EMITTER_SOL: &str = r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;
contract EventEmitter {
    event ResultSubmitted(bytes32 indexed resultId, bytes32 indexed modelHash, bytes32 inputHash, address indexed submitter);
    event ResultChallenged(bytes32 indexed resultId, address challenger);
    event ResultFinalized(bytes32 indexed resultId);
    event ResultExpired(bytes32 indexed resultId);

    function emitSubmitted(bytes32 rid, bytes32 mh, bytes32 ih, address sub) external {
        emit ResultSubmitted(rid, mh, ih, sub);
    }
    function emitChallenged(bytes32 rid, address ch) external {
        emit ResultChallenged(rid, ch);
    }
    function emitFinalized(bytes32 rid) external {
        emit ResultFinalized(rid);
    }
    function emitExpired(bytes32 rid) external {
        emit ResultExpired(rid);
    }
}
"#;

async fn deploy_event_emitter(rpc_url: &str) -> Address {
    let tmp_dir = tempfile::tempdir().unwrap();
    let sol_path = tmp_dir.path().join("EventEmitter.sol");
    std::fs::write(&sol_path, EVENT_EMITTER_SOL).unwrap();

    let output = Command::new("solc")
        .args(["--bin", "--optimize"])
        .arg(&sol_path)
        .output()
        .expect("solc not found — install solc");

    assert!(
        output.status.success(),
        "solc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let bytecode_hex = stdout
        .lines()
        .skip_while(|l| !l.contains("Binary:"))
        .nth(1)
        .expect("no bytecode in solc output")
        .trim();
    let bytecode = hex::decode(bytecode_hex).expect("invalid hex from solc");

    let sender: Address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        .parse()
        .unwrap();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse().unwrap());

    let tx = TransactionRequest::default()
        .from(sender)
        .input(Bytes::from(bytecode).into())
        .gas_limit(3_000_000);

    let receipt = provider
        .send_transaction(tx)
        .await
        .unwrap()
        .get_receipt()
        .await
        .expect("deploy receipt failed");

    receipt
        .contract_address
        .expect("no contract address in receipt")
}

// ---------------------------------------------------------------------------
// Calldata encoders for the EventEmitter
// ---------------------------------------------------------------------------

const ANVIL_SENDER: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

fn sel(sig: &str) -> [u8; 4] {
    let hash = keccak256(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

fn encode_submitted(rid: B256, mh: B256, ih: B256, sub: Address) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 128);
    data.extend_from_slice(&sel("emitSubmitted(bytes32,bytes32,bytes32,address)"));
    data.extend_from_slice(rid.as_slice());
    data.extend_from_slice(mh.as_slice());
    data.extend_from_slice(ih.as_slice());
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(sub.as_slice());
    data
}

fn encode_challenged(rid: B256, ch: Address) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 64);
    data.extend_from_slice(&sel("emitChallenged(bytes32,address)"));
    data.extend_from_slice(rid.as_slice());
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(ch.as_slice());
    data
}

fn encode_finalized(rid: B256) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&sel("emitFinalized(bytes32)"));
    data.extend_from_slice(rid.as_slice());
    data
}

fn encode_expired(rid: B256) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&sel("emitExpired(bytes32)"));
    data.extend_from_slice(rid.as_slice());
    data
}

async fn call_emitter(rpc_url: &str, contract: Address, calldata: Vec<u8>) {
    let sender: Address = ANVIL_SENDER.parse().unwrap();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse().unwrap());
    let tx = TransactionRequest::default()
        .from(sender)
        .to(contract)
        .input(Bytes::from(calldata).into())
        .gas_limit(200_000);
    let receipt = provider
        .send_transaction(tx)
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status(), "emitter call failed");
}

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

fn rid() -> B256 {
    B256::from([0xAA; 32])
}
fn model() -> B256 {
    B256::from([0xBB; 32])
}
fn input() -> B256 {
    B256::from([0xCC; 32])
}
fn submitter() -> Address {
    "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        .parse()
        .unwrap()
}
fn challenger_addr() -> Address {
    "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
        .parse()
        .unwrap()
}

// ===========================================================================
// Tests
// ===========================================================================

#[tokio::test]
#[ignore]
async fn test_watcher_detects_result_submitted() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_submitted(rid(), model(), input(), submitter()),
    )
    .await;

    let (events, next) = watcher.poll_events(0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert!(next > 0);

    match &events[0] {
        TEEEvent::ResultSubmitted {
            result_id,
            model_hash,
            input_hash,
            submitter: sub,
            block_number: _,
        } => {
            assert_eq!(*result_id, rid());
            assert_eq!(*model_hash, model());
            assert_eq!(*input_hash, input());
            assert_eq!(*sub, submitter());
        }
        other => panic!("expected ResultSubmitted, got {:?}", other),
    }
}

#[tokio::test]
#[ignore]
async fn test_watcher_detects_result_challenged() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_challenged(rid(), challenger_addr()),
    )
    .await;

    let (events, _) = watcher.poll_events(0).await.unwrap();
    assert_eq!(events.len(), 1);

    match &events[0] {
        TEEEvent::ResultChallenged {
            result_id,
            challenger,
        } => {
            assert_eq!(*result_id, rid());
            assert_eq!(*challenger, challenger_addr());
        }
        other => panic!("expected ResultChallenged, got {:?}", other),
    }
}

#[tokio::test]
#[ignore]
async fn test_watcher_detects_result_finalized() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    call_emitter(&anvil.rpc_url(), contract, encode_finalized(rid())).await;

    let (events, _) = watcher.poll_events(0).await.unwrap();
    assert_eq!(events.len(), 1);

    match &events[0] {
        TEEEvent::ResultFinalized { result_id } => assert_eq!(*result_id, rid()),
        other => panic!("expected ResultFinalized, got {:?}", other),
    }
}

#[tokio::test]
#[ignore]
async fn test_watcher_detects_result_expired() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    call_emitter(&anvil.rpc_url(), contract, encode_expired(rid())).await;

    let (events, _) = watcher.poll_events(0).await.unwrap();
    assert_eq!(events.len(), 1);

    match &events[0] {
        TEEEvent::ResultExpired { result_id } => assert_eq!(*result_id, rid()),
        other => panic!("expected ResultExpired, got {:?}", other),
    }
}

#[tokio::test]
#[ignore]
async fn test_watcher_no_events() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    let (events, next) = watcher.poll_events(0).await.unwrap();
    assert!(events.is_empty());
    assert!(next > 0);
}

#[tokio::test]
#[ignore]
async fn test_watcher_incremental_polling() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    // First event
    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_finalized(B256::from([0x11; 32])),
    )
    .await;

    let (events1, next1) = watcher.poll_events(0).await.unwrap();
    assert_eq!(events1.len(), 1);

    // Second event
    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_finalized(B256::from([0x22; 32])),
    )
    .await;

    let (events2, next2) = watcher.poll_events(next1).await.unwrap();
    assert_eq!(events2.len(), 1);
    assert!(next2 > next1);

    match &events2[0] {
        TEEEvent::ResultFinalized { result_id } => {
            assert_eq!(*result_id, B256::from([0x22; 32]))
        }
        other => panic!("expected ResultFinalized, got {:?}", other),
    }

    // No new events
    let (events3, next3) = watcher.poll_events(next2).await.unwrap();
    assert!(events3.is_empty());
    assert_eq!(next3, next2);
}

#[tokio::test]
#[ignore]
async fn test_watcher_get_challenges() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    let r1 = B256::from([0x11; 32]);
    let r2 = B256::from([0x22; 32]);

    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_challenged(r1, challenger_addr()),
    )
    .await;
    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_challenged(r2, submitter()),
    )
    .await;
    // Non-challenge event (should be filtered out)
    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_finalized(B256::from([0x33; 32])),
    )
    .await;

    let provider = ProviderBuilder::new().connect_http(anvil.rpc_url().parse().unwrap());
    let latest = provider.get_block_number().await.unwrap();

    let challenges = watcher.get_challenges(0, latest).await.unwrap();
    assert_eq!(challenges.len(), 2);
    assert_eq!(challenges[0].0, r1);
    assert_eq!(challenges[0].1, challenger_addr());
    assert_eq!(challenges[1].0, r2);
    assert_eq!(challenges[1].1, submitter());
}

#[tokio::test]
#[ignore]
async fn test_watcher_get_submissions() {
    let anvil = AnvilProc::spawn();
    anvil.wait_ready().await;

    let contract = deploy_event_emitter(&anvil.rpc_url()).await;
    let watcher = EventWatcher::new(&anvil.rpc_url(), contract);

    let r1 = B256::from([0x11; 32]);
    let r2 = B256::from([0x22; 32]);

    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_submitted(r1, model(), input(), submitter()),
    )
    .await;
    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_submitted(r2, model(), input(), submitter()),
    )
    .await;
    // Non-submission event (should be filtered out)
    call_emitter(
        &anvil.rpc_url(),
        contract,
        encode_challenged(B256::from([0x33; 32]), challenger_addr()),
    )
    .await;

    let provider = ProviderBuilder::new().connect_http(anvil.rpc_url().parse().unwrap());
    let latest = provider.get_block_number().await.unwrap();

    let submissions = watcher.get_submissions(0, latest).await.unwrap();
    assert_eq!(submissions.len(), 2);
    assert_eq!(submissions[0], r1);
    assert_eq!(submissions[1], r2);
}
