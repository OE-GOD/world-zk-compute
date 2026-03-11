//! Self-contained integration tests for TEEMLVerifier using local Anvil instances.
//!
//! Each test spawns its own Anvil node and deploys a fresh TEEMLVerifier contract,
//! so tests are fully independent and require no external setup.
//!
//! Prerequisites:
//!   - `anvil` must be installed (from foundry)
//!   - Contract must be compiled: `cd contracts && forge build --skip test --skip script`
//!   - The artifact at `contracts/out/TEEMLVerifier.sol/TEEMLVerifier.json` must exist
//!
//! Run:
//!   cd sdk && cargo test integration_tee -- --nocapture

use std::path::PathBuf;

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::node_bindings::Anvil;
use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::{ext::AnvilApi, Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::SolValue;

// --------------------------------------------------------------------------
// Contract binding with full ABI (no bytecode -- we load it from artifact)
// --------------------------------------------------------------------------

sol! {
    #[sol(rpc)]
    contract TEEMLVerifier {
        struct EnclaveInfo {
            bool registered;
            bool active;
            bytes32 enclaveImageHash;
            uint256 registeredAt;
        }

        struct MLResult {
            address enclave;
            address submitter;
            bytes32 modelHash;
            bytes32 inputHash;
            bytes32 resultHash;
            bytes result;
            uint256 submittedAt;
            uint256 challengeDeadline;
            uint256 disputeDeadline;
            uint256 challengeBond;
            uint256 proverStakeAmount;
            bool finalized;
            bool challenged;
            address challenger;
        }

        // Events
        event EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash);
        event EnclaveRevoked(address indexed enclaveKey);
        event ResultSubmitted(bytes32 indexed resultId, bytes32 modelHash, bytes32 inputHash);
        event ResultChallenged(bytes32 indexed resultId, address challenger);
        event DisputeResolved(bytes32 indexed resultId, bool proverWon);
        event ResultFinalized(bytes32 indexed resultId);
        event ResultExpired(bytes32 indexed resultId);
        event ChallengeBondUpdated(uint256 oldAmount, uint256 newAmount);
        event ProverStakeUpdated(uint256 oldAmount, uint256 newAmount);
        event RemainderVerifierUpdated(address oldVerifier, address newVerifier);
        event ConfigUpdated(string param, uint256 oldValue, uint256 newValue);
        event DisputeExtended(bytes32 indexed resultId, uint256 newDeadline);

        // Admin
        function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external;
        function revokeEnclave(address enclaveKey) external;
        function setRemainderVerifier(address _verifier) external;
        function setChallengeBondAmount(uint256 _amount) external;
        function setProverStake(uint256 _amount) external;

        // Submit
        function submitResult(bytes32 modelHash, bytes32 inputHash, bytes calldata result, bytes calldata attestation)
            external payable returns (bytes32 resultId);

        // Challenge
        function challenge(bytes32 resultId) external payable;

        // Dispute
        function resolveDispute(bytes32 resultId, bytes calldata proof, bytes32 circuitHash, bytes calldata publicInputs, bytes calldata gensData) external;
        function resolveDisputeByTimeout(bytes32 resultId) external;
        function extendDisputeWindow(bytes32 resultId) external;

        // Finalize
        function finalize(bytes32 resultId) external;

        // Query
        function getResult(bytes32 resultId) external view returns (MLResult memory);
        function isResultValid(bytes32 resultId) external view returns (bool);

        // Ownable2Step
        function owner() external view returns (address);
        function pendingOwner() external view returns (address);
        function transferOwnership(address newOwner) external;
        function acceptOwnership() external;

        // Pausable
        function pause() external;
        function unpause() external;
        function paused() external view returns (bool);

        // State
        function remainderVerifier() external view returns (address);
        function challengeBondAmount() external view returns (uint256);
        function proverStake() external view returns (uint256);
        function disputeResolved(bytes32 resultId) external view returns (bool);
        function disputeProverWon(bytes32 resultId) external view returns (bool);
        function enclaves(address key) external view returns (bool registered, bool active, bytes32 enclaveImageHash, uint256 registeredAt);

        // Constants
        function CHALLENGE_WINDOW() external view returns (uint256);
        function DISPUTE_WINDOW() external view returns (uint256);

        // Constructor
        constructor(address _admin, address _remainderVerifier);
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/// Load the compiled TEEMLVerifier bytecode from the forge artifact.
fn load_contract_bytecode() -> Bytes {
    let artifact_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/out/TEEMLVerifier.sol/TEEMLVerifier.json");
    assert!(
        artifact_path.exists(),
        "TEEMLVerifier artifact not found at {artifact_path:?}. Run: cd contracts && forge build --skip test --skip script"
    );
    let artifact: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(&artifact_path).unwrap()).unwrap();
    let bytecode_hex = artifact["bytecode"]["object"]
        .as_str()
        .expect("bytecode.object not found in artifact");
    bytecode_hex.parse::<Bytes>().expect("invalid bytecode hex")
}

/// Encode the constructor arguments (address _admin, address _remainderVerifier).
fn encode_constructor(admin: Address, remainder_verifier: Address) -> Vec<u8> {
    (admin, remainder_verifier).abi_encode_params()
}

/// Anvil pre-funded private keys (from Anvil's default accounts).
const ADMIN_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const USER_KEY: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const CHALLENGER_KEY: &str = "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const NEW_OWNER_KEY: &str = "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

fn parse_signer(key: &str) -> PrivateKeySigner {
    key.parse::<PrivateKeySigner>().unwrap()
}

/// Shared test context: Anvil instance + deployed contract.
struct TestContext {
    /// Keep anvil alive for the duration of the test.
    _anvil: alloy::node_bindings::AnvilInstance,
    rpc_url: String,
    contract_addr: Address,
    admin_signer: PrivateKeySigner,
}

impl TestContext {
    /// Spawn a fresh Anvil and deploy TEEMLVerifier.
    async fn new() -> Self {
        let anvil = Anvil::new().spawn();
        let rpc_url = anvil.endpoint();

        let admin_signer = parse_signer(ADMIN_KEY);
        let admin_addr = admin_signer.address();
        let wallet = EthereumWallet::from(admin_signer.clone());

        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(rpc_url.parse().unwrap());

        // Build deployment bytecode: creation code + constructor args
        let creation_code = load_contract_bytecode();
        let constructor_args = encode_constructor(admin_addr, Address::ZERO);

        let mut deploy_data = creation_code.to_vec();
        deploy_data.extend_from_slice(&constructor_args);

        // Deploy
        let tx = alloy::rpc::types::TransactionRequest::default()
            .with_deploy_code(deploy_data);
        let receipt = provider
            .send_transaction(tx)
            .await
            .expect("Failed to send deploy tx")
            .get_receipt()
            .await
            .expect("Failed to get deploy receipt");

        let contract_addr = receipt
            .contract_address
            .expect("No contract address in deploy receipt");

        Self {
            _anvil: anvil,
            rpc_url,
            contract_addr,
            admin_signer,
        }
    }

    /// Build a provider with the given private key.
    fn provider_with_key(
        &self,
        key: &str,
    ) -> impl alloy::providers::Provider + Clone {
        let signer = parse_signer(key);
        let wallet = EthereumWallet::from(signer);
        ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(self.rpc_url.parse().unwrap())
    }

    /// Build a contract handle for the given private key.
    fn contract_with_key(
        &self,
        key: &str,
    ) -> TEEMLVerifier::TEEMLVerifierInstance<
        impl alloy::providers::Provider + Clone,
    > {
        let provider = self.provider_with_key(key);
        TEEMLVerifier::new(self.contract_addr, provider)
    }

    /// Build an unauthenticated (read-only) provider.
    fn provider_readonly(&self) -> impl alloy::providers::Provider + Clone {
        ProviderBuilder::new().connect_http(self.rpc_url.parse().unwrap())
    }

    /// Fast-forward Anvil time by the given number of seconds and mine a block.
    async fn advance_time(&self, seconds: u64) {
        let provider = self.provider_readonly();
        provider.anvil_increase_time(seconds).await.unwrap();
        provider.anvil_mine(Some(1), None).await.unwrap();
    }
}

/// Build an ECDSA attestation matching what TEEMLVerifier.submitResult expects.
async fn build_attestation(
    enclave_key: &str,
    model_hash: B256,
    input_hash: B256,
    result: &[u8],
) -> Vec<u8> {
    let signer = parse_signer(enclave_key);
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

/// Extract the resultId from the first log topic of a submitResult receipt.
fn extract_result_id(receipt: &alloy::rpc::types::TransactionReceipt) -> B256 {
    // ResultSubmitted(bytes32 indexed resultId, ...)
    receipt.inner.logs()[0].topics()[1]
}

// ==========================================================================
// Tests
// ==========================================================================

/// 1. Deploy and verify initial state.
#[tokio::test]
async fn test_deploy_and_initial_state() {
    let ctx = TestContext::new().await;
    let contract = ctx.contract_with_key(ADMIN_KEY);

    // Owner should be the admin address
    let owner = contract.owner().call().await.unwrap();
    assert_eq!(
        owner,
        ctx.admin_signer.address(),
        "Owner should be the deployer (admin)"
    );

    // Contract should not be paused
    let paused = contract.paused().call().await.unwrap();
    assert!(!paused, "Contract should not be paused initially");

    // Remainder verifier should be zero (we deployed with Address::ZERO)
    let rv = contract.remainderVerifier().call().await.unwrap();
    assert_eq!(rv, Address::ZERO, "remainderVerifier should be zero");

    // Default stake and bond should be 0.1 ether
    let stake = contract.proverStake().call().await.unwrap();
    assert_eq!(
        stake,
        U256::from(100_000_000_000_000_000u128),
        "Default prover stake should be 0.1 ETH"
    );

    let bond = contract.challengeBondAmount().call().await.unwrap();
    assert_eq!(
        bond,
        U256::from(100_000_000_000_000_000u128),
        "Default challenge bond should be 0.1 ETH"
    );

    // Challenge window should be 1 hour
    let cw = contract.CHALLENGE_WINDOW().call().await.unwrap();
    assert_eq!(cw, U256::from(3600u64), "Challenge window should be 3600 seconds");

    // Dispute window should be 24 hours
    let dw = contract.DISPUTE_WINDOW().call().await.unwrap();
    assert_eq!(dw, U256::from(86400u64), "Dispute window should be 86400 seconds");

    println!("test_deploy_and_initial_state PASSED");
}

/// 2. Register an enclave and verify it is registered.
#[tokio::test]
async fn test_register_enclave() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    let enclave_signer = parse_signer(USER_KEY);
    let enclave_addr = enclave_signer.address();
    let image_hash = B256::from([0xABu8; 32]);

    // Register enclave
    let receipt = admin_contract
        .registerEnclave(enclave_addr, image_hash)
        .send()
        .await
        .expect("registerEnclave send failed")
        .get_receipt()
        .await
        .expect("registerEnclave receipt failed");

    assert!(receipt.status(), "registerEnclave tx should succeed");

    // Verify the EnclaveRegistered event was emitted
    assert!(
        !receipt.inner.logs().is_empty(),
        "Should emit at least one event"
    );
    let topic = receipt.inner.logs()[0].topics()[1];
    // topic[1] is the indexed enclaveKey (address, left-padded to 32 bytes)
    let expected_topic = B256::left_padding_from(enclave_addr.as_slice());
    assert_eq!(
        topic, expected_topic,
        "EnclaveRegistered event should contain enclave address"
    );

    // Verify enclave is registered via enclaves() mapping
    let info = admin_contract.enclaves(enclave_addr).call().await.unwrap();
    assert!(info.registered, "Enclave should be registered");
    assert!(info.active, "Enclave should be active");
    assert_eq!(
        info.enclaveImageHash, image_hash,
        "Image hash should match"
    );
    assert!(info.registeredAt > U256::ZERO, "registeredAt should be set");

    // Non-owner cannot register
    let user_contract = ctx.contract_with_key(USER_KEY);
    let result = user_contract
        .registerEnclave(Address::from([0x99u8; 20]), B256::ZERO)
        .send()
        .await;
    assert!(result.is_err(), "Non-owner should not be able to register enclave");

    println!("test_register_enclave PASSED");
}

/// 3. Submit a result with stake and verify the result struct.
#[tokio::test]
async fn test_submit_result_with_stake() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Register an enclave (use USER_KEY as enclave signer)
    let enclave_signer = parse_signer(USER_KEY);
    let enclave_addr = enclave_signer.address();
    admin_contract
        .registerEnclave(enclave_addr, B256::from([0x11u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit a result (admin acts as submitter)
    let model_hash = B256::from([0x01u8; 32]);
    let input_hash = B256::from([0x02u8; 32]);
    let result_data = b"prediction:class_0";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;

    let stake = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
    let receipt = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await
        .expect("submitResult send failed")
        .get_receipt()
        .await
        .expect("submitResult receipt failed");

    assert!(receipt.status(), "submitResult tx should succeed");

    // Extract resultId from event
    let result_id = extract_result_id(&receipt);
    assert!(!result_id.is_zero(), "Result ID should be non-zero");

    // Verify result struct
    let r = admin_contract.getResult(result_id).call().await.unwrap();
    assert_eq!(r.enclave, enclave_addr, "Enclave address should match");
    assert_eq!(
        r.submitter,
        ctx.admin_signer.address(),
        "Submitter should be admin"
    );
    assert_eq!(r.modelHash, model_hash);
    assert_eq!(r.inputHash, input_hash);
    assert_eq!(r.resultHash, keccak256(result_data));
    assert_eq!(r.proverStakeAmount, stake);
    assert!(!r.finalized);
    assert!(!r.challenged);

    // Result should not be valid yet (not finalized)
    let valid = admin_contract.isResultValid(result_id).call().await.unwrap();
    assert!(!valid, "Result should not be valid before finalization");

    // Insufficient stake should revert
    let attestation2 = build_attestation(USER_KEY, B256::from([0x03u8; 32]), input_hash, result_data).await;
    let result = admin_contract
        .submitResult(
            B256::from([0x03u8; 32]),
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation2),
        )
        .value(U256::from(1u64)) // 1 wei -- insufficient
        .send()
        .await;
    assert!(
        result.is_err(),
        "Should revert with insufficient stake"
    );

    println!("test_submit_result_with_stake PASSED");
}

/// 4. Submit, warp time past challenge window, finalize.
#[tokio::test]
async fn test_finalize_after_time_warp() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave
    let enclave_addr = parse_signer(USER_KEY).address();
    admin_contract
        .registerEnclave(enclave_addr, B256::from([0x22u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit result
    let model_hash = B256::from([0x10u8; 32]);
    let input_hash = B256::from([0x20u8; 32]);
    let result_data = b"test-finalize-result";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let receipt = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let result_id = extract_result_id(&receipt);

    // Try to finalize before challenge window passes -- should fail
    let early_finalize = admin_contract.finalize(result_id).send().await;
    assert!(
        early_finalize.is_err(),
        "Finalize should fail before challenge window passes"
    );

    // Get submitter balance before finalize
    let provider = ctx.provider_readonly();
    let balance_before = provider
        .get_balance(ctx.admin_signer.address())
        .await
        .unwrap();

    // Warp time past challenge window (1 hour + 1 second)
    ctx.advance_time(3601).await;

    // Finalize should now succeed
    let finalize_receipt = admin_contract
        .finalize(result_id)
        .send()
        .await
        .expect("finalize send failed")
        .get_receipt()
        .await
        .expect("finalize receipt failed");
    assert!(finalize_receipt.status(), "finalize tx should succeed");

    // Result should now be valid
    let valid = admin_contract.isResultValid(result_id).call().await.unwrap();
    assert!(valid, "Result should be valid after finalization");

    // Verify stake was returned (balance increased)
    let balance_after = provider
        .get_balance(ctx.admin_signer.address())
        .await
        .unwrap();
    // balance_after > balance_before - gas (stake returned, minus gas for finalize tx)
    // We cannot do exact comparison due to gas costs, but the difference should be
    // close to +stake (0.1 ETH returned minus gas). At minimum, balance should not
    // have decreased by more than gas cost.
    let gas_upper_bound = U256::from(1_000_000_000_000_000u128); // 0.001 ETH max gas
    assert!(
        balance_after > balance_before - gas_upper_bound,
        "Stake should have been returned (balance should not drop significantly)"
    );

    // Cannot finalize again
    let double_finalize = admin_contract.finalize(result_id).send().await;
    assert!(
        double_finalize.is_err(),
        "Should not be able to finalize twice"
    );

    println!("test_finalize_after_time_warp PASSED");
}

/// 5. Challenge flow: submit -> challenge -> verify challenged state.
#[tokio::test]
async fn test_challenge_flow() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave
    let enclave_addr = parse_signer(USER_KEY).address();
    admin_contract
        .registerEnclave(enclave_addr, B256::from([0x33u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit result
    let model_hash = B256::from([0x30u8; 32]);
    let input_hash = B256::from([0x40u8; 32]);
    let result_data = b"challenge-test-result";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let receipt = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let result_id = extract_result_id(&receipt);

    // Challenge with insufficient bond should fail
    let challenger_contract = ctx.contract_with_key(CHALLENGER_KEY);
    let insufficient_challenge = challenger_contract
        .challenge(result_id)
        .value(U256::from(1u64))
        .send()
        .await;
    assert!(
        insufficient_challenge.is_err(),
        "Challenge with insufficient bond should revert"
    );

    // Challenge with correct bond
    let bond = U256::from(100_000_000_000_000_000u128);
    let challenge_receipt = challenger_contract
        .challenge(result_id)
        .value(bond)
        .send()
        .await
        .expect("challenge send failed")
        .get_receipt()
        .await
        .expect("challenge receipt failed");
    assert!(challenge_receipt.status(), "challenge tx should succeed");

    // Verify result is now challenged
    let r = admin_contract.getResult(result_id).call().await.unwrap();
    assert!(r.challenged, "Result should be challenged");
    assert_eq!(
        r.challenger,
        parse_signer(CHALLENGER_KEY).address(),
        "Challenger address should match"
    );
    assert!(r.challengeBond > U256::ZERO, "Challenge bond should be non-zero");
    assert!(r.disputeDeadline > U256::ZERO, "Dispute deadline should be set");

    // Cannot finalize a challenged result (even after time warp)
    ctx.advance_time(7200).await; // 2 hours
    let finalize_result = admin_contract.finalize(result_id).send().await;
    assert!(
        finalize_result.is_err(),
        "Cannot finalize a challenged result"
    );

    // Cannot double-challenge
    let double_challenge = challenger_contract
        .challenge(result_id)
        .value(bond)
        .send()
        .await;
    assert!(
        double_challenge.is_err(),
        "Cannot challenge the same result twice"
    );

    // Challenge after window closed should fail (new submission)
    let model_hash2 = B256::from([0x31u8; 32]);
    let attestation2 = build_attestation(USER_KEY, model_hash2, input_hash, result_data).await;
    let receipt2 = admin_contract
        .submitResult(
            model_hash2,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation2),
        )
        .value(stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    let result_id_2 = extract_result_id(&receipt2);

    // Time is already advanced 2 hours past challenge window
    let late_challenge = challenger_contract
        .challenge(result_id_2)
        .value(bond)
        .send()
        .await;
    assert!(
        late_challenge.is_err(),
        "Challenge after window closes should revert"
    );

    println!("test_challenge_flow PASSED");
}

/// 6. Dispute resolution by timeout: submit -> challenge -> timeout -> challenger wins.
#[tokio::test]
async fn test_resolve_dispute_by_timeout() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave
    let enclave_addr = parse_signer(USER_KEY).address();
    admin_contract
        .registerEnclave(enclave_addr, B256::from([0x44u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit result
    let model_hash = B256::from([0x50u8; 32]);
    let input_hash = B256::from([0x60u8; 32]);
    let result_data = b"dispute-timeout-test";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let receipt = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let result_id = extract_result_id(&receipt);

    // Challenge
    let challenger_contract = ctx.contract_with_key(CHALLENGER_KEY);
    let bond = U256::from(100_000_000_000_000_000u128);
    challenger_contract
        .challenge(result_id)
        .value(bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Cannot resolve by timeout before deadline
    let early_resolve = challenger_contract
        .resolveDisputeByTimeout(result_id)
        .send()
        .await;
    assert!(
        early_resolve.is_err(),
        "resolveDisputeByTimeout should fail before deadline"
    );

    // Get challenger balance before resolution
    let provider = ctx.provider_readonly();
    let challenger_addr = parse_signer(CHALLENGER_KEY).address();
    let challenger_balance_before = provider.get_balance(challenger_addr).await.unwrap();

    // Warp past dispute window (24 hours + 1 second)
    ctx.advance_time(86401).await;

    // Now resolve by timeout -- challenger wins
    let resolve_receipt = challenger_contract
        .resolveDisputeByTimeout(result_id)
        .send()
        .await
        .expect("resolveDisputeByTimeout send failed")
        .get_receipt()
        .await
        .expect("resolveDisputeByTimeout receipt failed");
    assert!(resolve_receipt.status(), "resolveDisputeByTimeout tx should succeed");

    // Dispute should be resolved, prover did NOT win
    let resolved = admin_contract.disputeResolved(result_id).call().await.unwrap();
    assert!(resolved, "Dispute should be resolved");

    let prover_won = admin_contract.disputeProverWon(result_id).call().await.unwrap();
    assert!(!prover_won, "Prover should NOT have won (timeout)");

    // Result should not be valid
    let valid = admin_contract.isResultValid(result_id).call().await.unwrap();
    assert!(!valid, "Result should not be valid after challenger wins");

    // Challenger should have received both bonds (stake + bond)
    let challenger_balance_after = provider.get_balance(challenger_addr).await.unwrap();
    let _total_pot = stake + bond; // 0.2 ETH
    let gas_upper_bound = U256::from(1_000_000_000_000_000u128);
    // challenger_balance_after >= challenger_balance_before + total_pot - bond - gas
    // (they paid bond, then got total_pot back, minus gas)
    // Net gain = total_pot - bond = stake = 0.1 ETH, minus gas
    assert!(
        challenger_balance_after > challenger_balance_before + stake - gas_upper_bound - gas_upper_bound,
        "Challenger should have profited by approximately the prover stake"
    );

    // Cannot resolve again
    let double_resolve = challenger_contract
        .resolveDisputeByTimeout(result_id)
        .send()
        .await;
    assert!(
        double_resolve.is_err(),
        "Cannot resolve dispute twice"
    );

    println!("test_resolve_dispute_by_timeout PASSED");
}

/// 7. Pause/unpause flow: owner pauses, operations revert, owner unpauses, operations work again.
#[tokio::test]
async fn test_pause_unpause_flow() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave first (before pausing)
    let enclave_addr = parse_signer(USER_KEY).address();
    admin_contract
        .registerEnclave(enclave_addr, B256::from([0x55u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Initial state: not paused
    let paused = admin_contract.paused().call().await.unwrap();
    assert!(!paused, "Should not be paused initially");

    // Non-owner cannot pause
    let user_contract = ctx.contract_with_key(USER_KEY);
    let unauthorized_pause = user_contract.pause().send().await;
    assert!(
        unauthorized_pause.is_err(),
        "Non-owner should not be able to pause"
    );

    // Owner pauses
    admin_contract
        .pause()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let paused = admin_contract.paused().call().await.unwrap();
    assert!(paused, "Contract should be paused");

    // submitResult should revert when paused
    let model_hash = B256::from([0x70u8; 32]);
    let input_hash = B256::from([0x80u8; 32]);
    let result_data = b"pause-test-result";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let submit_while_paused = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await;
    assert!(
        submit_while_paused.is_err(),
        "submitResult should revert when paused"
    );

    // Non-owner cannot unpause
    let unauthorized_unpause = user_contract.unpause().send().await;
    assert!(
        unauthorized_unpause.is_err(),
        "Non-owner should not be able to unpause"
    );

    // Owner unpauses
    admin_contract
        .unpause()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let paused = admin_contract.paused().call().await.unwrap();
    assert!(!paused, "Contract should be unpaused");

    // submitResult should work again
    let receipt = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await
        .expect("submitResult should work after unpause")
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status(), "submitResult should succeed after unpause");

    println!("test_pause_unpause_flow PASSED");
}

/// 8. Ownership transfer via Ownable2Step: transfer -> accept -> verify new owner.
#[tokio::test]
async fn test_ownership_transfer() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    let new_owner_signer = parse_signer(NEW_OWNER_KEY);
    let new_owner_addr = new_owner_signer.address();

    // Verify current owner
    let owner = admin_contract.owner().call().await.unwrap();
    assert_eq!(owner, ctx.admin_signer.address());

    // Pending owner should be zero initially
    let pending = admin_contract.pendingOwner().call().await.unwrap();
    assert_eq!(pending, Address::ZERO, "Pending owner should be zero");

    // Non-owner cannot initiate transfer
    let user_contract = ctx.contract_with_key(USER_KEY);
    let unauthorized_transfer = user_contract
        .transferOwnership(new_owner_addr)
        .send()
        .await;
    assert!(
        unauthorized_transfer.is_err(),
        "Non-owner should not be able to transfer ownership"
    );

    // Owner initiates transfer
    admin_contract
        .transferOwnership(new_owner_addr)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Pending owner should be set
    let pending = admin_contract.pendingOwner().call().await.unwrap();
    assert_eq!(
        pending, new_owner_addr,
        "Pending owner should be the new owner"
    );

    // Owner is still the admin (not yet accepted)
    let owner = admin_contract.owner().call().await.unwrap();
    assert_eq!(
        owner,
        ctx.admin_signer.address(),
        "Owner should still be admin until accepted"
    );

    // Someone other than pending owner cannot accept
    let wrong_accepter = ctx.contract_with_key(USER_KEY);
    let wrong_accept = wrong_accepter.acceptOwnership().send().await;
    assert!(
        wrong_accept.is_err(),
        "Only pending owner can accept ownership"
    );

    // Pending owner accepts
    let new_owner_contract = ctx.contract_with_key(NEW_OWNER_KEY);
    new_owner_contract
        .acceptOwnership()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Ownership should be transferred
    let owner = admin_contract.owner().call().await.unwrap();
    assert_eq!(owner, new_owner_addr, "Owner should now be the new owner");

    // Old owner can no longer perform admin actions
    let old_owner_pause = admin_contract.pause().send().await;
    assert!(
        old_owner_pause.is_err(),
        "Old owner should not be able to pause"
    );

    // New owner can perform admin actions
    new_owner_contract
        .pause()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let paused = admin_contract.paused().call().await.unwrap();
    assert!(paused, "New owner should be able to pause the contract");

    println!("test_ownership_transfer PASSED");
}

/// 9. Revoke enclave: register, revoke, verify submission fails with revoked enclave.
#[tokio::test]
async fn test_revoke_enclave() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    let enclave_addr = parse_signer(USER_KEY).address();
    let image_hash = B256::from([0x66u8; 32]);

    // Register enclave
    admin_contract
        .registerEnclave(enclave_addr, image_hash)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Enclave should be active
    let enclave_info = admin_contract.enclaves(enclave_addr).call().await.unwrap();
    assert!(enclave_info.active, "Enclave should be active after registration");

    // Revoke enclave
    admin_contract
        .revokeEnclave(enclave_addr)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Enclave should be inactive
    let enclave_info = admin_contract.enclaves(enclave_addr).call().await.unwrap();
    assert!(enclave_info.registered, "Enclave should still be registered");
    assert!(!enclave_info.active, "Enclave should be inactive after revocation");

    // Submission with revoked enclave should fail
    let model_hash = B256::from([0x90u8; 32]);
    let input_hash = B256::from([0xA0u8; 32]);
    let result_data = b"revoked-enclave-test";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;

    let submit_result = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(U256::from(100_000_000_000_000_000u128))
        .send()
        .await;
    assert!(
        submit_result.is_err(),
        "Submission with revoked enclave should revert"
    );

    // Cannot revoke again
    let double_revoke = admin_contract.revokeEnclave(enclave_addr).send().await;
    assert!(
        double_revoke.is_err(),
        "Cannot revoke an already revoked enclave"
    );

    println!("test_revoke_enclave PASSED");
}

/// 10. Admin config: setProverStake and setChallengeBondAmount.
#[tokio::test]
async fn test_admin_config() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Set prover stake to 0.05 ETH
    let new_stake = U256::from(50_000_000_000_000_000u128);
    admin_contract
        .setProverStake(new_stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let stake = admin_contract.proverStake().call().await.unwrap();
    assert_eq!(stake, new_stake, "Prover stake should be updated");

    // Set challenge bond to 0.2 ETH
    let new_bond = U256::from(200_000_000_000_000_000u128);
    admin_contract
        .setChallengeBondAmount(new_bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let bond = admin_contract.challengeBondAmount().call().await.unwrap();
    assert_eq!(bond, new_bond, "Challenge bond should be updated");

    // Zero stake should revert
    let zero_stake = admin_contract
        .setProverStake(U256::ZERO)
        .send()
        .await;
    assert!(zero_stake.is_err(), "Zero prover stake should revert");

    // Zero bond should revert
    let zero_bond = admin_contract
        .setChallengeBondAmount(U256::ZERO)
        .send()
        .await;
    assert!(zero_bond.is_err(), "Zero challenge bond should revert");

    // Amount too high (>100 ETH) should revert
    let too_high = U256::from(101u64) * U256::from(10u64).pow(U256::from(18u64));
    let high_stake = admin_contract.setProverStake(too_high).send().await;
    assert!(high_stake.is_err(), "Stake >100 ETH should revert");

    // Non-owner cannot set config
    let user_contract = ctx.contract_with_key(USER_KEY);
    let unauthorized = user_contract
        .setProverStake(new_stake)
        .send()
        .await;
    assert!(
        unauthorized.is_err(),
        "Non-owner should not be able to set prover stake"
    );

    // Set remainder verifier
    let fake_verifier = Address::from([0xBBu8; 20]);
    admin_contract
        .setRemainderVerifier(fake_verifier)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let rv = admin_contract.remainderVerifier().call().await.unwrap();
    assert_eq!(rv, fake_verifier, "Remainder verifier should be updated");

    // Zero address should revert
    let zero_verifier = admin_contract
        .setRemainderVerifier(Address::ZERO)
        .send()
        .await;
    assert!(zero_verifier.is_err(), "Zero address verifier should revert");

    println!("test_admin_config PASSED");
}

/// 11. Dispute extension: submit -> challenge -> extend -> verify extended deadline.
#[tokio::test]
async fn test_dispute_extension() {
    let ctx = TestContext::new().await;
    let admin_contract = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave
    let enclave_addr = parse_signer(USER_KEY).address();
    admin_contract
        .registerEnclave(enclave_addr, B256::from([0x77u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit result
    let model_hash = B256::from([0xB0u8; 32]);
    let input_hash = B256::from([0xC0u8; 32]);
    let result_data = b"extend-test-result";
    let attestation = build_attestation(USER_KEY, model_hash, input_hash, result_data).await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let receipt = admin_contract
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let result_id = extract_result_id(&receipt);

    // Challenge
    let challenger_contract = ctx.contract_with_key(CHALLENGER_KEY);
    let bond = U256::from(100_000_000_000_000_000u128);
    challenger_contract
        .challenge(result_id)
        .value(bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Get initial dispute deadline
    let r_before = admin_contract.getResult(result_id).call().await.unwrap();
    let deadline_before = r_before.disputeDeadline;

    // Extend dispute window (admin is submitter)
    admin_contract
        .extendDisputeWindow(result_id)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Deadline should be extended by EXTENSION_PERIOD (30 minutes = 1800 seconds)
    let r_after = admin_contract.getResult(result_id).call().await.unwrap();
    let deadline_after = r_after.disputeDeadline;
    assert_eq!(
        deadline_after,
        deadline_before + U256::from(1800u64),
        "Dispute deadline should be extended by 30 minutes"
    );

    // Cannot extend again (MAX_EXTENSIONS = 1)
    let double_extend = admin_contract
        .extendDisputeWindow(result_id)
        .send()
        .await;
    assert!(
        double_extend.is_err(),
        "Cannot extend dispute window more than MAX_EXTENSIONS times"
    );

    // Non-submitter cannot extend
    let challenger_extend = challenger_contract
        .extendDisputeWindow(result_id)
        .send()
        .await;
    assert!(
        challenger_extend.is_err(),
        "Non-submitter should not be able to extend"
    );

    println!("test_dispute_extension PASSED");
}
