mod chain;
use tee_operator::config;
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
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tracing::Instrument;
use uuid::Uuid;

use config::{Config, ModelConfig};
use prover::ProofManager;
use secrecy::ExposeSecret;
use store::StateStore;
use tee_operator::alerting::{AlertConfig, AlertManager, AlertSeverity};
use tee_operator::audit;
use tee_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use watcher::{EventWatcher, TEEEvent};
use world_zk_sdk::abi::TEEMLVerifier::MLResult;

/// Cached attestation verification result with TTL.
struct CachedAttestation {
    #[allow(dead_code)]
    verified: nitro::VerifiedAttestation,
    fetched_at: Instant,
}

/// Global attestation cache for the submit command.
/// Avoids re-fetching and re-verifying attestation on every submit
/// within the TTL window.
///
/// Uses `tokio::sync::Mutex` so the lock can be held across async
/// operations, eliminating the TOCTOU race that existed when
/// `std::sync::Mutex` was dropped before the async fetch.
static ATTESTATION_CACHE: OnceLock<tokio::sync::Mutex<Option<CachedAttestation>>> = OnceLock::new();

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
        /// Port for the health/metrics HTTP server.
        /// Overrides the `METRICS_PORT` env var / config file value when provided.
        #[arg(long)]
        metrics_port: Option<u16>,
        /// Dry-run mode: monitor events and log what would happen, but do not
        /// submit any on-chain transactions.
        /// When set to true, overrides the `DRY_RUN` env var / config file value.
        #[arg(long, default_value = "false")]
        dry_run: bool,
    },
    /// Combined: submit + watch + prove
    Run {
        /// Feature vector as JSON array
        #[arg(long)]
        features: String,
        /// Port for the health/metrics HTTP server.
        /// Overrides the `METRICS_PORT` env var / config file value when provided.
        #[arg(long)]
        metrics_port: Option<u16>,
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
    /// Export audit records from the proof archive for compliance
    ExportAudit {
        /// Start date (inclusive, YYYY-MM-DD)
        #[arg(long)]
        from: String,
        /// End date (inclusive, YYYY-MM-DD)
        #[arg(long)]
        to: String,
        /// Output format: csv, json, or jsonl
        #[arg(long, default_value = "csv")]
        format: String,
        /// Output file path
        #[arg(long)]
        output: String,
        /// Proof archive directory (defaults to PROOF_ARCHIVE_DIR)
        #[arg(long)]
        archive_dir: Option<String>,
    },
    /// Rotate expired proofs to compressed cold storage
    RotateProofs {
        /// Proof age threshold in days. Proofs older than this are archived.
        /// Defaults to PROOF_TTL_DAYS env var, or 365 if not set.
        #[arg(long)]
        ttl_days: Option<u32>,
        /// Destination directory for compressed cold-storage proofs.
        /// Defaults to PROOF_ARCHIVE_DIR env var.
        #[arg(long)]
        archive_dir: Option<String>,
        /// Source proof directory (defaults to the PROOFS_DIR env var, then PROOF_ARCHIVE_DIR).
        #[arg(long)]
        proofs_dir: Option<String>,
        /// Preview which proofs would be archived without actually moving them.
        #[arg(long, default_value = "false")]
        dry_run: bool,
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

/// The text of a failed RPC call for a log line, an audit event or an alert.
/// It names the RPC URL, whose path often holds the provider's API key, so
/// URLs in it are cut to their origin.
fn rpc_error_text(e: &anyhow::Error) -> String {
    tee_watcher::redact::redact_urls(&e.to_string())
}

/// `e` with the URLs in what is printed for it (the error and its causes) cut
/// to their origin.
fn without_url_paths(e: anyhow::Error) -> anyhow::Error {
    let printed = tee_watcher::redact::redact_urls(&format!("{e:?}"));
    anyhow::anyhow!("{}", printed)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_setup::init_tracing("worldzk-operator", None);
    // The returned error is printed to stderr.
    run_command(Cli::parse()).await.map_err(without_url_paths)
}

async fn run_command(cli: Cli) -> anyhow::Result<()> {
    let config = Config::from_env(cli.config.as_deref())?;

    match cli.command {
        Commands::Submit { features, model } => {
            if let Err(msgs) = config.validate_for_command("submit") {
                for msg in &msgs {
                    tracing::error!("Config validation: {}", msg);
                }
                std::process::exit(1);
            }
            let selected = config.get_model(model.as_deref())?;
            cmd_submit(&config, &features, selected).await
        }
        Commands::Watch {
            metrics_port,
            dry_run,
        } => {
            if let Err(msgs) = config.validate_for_command("watch") {
                for msg in &msgs {
                    tracing::error!("Config validation: {}", msg);
                }
                std::process::exit(1);
            }
            let port = metrics_port.unwrap_or(config.metrics_port);
            let dry = dry_run || config.dry_run;
            cmd_watch(&config, port, dry).await
        }
        Commands::Run {
            features,
            metrics_port,
            model,
        } => {
            if let Err(msgs) = config.validate_for_command("run") {
                for msg in &msgs {
                    tracing::error!("Config validation: {}", msg);
                }
                std::process::exit(1);
            }
            let port = metrics_port.unwrap_or(config.metrics_port);
            let selected = config.get_model(model.as_deref())?;
            cmd_run(&config, &features, port, selected).await
        }
        Commands::Register {
            expected_pcr0,
            skip_verify,
        } => cmd_register(&config, expected_pcr0.as_deref(), skip_verify).await,
        Commands::Models => cmd_models(&config),
        Commands::ExportAudit {
            from,
            to,
            format,
            output,
            archive_dir,
        } => cmd_export_audit(
            &config,
            &from,
            &to,
            &format,
            &output,
            archive_dir.as_deref(),
        ),
        Commands::RotateProofs {
            ttl_days,
            archive_dir,
            proofs_dir,
            dry_run,
        } => {
            cmd_rotate_proofs(
                &config,
                ttl_days,
                archive_dir.as_deref(),
                proofs_dir.as_deref(),
                dry_run,
            )
            .await
        }
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
/// The tokio mutex is held across the entire check-then-fetch-then-update
/// sequence to prevent TOCTOU races where concurrent callers could both
/// see a stale cache and redundantly re-fetch attestation.
async fn verify_enclave_attestation(
    config: &Config,
    client: &enclave::EnclaveClient,
) -> anyhow::Result<()> {
    let cache = ATTESTATION_CACHE.get_or_init(|| tokio::sync::Mutex::new(None));

    // Hold the lock across the entire check-fetch-update sequence to prevent
    // TOCTOU races. tokio::sync::Mutex allows .await while the guard is held
    // without blocking the async runtime.
    let mut cached = cache.lock().await;

    // Check cache -- if still valid, return immediately (lock dropped on return).
    if let Some(ref c) = *cached {
        if c.fetched_at.elapsed().as_secs() < config.attestation_cache_ttl {
            tracing::debug!(
                "Using cached attestation (age={}s)",
                c.fetched_at.elapsed().as_secs()
            );
            return Ok(());
        }
    }

    // Cache miss or expired -- fetch a fresh attestation while still holding
    // the lock so no other caller duplicates this work.

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
    nitro::validate_freshness(&verified, config.attestation_freshness_secs)?;

    tracing::info!(
        "Enclave attestation verified (cert_chain={}, nonce_verified=true)",
        verified.cert_chain_verified
    );

    // Update cache while still holding the lock -- no TOCTOU possible.
    *cached = Some(CachedAttestation {
        verified,
        fetched_at: Instant::now(),
    });

    Ok(())
}

async fn cmd_submit(
    config: &Config,
    features_json: &str,
    model: &ModelConfig,
) -> anyhow::Result<()> {
    let background = submit(config, features_json, model).await?;
    // `submit` is a one-shot command: the process exits when this returns, which
    // would cancel the archive write and the proof pre-computation mid-way.
    for task in background {
        let _ = task.await;
    }
    Ok(())
}

/// Run one inference and submit it on-chain. Returns the handles of the
/// best-effort follow-up tasks (archive write, proof pre-computation), which keep
/// running in the background.
#[tracing::instrument(skip(config, features_json, model))]
async fn submit(
    config: &Config,
    features_json: &str,
    model: &ModelConfig,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    let request_id = Uuid::new_v4();
    let _submit_span = tracing::info_span!("submit", request_id = %request_id).entered();

    // 1. Parse features
    let feats: Vec<f64> = serde_json::from_str(features_json)
        .map_err(|e| anyhow::anyhow!("Invalid features JSON: {}", e))?;
    tracing::info!(
        request_id = %request_id,
        model_name = %model.name,
        model_path = %model.path,
        "Submitting inference for {} features using model '{}'",
        feats.len(),
        model.name
    );

    // 2. Call enclave /infer
    let enclave_client = enclave::EnclaveClient::with_config_timeout(
        &config.enclave_url,
        config.enclave_timeout_secs,
    )?;

    let health = enclave_client.health().await?;
    if !health {
        anyhow::bail!("Enclave is not healthy at {}", config.enclave_url);
    }

    // 2a. Verify enclave attestation if nitro verification is enabled
    if config.nitro_verification {
        verify_enclave_attestation(config, &enclave_client).await?;
    }

    let response = enclave_client
        .infer(&feats, Some(&config.tee_verifier_address))
        .await?;
    tracing::info!(
        "Enclave response: model_hash={}, result_hash={}",
        response.model_hash,
        response.result_hash
    );

    // 3. Submit on-chain
    let chain_client = chain::ChainClient::new(
        &config.rpc_url,
        config.private_key.expose_secret(),
        &config.tee_verifier_address,
    )?;

    let model_hash = hex_to_b256(&response.model_hash)?;
    let input_hash = hex_to_b256(&response.input_hash)?;
    let result_bytes = hex_to_bytes(&response.result)?;
    let attestation = hex_to_bytes(&response.attestation)?;
    let stake = U256::from_str_radix(&config.prover_stake_wei, 10)
        .map_err(|e| anyhow::anyhow!("Invalid stake: {}", e))?;

    let (tx_hash, result_id) = chain_client
        .submit_result(model_hash, input_hash, &result_bytes, &attestation, stake)
        .await?;
    tracing::info!("Submitted on-chain: tx={} result_id={}", tx_hash, result_id);

    // Audit log: result submission
    audit::log_result_submitted(
        &format!("0x{}", hex::encode(tx_hash)),
        &model.name,
        feats.len(),
    );

    let mut background = Vec::new();

    // 3a. Archive the submission (best-effort, non-blocking)
    if !config.proof_archive_dir.is_empty() {
        let archive_dir = config.proof_archive_dir.clone();
        let archive_entry = tee_operator::proof_archive::ProofArchiveEntry {
            id: request_id.to_string(),
            archived_at: format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ),
            result_id: format!("0x{}", hex::encode(result_id)),
            model_hash: response.model_hash.clone(),
            input_hash: response.input_hash.clone(),
            result_hash: response.result_hash.clone(),
            result: response.result.clone(),
            attestation: response.attestation.clone(),
            features: feats.clone(),
            chain_id: 0,
            enclave_address: String::new(),
            proof_path: None,
            disputed: false,
            finalized: false,
        };
        background.push(tokio::spawn(async move {
            match tee_operator::proof_archive::ProofArchive::new(&archive_dir).await {
                Ok(archive) => {
                    if let Err(e) = archive.store(&archive_entry).await {
                        tracing::warn!("Failed to archive proof: {}", e);
                    } else {
                        tracing::debug!("Proof archived: {}", archive_entry.id);
                    }
                }
                Err(e) => tracing::warn!("Failed to init proof archive: {}", e),
            }
        }));
    }

    // 4. Trigger proof pre-computation (best-effort, uses warm prover if PROVER_URL is set)
    let proof_mgr = ProofManager::with_config(
        &config.precompute_bin,
        &model.path,
        &config.proofs_dir,
        config.max_proof_retries,
        config.proof_retry_delay_secs,
        config.prover_timeout_secs,
        config.prover_url.as_deref(),
    )?;
    // Key the pre-computed proof by the on-chain result ID: that is what the watcher
    // looks it up by when the result is challenged.
    let result_id = format!("0x{}", hex::encode(result_id));
    let features_owned = features_json.to_string();
    background.push(tokio::spawn(async move {
        if let Err(e) = proof_mgr.generate_proof(&result_id, &features_owned).await {
            tracing::warn!("Proof pre-computation failed (best-effort): {}", e);
        }
    }));

    println!("tx_hash={}", tx_hash);
    Ok(background)
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

fn cmd_export_audit(
    config: &Config,
    from: &str,
    to: &str,
    format: &str,
    output: &str,
    archive_dir: Option<&str>,
) -> anyhow::Result<()> {
    use tee_operator::audit_export::{export_records, scan_archive, ExportFormat};

    // Validate date format (basic check)
    for (label, date) in [("from", from), ("to", to)] {
        if date.len() != 10 || date.chars().filter(|c| *c == '-').count() != 2 {
            anyhow::bail!(
                "Invalid {} date '{}': expected YYYY-MM-DD format",
                label,
                date
            );
        }
    }

    if from > to {
        anyhow::bail!(
            "--from date ({}) must not be after --to date ({})",
            from,
            to
        );
    }

    let fmt = ExportFormat::from_str(format).ok_or_else(|| {
        anyhow::anyhow!("Unsupported format '{}'. Use csv, json, or jsonl.", format)
    })?;

    // Determine archive directory: CLI override > config > error
    let dir = archive_dir
        .map(String::from)
        .unwrap_or_else(|| config.proof_archive_dir.clone());
    if dir.is_empty() {
        anyhow::bail!(
            "No archive directory specified. Use --archive-dir or set PROOF_ARCHIVE_DIR."
        );
    }

    let dir_path = std::path::Path::new(&dir);
    if !dir_path.is_dir() {
        anyhow::bail!("Archive directory does not exist: {}", dir);
    }

    tracing::info!(
        from = from,
        to = to,
        format = format,
        archive_dir = %dir,
        output = output,
        "Exporting audit records"
    );

    let records = scan_archive(dir_path, from, to)?;

    let output_path = std::path::Path::new(output);
    let count = export_records(&records, fmt, output_path)?;

    println!(
        "Exported {} audit record(s) to {} (format: {})",
        count,
        output,
        fmt.extension()
    );

    Ok(())
}

async fn cmd_rotate_proofs(
    config: &Config,
    ttl_days: Option<u32>,
    archive_dir: Option<&str>,
    proofs_dir: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    use tee_operator::proof_archive::{self, ProofArchive};

    let ttl = ttl_days.unwrap_or_else(proof_archive::default_ttl_days);

    // Determine cold-storage destination
    let cold_dir = archive_dir
        .map(String::from)
        .or_else(proof_archive::default_archive_dir)
        .unwrap_or_else(|| config.proof_archive_dir.clone());
    if cold_dir.is_empty() {
        anyhow::bail!(
            "No archive directory specified. Use --archive-dir or set PROOF_ARCHIVE_DIR."
        );
    }

    // Determine source proof directory
    let src_dir = proofs_dir
        .map(String::from)
        .or_else(|| std::env::var("PROOFS_DIR").ok())
        .unwrap_or_else(|| config.proof_archive_dir.clone());
    if src_dir.is_empty() {
        anyhow::bail!("No proofs directory specified. Use --proofs-dir or set PROOFS_DIR.");
    }

    let src_path = std::path::Path::new(&src_dir);
    if !src_path.is_dir() {
        anyhow::bail!("Proofs directory does not exist: {}", src_dir);
    }

    tracing::info!(
        ttl_days = ttl,
        archive_dir = %cold_dir,
        proofs_dir = %src_dir,
        dry_run,
        "Starting proof rotation"
    );

    let archive = ProofArchive::new(src_path).await?;

    // List expired first for reporting
    let expired = archive.list_expired(ttl).await?;
    println!(
        "Found {} expired proof(s) (older than {} days)",
        expired.len(),
        ttl
    );

    if expired.is_empty() {
        println!("Nothing to archive.");
        return Ok(());
    }

    if dry_run {
        println!("[dry-run] The following proofs would be archived:");
        for path in &expired {
            println!("  {}", path.display());
        }
        println!(
            "[dry-run] Would archive {} proof(s) to {}",
            expired.len(),
            cold_dir
        );
        // Still call archive_expired in dry-run mode for logging
        archive.archive_expired(ttl, &cold_dir, true).await?;
        return Ok(());
    }

    let archived = archive.archive_expired(ttl, &cold_dir, false).await?;
    println!(
        "Archived {} proof(s) to {} (compressed with gzip)",
        archived.len(),
        cold_dir
    );

    Ok(())
}

/// Shared shutdown state for graceful termination.
struct ShutdownState {
    shutting_down: AtomicBool,
    in_flight_tasks: AtomicU64,
    cancel_tx: tokio::sync::watch::Sender<bool>,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    reason: std::sync::Mutex<Option<String>>,
}

impl ShutdownState {
    fn new() -> Self {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        Self {
            shutting_down: AtomicBool::new(false),
            in_flight_tasks: AtomicU64::new(0),
            cancel_tx,
            cancel_rx,
            reason: std::sync::Mutex::new(None),
        }
    }

    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Signal shutdown without a specific reason.
    /// Only used in tests (production code always provides a reason via
    /// `signal_shutdown_with_reason`).
    #[cfg(test)]
    fn signal_shutdown(&self) {
        self.signal_shutdown_with_reason("unknown");
    }

    /// Signal shutdown with a human-readable reason (e.g. "SIGINT", "SIGTERM").
    fn signal_shutdown_with_reason(&self, reason: &str) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Ok(mut r) = self.reason.lock() {
            *r = Some(reason.to_string());
        }
        let _ = self.cancel_tx.send(true);
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

    /// Subscribe to shutdown cancellation notifications.
    fn subscribe_cancel(&self) -> tokio::sync::watch::Receiver<bool> {
        self.cancel_rx.clone()
    }

    /// Return the shutdown reason, if set.
    fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|r| r.clone())
    }
}

/// Wait for shutdown signal (SIGINT or SIGTERM) and return the reason string.
async fn shutdown_signal() -> &'static str {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            tracing::warn!("failed to install SIGTERM handler, using ctrl-c only");
            ctrl_c.await.ok();
            return "SIGINT";
        };
        tokio::select! {
            _ = ctrl_c => {
                "SIGINT"
            },
            _ = sigterm.recv() => {
                "SIGTERM"
            },
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        "SIGINT"
    }
}

/// Build the alert configuration from the value of `ALERT_CONFIG_JSON`.
///
/// An unset, empty or whitespace-only value gives the default (log-only)
/// configuration. The value holds webhook URLs and routing keys, and a
/// `serde_json` error message can quote the input, so a parse failure is
/// logged as error category, position and byte length only.
fn alert_config_from_env_value(value: Option<&str>) -> AlertConfig {
    let json = match value {
        Some(v) if !v.trim().is_empty() => v,
        _ => {
            tracing::debug!("ALERT_CONFIG_JSON not set, using default AlertConfig (log-only)");
            return AlertConfig::default();
        }
    };
    tracing::debug!("Parsing ALERT_CONFIG_JSON ({} bytes)", json.len());
    serde_json::from_str::<AlertConfig>(json).unwrap_or_else(|e| {
        tracing::warn!(
            error_kind = ?e.classify(),
            line = e.line(),
            column = e.column(),
            json_len = json.len(),
            "Failed to parse ALERT_CONFIG_JSON, falling back to defaults"
        );
        AlertConfig::default()
    })
}

#[tracing::instrument(skip(config))]
async fn cmd_watch(config: &Config, metrics_port: u16, dry_run: bool) -> anyhow::Result<()> {
    if dry_run {
        tracing::info!(
            "[DRY-RUN] Dry-run mode enabled. No on-chain transactions will be submitted."
        );
    }
    let contract_addr: Address = config
        .tee_verifier_address
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid contract address: {}", e))?;
    let watcher = EventWatcher::new(&config.rpc_url, contract_addr);
    let chain_client = Arc::new(chain::ChainClient::new_with_dry_run(
        &config.rpc_url,
        config.private_key.expose_secret(),
        &config.tee_verifier_address,
        dry_run,
    )?);
    let proof_mgr = Arc::new(ProofManager::with_config(
        &config.precompute_bin,
        &config.model_path,
        &config.proofs_dir,
        config.max_proof_retries,
        config.proof_retry_delay_secs,
        config.prover_timeout_secs,
        config.prover_url.as_deref(),
    )?);

    // Initialize webhook notifier (None if WEBHOOK_URL is not set)
    let notifier = notifications::WebhookNotifier::from_optional(config.webhook_url.as_deref());
    if notifier.is_some() {
        tracing::info!("Webhook notifications enabled");
    } else {
        tracing::debug!("Webhook notifications disabled (no WEBHOOK_URL configured)");
    }

    // Initialize AlertManager for multi-channel alerting (log-only by default)
    let alert_config =
        alert_config_from_env_value(std::env::var("ALERT_CONFIG_JSON").ok().as_deref());
    tracing::info!(
        channels = alert_config.channels.len(),
        dedup_window_secs = alert_config.dedup_window_secs,
        escalation_timeout_secs = alert_config.escalation_timeout_secs,
        "AlertManager initialized with {} alert channel(s)",
        alert_config.channels.len()
    );
    let alert_manager = Arc::new(AlertManager::new(alert_config));

    // Circuit breakers for RPC and chain calls
    let rpc_cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default()));
    let chain_cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 3,
        recovery_timeout: std::time::Duration::from_secs(60),
        success_threshold_for_close: 1,
    }));

    // Semaphore to limit concurrent proof submissions (T215)
    let proof_semaphore = Arc::new(tokio::sync::Semaphore::new(10));

    // Initialize local proof pre-verifier (if circuit descriptions are available)
    let pre_verifier: Option<Arc<tee_operator::verify::ProofPreVerifier>> =
        if !config.circuit_desc_dir.is_empty() {
            tracing::info!(
                circuit_desc_dir = %config.circuit_desc_dir,
                "Local proof pre-verification enabled"
            );
            Some(Arc::new(tee_operator::verify::ProofPreVerifier::new(
                &config.circuit_desc_dir,
            )))
        } else {
            tracing::debug!("Local proof pre-verification disabled (no CIRCUIT_DESC_DIR)");
            None
        };

    let shutdown = Arc::new(ShutdownState::new());

    // Initialize metrics state before shutdown handler so it can be marked
    let metrics_state = Arc::new(metrics::MetricsState::new());

    // Spawn shutdown signal handler
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let reason = shutdown_signal().await;
            tracing::info!(
                signal = reason,
                "shutting down gracefully, reason: {}",
                reason
            );
            shutdown.signal_shutdown_with_reason(reason);
        });
    }

    // Spawn HTTP metrics server with graceful shutdown support.
    // The cancel receiver fires when ShutdownState broadcasts the
    // cancellation signal. serve_metrics_with_shutdown marks MetricsState
    // as shutting_down so health/ready endpoints return 503.
    {
        let ms = metrics_state.clone();
        let cancel_rx = shutdown.subscribe_cancel();
        let bind_addr = config.metrics_bind_addr.clone();
        tokio::spawn(async move {
            metrics::serve_metrics_with_shutdown(ms, metrics_port, &bind_addr, cancel_rx).await;
        });
    }

    // Load persistent state for crash recovery
    let state_store = StateStore::new(&config.state_file);
    let mut op_state = state_store.load_or_default();

    // Audit log: configuration loaded for watch mode
    audit::log_config_loaded(
        &config.rpc_url,
        &config.tee_verifier_address,
        dry_run,
        config.nitro_verification,
    );

    tracing::info!("Watching for events on {}...", config.tee_verifier_address);

    let mut from_block = op_state.last_polled_block;
    let mut finalize_counter = 0u64;
    // Cache result_id → model_hash for multi-model proof selection
    let mut result_model_map: HashMap<String, String> = HashMap::new();

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
                        rpc_error_text(&e)
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

                    // Look up model hash for this result
                    if let Some(mh) = result_model_map.get(&rid_hex) {
                        if let Some(model) = config.find_model_by_hash(mh) {
                            tracing::info!(
                                result_id = %result_id,
                                model_name = %model.name,
                                model_hash = %mh,
                                "Challenge maps to model '{}'",
                                model.name
                            );
                        } else {
                            tracing::warn!(
                                result_id = %result_id,
                                model_hash = %mh,
                                "Challenge for unknown model hash (not in config)"
                            );
                        }
                    }

                    tracing::warn!(
                        result_id = %result_id,
                        challenger = %challenger,
                        "Challenge detected"
                    );

                    // Audit log: challenge detected
                    audit::log_challenge_detected(&rid_hex, &format!("{}", challenger), from_block);

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

                    if dry_run {
                        // In dry-run mode: log what would happen, skip transactions
                        tracing::info!(
                            result_id = %result_id,
                            challenger = %challenger,
                            "[DRY-RUN] Would submit proof to resolve dispute for {}",
                            rid_hex
                        );
                        op_state.processed_event_ids.insert(rid_hex);
                        continue;
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
                            let pv = pre_verifier.clone();
                            shutdown.track_task_start();
                            let sd = shutdown.clone();
                            let mut cancel_rx = shutdown.subscribe_cancel();
                            tokio::spawn(
                                async move {
                                    tokio::select! {
                                        _ = handle_challenge(&chain, &pm, rid, &am, &ms, &cb, pv.as_deref()) => {},
                                        _ = cancel_rx.changed() => {
                                            tracing::info!(
                                                result_id = %rid,
                                                "cancelling in-flight dispute task due to shutdown"
                                            );
                                        },
                                    }
                                    sd.track_task_done();
                                    drop(permit);
                                }
                                .instrument(dispute_span),
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                "Max concurrent proof submissions reached, skipping challenge (it is not polled again unless the operator replays from an earlier block): {}",
                                rid_hex
                            );
                            metrics_state.record_error();
                            // Not marked as processed, but the poll loop has
                            // already moved past this block, so the challenge
                            // is only seen again after a replay.
                            continue;
                        }
                    }

                    // Mark as processed after handling initiated
                    op_state.processed_event_ids.insert(rid_hex);
                }
                TEEEvent::ResultSubmitted {
                    result_id,
                    model_hash,
                    ..
                } => {
                    metrics_state.record_submission();
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    let mh_hex = format!("0x{}", hex::encode(model_hash));
                    result_model_map.insert(rid_hex.clone(), mh_hex.clone());
                    tracing::info!(
                        result_id = %result_id,
                        model_hash = %mh_hex,
                        "New result submitted"
                    );
                }
                TEEEvent::ResultFinalized { result_id } => {
                    metrics_state.record_finalization();
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    op_state.active_disputes.remove(&rid_hex);
                    result_model_map.remove(&rid_hex);
                    tracing::info!(result_id = %result_id, "Result finalized");
                }
                TEEEvent::ResultExpired { result_id } => {
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    op_state.active_disputes.remove(&rid_hex);
                    result_model_map.remove(&rid_hex);
                    tracing::info!(result_id = %result_id, "Result expired (unchallenged finalize)");
                }
                TEEEvent::DisputeResolved {
                    result_id,
                    prover_won,
                } => {
                    let rid_hex = format!("0x{}", hex::encode(result_id));
                    op_state.active_disputes.remove(&rid_hex);
                    result_model_map.remove(&rid_hex);
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
        if finalize_counter >= 12 && !dry_run {
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
        let mut cancel_rx = shutdown.subscribe_cancel();
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
            _ = cancel_rx.changed() => {
                break;
            },
        }
    }

    // Graceful shutdown: log reason
    let reason = shutdown.reason().unwrap_or_else(|| "unknown".to_string());
    tracing::info!(
        reason = %reason,
        in_flight_tasks = shutdown.in_flight_count(),
        "initiating graceful shutdown sequence"
    );

    // Set health endpoint to unhealthy (already done via ShutdownState sharing)
    // Wait for in-flight tasks to complete
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
                "Shutdown timeout -- exiting with in-flight tasks"
            );
        } else {
            tracing::info!("All in-flight tasks completed");
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

    // Flush pending logs and tracer spans
    tracing_setup::shutdown_tracer_provider();

    tracing::info!(reason = %reason, "shutdown complete");
    Ok(())
}

async fn cmd_run(
    config: &Config,
    features_json: &str,
    metrics_port: u16,
    model: &ModelConfig,
) -> anyhow::Result<()> {
    // 1. Submit (same as cmd_submit, but the follow-up tasks stay in the
    //    background while the watch loop runs)
    if let Err(e) = submit(config, features_json, model).await {
        tracing::error!("Submit failed: {}", rpc_error_text(&e));
        return Err(e);
    }

    // 2. Start watching (Run command never uses dry-run)
    tracing::info!("Starting watch loop...");
    cmd_watch(config, metrics_port, false).await
}

#[tracing::instrument(skip(config))]
async fn cmd_register(
    config: &Config,
    expected_pcr0: Option<&str>,
    skip_verify: bool,
) -> anyhow::Result<()> {
    let enclave_client = enclave::EnclaveClient::with_config_timeout(
        &config.enclave_url,
        config.enclave_timeout_secs,
    )?;

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
        config.private_key.expose_secret(),
        &config.tee_verifier_address,
    )?;

    let tx_hash = chain_client
        .register_enclave(enclave_addr, image_hash)
        .await?;
    tracing::info!("Enclave registered on-chain: tx={}", tx_hash);
    audit::log_enclave_registered(
        &format!("{}", enclave_addr),
        &format!("0x{}", hex::encode(image_hash)),
        &format!("0x{}", hex::encode(tx_hash)),
        &config.tee_verifier_address,
    );

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
    pre_verifier: Option<&tee_operator::verify::ProofPreVerifier>,
) {
    let rid_hex = format!("0x{}", hex::encode(result_id));

    // Local pre-verification helper: returns true if proof should be submitted,
    // false if pre-verification failed (bad proof, don't waste gas).
    // Converts the binary crate's StoredProof to the library crate's StoredProof
    // since they are compiled as separate crate types.
    let pre_verify_proof = |proof: &store::StoredProof, rid: &str| -> bool {
        if let Some(pv) = pre_verifier {
            let lib_proof = tee_operator::store::StoredProof {
                proof_hex: proof.proof_hex.clone(),
                circuit_hash: proof.circuit_hash.clone(),
                public_inputs_hex: proof.public_inputs_hex.clone(),
                gens_hex: proof.gens_hex.clone(),
            };
            match pv.pre_verify(&lib_proof) {
                tee_operator::verify::PreVerifyResult::Verified => {
                    tracing::info!("Pre-verification passed for {}", rid);
                    true
                }
                tee_operator::verify::PreVerifyResult::Failed(reason) => {
                    tracing::error!("Pre-verification FAILED for {}: {}", rid, reason);
                    false
                }
                tee_operator::verify::PreVerifyResult::Skipped(reason) => {
                    tracing::debug!("Pre-verification skipped for {}: {}", rid, reason);
                    true
                }
            }
        } else {
            true
        }
    };

    let resolve_result = match proof_mgr.read_proof(&rid_hex) {
        Ok(Some(proof)) => {
            tracing::info!("Found pre-computed proof for {}", rid_hex);
            if !pre_verify_proof(&proof, &rid_hex) {
                metrics.record_dispute_failed();
                return;
            }
            resolve_with_proof(chain, result_id, &proof, metrics).await
        }
        Ok(None) => {
            tracing::warn!(
                "No pre-computed proof for {}. Waiting up to 60s...",
                rid_hex
            );
            match proof_mgr.wait_for_proof(&rid_hex, 60).await {
                Ok(true) => {
                    if let Ok(Some(proof)) = proof_mgr.read_proof(&rid_hex) {
                        if !pre_verify_proof(&proof, &rid_hex) {
                            metrics.record_dispute_failed();
                            return;
                        }
                        resolve_with_proof(chain, result_id, &proof, metrics).await
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
            audit::log_dispute_submitted(&rid_hex, "", &format!("{}", tx));
            metrics.record_dispute_resolved();
            let mut meta = HashMap::new();
            meta.insert("result_id".to_string(), rid_hex);
            meta.insert("tx".to_string(), format!("{}", tx));
            if let Err(e) = alert_manager.send_alert(
                AlertSeverity::Info,
                "dispute_resolved",
                "operator",
                "Dispute resolved successfully",
                meta,
            ) {
                tracing::warn!("Failed to send dispute_resolved alert: {}", e);
            }
        }
        Err(e) => {
            chain_cb.record_failure();
            let error = rpc_error_text(&e);
            tracing::error!("Failed to resolve dispute for {}: {}", rid_hex, error);
            audit::log_dispute_failed(&rid_hex, "", &error);
            metrics.record_dispute_failed();
            let mut meta = HashMap::new();
            meta.insert("result_id".to_string(), rid_hex);
            meta.insert("error".to_string(), error.clone());
            if let Err(e) = alert_manager.send_alert(
                AlertSeverity::Critical,
                "dispute_failed",
                "operator",
                &format!("Dispute resolution failed: {}", error),
                meta,
            ) {
                tracing::warn!("Failed to send dispute_failed alert: {}", e);
            }
        }
    }
}

/// Gas threshold for classifying dispute resolution paths.
/// Disputes that use less than this amount of gas are classified as using the
/// Stylus (WASM) verification path; those above 200M gas are classified as
/// using the Solidity verification path.
const STYLUS_GAS_THRESHOLD: u64 = 30_000_000;
const SOLIDITY_GAS_THRESHOLD: u64 = 200_000_000;

async fn resolve_with_proof(
    chain: &chain::ChainClient,
    result_id: B256,
    proof: &store::StoredProof,
    metrics: &metrics::MetricsState,
) -> anyhow::Result<B256> {
    let proof_bytes = hex_to_bytes(&proof.proof_hex)?;
    let circuit_hash = hex_to_b256(&proof.circuit_hash)?;
    let public_inputs = hex_to_bytes(&proof.public_inputs_hex)?;
    let gens_data = hex_to_bytes(&proof.gens_hex)?;

    audit::log_proof_verification_submitted(
        &format!("0x{}", hex::encode(result_id)),
        &format!("0x{}", hex::encode(circuit_hash)),
        proof_bytes.len(),
    );

    let (tx_hash, gas_used) = chain
        .resolve_dispute_with_gas(
            result_id,
            &proof_bytes,
            circuit_hash,
            &public_inputs,
            &gens_data,
        )
        .await?;

    // Classify the verification path based on gas consumption.
    // Stylus (WASM) verification uses significantly less gas than the
    // on-chain Solidity verifier, so we can infer which path was taken.
    let verifier_path = if gas_used < STYLUS_GAS_THRESHOLD {
        "stylus"
    } else if gas_used > SOLIDITY_GAS_THRESHOLD {
        "solidity"
    } else {
        "unknown"
    };

    tracing::info!(
        gas_used,
        verifier_path,
        result_id = %result_id,
        "Dispute resolved"
    );

    match verifier_path {
        "stylus" => metrics.record_stylus_dispute_resolved(),
        "solidity" => metrics.record_solidity_dispute_resolved(),
        _ => {}
    }

    Ok(tx_hash)
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
            tracing::warn!(
                "Failed to poll for finalizeable results: {}",
                rpc_error_text(&e)
            );
            return;
        }
    };

    // The scan above returns every result of the window again on each pass, and
    // `finalize` reverts for all but pending results past their challenge window.
    // Read the state first so that those expected reverts are not sent and do not
    // count as chain failures.
    let now = chain_now(chain).await;
    for event in &events {
        if let TEEEvent::ResultSubmitted { result_id, .. } = event {
            if chain_cb.allow_request().is_err() {
                break; // Chain circuit breaker is open
            }
            match chain.get_result(*result_id).await {
                Ok(result) => {
                    rpc_cb.record_success();
                    if !is_finalizable(&result, now) {
                        continue;
                    }
                }
                Err(e) => {
                    rpc_cb.record_failure();
                    tracing::warn!(
                        "Failed to read result {}: {}",
                        result_id,
                        rpc_error_text(&e)
                    );
                    break;
                }
            }
            match chain.finalize(*result_id).await {
                Ok(tx) => {
                    chain_cb.record_success();
                    tracing::info!("Auto-finalized {}, tx={}", result_id, tx);
                }
                Err(e) => {
                    chain_cb.record_failure();
                    tracing::warn!(
                        "Failed to auto-finalize {}: {}",
                        result_id,
                        rpc_error_text(&e)
                    );
                }
            }
        }
    }
}

/// Whether `TEEMLVerifier.finalize` can succeed for a result in this state: it
/// reverts for unknown, finalized or challenged results and before the challenge
/// deadline.
fn is_finalizable(result: &MLResult, now: u64) -> bool {
    !result.submittedAt.is_zero()
        && !result.finalized
        && !result.challenged
        && U256::from(now) >= result.challengeDeadline
}

/// The time a transaction sent now would see: the later of the wall clock and the
/// latest block's timestamp (a dev chain can be warped ahead of the wall clock).
async fn chain_now(chain: &chain::ChainClient) -> u64 {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match chain.latest_block_timestamp().await {
        Ok(ts) => ts.max(wall),
        Err(_) => wall,
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

    #[tokio::test]
    async fn test_shutdown_cancels_pending_tasks() {
        let state = Arc::new(ShutdownState::new());
        let state_clone = state.clone();

        // Simulate a long-running task
        state.track_task_start();
        let handle = tokio::spawn(async move {
            // Simulate work that checks for shutdown
            while !state_clone.is_shutting_down() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            state_clone.track_task_done();
        });

        // Signal shutdown after a brief delay
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(state.in_flight_count(), 1);
        state.signal_shutdown();

        // Task should complete after shutdown signal
        handle.await.unwrap();
        assert_eq!(state.in_flight_count(), 0);
        assert!(state.is_shutting_down());
    }

    #[test]
    fn test_shutdown_state_multiple_tasks() {
        let state = ShutdownState::new();

        // Track multiple tasks
        for _ in 0..5 {
            state.track_task_start();
        }
        assert_eq!(state.in_flight_count(), 5);

        // Complete some
        state.track_task_done();
        state.track_task_done();
        assert_eq!(state.in_flight_count(), 3);

        // Signal shutdown while tasks are in-flight
        state.signal_shutdown();
        assert!(state.is_shutting_down());
        assert_eq!(state.in_flight_count(), 3);

        // Complete remaining
        state.track_task_done();
        state.track_task_done();
        state.track_task_done();
        assert_eq!(state.in_flight_count(), 0);
    }

    #[test]
    fn test_cli_watch_dry_run_flag() {
        // Verify the --dry-run flag is parsed correctly
        let cli = Cli::parse_from(["tee-operator", "watch", "--dry-run"]);
        match cli.command {
            Commands::Watch { dry_run, .. } => assert!(dry_run, "--dry-run should be true"),
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn test_cli_watch_default_no_dry_run() {
        // Verify the default is dry_run=false
        let cli = Cli::parse_from(["tee-operator", "watch"]);
        match cli.command {
            Commands::Watch { dry_run, .. } => assert!(!dry_run, "default dry_run should be false"),
            _ => panic!("expected Watch command"),
        }
    }

    #[test]
    fn test_cli_watch_metrics_port_with_dry_run() {
        // Verify --metrics-port and --dry-run can be combined
        let cli = Cli::parse_from([
            "tee-operator",
            "watch",
            "--metrics-port",
            "8080",
            "--dry-run",
        ]);
        match cli.command {
            Commands::Watch {
                metrics_port,
                dry_run,
            } => {
                assert_eq!(metrics_port, Some(8080));
                assert!(dry_run);
            }
            _ => panic!("expected Watch command"),
        }
    }

    // --- Graceful shutdown with reason tests ---

    #[test]
    fn test_shutdown_reason_initially_none() {
        let state = ShutdownState::new();
        assert!(state.reason().is_none());
    }

    #[test]
    fn test_shutdown_with_reason_stores_reason() {
        let state = ShutdownState::new();
        state.signal_shutdown_with_reason("SIGTERM");
        assert!(state.is_shutting_down());
        assert_eq!(state.reason(), Some("SIGTERM".to_string()));
    }

    #[test]
    fn test_shutdown_with_reason_sigint() {
        let state = ShutdownState::new();
        state.signal_shutdown_with_reason("SIGINT");
        assert!(state.is_shutting_down());
        assert_eq!(state.reason(), Some("SIGINT".to_string()));
    }

    #[test]
    fn test_signal_shutdown_no_args_stores_unknown() {
        let state = ShutdownState::new();
        state.signal_shutdown();
        assert!(state.is_shutting_down());
        assert_eq!(state.reason(), Some("unknown".to_string()));
    }

    #[tokio::test]
    async fn test_cancel_subscription_fires_on_shutdown() {
        let state = Arc::new(ShutdownState::new());
        let mut rx = state.subscribe_cancel();

        // Should not be cancelled yet
        assert!(!*rx.borrow());

        // Signal shutdown
        state.signal_shutdown_with_reason("SIGTERM");

        // The receiver should see the cancellation
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
    }

    #[tokio::test]
    async fn test_cancel_subscription_wakes_spawned_task() {
        let state = Arc::new(ShutdownState::new());
        let mut rx = state.subscribe_cancel();

        state.track_task_start();
        let sd = state.clone();
        let handle = tokio::spawn(async move {
            // Wait for cancel signal via watch channel
            let _ = rx.changed().await;
            sd.track_task_done();
        });

        // Brief delay then signal shutdown
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(state.in_flight_count(), 1);
        state.signal_shutdown_with_reason("SIGINT");

        handle.await.unwrap();
        assert_eq!(state.in_flight_count(), 0);
        assert_eq!(state.reason(), Some("SIGINT".to_string()));
    }

    #[tokio::test]
    async fn test_multiple_cancel_subscribers() {
        let state = Arc::new(ShutdownState::new());

        let mut rx1 = state.subscribe_cancel();
        let mut rx2 = state.subscribe_cancel();
        let mut rx3 = state.subscribe_cancel();

        state.signal_shutdown_with_reason("SIGTERM");

        // All receivers should fire
        rx1.changed().await.unwrap();
        rx2.changed().await.unwrap();
        rx3.changed().await.unwrap();

        assert!(*rx1.borrow());
        assert!(*rx2.borrow());
        assert!(*rx3.borrow());
    }

    #[test]
    fn test_shutdown_state_seqcst_ordering() {
        // Verify SeqCst is used for the shutting_down flag
        let state = ShutdownState::new();
        assert!(!state.is_shutting_down());
        state.signal_shutdown_with_reason("test");
        // SeqCst ensures this is immediately visible
        assert!(state.is_shutting_down());
    }

    // -----------------------------------------------------------------------
    // auto_finalize / submit against a stub chain and a stub enclave
    // -----------------------------------------------------------------------

    use alloy::sol_types::{SolCall, SolEvent};
    use std::sync::atomic::AtomicUsize;
    use world_zk_sdk::abi::TEEMLVerifier;

    /// Throwaway key and its address (Anvil account 0); holds no funds anywhere.
    const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_SENDER: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
    const TEST_CONTRACT: &str = "0xA51c1fc2f0D1a1b8494Ed1FE312d7C3a78Ed91C0";

    /// JSON-RPC stub of a chain with one TEEMLVerifier holding `results`.
    struct StubChain {
        results: Vec<(B256, MLResult)>,
        /// When true, `eth_estimateGas` answers "execution reverted" (what a node
        /// returns for a `finalize` the contract rejects).
        estimate_gas_reverts: bool,
        /// Number of transactions the client tried to send.
        estimate_gas_calls: AtomicUsize,
    }

    fn ml_result(deadline: u64, finalized: bool, challenged: bool) -> MLResult {
        MLResult {
            enclave: Address::ZERO,
            submitter: TEST_SENDER.parse().unwrap(),
            modelHash: B256::repeat_byte(0x11),
            inputHash: B256::repeat_byte(0x22),
            resultHash: B256::repeat_byte(0x33),
            result: vec![1u8].into(),
            submittedAt: U256::from(1_700_000_000u64),
            challengeDeadline: U256::from(deadline),
            disputeDeadline: U256::ZERO,
            challengeBond: U256::ZERO,
            proverStakeAmount: U256::ZERO,
            finalized,
            challenged,
            challenger: Address::ZERO,
        }
    }

    fn submitted_log(result_id: B256, index: usize) -> serde_json::Value {
        let sender: Address = TEST_SENDER.parse().unwrap();
        serde_json::json!({
            "address": TEST_CONTRACT,
            "topics": [
                TEEMLVerifier::ResultSubmitted::SIGNATURE_HASH,
                result_id,
                B256::repeat_byte(0x11),
                sender.into_word(),
            ],
            "data": B256::repeat_byte(0x22),
            "blockHash": B256::repeat_byte(0xbb),
            "blockNumber": "0x10",
            "transactionHash": B256::repeat_byte(0xcc),
            "transactionIndex": "0x0",
            "logIndex": format!("0x{:x}", index),
            "removed": false,
        })
    }

    async fn stub_rpc(
        axum::extract::State(chain): axum::extract::State<Arc<StubChain>>,
        axum::Json(req): axum::Json<serde_json::Value>,
    ) -> axum::Json<serde_json::Value> {
        let reply = |result: Result<serde_json::Value, &str>| {
            let mut body = serde_json::json!({ "jsonrpc": "2.0", "id": req["id"] });
            match result {
                Ok(value) => body["result"] = value,
                Err(msg) => body["error"] = serde_json::json!({ "code": 3, "message": msg }),
            }
            axum::Json(body)
        };
        let logs = || -> Vec<serde_json::Value> {
            chain
                .results
                .iter()
                .enumerate()
                .map(|(i, (id, _))| submitted_log(*id, i))
                .collect()
        };
        match req["method"].as_str().unwrap_or_default() {
            "eth_chainId" => reply(Ok("0x7a69".into())),
            "eth_blockNumber" => reply(Ok("0x64".into())),
            "eth_getTransactionCount" => reply(Ok("0x0".into())),
            "eth_getLogs" => reply(Ok(logs().into())),
            "eth_call" => {
                let call = &req["params"][0];
                let input = call["input"].as_str().or(call["data"].as_str()).unwrap();
                let input = hex::decode(input.trim_start_matches("0x")).unwrap();
                let id = TEEMLVerifier::getResultCall::abi_decode(&input)
                    .unwrap()
                    .resultId;
                let result = chain
                    .results
                    .iter()
                    .find(|(rid, _)| *rid == id)
                    .map(|(_, r)| r.clone())
                    .unwrap_or_else(|| ml_result(0, false, false));
                let encoded = TEEMLVerifier::getResultCall::abi_encode_returns(&result);
                reply(Ok(format!("0x{}", hex::encode(encoded)).into()))
            }
            "eth_estimateGas" => {
                chain.estimate_gas_calls.fetch_add(1, Ordering::SeqCst);
                if chain.estimate_gas_reverts {
                    reply(Err("execution reverted"))
                } else {
                    reply(Ok("0x30000".into()))
                }
            }
            "eth_feeHistory" => reply(Ok(serde_json::json!({
                "oldestBlock": "0x5a",
                "baseFeePerGas": vec!["0x1"; 11],
                "gasUsedRatio": vec![0.5; 10],
                "reward": vec![vec!["0x1"]; 10],
            }))),
            "eth_sendRawTransaction" => reply(Ok(serde_json::json!(B256::repeat_byte(0xcc)))),
            "eth_getTransactionReceipt" => reply(Ok(serde_json::json!({
                "type": "0x2",
                "status": "0x1",
                "cumulativeGasUsed": "0x30000",
                "logs": logs(),
                "logsBloom": format!("0x{}", "00".repeat(256)),
                "transactionHash": B256::repeat_byte(0xcc),
                "transactionIndex": "0x0",
                "blockHash": B256::repeat_byte(0xbb),
                "blockNumber": "0x10",
                "gasUsed": "0x30000",
                "effectiveGasPrice": "0x1",
                "from": TEST_SENDER,
                "to": TEST_CONTRACT,
                "contractAddress": null,
            }))),
            // eth_getBlockByNumber included: `chain_now` then uses the wall clock.
            _ => reply(Err("method not supported by stub")),
        }
    }

    /// Serve `app` on an ephemeral local port and return its base URL.
    async fn serve_stub(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    async fn serve_stub_chain(
        results: Vec<(B256, MLResult)>,
        estimate_gas_reverts: bool,
    ) -> (String, Arc<StubChain>) {
        let chain = Arc::new(StubChain {
            results,
            estimate_gas_reverts,
            estimate_gas_calls: AtomicUsize::new(0),
        });
        let app = axum::Router::new()
            .route("/", axum::routing::post(stub_rpc))
            .with_state(chain.clone());
        (serve_stub(app).await, chain)
    }

    fn chain_breaker() -> CircuitBreaker {
        // Same settings as the chain breaker in `cmd_watch`.
        CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: std::time::Duration::from_secs(60),
            success_threshold_for_close: 1,
        })
    }

    async fn run_auto_finalize(rpc_url: &str, chain_cb: &CircuitBreaker) {
        let watcher = EventWatcher::new(rpc_url, TEST_CONTRACT.parse().unwrap());
        let chain = chain::ChainClient::new(rpc_url, TEST_KEY, TEST_CONTRACT).unwrap();
        let rpc_cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        auto_finalize(&watcher, &chain, 0, &rpc_cb, chain_cb).await;
    }

    #[test]
    fn test_is_finalizable() {
        let now = 2_000_000_000u64;
        assert!(is_finalizable(&ml_result(now - 1, false, false), now));
        assert!(is_finalizable(&ml_result(now, false, false), now));
        // Challenge window still open
        assert!(!is_finalizable(&ml_result(now + 1, false, false), now));
        // Already finalized / under dispute
        assert!(!is_finalizable(&ml_result(now - 1, true, false), now));
        assert!(!is_finalizable(&ml_result(now - 1, false, true), now));
        // Unknown result (getResult returns an all-zero struct)
        let mut unknown = ml_result(0, false, false);
        unknown.submittedAt = U256::ZERO;
        assert!(!is_finalizable(&unknown, now));
    }

    /// Results that were already handled, or whose challenge window is still open,
    /// are scanned again on every pass. They must not be sent to `finalize`, whose
    /// reverts used to open the chain circuit breaker after three of them.
    #[tokio::test]
    async fn test_auto_finalize_skips_results_that_cannot_be_finalized() {
        let far_future = u64::MAX / 2;
        let results = vec![
            (B256::repeat_byte(1), ml_result(1, true, false)),
            (B256::repeat_byte(2), ml_result(1, true, false)),
            (B256::repeat_byte(3), ml_result(1, true, false)),
            (B256::repeat_byte(4), ml_result(1, false, true)),
            (B256::repeat_byte(5), ml_result(far_future, false, false)),
        ];
        let (rpc_url, stub) = serve_stub_chain(results, true).await;
        let chain_cb = chain_breaker();

        run_auto_finalize(&rpc_url, &chain_cb).await;

        assert_eq!(
            stub.estimate_gas_calls.load(Ordering::SeqCst),
            0,
            "no finalize transaction must be attempted"
        );
        assert_eq!(chain_cb.consecutive_failures(), 0);
        assert!(chain_cb.allow_request().is_ok(), "breaker must stay closed");
    }

    /// A pending result past its challenge window is still sent to `finalize`,
    /// also when it comes after results that are skipped.
    #[tokio::test]
    async fn test_auto_finalize_attempts_pending_result_past_deadline() {
        let results = vec![
            (B256::repeat_byte(1), ml_result(1, true, false)),
            (B256::repeat_byte(2), ml_result(1, true, false)),
            (B256::repeat_byte(3), ml_result(1, true, false)),
            (B256::repeat_byte(4), ml_result(1, false, false)),
        ];
        // The stub rejects the transaction, which is enough to see the attempt.
        let (rpc_url, stub) = serve_stub_chain(results, true).await;
        let chain_cb = chain_breaker();

        run_auto_finalize(&rpc_url, &chain_cb).await;

        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 1);
        assert_eq!(chain_cb.consecutive_failures(), 1);
    }

    fn submit_test_config(rpc_url: &str, enclave_url: &str, dir: &std::path::Path) -> Config {
        let model = ModelConfig {
            name: "default".to_string(),
            path: "model.json".to_string(),
            model_format: "auto".to_string(),
            model_hash: None,
        };
        Config {
            rpc_url: rpc_url.to_string(),
            private_key: TEST_KEY.to_string().into(),
            tee_verifier_address: TEST_CONTRACT.to_string(),
            enclave_url: enclave_url.to_string(),
            model_path: model.path.clone(),
            proofs_dir: dir.join("proofs").display().to_string(),
            prover_stake_wei: "0".to_string(),
            precompute_bin: dir.join("prover.sh").display().to_string(),
            nitro_verification: false,
            expected_pcr0: None,
            attestation_cache_ttl: 300,
            attestation_freshness_secs: 600,
            max_proof_retries: 0,
            proof_retry_delay_secs: 1,
            webhook_url: None,
            retry_base_delay_ms: 1000,
            retry_max_delay_ms: 30000,
            models: vec![model],
            state_file: dir.join("state.json").display().to_string(),
            contract_addresses: vec![TEST_CONTRACT.to_string()],
            enclave_timeout_secs: 5,
            prover_timeout_secs: 30,
            escalation_timeout_secs: 900,
            prover_url: None,
            metrics_port: 0,
            metrics_bind_addr: "127.0.0.1".to_string(),
            dry_run: false,
            retry_max_attempts: 0,
            prover_max_retries: 0,
            prover_retry_delay_secs: 1,
            prover_retry_max_delay_secs: 1,
            circuit_desc_dir: String::new(),
            proof_archive_dir: dir.join("archive").display().to_string(),
        }
    }

    /// `submit` is a one-shot command: when it returns the process exits. The
    /// pre-computed proof and the archive entry must exist by then, otherwise they
    /// are never written and a later challenge finds no proof.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_cmd_submit_waits_for_proof_and_archive() {
        use std::os::unix::fs::PermissionsExt;

        let result_id = B256::repeat_byte(0x77);
        let (rpc_url, _stub) =
            serve_stub_chain(vec![(result_id, ml_result(1, false, false))], false).await;

        let word = |b: u8| format!("0x{}", hex::encode([b; 32]));
        let infer = serde_json::json!({
            "result": "0x01",
            "model_hash": word(0x11),
            "input_hash": word(0x22),
            "result_hash": word(0x33),
            "attestation": format!("0x{}", "00".repeat(65)),
            "enclave_address": TEST_SENDER,
        });
        let enclave = axum::Router::new()
            .route("/health", axum::routing::get(|| async { "ok" }))
            .route(
                "/infer",
                axum::routing::post(move || async move { axum::Json(infer) }),
            );
        let enclave_url = serve_stub(enclave).await;

        // Stand-in prover: takes a moment, then writes the file named by --output.
        let dir = tempfile::tempdir().unwrap();
        let prover = dir.path().join("prover.sh");
        std::fs::write(
            &prover,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  [ \"$1\" = --output ] && out=\"$2\"\n  shift\ndone\nsleep 0.3\necho '{}' > \"$out\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&prover, std::fs::Permissions::from_mode(0o755)).unwrap();

        let config = submit_test_config(&rpc_url, &enclave_url, dir.path());
        cmd_submit(&config, "[1.0, 2.0]", &config.models[0])
            .await
            .unwrap();

        let proof = dir
            .path()
            .join("proofs")
            .join(format!("0x{}.json", hex::encode(result_id)));
        assert!(
            proof.exists(),
            "proof must be written before submit returns"
        );

        let archived: Vec<_> = std::fs::read_dir(dir.path().join("archive"))
            .unwrap()
            .flat_map(|day| std::fs::read_dir(day.unwrap().path()).unwrap())
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(archived.len(), 1, "one archive entry: {:?}", archived);
        assert!(
            archived[0].starts_with("proof-") && archived[0].ends_with(".json"),
            "archive entry must be complete, not a temp file: {:?}",
            archived
        );
    }

    /// Collects formatted log output.
    #[derive(Clone, Default)]
    struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

    /// Keep a second dispatcher registered next to a test's own. While only
    /// one is registered, tracing asks the default subscriber of the thread
    /// that reaches a callsite first whether the callsite is of interest.
    /// That can be another test's thread, which has none, and then the
    /// callsite stays silent for the test that captures.
    fn keep_second_dispatcher() {
        static SECOND: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
        SECOND.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    }

    impl LogBuffer {
        /// Subscriber that writes events at `level` and above into this buffer.
        fn subscriber(&self, level: tracing::Level) -> impl tracing::Subscriber + Send + Sync {
            keep_second_dispatcher();
            let writer = self.clone();
            tracing_subscriber::fmt()
                .with_max_level(level)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish()
        }

        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for LogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_alert_config_accepts_both_pagerduty_spellings() {
        use tee_operator::alerting::AlertChannel;

        for spelling in ["pagerduty", "pager_duty"] {
            let json = format!(
                r##"{{"channels":[{{"type":"slack_webhook","url":"https://hooks.slack.com/services/T00/B00/xxx","channel":"#alerts"}},{{"type":"{}","routing_key":"R0","service_name":"tee-operator"}}]}}"##,
                spelling
            );
            let config = alert_config_from_env_value(Some(&json));
            assert_eq!(config.channels.len(), 2, "type \"{}\"", spelling);
            assert!(
                matches!(config.channels[1], AlertChannel::PagerDuty { .. }),
                "type \"{}\" gave {:?}",
                spelling,
                config.channels[1]
            );
        }
    }

    #[test]
    fn test_alert_config_empty_value_is_treated_as_unset() {
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::WARN));

        for value in [None, Some(""), Some("  \n\t")] {
            let config = alert_config_from_env_value(value);
            assert!(config.channels.is_empty());
        }
        assert_eq!(logs.contents(), "", "no warning for an empty value");
    }

    /// The value holds credentials. A value that does not parse is reported
    /// without quoting it, and with no slicing that could split a character.
    #[test]
    fn test_alert_config_parse_failure_log_has_no_part_of_the_value() {
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::WARN));

        // Byte 120 is the second byte of an 'é'; "headers" must be a map, so
        // serde's own message would quote the string given for it.
        let prefix = r#"{"channels":[{"type":"generic_webhook","url":"https://hooks.example.com/T00/SECRETPATH","headers":"Bearer SECRETHEADER "#;
        let json = format!("{}{}\"}}]}}", prefix, "é".repeat(40));
        assert_eq!(prefix.len() % 2, 1);
        assert!(!json.is_char_boundary(120));

        let config = alert_config_from_env_value(Some(&json));
        assert!(config.channels.is_empty());

        let logged = logs.contents();
        assert!(
            logged.contains("Failed to parse ALERT_CONFIG_JSON"),
            "expected the parse warning, got: {}",
            logged
        );
        assert!(
            logged.contains(&format!("json_len={}", json.len())),
            "byte length must be logged: {}",
            logged
        );
        for secret in [
            "SECRETPATH",
            "SECRETHEADER",
            "hooks.example.com",
            "é",
            "channels",
            "generic_webhook",
            "https://",
            "Bearer",
        ] {
            assert!(
                !logged.contains(secret),
                "log contains {:?} from the value: {}",
                secret,
                logged
            );
        }
    }

    /// Port 1 on localhost refuses the connection at once, so every RPC call
    /// fails with an error that names this URL.
    const SECRET_RPC_URL: &str = "http://127.0.0.1:1/v2/SECRETKEY";

    /// The watch loop logs a failed poll without the RPC URL's path.
    #[tokio::test]
    async fn test_failed_poll_log_omits_rpc_url_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = submit_test_config(SECRET_RPC_URL, "http://127.0.0.1:1", dir.path());
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::INFO));

        let mut watch = std::pin::pin!(cmd_watch(&config, 0, true));
        for _ in 0..200 {
            let step = std::time::Duration::from_millis(50);
            if let Ok(result) = tokio::time::timeout(step, watch.as_mut()).await {
                panic!("cmd_watch returned: {:?}", result);
            }
            if logs.contents().contains("RPC poll failed") {
                break;
            }
        }

        let logged = logs.contents();
        assert!(
            logged.contains("RPC poll failed: error sending request for url (http://127.0.0.1:1)"),
            "logged: {}",
            logged
        );
        assert!(!logged.contains("SECRETKEY"), "logged: {}", logged);
    }

    /// So does the auto-finalize pass.
    #[tokio::test]
    async fn test_failed_finalize_poll_log_omits_rpc_url_path() {
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::INFO));

        run_auto_finalize(SECRET_RPC_URL, &chain_breaker()).await;

        let logged = logs.contents();
        assert!(
            logged.contains("Failed to poll for finalizeable results"),
            "logged: {}",
            logged
        );
        assert!(!logged.contains("SECRETKEY"), "logged: {}", logged);
    }

    /// What is printed when a command fails keeps the error and its causes,
    /// with URLs cut to their origin.
    #[test]
    fn test_command_error_omits_url_paths() {
        let cause = anyhow::anyhow!("error sending request for url ({})", SECRET_RPC_URL);
        let printed = format!("{:?}", without_url_paths(cause.context("submit failed")));
        assert!(printed.starts_with("submit failed"), "{}", printed);
        assert!(
            printed.contains("error sending request for url (http://127.0.0.1:1)"),
            "{}",
            printed
        );
        assert!(!printed.contains("SECRETKEY"), "{}", printed);
    }

    /// The log line for a failed webhook delivery has no part of the URL.
    #[tokio::test]
    async fn test_failed_webhook_log_omits_webhook_url() {
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::DEBUG));

        let notifier =
            notifications::WebhookNotifier::new("http://127.0.0.1:1/services/T00/B00/SECRETTOKEN");
        notifier.notify_challenge("0xabc", "0xdef", 1, 2);
        // The first attempt fails at once; the retry that follows is not
        // waited for. (Behind an HTTP proxy the proxy's answer is logged.)
        for _ in 0..200 {
            if logs.contents().contains("Webhook") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let logged = logs.contents();
        assert!(logged.contains("Webhook"), "logged: {}", logged);
        assert!(!logged.contains("SECRETTOKEN"), "logged: {}", logged);
    }

    /// The `config_loaded` audit event must carry the mode the watcher runs
    /// in, and of the RPC URL only its origin: the path can hold an API key.
    #[tokio::test]
    async fn test_cmd_watch_audit_event_reports_dry_run() {
        use std::future::Future;

        let dir = tempfile::tempdir().unwrap();
        let config = submit_test_config(
            "http://127.0.0.1:1/v2/SECRETKEY",
            "http://127.0.0.1:1",
            dir.path(),
        );

        for dry_run in [true, false] {
            let logs = LogBuffer::default();
            let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::INFO));

            // A single poll runs the start-up code up to the first RPC call. The
            // tasks it spawns (signal handler, metrics server) are never polled.
            let mut watch = std::pin::pin!(cmd_watch(&config, 0, dry_run));
            std::future::poll_fn(|cx| {
                let _ = watch.as_mut().poll(cx);
                std::task::Poll::Ready(())
            })
            .await;

            let logged = logs.contents();
            // The span prefix also shows `dry_run`; look at the event's own fields.
            let fields = logged
                .lines()
                .find_map(|line| line.split_once("event=\"config_loaded\""))
                .map(|(_, fields)| fields)
                .unwrap_or_else(|| panic!("no config_loaded audit event in: {}", logged));
            assert!(
                fields.contains(&format!("dry_run={}", dry_run)),
                "cmd_watch(dry_run={}) logged:{}",
                dry_run,
                fields
            );
            assert!(
                fields.contains("rpc_url=http://127.0.0.1:1 "),
                "config_loaded logged:{}",
                fields
            );
            assert!(!logged.contains("SECRETKEY"), "start-up logged: {}", logged);
        }
    }
}
