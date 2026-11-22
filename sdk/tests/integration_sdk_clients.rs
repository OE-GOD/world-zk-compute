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

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use serial_test::serial;

use world_zk_sdk::abi::TEEMLVerifier;
use world_zk_sdk::{
    compute_input_hash, compute_model_hash, compute_result_hash_from_bytes, Client, DAGFixture,
    DAGVerifier, ExecutionEngineClient, RequestStatus, TEEEvent, TEEEventWatcher, TEEVerifier,
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

    async fn chain_id(&self) -> u64 {
        self.admin_provider().get_chain_id().await.unwrap()
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
    admin.finalize(result_id).await.unwrap();
    assert!(admin.is_result_valid(result_id).await.unwrap());

    // The watcher reports both events, in chunks smaller than the block range.
    let watcher = TEEEventWatcher::new(&chain.rpc_url, tee_address).with_max_block_range(2);
    let (events, next_block) = watcher.poll_events(0).await.unwrap();
    let latest = chain.admin_provider().get_block_number().await.unwrap();
    assert!(latest > 2, "the range must span more than one chunk");
    assert_eq!(next_block, latest + 1);
    assert!(events.iter().any(|event| matches!(
        event,
        TEEEvent::ResultSubmitted { result_id: id, model_hash: model, input_hash: input, submitter, .. }
            if *id == result_id
                && *model == model_hash
                && *input == input_hash
                && *submitter == address_of(USER_KEY)
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TEEEvent::ResultFinalized { result_id: id, .. } if *id == result_id
    )));
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
    assert!(refused.is_err(), "a broken proof must not be sent");
    assert!(
        admin.get_result(result_id).await.unwrap().challenged
            && !admin.is_result_valid(result_id).await.unwrap(),
        "the dispute must still be open"
    );

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
