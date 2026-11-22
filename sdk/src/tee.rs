//! TEEVerifier client for the TEEMLVerifier contract.

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol_types::SolEvent;

use crate::abi::{RemainderVerifier, TEEMLVerifier};
use crate::client::{Client, PendingTransactionExt};

/// Gas kept on top of the computed `resolveDispute` limit for the differences
/// between the estimated and the real execution (cold account access, the
/// caller being the verifier contract instead of an externally owned account).
const DISPUTE_GAS_MARGIN: u64 = 100_000;

/// Gas limit for `resolveDispute`, given `eth_estimateGas` of `resolveDispute`
/// itself (`outer_estimate`) and of the proof verification it runs in an inner
/// call (`verification_gas`).
///
/// `outer_estimate` covers everything except the verification (it may come from
/// a run in which the inner call ran out of gas). The inner call receives at
/// most 63/64 of the remaining gas, so `verification_gas * 64 / 63` is added.
fn dispute_gas_limit(outer_estimate: u64, verification_gas: u64) -> u64 {
    outer_estimate
        .saturating_add(verification_gas)
        .saturating_add(verification_gas / 63 + 1)
        .saturating_add(DISPUTE_GAS_MARGIN)
}

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
            .get_successful_receipt()
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
            .get_successful_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Submit --

    /// Submit a TEE-attested ML result with a prover stake. Returns the transaction hash.
    ///
    /// The transaction hash is NOT the result ID. Use [`Self::submit_result_with_id`]
    /// when the result has to be referenced later (challenge, finalize, dispute).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(model_hash = %model_hash, input_hash = %input_hash)))]
    pub async fn submit_result(
        &self,
        model_hash: B256,
        input_hash: B256,
        result: &[u8],
        attestation: &[u8],
        stake: U256,
    ) -> anyhow::Result<B256> {
        let (tx_hash, _result_id) = self
            .submit_result_with_id(model_hash, input_hash, result, attestation, stake)
            .await?;
        Ok(tx_hash)
    }

    /// Submit a TEE-attested ML result and return `(transaction hash, result ID)`.
    ///
    /// The result ID is read from the `ResultSubmitted` event of the receipt. It is the
    /// identifier the contract uses for `challenge`, `finalize` and dispute resolution.
    pub async fn submit_result_with_id(
        &self,
        model_hash: B256,
        input_hash: B256,
        result: &[u8],
        attestation: &[u8],
        stake: U256,
    ) -> anyhow::Result<(B256, B256)> {
        let provider = self.build_provider();
        let contract_address = self.client.contract_address();
        let contract = TEEMLVerifier::new(contract_address, provider);

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
            .get_successful_receipt()
            .await?;

        let result_id = receipt
            .inner
            .logs()
            .iter()
            .find(|log| {
                log.address() == contract_address
                    && log.topic0() == Some(&TEEMLVerifier::ResultSubmitted::SIGNATURE_HASH)
            })
            .and_then(|log| log.topics().get(1).copied())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "ResultSubmitted event not found in receipt of {}",
                    receipt.transaction_hash
                )
            })?;

        Ok((receipt.transaction_hash, result_id))
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
            .get_successful_receipt()
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
            .get_successful_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Dispute --

    /// Resolve a dispute by submitting a ZK proof.
    ///
    /// See [`Self::resolve_dispute_with_gas`] for how the gas limit is chosen and
    /// for the check that runs before anything is sent.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn resolve_dispute(
        &self,
        result_id: B256,
        proof: &[u8],
        circuit_hash: B256,
        pub_inputs: &[u8],
        gens: &[u8],
    ) -> anyhow::Result<B256> {
        let (tx_hash, _gas_used) = self
            .resolve_dispute_with_gas(result_id, proof, circuit_hash, pub_inputs, gens)
            .await?;
        Ok(tx_hash)
    }

    /// Resolve a dispute by submitting a ZK proof, returning both the
    /// transaction hash and the gas used from the receipt.
    ///
    /// This is useful for classifying dispute resolution paths (e.g. Stylus
    /// vs Solidity) based on gas consumption.
    ///
    /// `resolveDispute` runs the proof verification in an inner call and settles
    /// the dispute for the challenger when that call fails, including when it
    /// only ran out of gas. `eth_estimateGas` on `resolveDispute` cannot tell the
    /// two outcomes apart (the transaction succeeds either way) and returns a
    /// limit that is too low for the verification. So this method:
    ///
    /// 1. estimates the verification call on the RemainderVerifier itself and
    ///    checks with `eth_call` that the proof verifies with that much gas
    ///    (an error is returned, and nothing is sent, if it does not);
    /// 2. sends `resolveDispute` with a gas limit that leaves the inner call
    ///    that much gas (EIP-150 forwards at most 63/64 of what is left).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(result_id = %result_id)))]
    pub async fn resolve_dispute_with_gas(
        &self,
        result_id: B256,
        proof: &[u8],
        circuit_hash: B256,
        pub_inputs: &[u8],
        gens: &[u8],
    ) -> anyhow::Result<(B256, u64)> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider.clone());

        let proof = Bytes::copy_from_slice(proof);
        let pub_inputs = Bytes::copy_from_slice(pub_inputs);
        let gens = Bytes::copy_from_slice(gens);

        // The verification exactly as resolveDispute will run it.
        let remainder_address = contract.remainderVerifier().call().await?;
        let remainder = RemainderVerifier::new(remainder_address, provider);
        let (verification_gas, verifies) = if contract.useStylusVerifier().call().await? {
            let call = remainder.verifyDAGProofStylus(
                proof.clone(),
                circuit_hash,
                pub_inputs.clone(),
                gens.clone(),
            );
            let gas = call.estimate_gas().await?;
            (gas, call.gas(gas).call().await?)
        } else {
            let call = remainder.verifyDAGProof(
                proof.clone(),
                circuit_hash,
                pub_inputs.clone(),
                gens.clone(),
            );
            let gas = call.estimate_gas().await?;
            (gas, call.gas(gas).call().await?)
        };
        anyhow::ensure!(
            verifies,
            "the proof does not verify on RemainderVerifier {remainder_address}; \
             resolveDispute would settle the dispute for the challenger, nothing was sent"
        );

        let call = contract.resolveDispute(result_id, proof, circuit_hash, pub_inputs, gens);
        let gas_limit = dispute_gas_limit(call.estimate_gas().await?, verification_gas);
        let receipt = call
            .gas(gas_limit)
            .send()
            .await?
            .get_successful_receipt()
            .await?;

        Ok((receipt.transaction_hash, receipt.gas_used))
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
            .get_successful_receipt()
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

    // -- Admin / UUPS / Pausable --

    /// Get the admin address (UUPS proxy pattern).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn admin(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let addr = contract.admin().call().await?;
        Ok(addr)
    }

    /// Change the admin address (onlyAdmin).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(new_admin = %new_admin)))]
    pub async fn change_admin(&self, new_admin: Address) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract
            .changeAdmin(new_admin)
            .send()
            .await?
            .get_successful_receipt()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Get the timelock controller address.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn timelock(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let addr = contract.timelock().call().await?;
        Ok(addr)
    }

    /// Set the timelock controller address (onlyAdmin).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(addr = %addr)))]
    pub async fn set_timelock(&self, addr: Address) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract
            .setTimelock(addr)
            .send()
            .await?
            .get_successful_receipt()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Get the current implementation address (UUPS proxy).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn implementation(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let addr = contract.implementation().call().await?;
        Ok(addr)
    }

    /// Pause the contract (onlyAdmin).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn pause(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract
            .pause()
            .send()
            .await?
            .get_successful_receipt()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Unpause the contract (onlyAdmin).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn unpause(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let receipt = contract
            .unpause()
            .send()
            .await?
            .get_successful_receipt()
            .await?;
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

    /// Get the stake `submitResult` requires as `msg.value` (wei).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn prover_stake(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let stake = contract.proverStake().call().await?;
        Ok(stake)
    }

    /// Get the bond `challenge` requires as `msg.value` (wei).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn challenge_bond_amount(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = TEEMLVerifier::new(self.client.contract_address(), provider);
        let bond = contract.challengeBondAmount().call().await?;
        Ok(bond)
    }

    // -- Internal --

    fn build_provider(&self) -> impl alloy::providers::Provider + Clone {
        ProviderBuilder::new()
            .wallet(self.client.wallet.clone())
            .connect_http(self.client.rpc_url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the limit from `dispute_gas_limit`, the inner call must be able to
    /// receive `verification_gas` under EIP-150 even when every unit of the
    /// outer estimate is spent outside the inner call.
    #[test]
    fn dispute_gas_limit_leaves_verification_gas_for_inner_call() {
        // Measured on Anvil for the phase1a fixture.
        let outer_estimate = 195_956_031u64;
        let verification_gas = 254_000_000u64;
        let limit = dispute_gas_limit(outer_estimate, verification_gas);

        let available_at_call = limit - outer_estimate;
        let forwarded = available_at_call - available_at_call / 64;
        assert!(forwarded >= verification_gas);
        assert!(limit > outer_estimate + verification_gas);
        // The plain estimate, which the SDK used to send, is not enough.
        assert!(outer_estimate - outer_estimate / 64 < verification_gas);
    }

    #[test]
    fn dispute_gas_limit_saturates() {
        assert_eq!(dispute_gas_limit(u64::MAX, u64::MAX), u64::MAX);
        assert_eq!(dispute_gas_limit(0, 0), DISPUTE_GAS_MARGIN + 1);
    }
}
