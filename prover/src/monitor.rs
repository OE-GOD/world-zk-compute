//! Request monitoring and processing

use crate::config::ProverConfig;
use crate::contracts::IExecutionEngine;
use crate::prover::execute_and_prove;
use alloy::{
    primitives::{Address, U256},
    providers::Provider,
};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Check for pending requests and process them
pub async fn check_and_process_requests<P: Provider + Clone>(
    provider: &Arc<P>,
    config: &ProverConfig,
) -> anyhow::Result<u64> {
    debug!("Checking for pending requests...");

    // Get pending requests from contract
    let engine = IExecutionEngine::new(config.engine_address, provider.clone());

    let pending = engine
        .getPendingRequests(U256::from(0), U256::from(10))
        .call()
        .await?;

    let request_ids = pending._0;

    if request_ids.is_empty() {
        debug!("No pending requests");
        return Ok(0);
    }

    info!("Found {} pending requests", request_ids.len());

    let mut processed = 0u64;

    for request_id in request_ids {
        match process_request(provider, config, request_id).await {
            Ok(true) => {
                processed += 1;
                info!("Successfully processed request {}", request_id);
            }
            Ok(false) => {
                debug!("Skipped request {} (not matching criteria)", request_id);
            }
            Err(e) => {
                warn!("Failed to process request {}: {:?}", request_id, e);
            }
        }
    }

    Ok(processed)
}

/// Process a single request
async fn process_request<P: Provider + Clone>(
    provider: &Arc<P>,
    config: &ProverConfig,
    request_id: U256,
) -> anyhow::Result<bool> {
    let engine = IExecutionEngine::new(config.engine_address, provider.clone());

    // Get request details
    let request = engine.getRequest(request_id).call().await?._0;

    // Check if image ID is allowed
    if !config.is_image_allowed(&request.imageId) {
        debug!("Image ID not in allowed list, skipping");
        return Ok(false);
    }

    // Check if tip is acceptable
    let current_tip = engine.getCurrentTip(request_id).call().await?._0;
    if !config.is_tip_acceptable(current_tip) {
        debug!("Tip too low ({} wei), skipping", current_tip);
        return Ok(false);
    }

    info!(
        "Processing request {}: image={}, tip={} wei",
        request_id, request.imageId, current_tip
    );

    // Claim the request
    info!("Claiming request {}...", request_id);
    let claim_tx = engine.claimExecution(request_id).send().await?;
    let claim_receipt = claim_tx.get_receipt().await?;
    info!("Claimed in tx: {:?}", claim_receipt.transaction_hash);

    // Execute and generate proof
    info!("Executing zkVM and generating proof...");
    let (seal, journal) = execute_and_prove(
        &request.imageId,
        &request.inputUrl,
        &request.inputDigest,
    )
    .await?;

    // Submit proof
    info!("Submitting proof...");
    let submit_tx = engine
        .submitProof(request_id, seal.into(), journal.into())
        .send()
        .await?;
    let submit_receipt = submit_tx.get_receipt().await?;
    info!(
        "Proof submitted in tx: {:?}",
        submit_receipt.transaction_hash
    );

    Ok(true)
}
