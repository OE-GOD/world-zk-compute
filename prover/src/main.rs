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
use tracing::{debug, info, error};

mod api;
mod bonsai;
mod cache;
mod cluster;
mod config;
mod config_file;
mod continuations;
mod contracts;
mod events;
mod fast_prove;
mod gpu_optimize;
mod health;
mod http;
mod ipfs;
mod logging;
mod metrics;
mod monitor;
mod nonce;
mod p2p;
mod parallel;
mod prefetch;
mod proof_cache;
mod prover;
mod queue;
mod ratelimit;
mod recovery;
mod retry;
mod shutdown;
mod snark;
mod validation;
mod memory_pool;
mod connection_pool;
mod proof_compression;
mod batch_tx;
mod alerting;
mod limits;
mod aggregation;
mod tracing_ext;
mod errors;
mod redis_queue;
mod scaling;
mod sharding;
mod circuit_breaker;
mod health_checks;
mod degradation;

use bonsai::ProvingMode;
use config::ProverConfig;

#[derive(Parser)]
#[command(name = "world-zk-prover")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Prover node for World ZK Compute - Earn rewards by generating zero-knowledge proofs")]
#[command(long_about = r#"
World ZK Compute Prover Node

This prover monitors the World Chain for execution requests, claims jobs,
generates zero-knowledge proofs using RISC Zero zkVM, and submits results
to earn rewards.

QUICK START:
  1. Set environment variables:
     export PRIVATE_KEY="0x..."
     export RPC_URL="https://worldchain-mainnet.g.alchemy.com/v2/..."
     export ENGINE_ADDRESS="0x..."

  2. Run the prover:
     world-zk-prover run

CONFIGURATION:
  Use --help with any subcommand for detailed options.
  Generate a sample config file with: world-zk-prover config --generate

PROVING MODES:
  - local:          CPU proving (slow, no cost)
  - gpu:            GPU proving (fast, requires CUDA/Metal)
  - gpu-fallback:   Try GPU, fall back to CPU
  - bonsai:         Cloud proving (fastest, requires API key)
  - bonsai-fallback: Try Bonsai, fall back to local

For more information: https://github.com/worldcoin/world-zk-compute
"#)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the prover node
    Run {
        /// RPC URL for World Chain (HTTP)
        #[arg(long, env = "RPC_URL")]
        rpc_url: String,

        /// WebSocket URL for event subscription (optional, derived from RPC URL if not set)
        /// Set to "none" to disable event subscription and use polling only
        #[arg(long, env = "WS_URL")]
        ws_url: Option<String>,

        /// Private key for signing transactions
        #[arg(long, env = "PRIVATE_KEY")]
        private_key: String,

        /// ExecutionEngine contract address
        #[arg(long, env = "ENGINE_ADDRESS")]
        engine_address: String,

        /// ProgramRegistry contract address (optional, for on-chain program lookup)
        #[arg(long, env = "REGISTRY_ADDRESS")]
        registry_address: Option<String>,

        /// Minimum tip to accept (in ETH)
        #[arg(long, default_value = "0.0001")]
        min_tip: f64,

        /// Program image IDs to accept (comma-separated, empty = all)
        #[arg(long, default_value = "")]
        image_ids: String,

        /// Proving mode:
        /// - local: CPU proving (slow but free)
        /// - gpu: Local GPU proving (CUDA/Metal, requires --features cuda or metal)
        /// - gpu-fallback: Try GPU first, fall back to CPU
        /// - bonsai: Cloud proving (fast, requires BONSAI_API_KEY)
        /// - bonsai-fallback: Try Bonsai first, fall back to CPU
        /// - bonsai-gpu: Try Bonsai first, fall back to GPU, then CPU
        #[arg(long, env = "PROVING_MODE", default_value = "gpu-fallback")]
        proving_mode: String,

        /// Maximum concurrent proofs (parallel processing)
        #[arg(long, env = "MAX_CONCURRENT", default_value = "4")]
        max_concurrent: usize,

        /// Convert STARK proofs to SNARKs (smaller, cheaper on-chain)
        #[arg(long, env = "USE_SNARK")]
        use_snark: bool,

        /// Memory cache size in MB for program ELFs
        #[arg(long, env = "CACHE_SIZE_MB", default_value = "256")]
        cache_size_mb: usize,

        /// Health check server port (0 to disable)
        #[arg(long, env = "HEALTH_PORT", default_value = "8081")]
        health_port: u16,

        /// Maximum job queue size
        #[arg(long, env = "QUEUE_SIZE", default_value = "1000")]
        queue_size: usize,

        /// Minimum profit margin (0.0 - 1.0, e.g., 0.2 = 20% profit required)
        #[arg(long, env = "MIN_PROFIT_MARGIN", default_value = "0.2")]
        min_profit_margin: f64,

        /// Skip profitability check (accept all jobs regardless of gas cost)
        #[arg(long, env = "SKIP_PROFITABILITY_CHECK")]
        skip_profitability_check: bool,
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

    /// Configuration management
    Config {
        /// Generate a sample configuration file
        #[arg(long)]
        generate: bool,

        /// Output path for generated config
        #[arg(long, default_value = "prover.toml")]
        output: String,

        /// Validate an existing configuration file
        #[arg(long)]
        validate: Option<String>,
    },

    /// Show system information and capabilities
    Info,

    /// Validate inputs (for testing/debugging)
    Validate {
        /// Address to validate
        #[arg(long)]
        address: Option<String>,

        /// Image ID to validate
        #[arg(long)]
        image_id: Option<String>,

        /// URL to validate
        #[arg(long)]
        url: Option<String>,
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
            ws_url,
            private_key,
            engine_address,
            registry_address,
            min_tip,
            image_ids,
            proving_mode,
            max_concurrent,
            use_snark,
            cache_size_mb,
            health_port,
            queue_size,
            min_profit_margin,
            skip_profitability_check,
        } => {
            run_prover(
                rpc_url,
                ws_url,
                private_key,
                engine_address,
                registry_address,
                min_tip,
                image_ids,
                proving_mode,
                max_concurrent,
                use_snark,
                cache_size_mb,
                health_port,
                queue_size,
                min_profit_margin,
                skip_profitability_check,
            ).await?;
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

        Commands::Config {
            generate,
            output,
            validate,
        } => {
            handle_config(generate, output, validate)?;
        }

        Commands::Info => {
            show_info();
        }

        Commands::Validate {
            address,
            image_id,
            url,
        } => {
            validate_inputs(address, image_id, url)?;
        }
    }

    Ok(())
}

async fn run_prover(
    rpc_url: String,
    ws_url: Option<String>,
    private_key: String,
    engine_address: String,
    registry_address: Option<String>,
    min_tip: f64,
    image_ids: String,
    proving_mode: String,
    max_concurrent: usize,
    use_snark: bool,
    cache_size_mb: usize,
    health_port: u16,
    queue_size: usize,
    min_profit_margin: f64,
    skip_profitability_check: bool,
) -> anyhow::Result<()> {
    // Parse proving mode
    let mode = ProvingMode::from_str(&proving_mode);

    // Detect GPU backend
    let gpu_backend = gpu_optimize::GpuBackend::detect();
    let gpu_status = if gpu_backend.is_gpu() {
        format!("{} (GPU detected)", gpu_backend)
    } else {
        "CPU only".to_string()
    };

    // Determine WebSocket URL
    let effective_ws_url = match ws_url.as_deref() {
        Some("none") | Some("disabled") => None,
        Some(url) => Some(url.to_string()),
        None => Some(rpc_url.replace("https://", "wss://").replace("http://", "ws://")),
    };

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║       World ZK Compute - Optimized Prover Node               ║");
    info!("╚══════════════════════════════════════════════════════════════╝");
    info!("");
    info!("Configuration:");
    info!("  RPC URL:        {}", rpc_url);
    info!("  WS URL:         {}", effective_ws_url.as_deref().unwrap_or("disabled (polling only)"));
    info!("  Engine:         {}", engine_address);
    info!("  Registry:       {}", registry_address.as_deref().unwrap_or("not configured (local only)"));
    info!("  Min tip:        {} ETH", min_tip);
    info!("  Proving mode:   {:?}", mode);
    info!("  GPU backend:    {}", gpu_status);
    info!("  Max concurrent: {}", max_concurrent);
    info!("  SNARK:          {}", if use_snark { "enabled" } else { "disabled" });
    info!("  Cache size:     {} MB", cache_size_mb);
    info!("  Queue size:     {}", queue_size);
    info!("  Health port:    {}", if health_port > 0 { health_port.to_string() } else { "disabled".to_string() });
    info!("  Profit margin:  {:.0}%{}", min_profit_margin * 100.0,
        if skip_profitability_check { " (DISABLED)" } else { "" });
    info!("");

    // Convert min tip to wei
    let min_tip_wei = U256::from((min_tip * 1e18) as u128);

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

    let wallet = EthereumWallet::from(signer.clone());
    // Note: We remove NonceFiller and manage nonces ourselves for parallel safety
    let provider = ProviderBuilder::new()
        .filler(alloy::providers::fillers::GasFiller)
        .filler(alloy::providers::fillers::BlobGasFiller)
        .filler(alloy::providers::fillers::ChainIdFiller::default())
        .wallet(wallet)
        .on_http(rpc_url.parse()?);

    let provider = Arc::new(provider);

    // Parse engine address
    let engine: Address = engine_address.parse()?;

    // Initialize nonce manager for parallel transaction safety
    let nonce_manager = Arc::new(
        nonce::NonceManager::new(provider.clone(), signer.address()).await?
    );
    info!("✓ Nonce manager initialized (current nonce: {})", nonce_manager.current());

    // Parse registry address if provided
    let registry: Option<Address> = registry_address
        .as_ref()
        .map(|s| s.parse())
        .transpose()?;

    // Create prover config
    let config = ProverConfig {
        engine_address: engine,
        registry_address: registry,
        min_tip_wei,
        allowed_image_ids: allowed_images.clone(),
        poll_interval_secs: 5,
        proving_mode: mode.clone(),
        bonsai_config: bonsai::BonsaiConfig::from_env().ok(),
        min_profit_margin,
        skip_profitability_check,
    };

    // ════════════════════════════════════════════════════════════════════
    // Initialize OptimizedProcessor (wires everything together!)
    // ════════════════════════════════════════════════════════════════════
    info!("Initializing optimized processor...");
    let processor = monitor::OptimizedProcessor::new(
        max_concurrent,
        min_tip_wei,
        cache_size_mb,
        mode.clone(),
    )?;
    info!("✓ Optimized processor ready");
    info!("  - Parallel processing: {} concurrent jobs", max_concurrent);
    info!("  - Job queue: priority-based ordering");
    info!("  - Program cache: {} MB (memory + disk)", cache_size_mb);
    info!("  - IPFS: multi-gateway (4 fallbacks)");
    info!("  - Fast prover: preflight + strategy selection");
    info!("  - Metrics: tracking all operations");

    // ════════════════════════════════════════════════════════════════════
    // Start Event Subscription (if WebSocket URL provided)
    // ════════════════════════════════════════════════════════════════════
    // Create a notify to wake up main loop when events arrive
    let event_notify = Arc::new(tokio::sync::Notify::new());
    let event_notify_clone = event_notify.clone();

    let _event_handle = if let Some(ws_url) = effective_ws_url.clone() {
        info!("Starting event subscription...");

        // Create event channel
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<events::NewJobEvent>(100);

        // Create event subscriber config
        let event_config = events::EventConfig {
            ws_url: ws_url.clone(),
            engine_address: engine,
            reconnect_delay: std::time::Duration::from_secs(1),
            max_reconnect_delay: std::time::Duration::from_secs(60),
            min_tip: min_tip_wei,
        };

        // Start event subscriber in background
        let subscriber = events::EventSubscriber::new(
            event_config,
            event_tx,
            allowed_images.clone(),
        );

        let subscriber_handle = tokio::spawn(async move {
            subscriber.run().await;
        });

        // Start event processor that notifies main loop
        let notify = event_notify_clone;
        let event_processor_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                info!(
                    "Event received: request_id={}, image={}, tip={} wei (instant!)",
                    event.request_id, event.image_id, event.tip
                );
                // Wake up the main loop immediately
                notify.notify_one();
            }
        });

        info!("✓ Event subscription: enabled (instant job detection)");
        Some((subscriber_handle, event_processor_handle))
    } else {
        info!("Event subscription: disabled (using polling only)");
        None
    };

    // Start health server if enabled
    let health_state = if health_port > 0 {
        let state = health::SharedState::new(
            wallet_address.to_string(),
            engine_address.clone(),
        );
        state.set_running(true).await;

        let health_server = health::HealthServer::new(health_port, state.clone());
        let _health_handle = health_server.start().await;
        info!("✓ Health server: http://0.0.0.0:{}", health_port);
        info!("  - GET /health  - liveness check");
        info!("  - GET /metrics - Prometheus metrics");
        info!("  - GET /status  - detailed status");
        Some(state)
    } else {
        None
    };

    info!("");
    info!("═══════════════════════════════════════════════════════════════");
    info!("  Prover node ready. Monitoring for execution requests...");
    info!("═══════════════════════════════════════════════════════════════");
    info!("");

    // Metrics logging interval
    let mut metrics_counter = 0u64;
    let metrics_interval = 12; // Log metrics every 12 iterations (60 seconds at 5s poll)

    // Main loop with instant event notification
    loop {
        match processor.check_and_process(&provider, &config, &nonce_manager).await {
            Ok(processed) => {
                if processed > 0 {
                    info!("✓ Processed {} requests", processed);

                    // Update health state
                    if let Some(ref state) = health_state {
                        state.update_active_proofs(
                            metrics::metrics().snapshot().active_proofs
                        ).await;
                    }
                }
            }
            Err(e) => {
                error!("✗ Error processing requests: {:?}", e);

                // Update health state with error
                if let Some(ref state) = health_state {
                    state.set_error(format!("{:?}", e)).await;
                }
            }
        }

        // Periodic metrics logging
        metrics_counter += 1;
        if metrics_counter % metrics_interval == 0 {
            let snapshot = metrics::metrics().snapshot();
            if snapshot.proofs_generated > 0 || snapshot.proofs_failed > 0 {
                info!("📊 Metrics: {} proofs ({:.1}% success), {:.1}/hr, avg {:?}",
                    snapshot.proofs_generated + snapshot.proofs_failed,
                    snapshot.success_rate(),
                    snapshot.proofs_per_hour(),
                    snapshot.avg_proof_time
                );
            }
        }

        // Wait for either:
        // 1. Event notification (instant wakeup when new job arrives)
        // 2. Poll interval timeout (fallback for any missed events)
        tokio::select! {
            _ = event_notify.notified() => {
                debug!("Woke up from event notification (instant!)");
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(config.poll_interval_secs)) => {
                debug!("Woke up from poll interval");
            }
        }
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

fn handle_config(generate: bool, output: String, validate: Option<String>) -> anyhow::Result<()> {
    if generate {
        let sample = config_file::Config::sample();
        std::fs::write(&output, &sample)?;
        println!("Generated sample configuration: {}", output);
        println!("\nEdit the file and set required values:");
        println!("  - prover.private_key (or use PRIVATE_KEY env var)");
        println!("  - prover.rpc_url");
        println!("  - prover.contract_address");
        return Ok(());
    }

    if let Some(path) = validate {
        match config_file::Config::from_file(&path) {
            Ok(config) => {
                match config.validate() {
                    Ok(()) => {
                        println!("✓ Configuration is valid: {}", path);
                        println!("\nSettings:");
                        println!("  RPC URL:     {}", config.prover.rpc_url);
                        println!("  Chain ID:    {}", config.prover.chain_id);
                        println!("  Proving:     {}", config.proving.mode);
                        println!("  Concurrent:  {}", config.proving.max_concurrent);
                        println!("  API enabled: {}", config.api.enabled);
                    }
                    Err(e) => {
                        println!("✗ Configuration validation failed: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                println!("✗ Failed to parse configuration: {}", e);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    println!("Usage:");
    println!("  world-zk-prover config --generate           Generate sample config");
    println!("  world-zk-prover config --validate <path>    Validate config file");
    Ok(())
}

fn show_info() {
    let gpu_backend = gpu_optimize::GpuBackend::detect();

    println!("World ZK Compute Prover");
    println!("=======================");
    println!();
    println!("Version:  {}", env!("CARGO_PKG_VERSION"));
    println!("Platform: {}", std::env::consts::OS);
    println!("Arch:     {}", std::env::consts::ARCH);
    println!();
    println!("Hardware:");
    println!("  CPU cores:  {}", num_cpus());
    println!("  GPU:        {}", gpu_backend);
    println!();
    println!("Capabilities:");
    println!("  RISC Zero:  zkVM 1.2.x");

    #[cfg(feature = "cuda")]
    println!("  CUDA:       enabled");
    #[cfg(not(feature = "cuda"))]
    println!("  CUDA:       disabled (build with --features cuda)");

    #[cfg(feature = "metal")]
    println!("  Metal:      enabled");
    #[cfg(not(feature = "metal"))]
    println!("  Metal:      disabled (build with --features metal)");

    if std::env::var("BONSAI_API_KEY").is_ok() {
        println!("  Bonsai:     API key configured");
    } else {
        println!("  Bonsai:     not configured (set BONSAI_API_KEY)");
    }
    println!();
    println!("Environment:");
    if std::env::var("PRIVATE_KEY").is_ok() {
        println!("  PRIVATE_KEY:    set");
    } else {
        println!("  PRIVATE_KEY:    not set");
    }
    if let Ok(url) = std::env::var("RPC_URL") {
        println!("  RPC_URL:        {}", mask_url(&url));
    } else {
        println!("  RPC_URL:        not set");
    }
    if std::env::var("ENGINE_ADDRESS").is_ok() {
        println!("  ENGINE_ADDRESS: set");
    } else {
        println!("  ENGINE_ADDRESS: not set");
    }
}

fn validate_inputs(
    address: Option<String>,
    image_id: Option<String>,
    url: Option<String>,
) -> anyhow::Result<()> {
    let validator = validation::Validator::new();
    let mut any_validated = false;

    if let Some(addr) = address {
        any_validated = true;
        match validator.validate_address(&addr) {
            Ok(normalized) => println!("✓ Address valid: {}", normalized),
            Err(e) => println!("✗ Address invalid: {}", e),
        }
    }

    if let Some(id) = image_id {
        any_validated = true;
        match validator.validate_image_id(&id) {
            Ok(normalized) => println!("✓ Image ID valid: {}", normalized),
            Err(e) => println!("✗ Image ID invalid: {}", e),
        }
    }

    if let Some(u) = url {
        any_validated = true;
        match validator.validate_url(&u) {
            Ok(validated) => println!("✓ URL valid: {}", validated),
            Err(e) => println!("✗ URL invalid: {}", e),
        }
    }

    if !any_validated {
        println!("Usage:");
        println!("  world-zk-prover validate --address 0x...");
        println!("  world-zk-prover validate --image-id 0x...");
        println!("  world-zk-prover validate --url https://...");
    }

    Ok(())
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
}

fn mask_url(url: &str) -> String {
    // Mask API keys in URLs
    if let Some(pos) = url.find("/v2/") {
        let prefix = &url[..pos + 4];
        format!("{}[MASKED]", prefix)
    } else {
        url.to_string()
    }
}
