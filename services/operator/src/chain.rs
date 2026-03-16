use alloy::primitives::{Address, B256, U256};
use world_zk_sdk::{Client, TEEVerifier};

/// On-chain operations via the TEEMLVerifier contract.
///
/// When `dry_run` is `true`, no transactions are submitted. Instead, each
/// method logs the calldata that **would** be sent and returns a zeroed-out
/// transaction hash (`B256::ZERO`).
pub struct ChainClient {
    tee_verifier: TEEVerifier,
    /// When true, skip on-chain submission and log instead.
    dry_run: bool,
}

impl ChainClient {
    pub fn new(rpc_url: &str, private_key: &str, contract_address: &str) -> anyhow::Result<Self> {
        let client = Client::new(rpc_url, private_key, contract_address)?;
        Ok(Self {
            tee_verifier: TEEVerifier::new(client),
            dry_run: false,
        })
    }

    /// Create a new `ChainClient` with dry-run mode.
    ///
    /// When `dry_run` is `true`, transaction-submitting methods will log
    /// the calldata that would be sent but will not actually broadcast
    /// any transactions on-chain.
    pub fn new_with_dry_run(
        rpc_url: &str,
        private_key: &str,
        contract_address: &str,
        dry_run: bool,
    ) -> anyhow::Result<Self> {
        let client = Client::new(rpc_url, private_key, contract_address)?;
        Ok(Self {
            tee_verifier: TEEVerifier::new(client),
            dry_run,
        })
    }

    /// Returns whether this client is in dry-run mode.
    #[allow(dead_code)]
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Submit a TEE-attested result on-chain. Returns the transaction hash.
    ///
    /// In dry-run mode, logs the call parameters and returns `B256::ZERO`.
    pub async fn submit_result(
        &self,
        model_hash: B256,
        input_hash: B256,
        result: &[u8],
        attestation: &[u8],
        stake: U256,
    ) -> anyhow::Result<B256> {
        if self.dry_run {
            tracing::info!(
                dry_run = true,
                action = "submit_result",
                model_hash = %model_hash,
                input_hash = %input_hash,
                result_len = result.len(),
                attestation_len = attestation.len(),
                stake = %stake,
                result_hex = %format!("0x{}", hex::encode(result)),
                "[DRY RUN] Would submit result on-chain"
            );
            return Ok(B256::ZERO);
        }
        self.tee_verifier
            .submit_result(model_hash, input_hash, result, attestation, stake)
            .await
    }

    /// Resolve a dispute by submitting a ZK proof.
    ///
    /// In dry-run mode, logs the call parameters and returns `B256::ZERO`.
    pub async fn resolve_dispute(
        &self,
        result_id: B256,
        proof: &[u8],
        circuit_hash: B256,
        public_inputs: &[u8],
        gens_data: &[u8],
    ) -> anyhow::Result<B256> {
        if self.dry_run {
            tracing::info!(
                dry_run = true,
                action = "resolve_dispute",
                result_id = %result_id,
                circuit_hash = %circuit_hash,
                proof_len = proof.len(),
                public_inputs_len = public_inputs.len(),
                gens_data_len = gens_data.len(),
                "[DRY RUN] Would resolve dispute on-chain"
            );
            return Ok(B256::ZERO);
        }
        self.tee_verifier
            .resolve_dispute(result_id, proof, circuit_hash, public_inputs, gens_data)
            .await
    }

    /// Finalize an unchallenged result after the challenge window.
    ///
    /// In dry-run mode, logs the call parameters and returns `B256::ZERO`.
    pub async fn finalize(&self, result_id: B256) -> anyhow::Result<B256> {
        if self.dry_run {
            tracing::info!(
                dry_run = true,
                action = "finalize",
                result_id = %result_id,
                "[DRY RUN] Would finalize result on-chain"
            );
            return Ok(B256::ZERO);
        }
        self.tee_verifier.finalize(result_id).await
    }

    /// Check if a result is valid.
    ///
    /// This is a read-only call and is always executed regardless of dry-run mode.
    #[allow(dead_code)]
    pub async fn is_result_valid(&self, result_id: B256) -> anyhow::Result<bool> {
        self.tee_verifier.is_result_valid(result_id).await
    }

    /// Register an enclave key.
    ///
    /// In dry-run mode, logs the call parameters and returns `B256::ZERO`.
    pub async fn register_enclave(&self, key: Address, image_hash: B256) -> anyhow::Result<B256> {
        if self.dry_run {
            tracing::info!(
                dry_run = true,
                action = "register_enclave",
                enclave_key = %key,
                image_hash = %image_hash,
                "[DRY RUN] Would register enclave on-chain"
            );
            return Ok(B256::ZERO);
        }
        self.tee_verifier.register_enclave(key, image_hash).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// A syntactically valid (but unreachable) HTTP RPC URL.
    const FAKE_RPC: &str = "http://127.0.0.1:1";

    /// A valid secp256k1 private key (32-byte hex). This is a throwaway key
    /// used only in tests; it has no funds anywhere.
    const FAKE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    /// A valid Ethereum address (checksummed).
    const FAKE_ADDR: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    /// Build a `ChainClient` in dry-run mode using the fake constants above.
    fn make_dry_run_client() -> ChainClient {
        ChainClient::new_with_dry_run(FAKE_RPC, FAKE_KEY, FAKE_ADDR, true)
            .expect("should construct with valid inputs")
    }

    /// Build a `ChainClient` in normal (non-dry-run) mode.
    fn make_normal_client() -> ChainClient {
        ChainClient::new(FAKE_RPC, FAKE_KEY, FAKE_ADDR)
            .expect("should construct with valid inputs")
    }

    // ---------------------------------------------------------------------------
    // Signature compile-time checks (original tests, kept for completeness)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_with_dry_run_signature_compiles() {
        let _: fn(&str, &str, &str, bool) -> anyhow::Result<ChainClient> =
            ChainClient::new_with_dry_run;
    }

    #[test]
    fn test_new_signature_compiles() {
        let _: fn(&str, &str, &str) -> anyhow::Result<ChainClient> = ChainClient::new;
    }

    // ---------------------------------------------------------------------------
    // Construction: success paths
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_constructs_with_valid_inputs() {
        let client = ChainClient::new(FAKE_RPC, FAKE_KEY, FAKE_ADDR);
        assert!(client.is_ok(), "ChainClient::new should succeed with valid inputs");
    }

    #[test]
    fn test_new_returns_non_dry_run_client() {
        let client = make_normal_client();
        assert!(!client.is_dry_run(), "ChainClient::new should produce a non-dry-run client");
    }

    #[test]
    fn test_new_with_dry_run_true() {
        let client =
            ChainClient::new_with_dry_run(FAKE_RPC, FAKE_KEY, FAKE_ADDR, true).unwrap();
        assert!(client.is_dry_run(), "dry_run=true should be reflected by is_dry_run()");
    }

    #[test]
    fn test_new_with_dry_run_false() {
        let client =
            ChainClient::new_with_dry_run(FAKE_RPC, FAKE_KEY, FAKE_ADDR, false).unwrap();
        assert!(!client.is_dry_run(), "dry_run=false should be reflected by is_dry_run()");
    }

    #[test]
    fn test_new_accepts_key_without_0x_prefix() {
        // Private key without the "0x" prefix should also be accepted.
        let bare_key = &FAKE_KEY[2..]; // strip "0x"
        let result = ChainClient::new(FAKE_RPC, bare_key, FAKE_ADDR);
        assert!(result.is_ok(), "Private key without 0x prefix should be accepted");
    }

    #[test]
    fn test_new_accepts_lowercase_address() {
        let lower = FAKE_ADDR.to_lowercase();
        let result = ChainClient::new(FAKE_RPC, FAKE_KEY, &lower);
        assert!(result.is_ok(), "Lowercase address should be accepted");
    }

    #[test]
    fn test_new_accepts_https_rpc_url() {
        let result = ChainClient::new("https://eth-mainnet.example.com", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_ok(), "HTTPS RPC URL should be accepted");
    }

    #[test]
    fn test_new_accepts_websocket_rpc_url() {
        let result = ChainClient::new("ws://127.0.0.1:8545", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_ok(), "WebSocket RPC URL should be accepted");
    }

    // ---------------------------------------------------------------------------
    // Construction: error paths — invalid RPC URL
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_rejects_empty_rpc_url() {
        let result = ChainClient::new("", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_err(), "Empty RPC URL should be rejected");
    }

    #[test]
    fn test_new_rejects_garbage_rpc_url() {
        let result = ChainClient::new("not-a-url", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_err(), "Non-URL string should be rejected");
    }

    #[test]
    fn test_new_rejects_rpc_url_missing_scheme() {
        let result = ChainClient::new("127.0.0.1:8545", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_err(), "URL without scheme should be rejected");
    }

    // ---------------------------------------------------------------------------
    // Construction: error paths — invalid private key
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_rejects_empty_private_key() {
        let result = ChainClient::new(FAKE_RPC, "", FAKE_ADDR);
        assert!(result.is_err(), "Empty private key should be rejected");
    }

    #[test]
    fn test_new_rejects_short_private_key() {
        let result = ChainClient::new(FAKE_RPC, "0xdead", FAKE_ADDR);
        assert!(result.is_err(), "Too-short private key should be rejected");
    }

    #[test]
    fn test_new_rejects_non_hex_private_key() {
        let result = ChainClient::new(FAKE_RPC, "not-a-hex-key-at-all-this-is-garbage-text", FAKE_ADDR);
        assert!(result.is_err(), "Non-hex private key should be rejected");
    }

    #[test]
    fn test_new_rejects_all_zero_private_key() {
        // The zero scalar is not a valid secp256k1 private key.
        let zero_key = "0x0000000000000000000000000000000000000000000000000000000000000000";
        let result = ChainClient::new(FAKE_RPC, zero_key, FAKE_ADDR);
        assert!(result.is_err(), "All-zero private key should be rejected");
    }

    #[test]
    fn test_new_rejects_oversized_private_key() {
        // 33 bytes (66 hex chars) is too long for a secp256k1 key.
        let long_key = format!("0x{}", "ab".repeat(33));
        let result = ChainClient::new(FAKE_RPC, &long_key, FAKE_ADDR);
        assert!(result.is_err(), "Oversized private key should be rejected");
    }

    // ---------------------------------------------------------------------------
    // Construction: error paths — invalid contract address
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_rejects_empty_address() {
        let result = ChainClient::new(FAKE_RPC, FAKE_KEY, "");
        assert!(result.is_err(), "Empty contract address should be rejected");
    }

    #[test]
    fn test_new_rejects_garbage_address() {
        let result = ChainClient::new(FAKE_RPC, FAKE_KEY, "definitely-not-an-address");
        assert!(result.is_err(), "Non-hex address should be rejected");
    }

    #[test]
    fn test_new_rejects_short_address() {
        let result = ChainClient::new(FAKE_RPC, FAKE_KEY, "0xdead");
        assert!(result.is_err(), "Too-short address should be rejected");
    }

    #[test]
    fn test_new_rejects_oversized_address() {
        // 21 bytes = 42 hex chars, one byte too long.
        let long_addr = format!("0x{}", "ab".repeat(21));
        let result = ChainClient::new(FAKE_RPC, FAKE_KEY, &long_addr);
        assert!(result.is_err(), "Oversized address should be rejected");
    }

    // ---------------------------------------------------------------------------
    // Construction: error propagation through new_with_dry_run
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_with_dry_run_propagates_rpc_error() {
        let result = ChainClient::new_with_dry_run("", FAKE_KEY, FAKE_ADDR, true);
        assert!(result.is_err(), "new_with_dry_run should propagate RPC URL errors");
    }

    #[test]
    fn test_new_with_dry_run_propagates_key_error() {
        let result = ChainClient::new_with_dry_run(FAKE_RPC, "bad", FAKE_ADDR, false);
        assert!(result.is_err(), "new_with_dry_run should propagate private key errors");
    }

    #[test]
    fn test_new_with_dry_run_propagates_address_error() {
        let result = ChainClient::new_with_dry_run(FAKE_RPC, FAKE_KEY, "bad", true);
        assert!(result.is_err(), "new_with_dry_run should propagate address errors");
    }

    // ---------------------------------------------------------------------------
    // Dry-run mode: submit_result
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dry_run_submit_result_returns_zero_hash() {
        let client = make_dry_run_client();
        let tx = client
            .submit_result(B256::ZERO, B256::ZERO, b"result-data", b"attestation", U256::ZERO)
            .await
            .expect("dry-run submit_result should succeed");
        assert_eq!(tx, B256::ZERO, "dry-run should return B256::ZERO");
    }

    #[tokio::test]
    async fn test_dry_run_submit_result_with_nonempty_data() {
        let client = make_dry_run_client();
        let model = B256::from([0xAA; 32]);
        let input = B256::from([0xBB; 32]);
        let result_data = b"prediction: class_1";
        let attestation = b"nitro-attestation-doc";
        let stake = U256::from(1_000_000u64);
        let tx = client
            .submit_result(model, input, result_data, attestation, stake)
            .await
            .expect("dry-run submit_result should succeed with non-trivial data");
        assert_eq!(tx, B256::ZERO);
    }

    #[tokio::test]
    async fn test_dry_run_submit_result_with_empty_slices() {
        let client = make_dry_run_client();
        let tx = client
            .submit_result(B256::ZERO, B256::ZERO, b"", b"", U256::ZERO)
            .await
            .expect("dry-run should handle empty byte slices");
        assert_eq!(tx, B256::ZERO);
    }

    // ---------------------------------------------------------------------------
    // Dry-run mode: resolve_dispute
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dry_run_resolve_dispute_returns_zero_hash() {
        let client = make_dry_run_client();
        let tx = client
            .resolve_dispute(B256::ZERO, b"proof", B256::ZERO, b"pub_inputs", b"gens")
            .await
            .expect("dry-run resolve_dispute should succeed");
        assert_eq!(tx, B256::ZERO);
    }

    #[tokio::test]
    async fn test_dry_run_resolve_dispute_with_large_data() {
        let client = make_dry_run_client();
        let big_proof = vec![0xFFu8; 10_000];
        let big_pub = vec![0x11u8; 5_000];
        let big_gens = vec![0x22u8; 3_000];
        let tx = client
            .resolve_dispute(
                B256::from([1u8; 32]),
                &big_proof,
                B256::from([2u8; 32]),
                &big_pub,
                &big_gens,
            )
            .await
            .expect("dry-run should handle large byte slices");
        assert_eq!(tx, B256::ZERO);
    }

    // ---------------------------------------------------------------------------
    // Dry-run mode: finalize
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dry_run_finalize_returns_zero_hash() {
        let client = make_dry_run_client();
        let tx = client
            .finalize(B256::ZERO)
            .await
            .expect("dry-run finalize should succeed");
        assert_eq!(tx, B256::ZERO);
    }

    #[tokio::test]
    async fn test_dry_run_finalize_with_specific_result_id() {
        let client = make_dry_run_client();
        let result_id = B256::from([0xCC; 32]);
        let tx = client
            .finalize(result_id)
            .await
            .expect("dry-run finalize should succeed with any result_id");
        assert_eq!(tx, B256::ZERO);
    }

    // ---------------------------------------------------------------------------
    // Dry-run mode: register_enclave
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dry_run_register_enclave_returns_zero_hash() {
        let client = make_dry_run_client();
        let key: Address = FAKE_ADDR.parse().unwrap();
        let image_hash = B256::from([0xDD; 32]);
        let tx = client
            .register_enclave(key, image_hash)
            .await
            .expect("dry-run register_enclave should succeed");
        assert_eq!(tx, B256::ZERO);
    }

    #[tokio::test]
    async fn test_dry_run_register_enclave_with_zero_address() {
        let client = make_dry_run_client();
        let tx = client
            .register_enclave(Address::ZERO, B256::ZERO)
            .await
            .expect("dry-run should accept zero address");
        assert_eq!(tx, B256::ZERO);
    }

    // ---------------------------------------------------------------------------
    // is_dry_run accessor
    // ---------------------------------------------------------------------------

    #[test]
    fn test_is_dry_run_true_for_dry_run_client() {
        let client = make_dry_run_client();
        assert!(client.is_dry_run());
    }

    #[test]
    fn test_is_dry_run_false_for_normal_client() {
        let client = make_normal_client();
        assert!(!client.is_dry_run());
    }

    // ---------------------------------------------------------------------------
    // Multiple dry-run calls on the same client (ensure no state mutation)
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dry_run_multiple_calls_remain_idempotent() {
        let client = make_dry_run_client();

        // Call each dry-run method multiple times; all should succeed.
        for _ in 0..3 {
            let tx1 = client
                .submit_result(B256::ZERO, B256::ZERO, b"r", b"a", U256::ZERO)
                .await
                .unwrap();
            let tx2 = client.finalize(B256::ZERO).await.unwrap();
            let tx3 = client
                .resolve_dispute(B256::ZERO, b"p", B256::ZERO, b"pi", b"g")
                .await
                .unwrap();
            let tx4 = client
                .register_enclave(Address::ZERO, B256::ZERO)
                .await
                .unwrap();

            assert_eq!(tx1, B256::ZERO);
            assert_eq!(tx2, B256::ZERO);
            assert_eq!(tx3, B256::ZERO);
            assert_eq!(tx4, B256::ZERO);
        }

        // Client should still report dry_run=true after repeated use.
        assert!(client.is_dry_run());
    }

    // ---------------------------------------------------------------------------
    // Edge case: valid-but-extreme inputs accepted by constructor
    // ---------------------------------------------------------------------------

    #[test]
    fn test_new_accepts_rpc_url_with_path() {
        // Some RPC providers use path-based routing.
        let result = ChainClient::new("http://rpc.example.com/v1/mainnet", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_ok(), "RPC URL with path should be accepted");
    }

    #[test]
    fn test_new_accepts_rpc_url_with_port() {
        let result = ChainClient::new("http://localhost:8545", FAKE_KEY, FAKE_ADDR);
        assert!(result.is_ok(), "RPC URL with port should be accepted");
    }

    #[test]
    fn test_new_accepts_zero_address_as_contract() {
        let zero_addr = "0x0000000000000000000000000000000000000000";
        let result = ChainClient::new(FAKE_RPC, FAKE_KEY, zero_addr);
        assert!(result.is_ok(), "Zero address should be parseable (even if useless)");
    }
}
