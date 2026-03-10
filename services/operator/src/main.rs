mod chain;
mod config;
mod enclave;
mod nitro;
mod prover;
mod store;
mod watcher;

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use clap::{Parser, Subcommand};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use config::Config;
use prover::ProofManager;
use watcher::{EventWatcher, TEEEvent};

/// Cached attestation verification result with TTL.
struct CachedAttestation {
    #[allow(dead_code)]
    verified: nitro::VerifiedAttestation,
    fetched_at: Instant,
}

/// Global attestation cache for the submit command.
/// Avoids re-fetching and re-verifying attestation on every submit
/// within the TTL window.
static ATTESTATION_CACHE: OnceLock<Mutex<Option<CachedAttestation>>> = OnceLock::new();

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
    /// Register an enclave on-chain (fetch attestation, verify, register)
    Register {
        /// Expected PCR0 value (optional -- if set, validates against it)
        #[arg(long)]
        expected_pcr0: Option<String>,
        /// Skip attestation verification (dev mode)
        #[arg(long, default_value = "false")]
        skip_verify: bool,
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
        Commands::Register {
            expected_pcr0,
            skip_verify,
        } => cmd_register(&config, expected_pcr0.as_deref(), skip_verify).await,
    }
}

/// Compute a nonce for attestation freshness verification.
///
/// `nonce = keccak256(chainId || blockNumber || enclaveAddress)`
///
/// This binds the attestation to a specific chain state, preventing replay attacks.
fn compute_attestation_nonce(chain_id: u64, block_number: u64, enclave_address: &str) -> String {
    let mut preimage = Vec::with_capacity(36); // 8 + 8 + 20
    preimage.extend_from_slice(&chain_id.to_be_bytes());
    preimage.extend_from_slice(&block_number.to_be_bytes());
    let addr_hex = enclave_address
        .strip_prefix("0x")
        .unwrap_or(enclave_address);
    if let Ok(addr_bytes) = hex::decode(addr_hex) {
        preimage.extend_from_slice(&addr_bytes);
    }
    hex::encode(keccak256(&preimage).as_slice())
}

/// Verify the enclave's attestation, using the cache if the TTL has not expired.
///
/// When `config.nitro_verification` is true, this fetches and verifies the
/// attestation document. Results are cached for `config.attestation_cache_ttl`
/// seconds to avoid redundant verification on rapid successive submits.
///
/// For fresh attestation fetches, a nonce is computed from the current chain
/// state and included in the request to prevent replay attacks.
async fn verify_enclave_attestation(
    config: &Config,
    client: &enclave::EnclaveClient,
) -> anyhow::Result<()> {
    let cache = ATTESTATION_CACHE.get_or_init(|| Mutex::new(None));
    let mut cached = cache.lock().unwrap();

    // Check if we have a valid cached attestation
    if let Some(ref c) = *cached {
        if c.fetched_at.elapsed().as_secs() < config.attestation_cache_ttl {
            tracing::debug!(
                "Using cached attestation (age={}s)",
                c.fetched_at.elapsed().as_secs()
            );
            return Ok(());
        }
    }

    // Compute nonce from chain state for replay prevention
    let nonce_hex = {
        let provider = ProviderBuilder::new().connect_http(config.rpc_url.parse()?);
        let chain_id = provider.get_chain_id().await?;
        let block_number = provider.get_block_number().await?;

        // Get enclave address from enclave info
        let info = client.info().await?;
        let nonce = compute_attestation_nonce(chain_id, block_number, &info.enclave_address);
        tracing::debug!(
            "Computed attestation nonce: {} (chainId={}, block={})",
            nonce,
            chain_id,
            block_number
        );
        nonce
    };

    // Fetch attestation with nonce for freshness binding
    tracing::info!("Fetching attestation for verification...");
    let att = client.attestation(Some(&nonce_hex)).await?;
    let verified = nitro::verify_attestation(&att.document)?;

    // Verify nonce matches what we sent (replay prevention)
    nitro::validate_nonce(&verified, &nonce_hex)?;

    if let Some(ref expected) = config.expected_pcr0 {
        nitro::validate_pcr0(&verified, expected)?;
    }
    nitro::validate_freshness(&verified, 600)?;

    tracing::info!(
        "Enclave attestation verified (cert_chain={}, nonce_verified=true)",
        verified.cert_chain_verified
    );

    *cached = Some(CachedAttestation {
        verified,
        fetched_at: Instant::now(),
    });

    Ok(())
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

    // 2a. Verify enclave attestation if nitro verification is enabled
    if config.nitro_verification {
        verify_enclave_attestation(config, &enclave_client).await?;
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

async fn cmd_register(
    config: &Config,
    expected_pcr0: Option<&str>,
    skip_verify: bool,
) -> anyhow::Result<()> {
    let enclave_client = enclave::EnclaveClient::new(&config.enclave_url);

    // 1. Get enclave address first (needed for nonce computation)
    let info = enclave_client.info().await?;

    // 2. Generate chain-bound nonce for replay prevention
    //    nonce = keccak256(chainId || blockNumber || enclaveAddress)
    let provider = ProviderBuilder::new().connect_http(
        config
            .rpc_url
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid RPC URL: {}", e))?,
    );
    let chain_id: u64 = provider.get_chain_id().await?;
    let block_number: u64 = provider.get_block_number().await?;

    let nonce_hex =
        compute_attestation_nonce(chain_id, block_number, &info.enclave_address);

    tracing::info!(
        "Generated nonce from chain_id={}, block={}, enclave={}: {}...",
        chain_id,
        block_number,
        &info.enclave_address,
        &nonce_hex[..16]
    );

    // 2. Fetch attestation document with nonce
    tracing::info!("Fetching attestation from {}...", config.enclave_url);
    let att_resp = enclave_client.attestation(Some(&nonce_hex)).await?;

    tracing::info!(
        "Attestation received: is_nitro={}, pcr0={}",
        att_resp.is_nitro,
        att_resp.pcr0
    );

    // 3. Verify attestation (unless skip_verify)
    let verified = if skip_verify {
        tracing::warn!("Skipping attestation verification (dev mode)");
        nitro::parse_attestation(&att_resp.document)?
    } else {
        let verified = nitro::verify_attestation(&att_resp.document)?;

        // Validate nonce matches what we sent
        nitro::validate_nonce(&verified, &nonce_hex)?;
        tracing::info!("Attestation nonce validated");

        // Validate PCR0 if expected value provided
        if let Some(pcr0) = expected_pcr0 {
            nitro::validate_pcr0(&verified, pcr0)?;
            tracing::info!("PCR0 validation passed");
        }

        // Validate freshness (5 minutes)
        nitro::validate_freshness(&verified, 300)?;
        tracing::info!("Attestation freshness validated");

        verified
    };

    // 4. Parse enclave address
    let enclave_addr: Address = verified
        .enclave_address
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid enclave address: {}", e))?;

    // 5. Convert PCR0 to bytes32 (take first 32 bytes of the 48-byte PCR0)
    let pcr0_bytes =
        hex::decode(&verified.pcr0).map_err(|e| anyhow::anyhow!("Invalid PCR0 hex: {}", e))?;
    let image_hash = if pcr0_bytes.len() >= 32 {
        B256::from_slice(&pcr0_bytes[..32])
    } else {
        let mut padded = [0u8; 32];
        padded[..pcr0_bytes.len()].copy_from_slice(&pcr0_bytes);
        B256::from(padded)
    };

    // 6. Register on-chain
    let chain_client = chain::ChainClient::new(
        &config.rpc_url,
        &config.private_key,
        &config.tee_verifier_address,
    )?;

    let tx_hash = chain_client
        .register_enclave(enclave_addr, image_hash)
        .await?;
    tracing::info!("Enclave registered on-chain: tx={}", tx_hash);

    println!("Enclave registered:");
    println!("  address:    {}", enclave_addr);
    println!("  pcr0:       0x{}", verified.pcr0);
    println!("  image_hash: 0x{}", hex::encode(image_hash));
    println!("  tx:         0x{}", hex::encode(tx_hash));

    Ok(())
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

    #[test]
    fn test_compute_attestation_nonce() {
        let nonce = compute_attestation_nonce(1, 12345, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        // Should be a 64-char hex string (SHA-256 = 32 bytes)
        assert_eq!(nonce.len(), 64);

        // Same inputs should produce same nonce
        let nonce2 = compute_attestation_nonce(1, 12345, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert_eq!(nonce, nonce2);

        // Different inputs should produce different nonce
        let nonce3 = compute_attestation_nonce(1, 12346, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert_ne!(nonce, nonce3);
    }
}
