use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::ProviderBuilder;

use crate::abi::TEEMLVerifier;
use crate::client::Client;

/// TEE ML Verifier — wraps the `TEEMLVerifier` contract.
pub struct TEEVerifier {
    client: Client,
}

impl TEEVerifier {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    // -- Admin --

    /// Register a TEE enclave key.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(key = %key)))]
    pub async fn register_enclave(&self, key: Address, image_hash: B256) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .registerEnclave(key, image_hash)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Revoke a TEE enclave key.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(key = %key)))]
    pub async fn revoke_enclave(&self, key: Address) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .revokeEnclave(key)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Submit --

    /// Submit a TEE-attested ML result with a prover stake.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(model_hash = %model_hash, input_hash = %input_hash)))]
    pub async fn submit_result(
        &self,
        model_hash: B256,
        input_hash: B256,
        result: &[u8],
        attestation: &[u8],
        stake: U256,
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .submitResult(
                model_hash,
                input_hash,
                Bytes::copy_from_slice(result),
                Bytes::copy_from_slice(attestation),
            )
            .value(stake)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Challenge --

    /// Challenge a submitted result by posting a bond.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn challenge(&self, result_id: B256, bond: U256) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .challenge(result_id)
            .value(bond)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Finalize --

    /// Finalize an unchallenged result after the challenge window.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn finalize(&self, result_id: B256) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .finalize(result_id)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Dispute --

    /// Resolve a dispute by submitting a ZK proof.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn resolve_dispute(
        &self,
        result_id: B256,
        proof: &[u8],
        circuit_hash: B256,
        pub_inputs: &[u8],
        gens: &[u8],
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .resolveDispute(
                result_id,
                Bytes::copy_from_slice(proof),
                circuit_hash,
                Bytes::copy_from_slice(pub_inputs),
                Bytes::copy_from_slice(gens),
            )
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Resolve a dispute by timeout (challenger wins if prover didn't submit proof).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn resolve_dispute_by_timeout(&self, result_id: B256) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .resolveDisputeByTimeout(result_id)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Query --

    /// Get the full result struct for a given result ID.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn get_result(&self, result_id: B256) -> anyhow::Result<TEEMLVerifier::MLResult> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let result = contract.getResult(result_id).call().await?;
        Ok(result)
    }

    /// Check if a result is valid (finalized and unchallenged, or dispute resolved in prover's favor).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn is_result_valid(&self, result_id: B256) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);

        let valid = contract.isResultValid(result_id).call().await?;
        Ok(valid)
    }

    // -- Owner / Pausable --

    /// Get the owner address (Ownable2Step).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn owner(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let addr = contract.owner().call().await?;
        Ok(addr)
    }

    /// Get the pending owner address (Ownable2Step).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn pending_owner(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let addr = contract.pendingOwner().call().await?;
        Ok(addr)
    }

    /// Initiate ownership transfer (Ownable2Step). New owner must call `accept_ownership()`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(new_owner = %new_owner)))]
    pub async fn transfer_ownership(&self, new_owner: Address) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract
            .transferOwnership(new_owner)
            .send()
            .await?
            .get_receipt()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Accept pending ownership transfer (Ownable2Step).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn accept_ownership(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract
            .acceptOwnership()
            .send()
            .await?
            .get_receipt()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Pause the contract (onlyOwner).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn pause(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract.pause().send().await?.get_receipt().await?;
        Ok(receipt.transaction_hash)
    }

    /// Unpause the contract (onlyOwner).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn unpause(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract.unpause().send().await?.get_receipt().await?;
        Ok(receipt.transaction_hash)
    }

    /// Check if the contract is paused.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn paused(&self) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let is_paused = contract.paused().call().await?;
        Ok(is_paused)
    }

    /// Get the RemainderVerifier address.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn remainder_verifier(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let addr = contract.remainderVerifier().call().await?;
        Ok(addr)
    }

    // -- Internal --

    fn build_provider(&self) -> impl alloy::providers::Provider + Clone {
        ProviderBuilder::new()
            .wallet(self.client.wallet.clone())
            .connect_http(self.client.rpc_url.clone())
    }
}
