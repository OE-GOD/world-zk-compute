//! World ZK Compute - Prover Node
//!
//! A prover node that:
//! 1. Monitors World Chain for execution requests
//! 2. Claims matching requests
//! 3. Fetches inputs and runs zkVM
//! 4. Submits proofs and collects rewards

use alloy::{
    primitives::{Address, B256, U256},
    providers::ProviderBuilder,
    signers::local::PrivateKeySigner,
    network::EthereumWallet,
};
use clap::{Parser, Subcommand};
use std::sync::Arc;
use tracing::{info, error};

mod bonsai;
mod config;
mod contracts;
mod prover;
mod monitor;

use bonsai::ProvingMode;
use config::ProverConfig;

#[derive(Parser)]
#[command(name = "world-zk-prover")]
#[command(about = "Prover node for World ZK Compute")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the prover node
    Run {
        /// RPC URL for World Chain
        #[arg(long, env = "RPC_URL")]
        rpc_url: String,

        /// Private key for signing transactions
        #[arg(long, env = "PRIVATE_KEY")]
        private_key: String,

        /// ExecutionEngine contract address
        #[arg(long, env = "ENGINE_ADDRESS")]
        engine_address: String,

        /// Minimum tip to accept (in ETH)
        #[arg(long, default_value = "0.0001")]
        min_tip: f64,

        /// Program image IDs to accept (comma-separated, empty = all)
        #[arg(long, default_value = "")]
        image_ids: String,

        /// Proving mode: local, bonsai, or bonsai-fallback
        #[arg(long, env = "PROVING_MODE", default_value = "local")]
        proving_mode: String,
    },

    /// Check status of a specific request
    Status {
        #[arg(long, env = "RPC_URL")]
        rpc_url: String,

        #[arg(long, env = "ENGINE_ADDRESS")]
        engine_address: String,

        /// Request ID to check
        #[arg(long)]
        request_id: u64,
    },

    /// List pending requests
    ListPending {
        #[arg(long, env = "RPC_URL")]
        rpc_url: String,

        #[arg(long, env = "ENGINE_ADDRESS")]
        engine_address: String,

        /// Maximum number to list
        #[arg(long, default_value = "10")]
        limit: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("world_zk_prover=info".parse()?)
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            rpc_url,
            private_key,
            engine_address,
            min_tip,
            image_ids,
            proving_mode,
        } => {
            run_prover(rpc_url, private_key, engine_address, min_tip, image_ids, proving_mode).await?;
        }

        Commands::Status {
            rpc_url,
            engine_address,
            request_id,
        } => {
            check_status(rpc_url, engine_address, request_id).await?;
        }

        Commands::ListPending {
            rpc_url,
            engine_address,
            limit,
        } => {
            list_pending(rpc_url, engine_address, limit).await?;
        }
    }

    Ok(())
}

async fn run_prover(
    rpc_url: String,
    private_key: String,
    engine_address: String,
    min_tip: f64,
    image_ids: String,
    proving_mode: String,
) -> anyhow::Result<()> {
    // Parse proving mode
    let mode = ProvingMode::from_str(&proving_mode);

    info!("Starting World ZK Compute Prover Node");
    info!("RPC: {}", rpc_url);
    info!("Engine: {}", engine_address);
    info!("Min tip: {} ETH", min_tip);
    info!("Proving mode: {:?}", mode);

    // Parse allowed image IDs
    let allowed_images: Vec<B256> = if image_ids.is_empty() {
        vec![] // Accept all
    } else {
        image_ids
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let bytes = hex::decode(s.trim_start_matches("0x"))
                    .expect("Invalid image ID hex");
                B256::from_slice(&bytes)
            })
            .collect()
    };

    if allowed_images.is_empty() {
        info!("Accepting all program image IDs");
    } else {
        info!("Accepting {} specific image IDs", allowed_images.len());
    }

    // Build provider with signer
    let signer: PrivateKeySigner = private_key.parse()?;
    let wallet_address = signer.address();
    info!("Prover wallet: {}", wallet_address);

    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new()
        .filler(alloy::providers::fillers::GasFiller)
        .filler(alloy::providers::fillers::BlobGasFiller)
        .filler(alloy::providers::fillers::NonceFiller::default())
        .filler(alloy::providers::fillers::ChainIdFiller::default())
        .wallet(wallet)
        .on_http(rpc_url.parse()?);

    let provider = Arc::new(provider);

    // Parse engine address
    let engine: Address = engine_address.parse()?;

    // Convert min tip to wei
    let min_tip_wei = U256::from((min_tip * 1e18) as u128);

    // Create prover config
    let config = ProverConfig {
        engine_address: engine,
        min_tip_wei,
        allowed_image_ids: allowed_images,
        poll_interval_secs: 5,
        proving_mode: mode,
        bonsai_config: bonsai::BonsaiConfig::from_env().ok(),
    };

    info!("Prover node ready. Monitoring for execution requests...");

    // Main loop
    loop {
        match monitor::check_and_process_requests(&provider, &config).await {
            Ok(processed) => {
                if processed > 0 {
                    info!("Processed {} requests", processed);
                }
            }
            Err(e) => {
                error!("Error processing requests: {:?}", e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(config.poll_interval_secs)).await;
    }
}

async fn check_status(
    rpc_url: String,
    engine_address: String,
    request_id: u64,
) -> anyhow::Result<()> {
    let _provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let _engine: Address = engine_address.parse()?;

    // TODO: Call getRequest on the contract
    info!("Checking status of request {}...", request_id);

    // This would call the contract - simplified for now
    println!("Request ID: {}", request_id);
    println!("Status: (would query contract)");

    Ok(())
}

async fn list_pending(
    rpc_url: String,
    engine_address: String,
    limit: u64,
) -> anyhow::Result<()> {
    let _provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let _engine: Address = engine_address.parse()?;

    info!("Listing pending requests (limit: {})...", limit);

    // TODO: Call getPendingRequests on the contract
    println!("Pending requests: (would query contract)");

    Ok(())
}
