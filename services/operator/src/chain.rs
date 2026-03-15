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

    // NOTE: We cannot construct a ChainClient in unit tests because it
    // requires a real RPC endpoint. Instead we test at the config layer
    // (see config.rs tests for DRY_RUN parsing) and rely on integration
    // tests for end-to-end dry-run verification.

    #[test]
    fn test_new_with_dry_run_signature_compiles() {
        // Verify the constructor signature accepts a dry_run bool.
        // The actual call would fail without a valid RPC, but this
        // ensures the API is correct at compile time.
        let _: fn(&str, &str, &str, bool) -> anyhow::Result<ChainClient> =
            ChainClient::new_with_dry_run;
    }

    #[test]
    fn test_new_signature_compiles() {
        // The original constructor should still be available.
        let _: fn(&str, &str, &str) -> anyhow::Result<ChainClient> = ChainClient::new;
    }
}
