mod chain;
mod config;
pub mod deadline_monitor;
mod enclave;
mod metrics;
mod nitro;
pub mod notifications;
mod prover;
mod store;
mod tracing_setup;
mod watcher;

use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tracing::Instrument;

use config::{Config, ModelConfig};
use prover::ProofManager;
use store::StateStore;
use tee_operator::alerting::{AlertConfig, AlertManager, AlertSeverity};
use tee_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
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
    /// Path to a TOML config file (optional). Values from the file are
    /// used as defaults; environment variables always take precedence.
    #[arg(long, global = true)]
    config: Option<String>,

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
        /// Name of the model to use (from [[models]] config). Defaults to first model.
        #[arg(long)]
        model: Option<String>,
    },
    /// Watch chain events and auto-resolve disputes
    Watch {
        /// Port for the health/metrics HTTP server
        #[arg(long, default_value = "9090")]
        metrics_port: u16,
    },
    /// Combined: submit + watch + prove
    Run {
        /// Feature vector as JSON array
        #[arg(long)]
        features: String,
        /// Port for the health/metrics HTTP server
        #[arg(long, default_value = "9090")]
        metrics_port: u16,
        /// Name of the model to use (from [[models]] config). Defaults to first model.
        #[arg(long)]
        model: Option<String>,
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
    /// List all registered models from the configuration
    Models,
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
    tracing_setup::init_tracing("worldzk-operator", None);
    let cli = Cli::parse();
    let config = Config::from_env(cli.config.as_deref())?;

    match cli.command {
        Commands::Submit { features, model } => {
            let selected = config.get_model(model.as_deref())?;
            cmd_submit(&config, &features, selected).await
        }
        Commands::Watch { metrics_port } => cmd_watch(&config, metrics_port).await,
        Commands::Run {
            features,
            metrics_port,
            model,
        } => {
            let selected = config.get_model(model.as_deref())?;
            cmd_run(&config, &features, metrics_port, selected).await
        }
        Commands::Register {
            expected_pcr0,
            skip_verify,
        } => cmd_register(&config, expected_pcr0.as_deref(), skip_verify).await,
        Commands::Models => cmd_models(&config),
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

    // Check cache (drop guard before any async work)
    {
        let cached = cache
            .lock()
            .map_err(|e| anyhow::anyhow!("attestation cache lock poisoned: {e}"))?;
        if let Some(ref c) = *cached {
            if c.fetched_at.elapsed().as_secs() < config.attestation_cache_ttl {
                tracing::debug!(
                    "Using cached attestation (age={}s)",
                    c.fetched_at.elapsed().as_secs()
                );
                return Ok(());
            }
        }
    }

    // Compute nonce from chain state for replay prevention
    let provider = ProviderBuilder::new().connect_http(config.rpc_url.parse()?);
    let chain_id = provider.get_chain_id().await?;
    let block_number = provider.get_block_number().await?;

    let info = client.info().await?;
    let nonce_hex = compute_attestation_nonce(chain_id, block_number, &info.enclave_address);
    tracing::debug!(
        "Computed attestation nonce: {} (chainId={}, block={})",
        nonce_hex,
        chain_id,
        block_number
    );

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

    // Re-acquire lock to update cache
    {
        let mut cached = cache
            .lock()
            .map_err(|e| anyhow::anyhow!("attestation cache lock poisoned: {e}"))?;
        *cached = Some(CachedAttestation {
            verified,
            fetched_at: Instant::now(),
        });
    }

    Ok(())
}

#[tracing::instrument(skip(config, features_json, model))]
async fn cmd_submit(
    config: &Config,
    features_json: &str,
    model: &ModelConfig,
) -> anyhow::Result<()> {
    // 1. Parse features
    let feats: Vec<f64> = serde_json::from_str(features_json)
        .map_err(|e| anyhow::anyhow!("Invalid features JSON: {}", e))?;
    tracing::info!(
        model_name = %model.name,
        model_path = %model.path,
        "Submitting inference for {} features using model '{}'",
        feats.len(),
        model.name
    );

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

    // 4. Trigger proof pre-computation (best-effort, uses warm prover if PROVER_URL is set)
    let proof_mgr = ProofManager::new(
        &config.precompute_bin,
        &model.path,
        &config.proofs_dir,
        config.max_proof_retries,
        config.proof_retry_delay_secs,
    );
    let result_id = format!("0x{}", hex::encode(tx_hash));
    let features_owned = features_json.to_string();
    tokio::spawn(async move {
        if let Err(e) = proof_mgr.generate_proof(&result_id, &features_owned).await {
            tracing::warn!("Proof pre-computation failed (best-effort): {}", e);
        }
    });

    println!("tx_hash={}", tx_hash);
    Ok(())
}

/// Print all registered models from the configuration.
fn cmd_models(config: &Config) -> anyhow::Result<()> {
    if config.models.is_empty() {
        println!("No models configured.");
        return Ok(());
    }

    println!("Registered models ({}):", config.models.len());
    println!("{:<20} {:<50} {:<12} HASH", "NAME", "PATH", "FORMAT");
    println!("{}", "-".repeat(100));
    for model in &config.models {
        let hash_display = model.model_hash.as_deref().unwrap_or("-");
        println!(
            "{:<20} {:<50} {:<12} {}",
            model.name, model.path, model.model_format, hash_display
        );
    }
    Ok(())
}

/// Shared shutdown state for graceful termination.
struct ShutdownState {
    shutting_down: AtomicBool,
    in_flight_tasks: AtomicU64,
}

impl ShutdownState {
    fn new() -> Self {
        Self {
            shutting_down: AtomicBool::new(false),
            in_flight_tasks: AtomicU64::new(0),
        }
    }

    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Relaxed)
    }

    fn signal_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
    }

    fn track_task_start(&self) {
        self.in_flight_tasks.fetch_add(1, Ordering::Relaxed);
    }

    fn track_task_done(&self) {
        self.in_flight_tasks.fetch_sub(1, Ordering::Relaxed);
    }

    fn in_flight_count(&self) -> u64 {
        self.in_flight_tasks.load(Ordering::Relaxed)
    }
}

/// Wait for shutdown signal (SIGINT or SIGTERM).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            tracing::warn!("failed to install SIGTERM handler, using ctrl-c only");
            ctrl_c.await.ok();
            return;
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}

#[tracing::instrument(skip(config))]
async fn cmd_watch(config: &Config, metrics_port: u16) -> anyhow::Result<()> {
    let contract_addr: Address = config
        .tee_verifier_address
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid contract address: {}", e))?;
    let watcher = EventWatcher::new(&config.rpc_url, contract_addr);
    let chain_client = Arc::new(chain::ChainClient::new(
        &config.rpc_url,
        &config.private_key,
        &config.tee_verifier_address,
    )?);
    let proof_mgr = Arc::new(ProofManager::new(
        &config.precompute_bin,
        &config.model_path,
        &config.proofs_dir,
        config.max_proof_retries,
        config.proof_retry_delay_secs,
    ));

    // Initialize webhook notifier (None if WEBHOOK_URL is not set)
    let notifier = notifications::WebhookNotifier::from_optional(config.webhook_url.as_deref());
    if notifier.is_some() {
        tracing::info!("Webhook notifications enabled");
    } else {
        tracing::debug!("Webhook notifications disabled (no WEBHOOK_URL configured)");
    }

    // Initialize AlertManager for multi-channel alerting (log-only by default)
    let alert_config = match std::env::var("ALERT_CONFIG_JSON") {
        Ok(json) => serde_json::from_str::<AlertConfig>(&json).unwrap_or_else(|e| {
            tracing::warn!("Invalid ALERT_CONFIG_JSON, using defaults: {}", e);
            AlertConfig::default()
        }),
        Err(_) => AlertConfig::default(),
    };
    let alert_manager = Arc::new(AlertManager::new(alert_config));
    tracing::info!("AlertManager initialized");

    // Circuit breakers for RPC and chain calls
    let rpc_cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default()));
    let chain_cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 3,
        recovery_timeout: std::time::Duration::from_secs(60),
        success_threshold_for_close: 1,
    }));

    // Semaphore to limit concurrent proof submissions (T215)
    let proof_semaphore = Arc::new(tokio::sync::Semaphore::new(10));

    let shutdown = Arc::new(ShutdownState::new());

    // Spawn shutdown signal handler
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            tracing::info!("Shutdown signal received");
            shutdown.signal_shutdown();
        });
    }

    // Initialize metrics and spawn HTTP server
    let metrics_state = Arc::new(metrics::MetricsState::new());
    {
        let ms = metrics_state.clone();
        tokio::spawn(async move {
            metrics::serve_metrics(ms, metrics_port).await;
        });
    }

    // Load persistent state for crash recovery
    let state_store = StateStore::new(&config.state_file);
    let mut op_state = state_store.load_or_default();

    tracing::info!("Watching for events on {}...", config.tee_verifier_address);

    let mut from_block = op_state.last_polled_block;
    let mut finalize_counter = 0u64;

    while !shutdown.is_shutting_down() {
        // Poll for new events (through RPC circuit breaker)
        let (events, next_block) = if let Err(e) = rpc_cb.allow_request() {
            tracing::warn!("RPC circuit breaker: {}", e);
            (vec![], from_block)
        } else {
            match watcher.poll_events(from_block).await {
                Ok(r) => {
                    rpc_cb.record_success();
                    r
                }
                Err(e) => {
                    rpc_cb.record_failure();
                    tracing::warn!(
                        consecutive_failures = rpc_cb.consecutive_failures(),
                        "RPC poll failed: {}",
                        e
                    );
                    (vec![], from_block)
                }
            }
        };

        let _watch_span = tracing_setup::span_watch_cycle(from_block, next_block, 0);
        let _watch_guard = _watch_span.enter();

        if !events.is_empty() {
            tracing::info!(
                block_number = from_block,
                event_count = events.len(),
                "Polled events"
            );
        }

        from_block = next_block;

        // Update last polled block
        metrics_state.set_last_block(from_block);

        for event in &events {
            if shutdown.is_shutting_down() {
                tracing::info!("Shutdown in progress, skipping remaining events");
                break;
            }
            match event {
                TEEEvent::ResultChallenged {
                    result_id,
                    challenger,
                } => {
                    let rid_hex = format!("0x{}", hex::encode(result_id));

                    // Dedup: skip events already processed in a prior session
                    if op_state.processed_event_ids.contains(&rid_hex) {
                        tracing::debug!(
                            result_id = %result_id,
                            "Skipping already-processed challenge event"
                        );
                        continue;
                    }

                    metrics_state.record_challenge();
                    tracing::warn!(
                        result_id = %result_id,
                        challenger = %challenger,
                        "Challenge detected"
                    );

                    // Track the dispute deadline in persisted state
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let deadline = now + 86400; // 24h dispute window
                    op_state.active_disputes.insert(rid_hex.clone(), deadline);

                    // Fire-and-forget webhook notification
                    notifications::maybe_notify_challenge(
                        &notifier,
                        &rid_hex,
                        &format!("{}", challenger),
                        now,
                        deadline,
                    );

                    // Alert via AlertManager (multi-channel)
                    let mut meta = HashMap::new();
                    meta.insert("result_id".to_string(), rid_hex.clone());
                    meta.insert("challenger".to_string(), format!("{}", challenger));
                    if let Err(e) = alert_manager.send_alert(
                        AlertSeverity::Warning,
                        "challenge_detected",
                        "operator",
                        &format!("Challenge detected for result {}", rid_hex),
                        meta,
                    ) {
                        tracing::debug!("Alert suppressed or failed: {}", e);
                    }

                    // Spawn proof resolution as a separate task to avoid blocking
                    // the event loop (T215)
                    let dispute_span =
                        tracing_setup::span_dispute(&rid_hex, &format!("{}", challenger));
                    let permit = proof_semaphore.clone().try_acquire_owned();
                    match permit {
                        Ok(permit) => {
                            let chain = chain_client.clone();
                            let pm = proof_mgr.clone();
                            let rid = *result_id;
                            let am = alert_manager.clone();
                            let ms = metrics_state.clone();
                            let cb = chain_cb.clone();
                            shutdown.track_task_start();
                            let sd = shutdown.clone();
                            tokio::spawn(
                                async move {
                                    handle_challenge(&chain, &pm, rid, &am, &ms, &cb).await;
                                    sd.track_task_done();
                                    drop(permit);
                                }
                                .instrument(dispute_span),
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                "Max concurrent proof submissions reached, skipping (will retry on next poll): {}",
                                rid_hex
                            );
                            metrics_state.record_error();
                            // Don't mark as processed — will be retried on next poll
                            continue;
                        }
                    }

                    // Mark as processed after handling initiated
                    op_state.processed_event_ids.insert(rid_hex);
                }
                TEEEvent::ResultSubmitted { result_id, .. } => {
                    metrics_state.record_submission();
                    tracing::info!(result_id = %result_id, "New result submitted");
                }
                TEEEvent::ResultFinalized { result_id } => {
                    metrics_state.record_finalization();
                    // Remove from active disputes if present
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    op_state.active_disputes.remove(&rid_hex);
                    tracing::info!(result_id = %result_id, "Result finalized");
                }
                TEEEvent::ResultExpired { result_id } => {
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    op_state.active_disputes.remove(&rid_hex);
                    tracing::info!(result_id = %result_id, "Result expired (unchallenged finalize)");
                }
                TEEEvent::DisputeResolved {
                    result_id,
                    prover_won,
                } => {
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    op_state.active_disputes.remove(&rid_hex);
                    tracing::info!(
                        result_id = %result_id,
                        prover_won = %prover_won,
                        "Dispute resolved"
                    );
                }
            }
        }

        // Update active dispute gauge after processing all events
        metrics_state.set_active_disputes(op_state.active_disputes.len() as u64);

        // Sync webhook failure count into Prometheus metrics
        if let Some(ref n) = notifier {
            metrics_state.set_webhook_failures(n.notification_failures());
        }

        // Persist state after each poll cycle for crash recovery
        op_state.last_polled_block = from_block;
        if let Err(e) = state_store.save(&op_state) {
            tracing::warn!("Failed to persist watcher state: {}", e);
        }

        // Every ~60 seconds (12 iterations * 5s), check for finalizeable results
        finalize_counter += 1;
        if finalize_counter >= 12 {
            finalize_counter = 0;
            auto_finalize(
                &watcher,
                &chain_client,
                from_block.saturating_sub(7200),
                &rpc_cb,
                &chain_cb,
            )
            .await;
        }

        // Use select to allow shutdown to interrupt the sleep
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
            _ = async {
                while !shutdown.is_shutting_down() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            } => {
                break;
            },
        }
    }

    // Graceful shutdown: wait for in-flight tasks
    let in_flight = shutdown.in_flight_count();
    if in_flight > 0 {
        tracing::info!(
            in_flight_tasks = in_flight,
            "Waiting for in-flight tasks to complete (60s timeout)"
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        while shutdown.in_flight_count() > 0 && Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let remaining = shutdown.in_flight_count();
        if remaining > 0 {
            tracing::warn!(
                remaining_tasks = remaining,
                "Shutdown timeout — exiting with in-flight tasks"
            );
        }
    }

    // Final state save before shutdown
    op_state.last_polled_block = from_block;
    if let Err(e) = state_store.save(&op_state) {
        tracing::warn!("Failed to save state on shutdown: {}", e);
    } else {
        tracing::info!(
            last_polled_block = from_block,
            "Watcher state saved to {:?}",
            state_store.path()
        );
    }

    tracing::info!("Shutdown complete");
    Ok(())
}

async fn cmd_run(
    config: &Config,
    features_json: &str,
    metrics_port: u16,
    model: &ModelConfig,
) -> anyhow::Result<()> {
    // 1. Submit (same as cmd_submit)
    let submit_result = cmd_submit(config, features_json, model).await;
    if let Err(e) = &submit_result {
        tracing::error!("Submit failed: {}", e);
        return submit_result;
    }

    // 2. Start watching
    tracing::info!("Starting watch loop...");
    cmd_watch(config, metrics_port).await
}

#[tracing::instrument(skip(config))]
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

    let nonce_hex = compute_attestation_nonce(chain_id, block_number, &info.enclave_address);

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

async fn handle_challenge(
    chain: &chain::ChainClient,
    proof_mgr: &ProofManager,
    result_id: B256,
    alert_manager: &AlertManager,
    metrics: &metrics::MetricsState,
    chain_cb: &CircuitBreaker,
) {
    let rid_hex = format!("0x{}", hex::encode(result_id));

    let resolve_result = match proof_mgr.read_proof(&rid_hex) {
        Ok(Some(proof)) => {
            tracing::info!("Found pre-computed proof for {}", rid_hex);
            resolve_with_proof(chain, result_id, &proof).await
        }
        Ok(None) => {
            tracing::warn!(
                "No pre-computed proof for {}. Waiting up to 60s...",
                rid_hex
            );
            match proof_mgr.wait_for_proof(&rid_hex, 60).await {
                Ok(true) => {
                    if let Ok(Some(proof)) = proof_mgr.read_proof(&rid_hex) {
                        resolve_with_proof(chain, result_id, &proof).await
                    } else {
                        Err(anyhow::anyhow!("Proof disappeared after wait"))
                    }
                }
                Ok(false) => {
                    tracing::error!("Proof not available after timeout for {}", rid_hex);
                    Err(anyhow::anyhow!("Proof timeout for {}", rid_hex))
                }
                Err(e) => {
                    tracing::error!("Error waiting for proof: {}", e);
                    Err(e)
                }
            }
        }
        Err(e) => {
            tracing::error!("Error reading proof: {}", e);
            Err(e)
        }
    };

    match resolve_result {
        Ok(tx) => {
            chain_cb.record_success();
            tracing::info!("Dispute resolved for {}! tx={}", rid_hex, tx);
            metrics.record_dispute_resolved();
            let mut meta = HashMap::new();
            meta.insert("result_id".to_string(), rid_hex);
            meta.insert("tx".to_string(), format!("{}", tx));
            let _ = alert_manager.send_alert(
                AlertSeverity::Info,
                "dispute_resolved",
                "operator",
                "Dispute resolved successfully",
                meta,
            );
        }
        Err(e) => {
            chain_cb.record_failure();
            tracing::error!("Failed to resolve dispute for {}: {}", rid_hex, e);
            metrics.record_dispute_failed();
            let mut meta = HashMap::new();
            meta.insert("result_id".to_string(), rid_hex);
            meta.insert("error".to_string(), format!("{}", e));
            let _ = alert_manager.send_alert(
                AlertSeverity::Critical,
                "dispute_failed",
                "operator",
                &format!("Dispute resolution failed: {}", e),
                meta,
            );
        }
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

async fn auto_finalize(
    watcher: &EventWatcher,
    chain: &chain::ChainClient,
    from_block: u64,
    rpc_cb: &CircuitBreaker,
    chain_cb: &CircuitBreaker,
) {
    if rpc_cb.allow_request().is_err() {
        return; // RPC circuit breaker is open, skip finalize poll
    }
    let (events, _) = match watcher.poll_events(from_block).await {
        Ok(r) => {
            rpc_cb.record_success();
            r
        }
        Err(e) => {
            rpc_cb.record_failure();
            tracing::warn!("Failed to poll for finalizeable results: {}", e);
            return;
        }
    };

    for event in &events {
        if let TEEEvent::ResultSubmitted { result_id, .. } = event {
            if chain_cb.allow_request().is_err() {
                break; // Chain circuit breaker is open
            }
            match chain.finalize(*result_id).await {
                Ok(tx) => {
                    chain_cb.record_success();
                    tracing::info!("Auto-finalized {}, tx={}", result_id, tx);
                }
                Err(_) => {
                    chain_cb.record_failure();
                }
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
        let nonce =
            compute_attestation_nonce(1, 12345, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        // Should be a 64-char hex string (keccak256 = 32 bytes)
        assert_eq!(nonce.len(), 64);

        // Same inputs should produce same nonce
        let nonce2 =
            compute_attestation_nonce(1, 12345, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert_eq!(nonce, nonce2);

        // Different inputs should produce different nonce
        let nonce3 =
            compute_attestation_nonce(1, 12346, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert_ne!(nonce, nonce3);
    }

    #[test]
    fn test_shutdown_state_initial() {
        let state = ShutdownState::new();
        assert!(!state.is_shutting_down());
        assert_eq!(state.in_flight_count(), 0);
    }

    #[test]
    fn test_shutdown_state_signal() {
        let state = ShutdownState::new();
        assert!(!state.is_shutting_down());
        state.signal_shutdown();
        assert!(state.is_shutting_down());
    }

    #[test]
    fn test_shutdown_state_in_flight_tracking() {
        let state = ShutdownState::new();
        assert_eq!(state.in_flight_count(), 0);

        state.track_task_start();
        assert_eq!(state.in_flight_count(), 1);

        state.track_task_start();
        assert_eq!(state.in_flight_count(), 2);

        state.track_task_done();
        assert_eq!(state.in_flight_count(), 1);

        state.track_task_done();
        assert_eq!(state.in_flight_count(), 0);
    }

    #[test]
    fn test_shutdown_state_arc_sharing() {
        let state = Arc::new(ShutdownState::new());
        let state2 = state.clone();

        assert!(!state.is_shutting_down());
        state2.signal_shutdown();
        assert!(state.is_shutting_down());

        state.track_task_start();
        assert_eq!(state2.in_flight_count(), 1);
    }
}
