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
//!   cd sdk && cargo test --test integration_tee -- --nocapture
//!
//! IMPORTANT: Expected-revert checks use `.call().await` (dry-run simulation) rather
//! than `.send().await`. Using `.send()` for reverts corrupts alloy's internal nonce
//! tracker, causing subsequent transactions to hang indefinitely.

use std::path::PathBuf;

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::node_bindings::Anvil;
use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol;
use alloy::sol_types::SolValue;

// --------------------------------------------------------------------------
// Contract binding with full ABI (bytecode loaded from forge artifact)
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
        event ResultSubmitted(bytes32 indexed resultId, bytes32 indexed modelHash, bytes32 inputHash, address indexed submitter);
        event ResultChallenged(bytes32 indexed resultId, address challenger);
        event DisputeResolved(bytes32 indexed resultId, bool proverWon);
        event ResultFinalized(bytes32 indexed resultId);
        event ResultExpired(bytes32 indexed resultId);

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

        // UUPS admin (replaces Ownable2Step)
        function admin() external view returns (address);
        function changeAdmin(address newAdmin) external;

        // Initialize (UUPS proxy pattern, no constructor)
        function initialize(address _admin, address _remainderVerifier) external;

        // Pausable
        function pause() external;
        function unpause() external;
        function paused() external view returns (bool);

        // State getters
        function remainderVerifier() external view returns (address);
        function challengeBondAmount() external view returns (uint256);
        function proverStake() external view returns (uint256);
        function disputeResolved(bytes32 resultId) external view returns (bool);
        function disputeProverWon(bytes32 resultId) external view returns (bool);
        function enclaves(address key) external view returns (bool registered, bool active, bytes32 enclaveImageHash, uint256 registeredAt);

        // Storage getters (mutable state, not constants)
        function challengeWindow() external view returns (uint256);
        function disputeWindow() external view returns (uint256);
    }
}

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/// Load compiled bytecode from a forge artifact JSON file.
fn load_bytecode_from_artifact(path: &std::path::Path) -> Bytes {
    assert!(
        path.exists(),
        "Artifact not found at {path:?}. \
         Run: cd contracts && forge build --skip test --skip script"
    );
    let artifact: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    let bytecode_hex = artifact["bytecode"]["object"]
        .as_str()
        .expect("bytecode.object not found in artifact");
    bytecode_hex.parse::<Bytes>().expect("invalid bytecode hex")
}

/// Load the compiled TEEMLVerifier implementation bytecode.
fn load_contract_bytecode() -> Bytes {
    let artifact_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/out/TEEMLVerifier.sol/TEEMLVerifier.json");
    load_bytecode_from_artifact(&artifact_path)
}

/// Load the compiled UUPSProxy bytecode.
fn load_proxy_bytecode() -> Bytes {
    let artifact_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../contracts/out/Upgradeable.sol/UUPSProxy.json");
    load_bytecode_from_artifact(&artifact_path)
}

/// Anvil default chain ID.
const ANVIL_CHAIN_ID: u64 = 31337;

/// Anvil pre-funded private keys (from Anvil default accounts).
const ADMIN_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const USER_KEY: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const CHALLENGER_KEY: &str = "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const NEW_OWNER_KEY: &str = "7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

fn parse_signer(key: &str) -> PrivateKeySigner {
    key.parse::<PrivateKeySigner>().unwrap()
}

/// Shared test context: Anvil instance + deployed contract.
struct TestContext {
    _anvil: alloy::node_bindings::AnvilInstance,
    rpc_url: String,
    contract_addr: Address,
    admin_addr: Address,
}

impl TestContext {
    /// Spawn a fresh Anvil and deploy TEEMLVerifier behind a UUPS proxy.
    async fn new() -> Self {
        let anvil = Anvil::new().spawn();
        let rpc_url = anvil.endpoint();

        let admin_signer = parse_signer(ADMIN_KEY);
        let admin_addr = admin_signer.address();
        let wallet = EthereumWallet::from(admin_signer);

        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(rpc_url.parse().unwrap());

        // Step 1: Deploy the TEEMLVerifier implementation (no constructor args)
        let impl_code = load_contract_bytecode();
        let impl_tx =
            alloy::rpc::types::TransactionRequest::default().with_deploy_code(impl_code.to_vec());
        let impl_receipt = provider
            .send_transaction(impl_tx)
            .await
            .expect("Failed to send impl deploy tx")
            .get_receipt()
            .await
            .expect("Failed to get impl deploy receipt");
        let impl_addr = impl_receipt
            .contract_address
            .expect("No contract address in impl deploy receipt");

        // Step 2: Encode the initialize(admin, remainderVerifier) calldata
        let tee_impl = TEEMLVerifier::new(impl_addr, &provider);
        let init_call = tee_impl.initialize(admin_addr, Address::ZERO);
        let init_data = init_call.calldata().clone();

        // Step 3: Deploy UUPSProxy(implementation, initData)
        let proxy_code = load_proxy_bytecode();
        let proxy_constructor_args = (impl_addr, init_data.to_vec()).abi_encode_params();
        let mut proxy_deploy_data = proxy_code.to_vec();
        proxy_deploy_data.extend_from_slice(&proxy_constructor_args);

        let proxy_tx =
            alloy::rpc::types::TransactionRequest::default().with_deploy_code(proxy_deploy_data);
        let proxy_receipt = provider
            .send_transaction(proxy_tx)
            .await
            .expect("Failed to send proxy deploy tx")
            .get_receipt()
            .await
            .expect("Failed to get proxy deploy receipt");
        let contract_addr = proxy_receipt
            .contract_address
            .expect("No contract address in proxy deploy receipt");

        Self {
            _anvil: anvil,
            rpc_url,
            contract_addr,
            admin_addr,
        }
    }

    /// Build a provider with the given private key's wallet.
    fn provider_with_key(&self, key: &str) -> impl alloy::providers::Provider + Clone {
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
    ) -> TEEMLVerifier::TEEMLVerifierInstance<impl alloy::providers::Provider + Clone> {
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
        let _: serde_json::Value = provider
            .raw_request("evm_increaseTime".into(), [U256::from(seconds)])
            .await
            .unwrap();
        let _: serde_json::Value = provider.raw_request("evm_mine".into(), ()).await.unwrap();
    }
}

/// Compute the EIP-712 domain separator matching TEEMLVerifier's `_computeDomainSeparator()`.
fn compute_domain_separator(chain_id: u64, verifying_contract: Address) -> B256 {
    let eip712_domain_typehash = keccak256(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = keccak256("TEEMLVerifier");
    let version_hash = keccak256("1");

    // abi.encode(typehash, nameHash, versionHash, chainId, verifyingContract)
    let encoded = (
        eip712_domain_typehash,
        name_hash,
        version_hash,
        U256::from(chain_id),
        verifying_contract,
    )
        .abi_encode_params();
    keccak256(&encoded)
}

/// Build an ECDSA attestation matching what TEEMLVerifier.submitResult expects.
/// Uses EIP-712 structured data signing with the contract's domain separator.
async fn build_attestation(
    enclave_key: &str,
    model_hash: B256,
    input_hash: B256,
    result: &[u8],
    chain_id: u64,
    verifying_contract: Address,
) -> Vec<u8> {
    let signer = parse_signer(enclave_key);
    let result_hash = keccak256(result);

    // RESULT_TYPEHASH = keccak256("TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)")
    let result_typehash =
        keccak256("TEEMLResult(bytes32 modelHash,bytes32 inputHash,bytes32 resultHash)");

    // structHash = keccak256(abi.encode(RESULT_TYPEHASH, modelHash, inputHash, resultHash))
    let struct_hash =
        keccak256((result_typehash, model_hash, input_hash, result_hash).abi_encode_params());

    // domainSeparator
    let domain_separator = compute_domain_separator(chain_id, verifying_contract);

    // digest = keccak256("\x19\x01" || domainSeparator || structHash)
    let digest = keccak256(
        [
            b"\x19\x01".as_slice(),
            domain_separator.as_slice(),
            struct_hash.as_slice(),
        ]
        .concat(),
    );

    // Sign the raw EIP-712 digest (no personal message prefix)
    let sig = signer.sign_hash(&digest).await.unwrap();
    sig.as_bytes().to_vec()
}

/// Extract the resultId from the first log topic of a submitResult receipt.
fn extract_result_id(receipt: &alloy::rpc::types::TransactionReceipt) -> B256 {
    receipt.inner.logs()[0].topics()[1]
}

/// Helper: register an enclave (USER_KEY) and submit a result, returning the resultId.
async fn setup_submitted_result(ctx: &TestContext) -> B256 {
    let admin = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave
    let enclave_addr = parse_signer(USER_KEY).address();
    admin
        .registerEnclave(enclave_addr, B256::from([0xAAu8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit result
    let model_hash = B256::from([0x01u8; 32]);
    let input_hash = B256::from([0x02u8; 32]);
    let result_data = b"test-result-data";
    let attestation = build_attestation(
        USER_KEY,
        model_hash,
        input_hash,
        result_data,
        ANVIL_CHAIN_ID,
        ctx.contract_addr,
    )
    .await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let receipt = admin
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

    extract_result_id(&receipt)
}

// ==========================================================================
// Tests
// ==========================================================================

/// 1. Deploy and verify initial state.
#[tokio::test]
async fn test_deploy_and_initial_state() {
    let ctx = TestContext::new().await;
    let contract = ctx.contract_with_key(ADMIN_KEY);

    // Admin should be the deployer address
    let admin_addr = contract.admin().call().await.unwrap();
    assert_eq!(admin_addr, ctx.admin_addr, "Admin should be the deployer");

    // Contract should not be paused
    let paused = contract.paused().call().await.unwrap();
    assert!(!paused, "Contract should not be paused initially");

    // Remainder verifier should be zero (deployed with Address::ZERO)
    let rv = contract.remainderVerifier().call().await.unwrap();
    assert_eq!(rv, Address::ZERO);

    // Default stake and bond = 0.1 ether
    let stake = contract.proverStake().call().await.unwrap();
    assert_eq!(stake, U256::from(100_000_000_000_000_000u128));

    let bond = contract.challengeBondAmount().call().await.unwrap();
    assert_eq!(bond, U256::from(100_000_000_000_000_000u128));

    // Challenge window = 1 hour, Dispute window = 24 hours
    let cw = contract.challengeWindow().call().await.unwrap();
    assert_eq!(cw, U256::from(3600u64));

    let dw = contract.disputeWindow().call().await.unwrap();
    assert_eq!(dw, U256::from(86400u64));

    println!("test_deploy_and_initial_state PASSED");
}

/// 2. Register an enclave and verify it is registered.
#[tokio::test]
async fn test_register_enclave() {
    let ctx = TestContext::new().await;
    let admin = ctx.contract_with_key(ADMIN_KEY);

    let enclave_addr = parse_signer(USER_KEY).address();
    let image_hash = B256::from([0xABu8; 32]);

    // Register enclave
    let receipt = admin
        .registerEnclave(enclave_addr, image_hash)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status());

    // Verify event topic contains enclave address
    let topic = receipt.inner.logs()[0].topics()[1];
    let expected = B256::left_padding_from(enclave_addr.as_slice());
    assert_eq!(topic, expected);

    // Verify enclave state
    let info = admin.enclaves(enclave_addr).call().await.unwrap();
    assert!(info.registered);
    assert!(info.active);
    assert_eq!(info.enclaveImageHash, image_hash);
    assert!(info.registeredAt > U256::ZERO);

    // Non-owner cannot register (use .call() to avoid nonce corruption)
    let user = ctx.contract_with_key(USER_KEY);
    let result = user
        .registerEnclave(Address::from([0x99u8; 20]), B256::ZERO)
        .call()
        .await;
    assert!(
        result.is_err(),
        "Non-owner should not be able to register enclave"
    );

    println!("test_register_enclave PASSED");
}

/// 3. Submit a result with stake and verify the result struct.
#[tokio::test]
async fn test_submit_result_with_stake() {
    let ctx = TestContext::new().await;
    let admin = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave
    let enclave_addr = parse_signer(USER_KEY).address();
    admin
        .registerEnclave(enclave_addr, B256::from([0x11u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Submit result
    let model_hash = B256::from([0x01u8; 32]);
    let input_hash = B256::from([0x02u8; 32]);
    let result_data = b"prediction:class_0";
    let attestation = build_attestation(
        USER_KEY,
        model_hash,
        input_hash,
        result_data,
        ANVIL_CHAIN_ID,
        ctx.contract_addr,
    )
    .await;
    let stake = U256::from(100_000_000_000_000_000u128);

    let receipt = admin
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
    assert!(receipt.status());

    let result_id = extract_result_id(&receipt);
    assert!(!result_id.is_zero());

    // Verify result struct
    let r = admin.getResult(result_id).call().await.unwrap();
    assert_eq!(r.enclave, enclave_addr);
    assert_eq!(r.submitter, ctx.admin_addr);
    assert_eq!(r.modelHash, model_hash);
    assert_eq!(r.inputHash, input_hash);
    assert_eq!(r.resultHash, keccak256(result_data));
    assert_eq!(r.proverStakeAmount, stake);
    assert!(!r.finalized);
    assert!(!r.challenged);

    // Not valid yet (not finalized)
    let valid = admin.isResultValid(result_id).call().await.unwrap();
    assert!(!valid);

    // Insufficient stake should revert (use .call() to check)
    let attestation2 = build_attestation(
        USER_KEY,
        B256::from([0x03u8; 32]),
        input_hash,
        result_data,
        ANVIL_CHAIN_ID,
        ctx.contract_addr,
    )
    .await;
    let result = admin
        .submitResult(
            B256::from([0x03u8; 32]),
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation2),
        )
        .value(U256::from(1u64))
        .call()
        .await;
    assert!(result.is_err(), "Should revert with insufficient stake");

    println!("test_submit_result_with_stake PASSED");
}

/// 4. Submit, warp time past challenge window, finalize, verify stake returned.
#[tokio::test]
async fn test_finalize_after_time_warp() {
    let ctx = TestContext::new().await;
    let result_id = setup_submitted_result(&ctx).await;
    let admin = ctx.contract_with_key(ADMIN_KEY);

    // Cannot finalize before challenge window
    let early = admin.finalize(result_id).call().await;
    assert!(
        early.is_err(),
        "Should revert before challenge window passes"
    );

    // Get balance before finalize
    let provider = ctx.provider_readonly();
    let balance_before = provider.get_balance(ctx.admin_addr).await.unwrap();

    // Warp past challenge window (1 hour + 1 second)
    ctx.advance_time(3601).await;

    // Finalize
    let receipt = admin
        .finalize(result_id)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status());

    // Result should now be valid
    let valid = admin.isResultValid(result_id).call().await.unwrap();
    assert!(valid, "Result should be valid after finalization");

    // Stake returned (balance should not have dropped much)
    let balance_after = provider.get_balance(ctx.admin_addr).await.unwrap();
    let gas_budget = U256::from(1_000_000_000_000_000u128); // 0.001 ETH
    assert!(
        balance_after > balance_before - gas_budget,
        "Stake should have been returned"
    );

    // Cannot finalize again
    let double = admin.finalize(result_id).call().await;
    assert!(double.is_err(), "Should not finalize twice");

    println!("test_finalize_after_time_warp PASSED");
}

/// 5. Challenge flow: submit -> challenge -> verify challenged state.
#[tokio::test]
async fn test_challenge_flow() {
    let ctx = TestContext::new().await;
    let result_id = setup_submitted_result(&ctx).await;
    let admin = ctx.contract_with_key(ADMIN_KEY);
    let challenger = ctx.contract_with_key(CHALLENGER_KEY);
    let bond = U256::from(100_000_000_000_000_000u128);

    // Insufficient bond should revert
    let bad = challenger
        .challenge(result_id)
        .value(U256::from(1u64))
        .call()
        .await;
    assert!(
        bad.is_err(),
        "Challenge with insufficient bond should revert"
    );

    // Challenge with correct bond
    let receipt = challenger
        .challenge(result_id)
        .value(bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status());

    // Verify challenged state
    let r = admin.getResult(result_id).call().await.unwrap();
    assert!(r.challenged);
    assert_eq!(r.challenger, parse_signer(CHALLENGER_KEY).address());
    assert!(r.challengeBond > U256::ZERO);
    assert!(r.disputeDeadline > U256::ZERO);

    // Cannot finalize a challenged result
    ctx.advance_time(7200).await;
    let finalize = admin.finalize(result_id).call().await;
    assert!(finalize.is_err(), "Cannot finalize a challenged result");

    // Cannot double-challenge
    let double = challenger.challenge(result_id).value(bond).call().await;
    assert!(double.is_err(), "Cannot challenge twice");

    println!("test_challenge_flow PASSED");
}

/// 6. Dispute resolution by timeout: submit -> challenge -> timeout -> challenger wins.
#[tokio::test]
async fn test_resolve_dispute_by_timeout() {
    let ctx = TestContext::new().await;
    let result_id = setup_submitted_result(&ctx).await;
    let admin = ctx.contract_with_key(ADMIN_KEY);
    let challenger = ctx.contract_with_key(CHALLENGER_KEY);

    let bond = U256::from(100_000_000_000_000_000u128);
    let stake = U256::from(100_000_000_000_000_000u128);

    // Challenge
    challenger
        .challenge(result_id)
        .value(bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Cannot resolve before deadline
    let early = challenger.resolveDisputeByTimeout(result_id).call().await;
    assert!(early.is_err(), "Should fail before dispute deadline");

    // Record challenger balance before
    let provider = ctx.provider_readonly();
    let challenger_addr = parse_signer(CHALLENGER_KEY).address();
    let bal_before = provider.get_balance(challenger_addr).await.unwrap();

    // Warp past dispute window (24h + 1s)
    ctx.advance_time(86401).await;

    // Resolve by timeout -- challenger wins
    let receipt = challenger
        .resolveDisputeByTimeout(result_id)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status());

    // Dispute resolved, prover lost
    let resolved = admin.disputeResolved(result_id).call().await.unwrap();
    assert!(resolved);
    let prover_won = admin.disputeProverWon(result_id).call().await.unwrap();
    assert!(!prover_won);

    // Result not valid
    let valid = admin.isResultValid(result_id).call().await.unwrap();
    assert!(!valid);

    // Challenger profit: received stake + bond back, minus gas
    let bal_after = provider.get_balance(challenger_addr).await.unwrap();
    let gas_budget = U256::from(1_000_000_000_000_000u128);
    assert!(
        bal_after > bal_before + stake - gas_budget - gas_budget,
        "Challenger should have profited by approximately the prover stake"
    );

    // Cannot resolve again
    let double = challenger.resolveDisputeByTimeout(result_id).call().await;
    assert!(double.is_err(), "Cannot resolve dispute twice");

    println!("test_resolve_dispute_by_timeout PASSED");
}

/// 7. Pause/unpause: owner pauses, operations revert, owner unpauses.
#[tokio::test]
async fn test_pause_unpause_flow() {
    let ctx = TestContext::new().await;
    let admin = ctx.contract_with_key(ADMIN_KEY);

    // Register enclave before pausing
    let enclave_addr = parse_signer(USER_KEY).address();
    admin
        .registerEnclave(enclave_addr, B256::from([0x55u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Not paused initially
    assert!(!admin.paused().call().await.unwrap());

    // Non-owner cannot pause
    let user = ctx.contract_with_key(USER_KEY);
    assert!(user.pause().call().await.is_err());

    // Owner pauses
    admin
        .pause()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(admin.paused().call().await.unwrap());

    // Submit reverts when paused
    let model_hash = B256::from([0x70u8; 32]);
    let input_hash = B256::from([0x80u8; 32]);
    let result_data = b"pause-test";
    let attestation = build_attestation(
        USER_KEY,
        model_hash,
        input_hash,
        result_data,
        ANVIL_CHAIN_ID,
        ctx.contract_addr,
    )
    .await;
    let submit = admin
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(U256::from(100_000_000_000_000_000u128))
        .call()
        .await;
    assert!(submit.is_err(), "submitResult should revert when paused");

    // Non-owner cannot unpause
    assert!(user.unpause().call().await.is_err());

    // Owner unpauses
    admin
        .unpause()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(!admin.paused().call().await.unwrap());

    // Submit works after unpause
    let receipt = admin
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(U256::from(100_000_000_000_000_000u128))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(receipt.status());

    println!("test_pause_unpause_flow PASSED");
}

/// 8. Admin transfer via changeAdmin (UUPS pattern).
#[tokio::test]
async fn test_admin_transfer() {
    let ctx = TestContext::new().await;
    let admin = ctx.contract_with_key(ADMIN_KEY);
    let new_admin_addr = parse_signer(NEW_OWNER_KEY).address();

    // Current admin
    assert_eq!(admin.admin().call().await.unwrap(), ctx.admin_addr);

    // Non-admin cannot change admin
    let user = ctx.contract_with_key(USER_KEY);
    assert!(
        user.changeAdmin(new_admin_addr).call().await.is_err(),
        "Non-admin should not be able to change admin"
    );

    // Admin changes admin (takes effect immediately in UUPS pattern)
    admin
        .changeAdmin(new_admin_addr)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    assert_eq!(admin.admin().call().await.unwrap(), new_admin_addr);

    // Old admin cannot perform admin actions
    assert!(admin.pause().call().await.is_err());

    // New admin can perform admin actions
    let new_admin = ctx.contract_with_key(NEW_OWNER_KEY);
    new_admin
        .pause()
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(admin.paused().call().await.unwrap());

    println!("test_admin_transfer PASSED");
}

/// 9. Revoke enclave: register, revoke, submission with revoked enclave fails.
#[tokio::test]
async fn test_revoke_enclave() {
    let ctx = TestContext::new().await;
    let admin = ctx.contract_with_key(ADMIN_KEY);

    let enclave_addr = parse_signer(USER_KEY).address();
    admin
        .registerEnclave(enclave_addr, B256::from([0x66u8; 32]))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Active after registration
    let info = admin.enclaves(enclave_addr).call().await.unwrap();
    assert!(info.active);

    // Revoke
    admin
        .revokeEnclave(enclave_addr)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    let info = admin.enclaves(enclave_addr).call().await.unwrap();
    assert!(info.registered);
    assert!(!info.active);

    // Submit with revoked enclave should fail
    let model_hash = B256::from([0x90u8; 32]);
    let input_hash = B256::from([0xA0u8; 32]);
    let result_data = b"revoked-test";
    let attestation = build_attestation(
        USER_KEY,
        model_hash,
        input_hash,
        result_data,
        ANVIL_CHAIN_ID,
        ctx.contract_addr,
    )
    .await;
    let submit = admin
        .submitResult(
            model_hash,
            input_hash,
            Bytes::copy_from_slice(result_data),
            Bytes::copy_from_slice(&attestation),
        )
        .value(U256::from(100_000_000_000_000_000u128))
        .call()
        .await;
    assert!(submit.is_err(), "Submit with revoked enclave should revert");

    // Cannot revoke again
    assert!(admin.revokeEnclave(enclave_addr).call().await.is_err());

    println!("test_revoke_enclave PASSED");
}

/// 10. Admin config: setProverStake, setChallengeBondAmount, setRemainderVerifier.
#[tokio::test]
async fn test_admin_config() {
    let ctx = TestContext::new().await;
    let admin = ctx.contract_with_key(ADMIN_KEY);

    // Set prover stake
    let new_stake = U256::from(50_000_000_000_000_000u128);
    admin
        .setProverStake(new_stake)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert_eq!(admin.proverStake().call().await.unwrap(), new_stake);

    // Set challenge bond
    let new_bond = U256::from(200_000_000_000_000_000u128);
    admin
        .setChallengeBondAmount(new_bond)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert_eq!(admin.challengeBondAmount().call().await.unwrap(), new_bond);

    // Zero stake/bond should revert (use .call())
    assert!(admin.setProverStake(U256::ZERO).call().await.is_err());
    assert!(admin
        .setChallengeBondAmount(U256::ZERO)
        .call()
        .await
        .is_err());

    // Too high (>100 ETH) should revert
    let too_high = U256::from(101u64) * U256::from(10u64).pow(U256::from(18u64));
    assert!(admin.setProverStake(too_high).call().await.is_err());

    // Non-owner cannot set config
    let user = ctx.contract_with_key(USER_KEY);
    assert!(user.setProverStake(new_stake).call().await.is_err());

    // Set remainder verifier
    let fake_verifier = Address::from([0xBBu8; 20]);
    admin
        .setRemainderVerifier(fake_verifier)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert_eq!(
        admin.remainderVerifier().call().await.unwrap(),
        fake_verifier
    );

    // Zero address should revert
    assert!(admin
        .setRemainderVerifier(Address::ZERO)
        .call()
        .await
        .is_err());

    println!("test_admin_config PASSED");
}

/// 11. Dispute extension: submit -> challenge -> extend -> verify extended deadline.
#[tokio::test]
async fn test_dispute_extension() {
    let ctx = TestContext::new().await;
    let result_id = setup_submitted_result(&ctx).await;
    let admin = ctx.contract_with_key(ADMIN_KEY);
    let challenger = ctx.contract_with_key(CHALLENGER_KEY);

    // Challenge
    challenger
        .challenge(result_id)
        .value(U256::from(100_000_000_000_000_000u128))
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Get initial dispute deadline
    let r_before = admin.getResult(result_id).call().await.unwrap();
    let deadline_before = r_before.disputeDeadline;

    // Extend (admin is submitter)
    admin
        .extendDisputeWindow(result_id)
        .send()
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();

    // Deadline extended by 30 minutes (1800 seconds)
    let r_after = admin.getResult(result_id).call().await.unwrap();
    assert_eq!(
        r_after.disputeDeadline,
        deadline_before + U256::from(1800u64),
    );

    // Cannot extend again (MAX_EXTENSIONS = 1)
    assert!(admin.extendDisputeWindow(result_id).call().await.is_err());

    // Non-submitter cannot extend
    assert!(challenger
        .extendDisputeWindow(result_id)
        .call()
        .await
        .is_err());

    println!("test_dispute_extension PASSED");
}
