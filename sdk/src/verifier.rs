//! DAGVerifier client for on-chain DAG circuit verification.

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol_types::SolType;
use sha2::{Digest, Sha256};

use crate::abi::RemainderVerifier::{self, DAGCircuitDescription};
use crate::client::Client;
use crate::fixture::ProofData;

/// Batch verification progress updates.
#[derive(Debug, Clone)]
pub enum BatchProgress {
    Started {
        session_id: B256,
        total_batches: u64,
    },
    Computing {
        batch: u64,
        total: u64,
    },
    Finalizing {
        step: u64,
    },
    Complete,
}

/// Batch session status returned from on-chain query.
#[derive(Debug, Clone)]
pub struct BatchSession {
    pub circuit_hash: B256,
    pub next_batch_idx: u64,
    pub total_batches: u64,
    pub finalized: bool,
    pub finalize_input_idx: u64,
    pub finalize_groups_done: u64,
}

/// DAG verification orchestrator — wraps the `RemainderVerifier` contract.
pub struct DAGVerifier {
    client: Client,
}

impl DAGVerifier {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    // -- Admin Methods --

    /// Register a DAG circuit on-chain.
    ///
    /// `description` is ABI-encoded into `descData`.
    /// `gens_data` is hashed to produce `gensHash`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %circuit_hash, name = %name)))]
    pub async fn register_circuit(
        &self,
        circuit_hash: B256,
        description: &DAGCircuitDescription,
        name: &str,
        gens_data: &[u8],
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);

        let desc_data = DAGCircuitDescription::abi_encode(description);
        let gens_hash = B256::from_slice(&Sha256::digest(gens_data));

        let receipt = contract
            .registerDAGCircuit(circuit_hash, desc_data.into(), name.to_string(), gens_hash)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Set the Stylus verifier address for a circuit.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %circuit_hash)))]
    pub async fn set_stylus_verifier(
        &self,
        circuit_hash: B256,
        stylus_address: Address,
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .setDAGStylusVerifier(circuit_hash, stylus_address)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Set the Groth16 verifier for a circuit.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %circuit_hash)))]
    pub async fn set_groth16_verifier(
        &self,
        circuit_hash: B256,
        verifier_address: Address,
        input_count: u64,
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);

        let receipt = contract
            .setDAGCircuitGroth16Verifier(circuit_hash, verifier_address, U256::from(input_count))
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // -- Single-TX Verification --

    /// Verify a DAG proof in a single call (view function, no gas limit on-chain).
    /// Uses `.gas(500_000_000)` for Anvil compatibility.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %proof.circuit_hash)))]
    pub async fn verify_single_tx(&self, proof: &ProofData) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);

        let result = contract
            .verifyDAGProof(
                Bytes::copy_from_slice(&proof.proof_bytes),
                proof.circuit_hash,
                Bytes::copy_from_slice(&proof.public_inputs),
                Bytes::copy_from_slice(&proof.gens_data),
            )
            .gas(500_000_000)
            .call()
            .await?;

        Ok(result)
    }

    /// Verify a DAG proof via the Stylus verifier.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %proof.circuit_hash)))]
    pub async fn verify_stylus(&self, proof: &ProofData) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);

        let result = contract
            .verifyDAGProofStylus(
                Bytes::copy_from_slice(&proof.proof_bytes),
                proof.circuit_hash,
                Bytes::copy_from_slice(&proof.public_inputs),
                Bytes::copy_from_slice(&proof.gens_data),
            )
            .gas(500_000_000)
            .call()
            .await?;

        Ok(result)
    }

    // -- Batch Verification --

    /// Run multi-tx batch verification: start -> continueXN -> finalizeXM -> cleanup.
    ///
    /// Calls `on_progress` at each step. Returns the session ID on success.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %proof.circuit_hash)))]
    pub async fn verify_batch<F>(&self, proof: &ProofData, on_progress: F) -> anyhow::Result<B256>
    where
        F: Fn(BatchProgress),
    {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), &provider);

        let proof_bytes = Bytes::copy_from_slice(&proof.proof_bytes);
        let pub_inputs = Bytes::copy_from_slice(&proof.public_inputs);
        let gens = Bytes::copy_from_slice(&proof.gens_data);

        // 1. Start
        let receipt = contract
            .startDAGBatchVerify(
                proof_bytes.clone(),
                proof.circuit_hash,
                pub_inputs.clone(),
                gens.clone(),
            )
            .send()
            .await?
            .get_receipt()
            .await?;

        // Extract session ID from event log
        let session_id = receipt
            .inner
            .logs()
            .iter()
            .find_map(|log| {
                log.log_decode::<RemainderVerifier::DAGBatchSessionStarted>()
                    .ok()
                    .map(|e| e.inner.data.sessionId)
            })
            .ok_or_else(|| anyhow::anyhow!("DAGBatchSessionStarted event not found"))?;

        // Query session for total batches
        let session = self.get_session_status_with(&contract, session_id).await?;
        let total_batches = session.total_batches;

        on_progress(BatchProgress::Started {
            session_id,
            total_batches,
        });

        // 2. Continue (compute batches)
        for batch in 0..total_batches {
            contract
                .continueDAGBatchVerify(
                    session_id,
                    proof_bytes.clone(),
                    pub_inputs.clone(),
                    gens.clone(),
                )
                .send()
                .await?
                .get_receipt()
                .await?;

            on_progress(BatchProgress::Computing {
                batch: batch + 1,
                total: total_batches,
            });
        }

        // 3. Finalize (may take multiple txs)
        let mut finalize_step = 0u64;
        loop {
            let result = contract
                .finalizeDAGBatchVerify(
                    session_id,
                    proof_bytes.clone(),
                    pub_inputs.clone(),
                    gens.clone(),
                )
                .send()
                .await?
                .get_receipt()
                .await?;

            finalize_step += 1;
            on_progress(BatchProgress::Finalizing {
                step: finalize_step,
            });

            // Check if finalized
            let session = self.get_session_status_with(&contract, session_id).await?;
            if session.finalized {
                break;
            }

            // Safety: prevent infinite loop
            if finalize_step > 100 {
                anyhow::bail!("Finalization exceeded 100 steps");
            }

            // Use result to avoid unused variable warning
            let _ = result;
        }

        // 4. Cleanup
        contract
            .cleanupDAGBatchSession(session_id)
            .send()
            .await?
            .get_receipt()
            .await?;

        on_progress(BatchProgress::Complete);

        Ok(session_id)
    }

    // -- Session Query --

    /// Query on-chain batch session status.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(session_id = %session_id)))]
    pub async fn get_session_status(&self, session_id: B256) -> anyhow::Result<BatchSession> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);
        self.get_session_status_with(&contract, session_id).await
    }

    /// Check if a circuit is registered and active.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(circuit_hash = %circuit_hash)))]
    pub async fn is_circuit_active(&self, circuit_hash: B256) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), provider);
        let result = contract.isDAGCircuitActive(circuit_hash).call().await?;
        Ok(result)
    }

    // -- Internal Helpers --

    fn build_provider(&self) -> impl alloy::providers::Provider + Clone {
        ProviderBuilder::new()
            .wallet(self.client.wallet.clone())
            .connect_http(self.client.rpc_url.clone())
    }

    async fn get_session_status_with<P: alloy::providers::Provider + Clone>(
        &self,
        contract: &RemainderVerifier::RemainderVerifierInstance<P>,
        session_id: B256,
    ) -> anyhow::Result<BatchSession> {
        let result = contract.getDAGBatchSession(session_id).call().await?;
        Ok(BatchSession {
            circuit_hash: result.circuitHash,
            next_batch_idx: result.nextBatchIdx.to::<u64>(),
            total_batches: result.totalBatches.to::<u64>(),
            finalized: result.finalized,
            finalize_input_idx: result.finalizeInputIdx.to::<u64>(),
            finalize_groups_done: result.finalizeGroupsDone.to::<u64>(),
        })
    }
}
