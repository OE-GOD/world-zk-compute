//! End-to-end lifecycle integration tests for the TEE ML verification flow.
//!
//! These tests verify the full happy path and dispute resolution flows by
//! deploying the TEEMLVerifier contract on a local Anvil node and interacting
//! with it via alloy.
//!
//! Prerequisites:
//!   - `anvil` must be on PATH (part of foundry)
//!   - The TEEMLVerifier forge artifact must exist at
//!     `contracts/out/TEEMLVerifier.sol/TEEMLVerifier.json`
//!     (run `cd contracts && forge build --skip test --skip script`)
//!
//! All tests are marked `#[ignore]` because they require foundry tooling and
//! compiled contracts that may not be available in every CI environment.
//!
//! Run them manually with:
//! ```sh
//! cd contracts && forge build --skip test --skip script
//! cargo test -p chaos-tests --test e2e_lifecycle -- --ignored --nocapture
//! ```

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::Arc;

    use alloy::{
        network::{EthereumWallet, TransactionBuilder},
        node_bindings::Anvil,
        primitives::{keccak256, Address, Bytes, FixedBytes, U256},
        providers::{Provider, ProviderBuilder},
        rpc::types::TransactionRequest,
        signers::local::PrivateKeySigner,
        signers::SignerSync,
        sol,
        sol_types::SolValue,
    };

    // -----------------------------------------------------------------------
    // Anvil deterministic private keys (default mnemonic)
    // -----------------------------------------------------------------------

    /// Account 0 -- used as contract admin / result submitter.
    const ADMIN_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    /// Account 1 -- used as the challenger.
    const CHALLENGER_KEY: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    /// Account 2 -- used as the TEE enclave signing key.
    const ENCLAVE_KEY: &str = "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";

    /// Challenge window in seconds (matches CHALLENGE_WINDOW in TEEMLVerifier.sol).
    const CHALLENGE_WINDOW_SECS: u64 = 3600;

    /// Dispute window in seconds (matches DISPUTE_WINDOW in TEEMLVerifier.sol).
    const DISPUTE_WINDOW_SECS: u64 = 24 * 3600;

    /// Default prover stake: 0.1 ETH.
    const PROVER_STAKE_WEI: u128 = 100_000_000_000_000_000;

    /// Default challenge bond: 0.1 ETH.
    const CHALLENGE_BOND_WEI: u128 = 100_000_000_000_000_000;

    // -----------------------------------------------------------------------
    // Solidity bindings (no bytecode -- deployed from forge artifact)
    // -----------------------------------------------------------------------

    sol! {
        #[sol(rpc)]
        contract TEEMLVerifier {
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

            event ResultSubmitted(
                bytes32 indexed resultId,
                bytes32 indexed modelHash,
                bytes32 inputHash,
                address indexed submitter
            );
            event ResultChallenged(bytes32 indexed resultId, address challenger);
            event DisputeResolved(bytes32 indexed resultId, bool proverWon);
            event ResultFinalized(bytes32 indexed resultId);
            event ResultExpired(bytes32 indexed resultId);
            event EnclaveRegistered(address indexed enclaveKey, bytes32 enclaveImageHash);

            function registerEnclave(address enclaveKey, bytes32 enclaveImageHash) external;
            function submitResult(
                bytes32 modelHash,
                bytes32 inputHash,
                bytes calldata result,
                bytes calldata attestation
            ) external payable returns (bytes32 resultId);
            function challenge(bytes32 resultId) external payable;
            function resolveDispute(
                bytes32 resultId,
                bytes calldata proof,
                bytes32 circuitHash,
                bytes calldata publicInputs,
                bytes calldata gensData
            ) external;
            function resolveDisputeByTimeout(bytes32 resultId) external;
            function finalize(bytes32 resultId) external;
            function getResult(bytes32 resultId) external view returns (MLResult memory);
            function isResultValid(bytes32 resultId) external view returns (bool);
            function disputeResolved(bytes32 resultId) external view returns (bool);
            function disputeProverWon(bytes32 resultId) external view returns (bool);

            constructor(address _admin, address _remainderVerifier);
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Load the compiled TEEMLVerifier creation bytecode from the forge artifact.
    fn load_tee_bytecode() -> Bytes {
        let artifact_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/out/TEEMLVerifier.sol/TEEMLVerifier.json");
        assert!(
            artifact_path.exists(),
            "TEEMLVerifier artifact not found at {artifact_path:?}. \
             Run: cd contracts && forge build --skip test --skip script"
        );
        let artifact: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&artifact_path).unwrap()).unwrap();
        let bytecode_hex = artifact["bytecode"]["object"]
            .as_str()
            .expect("bytecode.object not found in artifact");
        bytecode_hex.parse::<Bytes>().expect("invalid bytecode hex")
    }

    fn parse_signer(key: &str) -> PrivateKeySigner {
        key.parse::<PrivateKeySigner>().unwrap()
    }

    /// Build an ECDSA attestation matching TEEMLVerifier.submitResult's expectations.
    fn sign_attestation(
        enclave_signer: &PrivateKeySigner,
        model_hash: FixedBytes<32>,
        input_hash: FixedBytes<32>,
        result_data: &[u8],
    ) -> Bytes {
        let result_hash = keccak256(result_data);
        let mut packed = Vec::with_capacity(96);
        packed.extend_from_slice(model_hash.as_slice());
        packed.extend_from_slice(input_hash.as_slice());
        packed.extend_from_slice(result_hash.as_slice());
        let message = keccak256(&packed);

        // sign_message_sync applies EIP-191 prefix internally
        let sig = enclave_signer
            .sign_message_sync(message.as_slice())
            .expect("attestation signing failed");

        Bytes::from(sig.as_bytes().to_vec())
    }

    /// Extract the resultId from the first log topic of a submitResult receipt.
    fn extract_result_id(receipt: &alloy::rpc::types::TransactionReceipt) -> FixedBytes<32> {
        receipt
            .inner
            .logs()
            .iter()
            .find_map(|log| {
                if log.topics().len() >= 2 {
                    Some(log.topics()[1])
                } else {
                    None
                }
            })
            .expect("ResultSubmitted event not found in receipt logs")
    }

    // -----------------------------------------------------------------------
    // Shared test context: spawns Anvil + deploys TEEMLVerifier
    // -----------------------------------------------------------------------

    struct TestCtx {
        /// Keep Anvil alive for the duration of the test.
        _anvil: alloy::node_bindings::AnvilInstance,
        rpc_url: String,
        contract_addr: Address,
        #[allow(dead_code)]
        admin_addr: Address,
    }

    impl TestCtx {
        /// Spawn a fresh Anvil and deploy TEEMLVerifier.
        async fn new() -> Self {
            let anvil = Anvil::new().spawn();
            let rpc_url = anvil.endpoint();

            let admin_signer = parse_signer(ADMIN_KEY);
            let admin_addr = admin_signer.address();
            let wallet = EthereumWallet::from(admin_signer);

            let provider = ProviderBuilder::new()
                .wallet(wallet)
                .connect_http(rpc_url.parse().unwrap());

            // Build deployment tx: creation code + constructor(admin, remainderVerifier=0)
            let creation_code = load_tee_bytecode();
            let constructor_args = (admin_addr, Address::ZERO).abi_encode_params();
            let mut deploy_data = creation_code.to_vec();
            deploy_data.extend_from_slice(&constructor_args);

            let tx = TransactionRequest::default().with_deploy_code(deploy_data);
            let receipt = provider
                .send_transaction(tx)
                .await
                .expect("deploy tx send failed")
                .get_receipt()
                .await
                .expect("deploy receipt failed");

            let contract_addr = receipt
                .contract_address
                .expect("no contract address in deploy receipt");

            Self {
                _anvil: anvil,
                rpc_url,
                contract_addr,
                admin_addr,
            }
        }

        fn provider_with(&self, key: &str) -> impl Provider + Clone {
            let signer = parse_signer(key);
            let wallet = EthereumWallet::from(signer);
            ProviderBuilder::new()
                .wallet(wallet)
                .connect_http(self.rpc_url.parse().unwrap())
        }

        fn contract_with(
            &self,
            key: &str,
        ) -> TEEMLVerifier::TEEMLVerifierInstance<impl Provider + Clone> {
            TEEMLVerifier::new(self.contract_addr, self.provider_with(key))
        }

        fn readonly_provider(&self) -> impl Provider + Clone {
            ProviderBuilder::new().connect_http(self.rpc_url.parse().unwrap())
        }

        /// Advance Anvil time by `seconds` and mine a block.
        async fn advance_time(&self, seconds: u64) {
            let provider = self.readonly_provider();
            let _: serde_json::Value = provider
                .raw_request("evm_increaseTime".into(), [U256::from(seconds)])
                .await
                .unwrap();
            let _: serde_json::Value = provider.raw_request("evm_mine".into(), ()).await.unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // Test data
    // -----------------------------------------------------------------------

    fn model_hash() -> FixedBytes<32> {
        keccak256(b"xgboost-model-weights")
    }

    fn input_hash() -> FixedBytes<32> {
        keccak256(b"test-input-data")
    }

    fn result_data() -> Vec<u8> {
        b"[0.85]".to_vec()
    }

    fn image_hash() -> FixedBytes<32> {
        keccak256(b"test-enclave-image-v1")
    }

    fn enclave_signer() -> PrivateKeySigner {
        parse_signer(ENCLAVE_KEY)
    }

    fn enclave_address() -> Address {
        enclave_signer().address()
    }

    // -----------------------------------------------------------------------
    // Test 1: Full lifecycle -- inference -> submission -> finalization
    // -----------------------------------------------------------------------

    /// Exercises the complete happy path:
    ///   1. Deploy TEEMLVerifier on Anvil
    ///   2. Register a test enclave
    ///   3. Sign an attestation (simulating enclave inference)
    ///   4. Submit the result on-chain
    ///   5. Advance time past the challenge window
    ///   6. Finalize the result
    ///   7. Verify isResultValid == true
    #[tokio::test]
    #[ignore = "requires foundry: forge build + anvil on PATH"]
    async fn test_full_lifecycle_inference_to_finalization() {
        let ctx = TestCtx::new().await;
        let admin = ctx.contract_with(ADMIN_KEY);

        // -- Register enclave --
        admin
            .registerEnclave(enclave_address(), image_hash())
            .send()
            .await
            .expect("registerEnclave send failed")
            .watch()
            .await
            .expect("registerEnclave watch failed");

        // -- Simulate inference: sign attestation --
        let attestation = sign_attestation(
            &enclave_signer(),
            model_hash(),
            input_hash(),
            &result_data(),
        );

        // -- Submit result --
        let submit_receipt = admin
            .submitResult(
                model_hash(),
                input_hash(),
                Bytes::from(result_data()),
                attestation,
            )
            .value(U256::from(PROVER_STAKE_WEI))
            .send()
            .await
            .expect("submitResult send failed")
            .get_receipt()
            .await
            .expect("submitResult receipt failed");

        assert!(submit_receipt.status(), "submitResult should succeed");

        let result_id = extract_result_id(&submit_receipt);

        // -- Verify initial state --
        let r = admin.getResult(result_id).call().await.unwrap();
        assert_eq!(r.enclave, enclave_address(), "enclave mismatch");
        assert!(!r.finalized, "should not be finalized yet");
        assert!(!r.challenged, "should not be challenged");
        assert_eq!(r.proverStakeAmount, U256::from(PROVER_STAKE_WEI));

        let valid = admin.isResultValid(result_id).call().await.unwrap();
        assert!(!valid, "should not be valid before finalization");

        // -- Advance time past challenge window --
        ctx.advance_time(CHALLENGE_WINDOW_SECS + 1).await;

        // -- Finalize --
        let finalize_receipt = admin
            .finalize(result_id)
            .send()
            .await
            .expect("finalize send failed")
            .get_receipt()
            .await
            .expect("finalize receipt failed");

        assert!(finalize_receipt.status(), "finalize should succeed");

        // -- Verify finalized state --
        let final_state = admin.getResult(result_id).call().await.unwrap();
        assert!(final_state.finalized, "should be finalized");
        assert!(!final_state.challenged, "should remain unchallenged");

        let valid = admin.isResultValid(result_id).call().await.unwrap();
        assert!(valid, "should be valid after finalization");
    }

    // -----------------------------------------------------------------------
    // Test 2: Challenge -> dispute resolution by timeout (challenger wins)
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "requires foundry: forge build + anvil on PATH"]
    async fn test_dispute_resolution_by_timeout_challenger_wins() {
        let ctx = TestCtx::new().await;
        let admin = ctx.contract_with(ADMIN_KEY);

        admin
            .registerEnclave(enclave_address(), image_hash())
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let attestation = sign_attestation(
            &enclave_signer(),
            model_hash(),
            input_hash(),
            &result_data(),
        );

        let submit_receipt = admin
            .submitResult(
                model_hash(),
                input_hash(),
                Bytes::from(result_data()),
                attestation,
            )
            .value(U256::from(PROVER_STAKE_WEI))
            .send()
            .await
            .unwrap()
            .get_receipt()
            .await
            .unwrap();

        let result_id = extract_result_id(&submit_receipt);

        // -- Challenge from a different account --
        let challenger = ctx.contract_with(CHALLENGER_KEY);
        challenger
            .challenge(result_id)
            .value(U256::from(CHALLENGE_BOND_WEI))
            .send()
            .await
            .expect("challenge send failed")
            .watch()
            .await
            .expect("challenge watch failed");

        // Verify challenged state
        let state = admin.getResult(result_id).call().await.unwrap();
        assert!(state.challenged, "should be challenged");
        assert!(
            state.disputeDeadline > U256::ZERO,
            "dispute deadline should be set"
        );

        // -- Advance past dispute window without submitting proof --
        ctx.advance_time(DISPUTE_WINDOW_SECS + 1).await;

        // -- Resolve by timeout --
        let timeout_receipt = admin
            .resolveDisputeByTimeout(result_id)
            .send()
            .await
            .expect("resolveDisputeByTimeout send failed")
            .get_receipt()
            .await
            .expect("resolveDisputeByTimeout receipt failed");

        assert!(
            timeout_receipt.status(),
            "resolveDisputeByTimeout should succeed"
        );

        // -- Verify challenger won --
        let resolved = admin.disputeResolved(result_id).call().await.unwrap();
        assert!(resolved, "dispute should be resolved");

        let prover_won = admin.disputeProverWon(result_id).call().await.unwrap();
        assert!(!prover_won, "prover should NOT win on timeout");

        let valid = admin.isResultValid(result_id).call().await.unwrap();
        assert!(!valid, "result should be invalid when challenger wins");
    }

    // -----------------------------------------------------------------------
    // Test 3: Finalize must fail before challenge window passes
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "requires foundry: forge build + anvil on PATH"]
    async fn test_cannot_finalize_before_challenge_window() {
        let ctx = TestCtx::new().await;
        let admin = ctx.contract_with(ADMIN_KEY);

        admin
            .registerEnclave(enclave_address(), image_hash())
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let attestation = sign_attestation(
            &enclave_signer(),
            model_hash(),
            input_hash(),
            &result_data(),
        );

        let submit_receipt = admin
            .submitResult(
                model_hash(),
                input_hash(),
                Bytes::from(result_data()),
                attestation,
            )
            .value(U256::from(PROVER_STAKE_WEI))
            .send()
            .await
            .unwrap()
            .get_receipt()
            .await
            .unwrap();

        let result_id = extract_result_id(&submit_receipt);

        // Try finalize immediately -- should revert
        let early_finalize: Result<_, alloy::contract::Error> =
            admin.finalize(result_id).send().await;

        assert!(
            early_finalize.is_err(),
            "finalize should fail before challenge window passes"
        );

        // Advance halfway -- still should fail
        ctx.advance_time(CHALLENGE_WINDOW_SECS / 2).await;

        let halfway_finalize: Result<_, alloy::contract::Error> =
            admin.finalize(result_id).send().await;

        assert!(
            halfway_finalize.is_err(),
            "finalize should fail halfway through challenge window"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Full lifecycle with mock enclave HTTP server
    //
    // This test exercises the real inference-to-finalization pipeline:
    //   1. Start a mock enclave HTTP server that handles POST /infer
    //   2. Deploy TEEMLVerifier on Anvil
    //   3. Register the mock enclave's signing key
    //   4. Call the mock enclave's /infer endpoint via HTTP
    //   5. Parse the response and submit the result on-chain
    //   6. Advance time past challenge window
    //   7. Finalize and verify
    // -----------------------------------------------------------------------

    /// JSON request body for the mock enclave /infer endpoint.
    #[derive(serde::Serialize)]
    struct MockInferRequest {
        features: Vec<f64>,
    }

    /// JSON response from the mock enclave /infer endpoint.
    #[derive(serde::Deserialize, Debug)]
    struct MockInferResponse {
        result: String,
        model_hash: String,
        input_hash: String,
        #[allow(dead_code)]
        result_hash: String,
        attestation: String,
        #[allow(dead_code)]
        enclave_address: String,
    }

    /// Enclave state shared between axum handler threads.
    struct EnclaveState {
        signer: PrivateKeySigner,
        model_hash: FixedBytes<32>,
    }

    /// Start a mock enclave HTTP server that responds to POST /infer.
    /// Returns the server address (e.g. "127.0.0.1:PORT") and a handle to
    /// shut it down later.
    async fn start_mock_enclave(enclave_key: &str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        use axum::{extract::State as AxumState, routing::post, Json, Router};

        let signer = parse_signer(enclave_key);

        let state = Arc::new(EnclaveState {
            signer,
            model_hash: keccak256(b"mock-model"),
        });

        async fn infer_handler(
            AxumState(state): AxumState<Arc<EnclaveState>>,
            Json(req): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            use alloy::signers::SignerSync as _;
            // Extract features and compute input hash
            let features = req
                .get("features")
                .and_then(|v| v.as_array())
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_f64())
                .collect::<Vec<_>>();

            let input_json = serde_json::to_vec(&features).unwrap();
            let input_hash = keccak256(&input_json);

            // Produce result
            let scores = vec![0.85_f64];
            let result_bytes = serde_json::to_vec(&scores).unwrap();
            let result_hash = keccak256(&result_bytes);

            // Sign attestation: keccak256(modelHash || inputHash || resultHash)
            let mut packed = Vec::with_capacity(96);
            packed.extend_from_slice(state.model_hash.as_slice());
            packed.extend_from_slice(input_hash.as_slice());
            packed.extend_from_slice(result_hash.as_slice());
            let message = keccak256(&packed);

            let sig = state
                .signer
                .sign_message_sync(message.as_slice())
                .expect("signing failed");

            Json(serde_json::json!({
                "result": format!("0x{}", hex::encode(&result_bytes)),
                "model_hash": format!("0x{}", hex::encode(state.model_hash)),
                "input_hash": format!("0x{}", hex::encode(input_hash)),
                "result_hash": format!("0x{}", hex::encode(result_hash)),
                "attestation": format!("0x{}", hex::encode(sig.as_bytes())),
                "enclave_address": format!("{}", state.signer.address()),
            }))
        }

        let app = Router::new()
            .route("/infer", post(infer_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind mock enclave");
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (addr, handle)
    }

    #[tokio::test]
    #[ignore = "requires foundry: forge build + anvil on PATH"]
    async fn test_full_lifecycle_with_mock_enclave_http() {
        // -- Step 1: Start mock enclave --
        let (enclave_addr, enclave_handle) = start_mock_enclave(ENCLAVE_KEY).await;
        let enclave_url = format!("http://{}", enclave_addr);

        // -- Step 2: Deploy contracts on Anvil --
        let ctx = TestCtx::new().await;
        let admin = ctx.contract_with(ADMIN_KEY);

        // Register the mock enclave's signing key
        let signer = enclave_signer();
        admin
            .registerEnclave(signer.address(), image_hash())
            .send()
            .await
            .expect("registerEnclave send failed")
            .watch()
            .await
            .expect("registerEnclave watch failed");

        // -- Step 3: Call the enclave /infer endpoint --
        let http_client = reqwest::Client::new();
        let infer_response: MockInferResponse = http_client
            .post(format!("{}/infer", enclave_url))
            .json(&MockInferRequest {
                features: vec![1.0, 2.0, 3.0, 4.0],
            })
            .send()
            .await
            .expect("HTTP request to mock enclave failed")
            .json()
            .await
            .expect("failed to parse infer response");

        // -- Step 4: Parse enclave response into on-chain params --
        let model_hash_bytes: [u8; 32] = hex::decode(
            infer_response
                .model_hash
                .strip_prefix("0x")
                .unwrap_or(&infer_response.model_hash),
        )
        .expect("bad model_hash hex")
        .try_into()
        .expect("model_hash wrong length");

        let input_hash_bytes: [u8; 32] = hex::decode(
            infer_response
                .input_hash
                .strip_prefix("0x")
                .unwrap_or(&infer_response.input_hash),
        )
        .expect("bad input_hash hex")
        .try_into()
        .expect("input_hash wrong length");

        let result_bytes = hex::decode(
            infer_response
                .result
                .strip_prefix("0x")
                .unwrap_or(&infer_response.result),
        )
        .expect("bad result hex");

        let attestation_bytes = hex::decode(
            infer_response
                .attestation
                .strip_prefix("0x")
                .unwrap_or(&infer_response.attestation),
        )
        .expect("bad attestation hex");

        let on_chain_model_hash = FixedBytes::<32>::from(model_hash_bytes);
        let on_chain_input_hash = FixedBytes::<32>::from(input_hash_bytes);

        // -- Step 5: Submit on-chain --
        let submit_receipt = admin
            .submitResult(
                on_chain_model_hash,
                on_chain_input_hash,
                Bytes::from(result_bytes),
                Bytes::from(attestation_bytes),
            )
            .value(U256::from(PROVER_STAKE_WEI))
            .send()
            .await
            .expect("submitResult send failed")
            .get_receipt()
            .await
            .expect("submitResult receipt failed");

        assert!(submit_receipt.status(), "submitResult should succeed");

        let result_id = extract_result_id(&submit_receipt);

        // Verify the result is stored correctly
        let state = admin.getResult(result_id).call().await.unwrap();
        assert_eq!(state.modelHash, on_chain_model_hash);
        assert_eq!(state.inputHash, on_chain_input_hash);
        assert!(!state.finalized);
        assert!(!state.challenged);

        // -- Step 6: Wait for challenge window to pass --
        ctx.advance_time(CHALLENGE_WINDOW_SECS + 1).await;

        // -- Step 7: Finalize --
        let finalize_receipt = admin
            .finalize(result_id)
            .send()
            .await
            .expect("finalize send failed")
            .get_receipt()
            .await
            .expect("finalize receipt failed");

        assert!(finalize_receipt.status(), "finalize should succeed");

        // -- Step 8: Verify finalized state --
        let final_state = admin.getResult(result_id).call().await.unwrap();
        assert!(final_state.finalized, "result should be finalized");
        assert!(!final_state.challenged, "should not be challenged");

        let valid = admin.isResultValid(result_id).call().await.unwrap();
        assert!(valid, "result should be valid after finalization");

        // Clean up mock enclave
        enclave_handle.abort();
    }
}
