//! Integration tests that drive the SDK's own clients (`TEEVerifier`, `DAGVerifier`,
//! `ExecutionEngineClient`, `TEEEventWatcher`) against contracts deployed on a local
//! Anvil node. (`integration_tee.rs` tests the contract through its own bindings and
//! never goes through these clients.)
//!
//! Prerequisites:
//!   - `anvil` must be installed (from foundry)
//!   - Contracts must be compiled: `cd contracts && forge build --skip test --skip script`
//!
//! Run:
//!   cargo test -p world-zk-sdk --test integration_sdk_clients -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use serial_test::serial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Barrier;

use world_zk_sdk::abi::TEEMLVerifier;
use world_zk_sdk::{
    compute_input_hash, compute_model_hash, compute_result_hash_from_bytes, Client, DAGFixture,
    DAGVerifier, ExecutionEngineClient, RequestStatus, TEEEvent, TEEEventWatcher, TEEVerifier,
    TransactionReverted,
};

sol! {
    #[sol(rpc)]
    contract ProgramRegistry {
        function registerProgram(bytes32 imageId, string calldata name, string calldata programUrl, bytes32 inputSchema) external;
    }

    /// `RemainderVerifier.initialize`, called once through the proxy constructor.
    function initialize(address admin) external;
}

/// Anvil pre-funded private keys.
const ADMIN_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const USER_KEY: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const CHALLENGER_KEY: &str = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const ENCLAVE_KEY: &str = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";
const SECOND_CHALLENGER_KEY: &str =
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a";

fn address_of(key: &str) -> Address {
    key.parse::<PrivateKeySigner>().unwrap().address()
}

fn contracts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../contracts")
}

/// Creation bytecode of a compiled contract, e.g. `("Upgradeable.sol", "UUPSProxy")`.
fn creation_code(source_file: &str, contract: &str) -> Vec<u8> {
    let path = contracts_dir()
        .join("out")
        .join(source_file)
        .join(format!("{contract}.json"));
    assert!(
        path.exists(),
        "Artifact not found at {path:?}. \
         Run: cd contracts && forge build --skip test --skip script"
    );
    let artifact: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
    artifact["bytecode"]["object"]
        .as_str()
        .expect("bytecode.object not found in artifact")
        .parse::<Bytes>()
        .expect("invalid bytecode hex")
        .to_vec()
}

/// A local chain with the admin account's provider.
struct Chain {
    _anvil: AnvilInstance,
    rpc_url: String,
}

impl Chain {
    fn spawn(anvil: Anvil) -> Self {
        let anvil = anvil.spawn();
        let rpc_url = anvil.endpoint();
        Self {
            _anvil: anvil,
            rpc_url,
        }
    }

    fn admin_provider(&self) -> impl Provider + Clone {
        let signer: PrivateKeySigner = ADMIN_KEY.parse().unwrap();
        ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer))
            .connect_http(self.rpc_url.parse().unwrap())
    }

    /// Deploy `creation code ++ constructor args` from the admin account.
    async fn deploy(&self, mut code: Vec<u8>, constructor_args: Vec<u8>) -> Address {
        code.extend_from_slice(&constructor_args);
        let tx = alloy::rpc::types::TransactionRequest::default().with_deploy_code(code);
        let receipt = self
            .admin_provider()
            .send_transaction(tx)
            .await
            .expect("deploy transaction rejected")
            .get_receipt()
            .await
            .expect("no deploy receipt");
        assert!(receipt.status(), "deployment reverted");
        receipt.contract_address.expect("no contract address")
    }

    /// Deploy an upgradeable contract behind `UUPSProxy(implementation, initData)`.
    async fn deploy_proxy(&self, source_file: &str, contract: &str, init_data: Vec<u8>) -> Address {
        let implementation = self
            .deploy(creation_code(source_file, contract), vec![])
            .await;
        self.deploy(
            creation_code("Upgradeable.sol", "UUPSProxy"),
            (implementation, Bytes::from(init_data)).abi_encode_params(),
        )
        .await
    }

    async fn deploy_tee_verifier(&self, remainder_verifier: Address) -> Address {
        let init = TEEMLVerifier::initializeCall {
            _admin: address_of(ADMIN_KEY),
            _remainderVerifier: remainder_verifier,
        };
        self.deploy_proxy("TEEMLVerifier.sol", "TEEMLVerifier", init.abi_encode())
            .await
    }

    fn client(&self, key: &str, contract: Address) -> Client {
        Client::new(&self.rpc_url, key, &contract.to_string()).unwrap()
    }

    async fn increase_time(&self, seconds: u64) {
        let provider = self.admin_provider();
        let _: serde_json::Value = provider
            .raw_request("evm_increaseTime".into(), [U256::from(seconds)])
            .await
            .unwrap();
        let _: serde_json::Value = provider.raw_request("evm_mine".into(), ()).await.unwrap();
    }

    /// Change the block gas limit and mine a block that has it.
    async fn set_block_gas_limit(&self, gas_limit: u64) {
        let provider = self.admin_provider();
        let _: serde_json::Value = provider
            .raw_request("evm_setBlockGasLimit".into(), [U256::from(gas_limit)])
            .await
            .unwrap();
        let _: serde_json::Value = provider.raw_request("evm_mine".into(), ()).await.unwrap();
    }

    async fn chain_id(&self) -> u64 {
        self.admin_provider().get_chain_id().await.unwrap()
    }

    /// Number of the block a transaction was mined in.
    async fn block_of(&self, tx_hash: B256) -> u64 {
        self.admin_provider()
            .get_transaction_receipt(tx_hash)
            .await
            .unwrap()
            .expect("no receipt")
            .block_number
            .expect("receipt without a block number")
    }
}

/// EIP-712 signature `TEEMLVerifier.submitResult` recovers the enclave key from.
async fn sign_attestation(
    enclave_key: &str,
    model_hash: B256,
    input_hash: B256,
    result: &[u8],
    chain_id: u64,
    verifying_contract: Address,
) -> Vec<u8> {
    let domain_separator = keccak256(
        (
            keccak256(
                "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
            ),
            keccak256("TEEMLVerifier"),
            keccak256("1"),
            U256::from(chain_id),
            verifying_contract,
        )
            .abi_encode_params(),
    );
    let struct_hash = keccak256(
        (
            keccak256("TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)"),
            model_hash,
            input_hash,
            keccak256(result),
        )
            .abi_encode_params(),
    );
    let digest = keccak256(
        [
            b"\x19\x01".as_slice(),
            domain_separator.as_slice(),
            struct_hash.as_slice(),
        ]
        .concat(),
    );
    let signer: PrivateKeySigner = enclave_key.parse().unwrap();
    signer.sign_hash(&digest).await.unwrap().as_bytes().to_vec()
}

/// Submit, read back, finalize and watch a result using only SDK clients.
#[serial]
#[tokio::test]
async fn tee_lifecycle_through_sdk_clients() {
    let chain = Chain::spawn(Anvil::new());
    // Any non-zero address: this test resolves no dispute with a proof.
    let tee_address = chain.deploy_tee_verifier(Address::repeat_byte(0xDD)).await;

    let admin = TEEVerifier::new(chain.client(ADMIN_KEY, tee_address));
    let user = TEEVerifier::new(chain.client(USER_KEY, tee_address));

    assert_eq!(admin.admin().await.unwrap(), address_of(ADMIN_KEY));
    assert!(!admin.paused().await.unwrap());
    let stake = admin.prover_stake().await.unwrap();
    assert_eq!(stake, U256::from(100_000_000_000_000_000u128));
    assert_eq!(admin.challenge_bond_amount().await.unwrap(), stake);

    let enclave = address_of(ENCLAVE_KEY);
    admin
        .register_enclave(enclave, B256::repeat_byte(0x42))
        .await
        .unwrap();

    let model_hash = compute_model_hash(b"model file bytes");
    let input_hash = compute_input_hash(&[1.0, 2.5, 3.7]);
    let result = b"[0.85]";
    let chain_id = chain.chain_id().await;

    // An attestation that no registered enclave signed is rejected before sending.
    let rejected = user
        .submit_result(model_hash, input_hash, result, &[0u8; 65], stake)
        .await;
    assert!(rejected.is_err(), "all-zero attestation must be rejected");

    let attestation = sign_attestation(
        ENCLAVE_KEY,
        model_hash,
        input_hash,
        result,
        chain_id,
        tee_address,
    )
    .await;
    let (tx_hash, result_id) = user
        .submit_result_with_id(model_hash, input_hash, result, &attestation, stake)
        .await
        .unwrap();
    assert_ne!(tx_hash, result_id);

    let stored = admin.get_result(result_id).await.unwrap();
    assert_eq!(stored.enclave, enclave);
    assert_eq!(stored.submitter, address_of(USER_KEY));
    assert_eq!(stored.modelHash, model_hash);
    assert_eq!(stored.inputHash, input_hash);
    assert_eq!(stored.resultHash, compute_result_hash_from_bytes(result));
    assert_eq!(stored.proverStakeAmount, stake);
    assert!(!admin.is_result_valid(result_id).await.unwrap());

    // Finalizing inside the challenge window fails; after it, it succeeds.
    assert!(admin.finalize(result_id).await.is_err());
    let window = stored.challengeDeadline - stored.submittedAt;
    chain.increase_time(window.to::<u64>() + 1).await;
    let finalize_tx_hash = admin.finalize(result_id).await.unwrap();
    assert!(admin.is_result_valid(result_id).await.unwrap());

    // The watcher reports each event once and in order, whatever the chunk size
    // and whichever block the chunks start at.
    let submitted_block = chain.block_of(tx_hash).await;
    let finalized_block = chain.block_of(finalize_tx_hash).await;
    let expected = vec![
        TEEEvent::ResultSubmitted {
            result_id,
            model_hash,
            input_hash,
            submitter: address_of(USER_KEY),
            block_number: submitted_block,
        },
        TEEEvent::ResultExpired {
            result_id,
            block_number: finalized_block,
        },
        TEEEvent::ResultFinalized {
            result_id,
            block_number: finalized_block,
        },
    ];
    let latest = chain.admin_provider().get_block_number().await.unwrap();
    assert!(submitted_block > 1 && finalized_block > submitted_block);
    for (max_block_range, from_block) in [(2, 0), (1, 0), (1, 1)] {
        let watcher =
            TEEEventWatcher::new(&chain.rpc_url, tee_address).with_max_block_range(max_block_range);
        // The time limit turns a chunk loop that does not advance into a failure.
        let (events, next_block) =
            tokio::time::timeout(Duration::from_secs(30), watcher.poll_events(from_block))
                .await
                .expect("poll_events did not return")
                .unwrap();
        assert_eq!(
            events, expected,
            "max_block_range {max_block_range}, from block {from_block}"
        );
        assert_eq!(next_block, latest + 1);
    }
}

/// Start a JSON-RPC proxy in front of `upstream` that holds every
/// `eth_sendRawTransaction` until `senders` of them have arrived, and return its URL.
///
/// Anvil estimates gas on top of the transactions in its pool. Of two conflicting
/// calls, one that estimates after the other was sent is refused and never sent.
/// Behind this proxy every caller has estimated before any transaction is in the pool.
async fn spawn_send_gate(upstream: String, senders: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let all_arrived = Arc::new(Barrier::new(senders));
    let http = reqwest::Client::new();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (upstream, all_arrived, http) =
                (upstream.clone(), all_arrived.clone(), http.clone());
            tokio::spawn(async move {
                let contains = |haystack: &[u8], needle: &[u8]| {
                    haystack
                        .windows(needle.len())
                        .position(|window| window == needle)
                };
                let mut received = Vec::new();
                // One HTTP/1.1 request per iteration: the headers, then
                // `Content-Length` bytes of JSON-RPC.
                loop {
                    let body_start = loop {
                        if let Some(end) = contains(&received, b"\r\n\r\n") {
                            break end + 4;
                        }
                        if socket.read_buf(&mut received).await.unwrap_or(0) == 0 {
                            return;
                        }
                    };
                    let body_len: usize = String::from_utf8_lossy(&received[..body_start])
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .expect("request without Content-Length");
                    while received.len() < body_start + body_len {
                        if socket.read_buf(&mut received).await.unwrap_or(0) == 0 {
                            return;
                        }
                    }
                    let body: Vec<u8> = received
                        .drain(..body_start + body_len)
                        .skip(body_start)
                        .collect();

                    if contains(&body, b"eth_sendRawTransaction").is_some() {
                        all_arrived.wait().await;
                    }
                    let response = http
                        .post(&upstream)
                        .header("content-type", "application/json")
                        .body(body)
                        .send()
                        .await
                        .unwrap()
                        .bytes()
                        .await
                        .unwrap();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                        response.len()
                    );
                    if socket.write_all(head.as_bytes()).await.is_err()
                        || socket.write_all(&response).await.is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    url
}

/// A transaction that is mined but reverts makes the SDK method return an error.
///
/// Two challengers challenge the same result, and both transactions are mined in
/// the same block: the one that executes second reverts with `AlreadyChallenged`.
#[serial]
#[tokio::test]
async fn mined_but_reverted_transaction_is_an_error() {
    let chain = Chain::spawn(Anvil::new());
    let tee_address = chain.deploy_tee_verifier(Address::repeat_byte(0xDD)).await;

    let admin = TEEVerifier::new(chain.client(ADMIN_KEY, tee_address));
    let user = TEEVerifier::new(chain.client(USER_KEY, tee_address));
    admin
        .register_enclave(address_of(ENCLAVE_KEY), B256::repeat_byte(0x42))
        .await
        .unwrap();
    let model_hash = compute_model_hash(b"model file bytes");
    let input_hash = compute_input_hash(&[1.0]);
    let result = b"[0.85]";
    let attestation = sign_attestation(
        ENCLAVE_KEY,
        model_hash,
        input_hash,
        result,
        chain.chain_id().await,
        tee_address,
    )
    .await;
    let stake = user.prover_stake().await.unwrap();
    let (_, result_id) = user
        .submit_result_with_id(model_hash, input_hash, result, &attestation, stake)
        .await
        .unwrap();
    let bond = admin.challenge_bond_amount().await.unwrap();

    // From here on blocks are only mined on request.
    let provider = chain.admin_provider();
    let _: serde_json::Value = provider
        .raw_request("evm_setAutomine".into(), [false])
        .await
        .unwrap();

    let gate_url = spawn_send_gate(chain.rpc_url.clone(), 2).await;
    let challenger = |key: &str| {
        TEEVerifier::new(Client::new(&gate_url, key, &tee_address.to_string()).unwrap())
    };
    let (first, second) = (
        challenger(CHALLENGER_KEY),
        challenger(SECOND_CHALLENGER_KEY),
    );
    let mine_once_both_are_pending = async {
        loop {
            let pool: serde_json::Value = provider
                .raw_request("txpool_status".into(), ())
                .await
                .unwrap();
            if pool["pending"] == "0x2" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _: serde_json::Value = provider.raw_request("evm_mine".into(), ()).await.unwrap();
    };
    let (first_outcome, second_outcome, ()) = tokio::join!(
        first.challenge(result_id, bond),
        second.challenge(result_id, bond),
        mine_once_both_are_pending,
    );

    let (winner_key, winner_tx_hash, error) = match (first_outcome, second_outcome) {
        (Ok(tx_hash), Err(error)) => (CHALLENGER_KEY, tx_hash, error),
        (Err(error), Ok(tx_hash)) => (SECOND_CHALLENGER_KEY, tx_hash, error),
        (first, second) => {
            panic!("one challenge must succeed and the other revert, got {first:?} and {second:?}")
        }
    };
    let msg = error.to_string();
    assert!(
        msg.contains("was mined in block") && msg.contains("but reverted"),
        "{msg}"
    );
    let reverted = error
        .downcast_ref::<TransactionReverted>()
        .expect("the error must be a TransactionReverted");
    assert_eq!(
        reverted.block_number,
        Some(chain.block_of(winner_tx_hash).await),
        "both transactions must be in the same block"
    );
    assert_eq!(
        admin.get_result(result_id).await.unwrap().challenger,
        address_of(winner_key)
    );
}

/// Register a circuit through the SDK, verify the fixture proof against it, and win
/// a dispute with that proof.
///
/// Guards two defects: `register_circuit` used to register a SHA-256 generator hash
/// (the contract compares keccak256, so every verification reverted), and
/// `resolve_dispute` used to send `eth_estimateGas`'s limit, with which the inner
/// verification ran out of gas and the dispute was settled for the challenger.
#[serial]
#[tokio::test]
async fn dag_register_verify_and_win_dispute_through_sdk_clients() {
    // RemainderVerifier exceeds the 24,576-byte code limit and one verification
    // needs about 200M gas.
    let chain =
        Chain::spawn(Anvil::new().args(["--disable-code-size-limit", "--gas-limit", "500000000"]));

    let init = initializeCall {
        admin: address_of(ADMIN_KEY),
    };
    let verifier_address = chain
        .deploy_proxy(
            "RemainderVerifier.sol",
            "RemainderVerifier",
            init.abi_encode(),
        )
        .await;

    let fixture =
        DAGFixture::load(contracts_dir().join("test/fixtures/phase1a_dag_fixture.json")).unwrap();
    let proof = fixture.to_proof_data().unwrap();
    let description = fixture.to_dag_description().unwrap();

    let dag = DAGVerifier::new(chain.client(ADMIN_KEY, verifier_address));
    assert!(!dag.is_circuit_active(proof.circuit_hash).await.unwrap());
    dag.register_circuit(
        proof.circuit_hash,
        &description,
        "xgboost-phase1a",
        &proof.gens_data,
    )
    .await
    .unwrap();
    assert!(dag.is_circuit_active(proof.circuit_hash).await.unwrap());
    assert!(
        dag.verify_single_tx(&proof).await.unwrap(),
        "the fixture proof must verify against a circuit registered through the SDK"
    );

    // A dispute over a TEE result, resolved with the same proof.
    let tee_address = chain.deploy_tee_verifier(verifier_address).await;
    let admin = TEEVerifier::new(chain.client(ADMIN_KEY, tee_address));
    let user = TEEVerifier::new(chain.client(USER_KEY, tee_address));
    let challenger = TEEVerifier::new(chain.client(CHALLENGER_KEY, tee_address));

    admin
        .register_enclave(address_of(ENCLAVE_KEY), B256::repeat_byte(0x42))
        .await
        .unwrap();
    let model_hash = compute_model_hash(b"model file bytes");
    let input_hash = compute_input_hash(&[7.0]);
    let result = b"[0.85]";
    let attestation = sign_attestation(
        ENCLAVE_KEY,
        model_hash,
        input_hash,
        result,
        chain.chain_id().await,
        tee_address,
    )
    .await;
    let stake = user.prover_stake().await.unwrap();
    let (_, result_id) = user
        .submit_result_with_id(model_hash, input_hash, result, &attestation, stake)
        .await
        .unwrap();
    let bond = challenger.challenge_bond_amount().await.unwrap();
    challenger.challenge(result_id, bond).await.unwrap();
    assert!(!admin.is_result_valid(result_id).await.unwrap());

    // A proof that does not verify is refused before anything is sent.
    let mut broken_proof = proof.proof_bytes.clone();
    let last = broken_proof.len() - 1;
    broken_proof[last] ^= 0xff;
    let refused = user
        .resolve_dispute(
            result_id,
            &broken_proof,
            proof.circuit_hash,
            &proof.public_inputs,
            &proof.gens_data,
        )
        .await;
    let refused = format!(
        "{:#}",
        refused.expect_err("a broken proof must not be sent")
    );
    assert!(
        refused.contains(&format!(
            "verifyDAGProof on RemainderVerifier {verifier_address}"
        )),
        "{refused}"
    );
    assert!(refused.contains("nothing was sent"), "{refused}");
    assert!(
        admin.get_result(result_id).await.unwrap().challenged
            && !admin.is_result_valid(result_id).await.unwrap(),
        "the dispute must still be open"
    );

    // A valid proof is refused too while the block gas limit is below the gas
    // limit resolveDispute has to be sent with.
    chain.set_block_gas_limit(300_000_000).await;
    let refused = user
        .resolve_dispute(
            result_id,
            &proof.proof_bytes,
            proof.circuit_hash,
            &proof.public_inputs,
            &proof.gens_data,
        )
        .await;
    let refused = format!(
        "{:#}",
        refused.expect_err("a transaction no block can hold must not be sent")
    );
    assert!(
        refused.contains("block gas limit is 300000000"),
        "{refused}"
    );
    assert!(refused.contains("nothing was sent"), "{refused}");
    assert!(
        admin.get_result(result_id).await.unwrap().challenged
            && !admin.is_result_valid(result_id).await.unwrap(),
        "the dispute must still be open"
    );
    chain.set_block_gas_limit(500_000_000).await;

    user.resolve_dispute(
        result_id,
        &proof.proof_bytes,
        proof.circuit_hash,
        &proof.public_inputs,
        &proof.gens_data,
    )
    .await
    .unwrap();
    assert!(
        admin.is_result_valid(result_id).await.unwrap(),
        "a valid proof must settle the dispute for the prover"
    );
}

/// Request, claim and prove one execution through `ExecutionEngineClient`.
#[serial]
#[tokio::test]
async fn execution_request_lifecycle_through_sdk_client() {
    let chain = Chain::spawn(Anvil::new());
    let admin_address = address_of(ADMIN_KEY);

    let mock_verifier = chain
        .deploy(
            creation_code("MockRiscZeroVerifier.sol", "MockRiscZeroVerifier"),
            vec![],
        )
        .await;
    let registry = chain
        .deploy(
            creation_code("ProgramRegistry.sol", "ProgramRegistry"),
            admin_address.abi_encode(),
        )
        .await;
    let engine_address = chain
        .deploy(
            creation_code("ExecutionEngine.sol", "ExecutionEngine"),
            (admin_address, registry, mock_verifier, admin_address).abi_encode_params(),
        )
        .await;

    let image_id = B256::repeat_byte(0x77);
    let registered = ProgramRegistry::new(registry, chain.admin_provider())
        .registerProgram(
            image_id,
            "program".to_string(),
            "https://example.invalid/program".to_string(),
            B256::ZERO,
        )
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(registered.status());

    let requester = ExecutionEngineClient::new(chain.client(USER_KEY, engine_address));
    let prover = ExecutionEngineClient::new(chain.client(CHALLENGER_KEY, engine_address));

    let request_id = requester.next_request_id().await.unwrap().to::<u64>();
    let tip = U256::from(10_000_000_000_000_000u64);
    requester
        .submit_request(
            image_id,
            B256::repeat_byte(0x02),
            "https://example.invalid/input",
            Address::ZERO,
            3600,
            0,
            tip,
        )
        .await
        .unwrap();

    let request = requester.get_request(request_id).await.unwrap();
    assert_eq!(request.imageId, image_id);
    assert_eq!(request.requester, address_of(USER_KEY));
    assert_eq!(request.maxTip, tip);
    assert_eq!(
        requester.get_request_status(request_id).await.unwrap(),
        RequestStatus::Pending
    );
    assert_eq!(
        requester.get_pending_requests(0, 10).await.unwrap(),
        vec![U256::from(request_id)]
    );

    prover.claim_job(request_id).await.unwrap();
    assert_eq!(
        requester.get_request_status(request_id).await.unwrap(),
        RequestStatus::Claimed
    );
    // Only the claimant may submit the proof.
    assert!(requester
        .submit_result(request_id, b"seal", b"journal")
        .await
        .is_err());
    prover
        .submit_result(request_id, b"seal", b"journal")
        .await
        .unwrap();
    assert_eq!(
        requester.get_request_status(request_id).await.unwrap(),
        RequestStatus::Completed
    );

    let (completed, earnings) = prover
        .get_prover_stats(address_of(CHALLENGER_KEY))
        .await
        .unwrap();
    assert_eq!(completed, U256::from(1));
    assert!(earnings > U256::ZERO && earnings <= tip);
}
