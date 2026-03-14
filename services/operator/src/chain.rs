use alloy::primitives::{Address, B256, U256};
use world_zk_sdk::{Client, TEEVerifier};

/// On-chain operations via the TEEMLVerifier contract.
pub struct ChainClient {
    tee_verifier: TEEVerifier,
}

impl ChainClient {
    pub fn new(rpc_url: &str, private_key: &str, contract_address: &str) -> anyhow::Result<Self> {
        let client = Client::new(rpc_url, private_key, contract_address)?;
        Ok(Self {
            tee_verifier: TEEVerifier::new(client),
        })
    }

    /// Submit a TEE-attested result on-chain. Returns the transaction hash.
    pub async fn submit_result(
        &self,
        model_hash: B256,
        input_hash: B256,
        result: &[u8],
        attestation: &[u8],
        stake: U256,
    ) -> anyhow::Result<B256> {
        self.tee_verifier
            .submit_result(model_hash, input_hash, result, attestation, stake)
            .await
    }

    /// Resolve a dispute by submitting a ZK proof.
    pub async fn resolve_dispute(
        &self,
        result_id: B256,
        proof: &[u8],
        circuit_hash: B256,
        public_inputs: &[u8],
        gens_data: &[u8],
    ) -> anyhow::Result<B256> {
        self.tee_verifier
            .resolve_dispute(result_id, proof, circuit_hash, public_inputs, gens_data)
            .await
    }

    /// Finalize an unchallenged result after the challenge window.
    pub async fn finalize(&self, result_id: B256) -> anyhow::Result<B256> {
        self.tee_verifier.finalize(result_id).await
    }

    /// Check if a result is valid.
    #[allow(dead_code)]
    pub async fn is_result_valid(&self, result_id: B256) -> anyhow::Result<bool> {
        self.tee_verifier.is_result_valid(result_id).await
    }

    /// Register an enclave key.
    pub async fn register_enclave(&self, key: Address, image_hash: B256) -> anyhow::Result<B256> {
        self.tee_verifier.register_enclave(key, image_hash).await
    }
}
