mod chain;
mod config;
mod enclave;
mod prover;
mod store;
mod watcher;

use alloy::primitives::{Address, B256, U256};
use clap::{Parser, Subcommand};

use config::Config;
use prover::ProofManager;
use watcher::{EventWatcher, TEEEvent};

#[derive(Parser)]
#[command(name = "tee-operator", about = "TEE ML Operator Service")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Submit an inference request: call enclave, submit on-chain, trigger proof
    Submit {
        /// Feature vector as JSON array, e.g. '[5.0, 3.5, 1.5, 0.3]'
        #[arg(long)]
        features: String,
    },
    /// Watch chain events and auto-resolve disputes
    Watch,
    /// Combined: submit + watch + prove
    Run {
        /// Feature vector as JSON array
        #[arg(long)]
        features: String,
    },
}

fn hex_to_b256(hex_str: &str) -> anyhow::Result<B256> {
    let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(stripped)?;
    if bytes.len() != 32 {
        anyhow::bail!("Expected 32 bytes, got {}", bytes.len());
    }
    Ok(B256::from_slice(&bytes))
}

fn hex_to_bytes(hex_str: &str) -> anyhow::Result<Vec<u8>> {
    let stripped = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    Ok(hex::decode(stripped)?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let config = Config::from_env()?;

    match cli.command {
        Commands::Submit { features } => cmd_submit(&config, &features).await,
        Commands::Watch => cmd_watch(&config).await,
        Commands::Run { features } => cmd_run(&config, &features).await,
    }
}

async fn cmd_submit(config: &Config, features_json: &str) -> anyhow::Result<()> {
    // 1. Parse features
    let feats: Vec<f64> = serde_json::from_str(features_json)
        .map_err(|e| anyhow::anyhow!("Invalid features JSON: {}", e))?;
    tracing::info!("Submitting inference for {} features", feats.len());

    // 2. Call enclave /infer
    let enclave_client = enclave::EnclaveClient::new(&config.enclave_url);

    let health = enclave_client.health().await?;
    if !health {
        anyhow::bail!("Enclave is not healthy at {}", config.enclave_url);
    }

    let response = enclave_client.infer(&feats).await?;
    tracing::info!(
        "Enclave response: model_hash={}, result_hash={}",
        response.model_hash,
        response.result_hash
    );

    // 3. Submit on-chain
    let chain_client = chain::ChainClient::new(
        &config.rpc_url,
        &config.private_key,
        &config.tee_verifier_address,
    )?;

    let model_hash = hex_to_b256(&response.model_hash)?;
    let input_hash = hex_to_b256(&response.input_hash)?;
    let result_bytes = hex_to_bytes(&response.result)?;
    let attestation = hex_to_bytes(&response.attestation)?;
    let stake = U256::from_str_radix(&config.prover_stake_wei, 10)
        .map_err(|e| anyhow::anyhow!("Invalid stake: {}", e))?;

    let tx_hash = chain_client
        .submit_result(model_hash, input_hash, &result_bytes, &attestation, stake)
        .await?;
    tracing::info!("Submitted on-chain: tx={}", tx_hash);

    // 4. Trigger proof pre-computation (best-effort)
    let output_path = format!("{}/{}.json", config.proofs_dir, tx_hash);
    let child = tokio::process::Command::new(&config.precompute_bin)
        .arg("--model")
        .arg(&config.model_path)
        .arg("--features")
        .arg(features_json)
        .arg("--output")
        .arg(&output_path)
        .spawn();

    match child {
        Ok(_) => tracing::info!("Proof generation started: {}", output_path),
        Err(e) => tracing::warn!("Could not start proof generation: {}", e),
    }

    println!("tx_hash={}", tx_hash);
    Ok(())
}

async fn cmd_watch(config: &Config) -> anyhow::Result<()> {
    let contract_addr: Address = config
        .tee_verifier_address
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid contract address: {}", e))?;
    let watcher = EventWatcher::new(&config.rpc_url, contract_addr);
    let chain_client = chain::ChainClient::new(
        &config.rpc_url,
        &config.private_key,
        &config.tee_verifier_address,
    )?;
    let proof_mgr = ProofManager::new(
        &config.precompute_bin,
        &config.model_path,
        &config.proofs_dir,
    );

    tracing::info!("Watching for events on {}...", config.tee_verifier_address);

    let mut from_block = 0u64;
    let mut finalize_counter = 0u64;

    loop {
        // Poll for new events
        let (events, next_block) = watcher.poll_events(from_block).await?;
        from_block = next_block;

        for event in &events {
            match event {
                TEEEvent::ResultChallenged {
                    result_id,
                    challenger,
                } => {
                    tracing::warn!(
                        "Challenge detected! resultId={}, challenger={}",
                        result_id,
                        challenger
                    );
                    handle_challenge(&chain_client, &proof_mgr, *result_id).await;
                }
                TEEEvent::ResultSubmitted { result_id, .. } => {
                    tracing::info!("New result submitted: {}", result_id);
                }
                TEEEvent::ResultFinalized { result_id } => {
                    tracing::info!("Result finalized: {}", result_id);
                }
            }
        }

        // Every ~60 seconds (12 iterations * 5s), check for finalizeable results
        finalize_counter += 1;
        if finalize_counter >= 12 {
            finalize_counter = 0;
            auto_finalize(&watcher, &chain_client, from_block.saturating_sub(7200)).await;
        }

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn cmd_run(config: &Config, features_json: &str) -> anyhow::Result<()> {
    // 1. Submit (same as cmd_submit)
    let submit_result = cmd_submit(config, features_json).await;
    if let Err(e) = &submit_result {
        tracing::error!("Submit failed: {}", e);
        return submit_result;
    }

    // 2. Start watching
    tracing::info!("Starting watch loop...");
    cmd_watch(config).await
}

async fn handle_challenge(chain: &chain::ChainClient, proof_mgr: &ProofManager, result_id: B256) {
    let rid_hex = format!("0x{}", hex::encode(result_id));

    // Try to load pre-computed proof
    match proof_mgr.read_proof(&rid_hex) {
        Ok(Some(proof)) => {
            tracing::info!("Found pre-computed proof for {}", rid_hex);
            match resolve_with_proof(chain, result_id, &proof).await {
                Ok(tx) => tracing::info!("Dispute resolved! tx={}", tx),
                Err(e) => tracing::error!("Failed to resolve dispute: {}", e),
            }
        }
        Ok(None) => {
            tracing::warn!(
                "No pre-computed proof for {}. Waiting up to 60s...",
                rid_hex
            );
            match proof_mgr.wait_for_proof(&rid_hex, 60).await {
                Ok(true) => {
                    if let Ok(Some(proof)) = proof_mgr.read_proof(&rid_hex) {
                        match resolve_with_proof(chain, result_id, &proof).await {
                            Ok(tx) => tracing::info!("Dispute resolved (after wait)! tx={}", tx),
                            Err(e) => tracing::error!("Failed to resolve dispute: {}", e),
                        }
                    }
                }
                Ok(false) => tracing::error!("Proof not available after timeout for {}", rid_hex),
                Err(e) => tracing::error!("Error waiting for proof: {}", e),
            }
        }
        Err(e) => tracing::error!("Error reading proof: {}", e),
    }
}

async fn resolve_with_proof(
    chain: &chain::ChainClient,
    result_id: B256,
    proof: &store::StoredProof,
) -> anyhow::Result<B256> {
    let proof_bytes = hex_to_bytes(&proof.proof_hex)?;
    let circuit_hash = hex_to_b256(&proof.circuit_hash)?;
    let public_inputs = hex_to_bytes(&proof.public_inputs_hex)?;
    let gens_data = hex_to_bytes(&proof.gens_hex)?;

    chain
        .resolve_dispute(
            result_id,
            &proof_bytes,
            circuit_hash,
            &public_inputs,
            &gens_data,
        )
        .await
}

async fn auto_finalize(watcher: &EventWatcher, chain: &chain::ChainClient, from_block: u64) {
    let (events, _) = match watcher.poll_events(from_block).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to poll for finalizeable results: {}", e);
            return;
        }
    };

    for event in &events {
        if let TEEEvent::ResultSubmitted { result_id, .. } = event {
            // Try to finalize — the contract will revert if not ready
            if let Ok(tx) = chain.finalize(*result_id).await {
                tracing::info!("Auto-finalized {}, tx={}", result_id, tx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_to_b256() {
        let hex = "0x8e7c338859ba0bcb6911e6b68794f5449c0bb36a0e2ce47cb5dc96e8eb56e909";
        let b256 = hex_to_b256(hex).unwrap();
        assert_eq!(format!("0x{}", hex::encode(b256)), hex);
    }

    #[test]
    fn test_hex_to_b256_no_prefix() {
        let hex = "8e7c338859ba0bcb6911e6b68794f5449c0bb36a0e2ce47cb5dc96e8eb56e909";
        let b256 = hex_to_b256(hex).unwrap();
        assert_eq!(hex::encode(b256), hex);
    }

    #[test]
    fn test_hex_to_b256_invalid_length() {
        let result = hex_to_b256("0xaabb");
        assert!(result.is_err());
    }

    #[test]
    fn test_hex_to_bytes() {
        assert_eq!(
            hex_to_bytes("0xdeadbeef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(hex_to_bytes("cafe").unwrap(), vec![0xca, 0xfe]);
    }
}
