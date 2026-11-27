mod chain;
use tee_operator::config;
pub mod deadline_monitor;
mod enclave;
mod metrics;
mod nitro;
pub mod notifications;
mod open_challenges;
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
use open_challenges::{challenge_id, OpenChallenges};
use prover::ProofManager;
use secrecy::ExposeSecret;
use store::{OperatorState, StateStore};
use tee_operator::alerting::{AlertConfig, AlertManager, AlertSeverity};
use tee_operator::audit;
use tee_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use tee_operator::verify::{PreVerifyResult, ProofPreVerifier};
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
///
/// With `config.dry_run` nothing is sent, so there is no result to archive or to
/// prove and no follow-up task.
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
    let chain_client = chain::ChainClient::new_with_dry_run(
        &config.rpc_url,
        config.private_key.expose_secret(),
        &config.tee_verifier_address,
        config.dry_run,
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
    if config.dry_run {
        println!("Dry run: the result was not submitted, no transaction was sent");
        return Ok(Vec::new());
    }
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

    /// Release, paired with the Acquire in [`Self::in_flight_count`]: a task's
    /// report, sent before this, is there for whoever reads the count it left.
    fn track_task_done(&self) {
        self.in_flight_tasks.fetch_sub(1, Ordering::Release);
    }

    fn in_flight_count(&self) -> u64 {
        self.in_flight_tasks.load(Ordering::Acquire)
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

/// How long the watch loop waits.
struct WatchTiming {
    /// Pause between two polls.
    poll_interval: std::time::Duration,
    /// Longest wait for running dispute tasks on shutdown.
    drain_timeout: std::time::Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(5),
            drain_timeout: std::time::Duration::from_secs(60),
        }
    }
}

/// Limit on dispute tasks running at the same time.
const MAX_CONCURRENT_DISPUTES: usize = 10;

/// What a dispute task needs, and the slots that limit how many run at once.
struct DisputeRunner {
    chain: Arc<chain::ChainClient>,
    proof_mgr: Arc<ProofManager>,
    alert_manager: Arc<AlertManager>,
    metrics: Arc<metrics::MetricsState>,
    chain_cb: Arc<CircuitBreaker>,
    pre_verifier: Option<Arc<ProofPreVerifier>>,
    shutdown: Arc<ShutdownState>,
    slots: Arc<tokio::sync::Semaphore>,
    /// Receives the id of every challenge whose task has finished.
    finished_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl DisputeRunner {
    /// Start a task for each waiting challenge, oldest first, while slots are free.
    ///
    /// Proof resolution runs in tasks of its own so that it does not block the
    /// event loop. A shutdown does not cancel them.
    fn start_waiting(&self, challenges: &mut OpenChallenges) {
        self.start_waiting_with(challenges, |result_id| self.dispute(result_id));
    }

    /// [`Self::start_waiting`] with `dispute` as the work of each task.
    fn start_waiting_with<F>(&self, challenges: &mut OpenChallenges, dispute: impl Fn(B256) -> F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        loop {
            let Ok(permit) = self.slots.clone().try_acquire_owned() else {
                return;
            };
            let Some(challenge) = challenges.next_waiting() else {
                return;
            };
            let id = challenge_id(&challenge.result_id);
            let dispute_span =
                tracing_setup::span_dispute(&id, &format!("{}", challenge.challenger));
            self.shutdown.track_task_start();
            let report = FinishedReport {
                id,
                finished_tx: self.finished_tx.clone(),
                shutdown: self.shutdown.clone(),
                _permit: permit,
                completed: false,
            };
            let task = dispute(challenge.result_id);
            tokio::spawn(
                async move {
                    task.await;
                    report.finish();
                }
                .instrument(dispute_span),
            );
        }
    }

    /// The dispute of one challenge, ready to be spawned.
    fn dispute(&self, result_id: B256) -> impl std::future::Future<Output = ()> + Send + 'static {
        let chain = self.chain.clone();
        let proof_mgr = self.proof_mgr.clone();
        let alert_manager = self.alert_manager.clone();
        let metrics = self.metrics.clone();
        let chain_cb = self.chain_cb.clone();
        let pre_verifier = self.pre_verifier.clone();
        async move {
            handle_challenge(
                &chain,
                &proof_mgr,
                result_id,
                &alert_manager,
                &metrics,
                &chain_cb,
                pre_verifier.as_ref(),
            )
            .await;
        }
    }
}

/// Ends a dispute task's turn when it is dropped: it frees the slot and stops
/// counting as in flight. At the end of the task, or when the task panics, it
/// also reports the challenge as finished, so that it is closed and recorded as
/// processed. Otherwise a task that panics would leave its challenge open for
/// ever, hold the saved block at the poll that delivered it and keep counting as
/// in flight. A task that the runtime drops unfinished reports nothing: its
/// dispute may not have been sent, so the challenge stays open and the next
/// start handles it again.
struct FinishedReport {
    id: String,
    finished_tx: tokio::sync::mpsc::UnboundedSender<String>,
    shutdown: Arc<ShutdownState>,
    /// The task's slot, freed with the report.
    _permit: tokio::sync::OwnedSemaphorePermit,
    /// Set when the dispute has returned.
    completed: bool,
}

impl FinishedReport {
    /// Report the dispute as returned. Taking the report whole keeps it inside
    /// the task: an async block that only set the field would capture the field
    /// alone and drop the report at once.
    fn finish(mut self) {
        self.completed = true;
    }
}

impl Drop for FinishedReport {
    fn drop(&mut self) {
        // Tokio drops the future of a task that panics while the panic unwinds.
        let report = if self.completed {
            true
        } else if std::thread::panicking() {
            tracing::error!(
                "Dispute task for {} panicked, the challenge is closed like a failed dispute",
                self.id
            );
            true
        } else {
            tracing::warn!(
                "Dispute task for {} was stopped before it finished, the challenge stays open",
                self.id
            );
            false
        };
        // Reported before the task stops counting as in flight: the shutdown
        // reads the reports once no task is left.
        if report {
            let _ = self.finished_tx.send(std::mem::take(&mut self.id));
        }
        self.shutdown.track_task_done();
    }
}

/// Close the challenges whose dispute task has finished and record them as processed.
fn record_finished(
    finished_rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    challenges: &mut OpenChallenges,
    op_state: &mut OperatorState,
) {
    while let Ok(id) = finished_rx.try_recv() {
        challenges.close(&id);
        op_state.processed_event_ids.insert(id);
    }
}

#[tracing::instrument(skip(config))]
async fn cmd_watch(config: &Config, metrics_port: u16, dry_run: bool) -> anyhow::Result<()> {
    let shutdown = Arc::new(ShutdownState::new());

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

    watch(
        config,
        metrics_port,
        dry_run,
        shutdown,
        Arc::new(metrics::MetricsState::new()),
        WatchTiming::default(),
    )
    .await
}

/// Watch the chain and handle challenges until `shutdown` is signalled.
async fn watch(
    config: &Config,
    metrics_port: u16,
    dry_run: bool,
    shutdown: Arc<ShutdownState>,
    metrics_state: Arc<metrics::MetricsState>,
    timing: WatchTiming,
) -> anyhow::Result<()> {
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

    // Initialize local proof pre-verifier (if circuit descriptions are available)
    let pre_verifier: Option<Arc<ProofPreVerifier>> = if !config.circuit_desc_dir.is_empty() {
        tracing::info!(
            circuit_desc_dir = %config.circuit_desc_dir,
            "Local proof pre-verification enabled"
        );
        Some(Arc::new(ProofPreVerifier::new(&config.circuit_desc_dir)))
    } else {
        tracing::debug!("Local proof pre-verification disabled (no CIRCUIT_DESC_DIR)");
        None
    };

    let (finished_tx, mut finished_rx) = tokio::sync::mpsc::unbounded_channel();
    let disputes = DisputeRunner {
        chain: chain_client.clone(),
        proof_mgr,
        alert_manager: alert_manager.clone(),
        metrics: metrics_state.clone(),
        chain_cb: chain_cb.clone(),
        pre_verifier,
        shutdown: shutdown.clone(),
        slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DISPUTES)),
        finished_tx,
    };

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
    let mut open_challenges = OpenChallenges::default();

    while !shutdown.is_shutting_down() {
        // Challenges that waited for a slot get the ones freed since the last pass
        record_finished(&mut finished_rx, &mut open_challenges, &mut op_state);
        disputes.start_waiting(&mut open_challenges);

        // Poll for new events (through RPC circuit breaker)
        let (events, next_block) = if let Err(e) = rpc_cb.allow_request() {
            tracing::warn!("RPC circuit breaker: {}", e);
            (vec![], from_block)
        } else {
            match watcher.poll_events(from_block).await {
                Ok(r) => {
                    rpc_cb.record_success();
                    metrics_state.mark_polled();
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

        let poll_block = from_block;
        from_block = next_block;

        // Update last polled block
        metrics_state.set_last_block(from_block);

        for event in &events {
            if shutdown.is_shutting_down() {
                tracing::info!("Shutdown in progress, skipping remaining events");
                // The next start polls this range again
                from_block = poll_block;
                break;
            }
            match event {
                TEEEvent::ResultChallenged {
                    result_id,
                    challenger,
                } => {
                    let rid_hex = challenge_id(result_id);

                    // Dedup: skip events already processed in a prior session
                    if op_state.processed_event_ids.contains(&rid_hex) {
                        tracing::debug!(
                            result_id = %result_id,
                            "Skipping already-processed challenge event"
                        );
                        continue;
                    }

                    // Dedup: skip events whose challenge is still being handled
                    if open_challenges.is_open(&rid_hex) {
                        tracing::debug!(
                            result_id = %result_id,
                            "Skipping challenge event that is already being handled"
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

                    // Open until its dispute task has finished; only then is it
                    // recorded as processed
                    open_challenges.open(*result_id, *challenger, poll_block);
                    disputes.start_waiting(&mut open_challenges);
                    if open_challenges.waiting_count() > 0 {
                        tracing::warn!(
                            waiting = open_challenges.waiting_count(),
                            "Max concurrent proof submissions reached, challenge queued until a slot is free: {}",
                            rid_hex
                        );
                        metrics_state.record_error();
                    }
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
        op_state.last_polled_block = open_challenges.resume_block(from_block);
        if let Err(e) = state_store.save(&op_state) {
            tracing::warn!("Failed to persist watcher state: {}", e);
        }

        // Every ~60 seconds (12 iterations * 5s poll interval), check for finalizeable results
        finalize_counter += 1;
        if finalize_counter >= 12 && !dry_run {
            finalize_counter = 0;
            auto_finalize(
                &watcher,
                &chain_client,
                from_block.saturating_sub(7200),
                &rpc_cb,
                &chain_cb,
                &shutdown,
            )
            .await;
        }

        // Use select to allow shutdown to interrupt the sleep
        let mut cancel_rx = shutdown.subscribe_cancel();
        tokio::select! {
            _ = tokio::time::sleep(timing.poll_interval) => {},
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
            "Waiting for in-flight tasks to complete ({}s timeout)",
            timing.drain_timeout.as_secs()
        );
        let deadline = Instant::now() + timing.drain_timeout;
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

    // Final state save before shutdown. Challenges that are still open are not
    // in it as processed, and the next start polls their blocks again.
    record_finished(&mut finished_rx, &mut open_challenges, &mut op_state);
    op_state.last_polled_block = open_challenges.resume_block(from_block);
    if let Err(e) = state_store.save(&op_state) {
        tracing::warn!("Failed to save state on shutdown: {}", e);
    } else {
        tracing::info!(
            last_polled_block = op_state.last_polled_block,
            open_challenges = open_challenges.len(),
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

    // 2. Start watching
    tracing::info!("Starting watch loop...");
    cmd_watch(config, metrics_port, config.dry_run).await
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
    let chain_client = chain::ChainClient::new_with_dry_run(
        &config.rpc_url,
        config.private_key.expose_secret(),
        &config.tee_verifier_address,
        config.dry_run,
    )?;

    let tx_hash = chain_client
        .register_enclave(enclave_addr, image_hash)
        .await?;
    if config.dry_run {
        println!("Dry run: the enclave was not registered, no transaction was sent");
        return Ok(());
    }
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
    pre_verifier: Option<&Arc<ProofPreVerifier>>,
) {
    let rid_hex = challenge_id(&result_id);

    // After a restart a challenge comes round again whose dispute may have been
    // mined before the stop. Disputing it again would only revert. This reads
    // mined state alone: it does not stop a second transaction while the first
    // is still pending.
    let state = tokio::time::timeout(DISPUTE_STATE_TIMEOUT, dispute_state(chain, result_id)).await;
    match state {
        Ok(Ok(DisputeState::Settled)) => {
            tracing::info!(
                "Challenge on {} needs no dispute any more, it is settled on chain",
                rid_hex
            );
            return;
        }
        Ok(Ok(DisputeState::Lost)) => {
            report_lost_dispute(&rid_hex, alert_manager, metrics);
            return;
        }
        Ok(Ok(DisputeState::Open)) => {}
        Ok(Err(e)) => tracing::warn!(
            "Could not read the state of {}, disputing it anyway: {}",
            rid_hex,
            rpc_error_text(&e)
        ),
        Err(_) => tracing::warn!(
            "Reading the state of {} took over {} s, disputing it anyway",
            rid_hex,
            DISPUTE_STATE_TIMEOUT.as_secs()
        ),
    }

    let resolve_result = match verified_proof(proof_mgr, &rid_hex, pre_verifier).await {
        Ok(proof) => resolve_with_proof(chain, result_id, &proof, metrics, chain_cb).await,
        Err(e) => Err(e),
    };

    match resolve_result {
        Ok(tx) => {
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

/// Longest wait for the state read before a dispute; a node that does not answer
/// must not hold the dispute up until its deadline.
const DISPUTE_STATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the chain says about the challenge on a result.
#[derive(Debug, PartialEq, Eq)]
enum DisputeState {
    /// Nothing settled yet: dispute it.
    Open,
    /// The contract resolved the dispute for the prover.
    Settled,
    /// The contract resolved the dispute against the prover (timeout, or a
    /// proof that failed on chain).
    Lost,
}

/// The state of the challenge on a result, read from `disputeResolved` and
/// `disputeProverWon` only. The contract sets both, with `finalized`, in the
/// transaction that settles a dispute, and they never change after it, so on
/// one node the two reads cannot disagree the way a read of `finalized` taken
/// before or after the settlement could. Reads that reach nodes at different
/// heights can report a won dispute as lost (a spurious dispute_lost alert),
/// never a lost one as settled. Anything short of a resolved dispute is
/// disputed: a node that lags behind cannot make the operator skip a dispute,
/// and a dispute of a result that is settled some other way reverts and is
/// reported.
async fn dispute_state(
    chain: &chain::ChainClient,
    result_id: B256,
) -> anyhow::Result<DisputeState> {
    if !chain.dispute_resolved(result_id).await? {
        return Ok(DisputeState::Open);
    }
    Ok(if chain.dispute_prover_won(result_id).await? {
        DisputeState::Settled
    } else {
        DisputeState::Lost
    })
}

/// Report a dispute that the contract resolved against the prover. Nothing is
/// sent: the dispute is over. It is a failed dispute, but not a chain failure.
fn report_lost_dispute(
    rid_hex: &str,
    alert_manager: &AlertManager,
    metrics: &metrics::MetricsState,
) {
    let error = "the dispute was resolved against the prover";
    tracing::error!("Dispute on {} is lost: {}", rid_hex, error);
    audit::log_dispute_failed(rid_hex, "", error);
    metrics.record_dispute_failed();
    let mut meta = HashMap::new();
    meta.insert("result_id".to_string(), rid_hex.to_string());
    if let Err(e) = alert_manager.send_alert(
        AlertSeverity::Critical,
        "dispute_lost",
        "operator",
        &format!("Dispute lost: {}", error),
        meta,
    ) {
        tracing::warn!("Failed to send dispute_lost alert: {}", e);
    }
}

/// The proof to submit for a challenged result: read from the proofs directory,
/// waiting up to 60 s for it, and checked by the local pre-verifier, if there is one.
async fn verified_proof(
    proof_mgr: &ProofManager,
    rid_hex: &str,
    pre_verifier: Option<&Arc<ProofPreVerifier>>,
) -> anyhow::Result<store::StoredProof> {
    let proof = match proof_mgr.read_proof(rid_hex) {
        Ok(Some(proof)) => {
            tracing::info!("Found pre-computed proof for {}", rid_hex);
            proof
        }
        Ok(None) => {
            tracing::warn!(
                "No pre-computed proof for {}. Waiting up to 60s...",
                rid_hex
            );
            match proof_mgr.wait_for_proof(rid_hex, 60).await {
                Ok(true) => match proof_mgr.read_proof(rid_hex) {
                    Ok(Some(proof)) => proof,
                    _ => anyhow::bail!("Proof disappeared after wait"),
                },
                Ok(false) => {
                    tracing::error!("Proof not available after timeout for {}", rid_hex);
                    anyhow::bail!("Proof timeout for {}", rid_hex);
                }
                Err(e) => {
                    tracing::error!("Error waiting for proof: {}", e);
                    return Err(e);
                }
            }
        }
        Err(e) => {
            tracing::error!("Error reading proof: {}", e);
            return Err(e);
        }
    };

    if let Some(pv) = pre_verifier {
        // Converts the binary crate's StoredProof to the library crate's StoredProof
        // since they are compiled as separate crate types.
        let lib_proof = tee_operator::store::StoredProof {
            proof_hex: proof.proof_hex.clone(),
            circuit_hash: proof.circuit_hash.clone(),
            public_inputs_hex: proof.public_inputs_hex.clone(),
            gens_hex: proof.gens_hex.clone(),
        };
        // Verification takes seconds of CPU time: keep it off the async workers.
        let pv = pv.clone();
        let verdict = tokio::task::spawn_blocking(move || pv.pre_verify(&lib_proof))
            .await
            .map_err(|e| anyhow::anyhow!("Pre-verification task failed: {}", e))?;
        match verdict {
            PreVerifyResult::Verified => {
                tracing::info!("Pre-verification passed for {}", rid_hex);
            }
            PreVerifyResult::Failed(reason) => {
                // A bad proof is not submitted: it would only waste gas.
                tracing::error!("Pre-verification FAILED for {}: {}", rid_hex, reason);
                anyhow::bail!("Pre-verification failed: {}", reason);
            }
            PreVerifyResult::Skipped(reason) => {
                tracing::debug!("Pre-verification skipped for {}: {}", rid_hex, reason);
            }
        }
    }

    Ok(proof)
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
    chain_cb: &CircuitBreaker,
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
        .await
        .inspect_err(|_| chain_cb.record_failure())?;
    chain_cb.record_success();

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
    shutdown: &ShutdownState,
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
            // Each finalize waits for its receipt; the shutdown's drain should
            // not wait for the rest of the window.
            if shutdown.is_shutting_down() {
                tracing::info!("Shutdown in progress, stopping auto-finalize");
                break;
            }
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

    /// JSON-RPC stub of a chain with one TEEMLVerifier holding `results`. A
    /// result it does not hold reads as challenged and not finalized.
    struct StubChain {
        results: Vec<(B256, MLResult)>,
        /// Results the contract reports as challenged.
        challenges: Vec<B256>,
        /// Results whose dispute the contract reports as resolved, won by the
        /// prover unless the result is in `prover_lost` too.
        resolved: Vec<B256>,
        /// Resolved results whose dispute the prover lost.
        prover_lost: Vec<B256>,
        /// When true, reading a result or its dispute state fails.
        reads_fail: bool,
        /// When true, reading a result or its dispute state gets no answer.
        reads_hang: bool,
        /// When true, `eth_blockNumber` fails, and so every poll of the watcher.
        polls_fail: AtomicBool,
        /// When true, `eth_estimateGas` answers "execution reverted" (what a node
        /// returns for a `finalize` the contract rejects).
        estimate_gas_reverts: bool,
        /// Number of transactions the client tried to send.
        estimate_gas_calls: AtomicUsize,
        /// Number of `eth_blockNumber` requests: one per poll of the watcher.
        polls: AtomicUsize,
        /// Number of disputes the client started to resolve. Each is answered
        /// with an error once it has a permit of `dispute_gate`.
        disputes: AtomicUsize,
        dispute_gate: tokio::sync::Semaphore,
        /// Signalled before the answer to `eth_getLogs`.
        shutdown_on_logs: Option<Arc<ShutdownState>>,
    }

    /// A chain without results. Disputes stay unanswered until the test adds
    /// permits to `dispute_gate`.
    fn empty_chain() -> StubChain {
        StubChain {
            results: Vec::new(),
            challenges: Vec::new(),
            resolved: Vec::new(),
            prover_lost: Vec::new(),
            reads_fail: false,
            reads_hang: false,
            polls_fail: AtomicBool::new(false),
            estimate_gas_reverts: false,
            estimate_gas_calls: AtomicUsize::new(0),
            polls: AtomicUsize::new(0),
            disputes: AtomicUsize::new(0),
            dispute_gate: tokio::sync::Semaphore::new(0),
            shutdown_on_logs: None,
        }
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

    fn challenged_log(result_id: B256, index: usize) -> serde_json::Value {
        let challenger: Address = TEST_SENDER.parse().unwrap();
        serde_json::json!({
            "address": TEST_CONTRACT,
            "topics": [TEEMLVerifier::ResultChallenged::SIGNATURE_HASH, result_id],
            "data": challenger.into_word(),
            "blockHash": B256::repeat_byte(0xbb),
            "blockNumber": "0x11",
            "transactionHash": B256::repeat_byte(0xcd),
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
            let submitted = chain
                .results
                .iter()
                .enumerate()
                .map(|(i, (id, _))| submitted_log(*id, i));
            let challenged = chain
                .challenges
                .iter()
                .enumerate()
                .map(|(i, id)| challenged_log(*id, i));
            submitted.chain(challenged).collect()
        };
        match req["method"].as_str().unwrap_or_default() {
            "eth_chainId" => reply(Ok("0x7a69".into())),
            "eth_blockNumber" => {
                chain.polls.fetch_add(1, Ordering::SeqCst);
                if chain.polls_fail.load(Ordering::SeqCst) {
                    return reply(Err("stub chain is down"));
                }
                reply(Ok("0x64".into()))
            }
            "eth_getTransactionCount" => reply(Ok("0x0".into())),
            "eth_getLogs" => {
                if let Some(shutdown) = &chain.shutdown_on_logs {
                    shutdown.signal_shutdown();
                }
                reply(Ok(logs().into()))
            }
            "eth_call" => {
                let call = &req["params"][0];
                let input = call["input"].as_str().or(call["data"].as_str()).unwrap();
                let input = hex::decode(input.trim_start_matches("0x")).unwrap();
                // The first call of every dispute resolution.
                if input[..4] == TEEMLVerifier::remainderVerifierCall::SELECTOR {
                    chain.disputes.fetch_add(1, Ordering::SeqCst);
                    chain.dispute_gate.acquire().await.unwrap().forget();
                    return reply(Err("no RemainderVerifier in the stub"));
                }
                if chain.reads_fail {
                    return reply(Err("stub cannot read results"));
                }
                if chain.reads_hang {
                    std::future::pending::<()>().await;
                }
                if input[..4] == TEEMLVerifier::disputeResolvedCall::SELECTOR {
                    let id = TEEMLVerifier::disputeResolvedCall::abi_decode(&input)
                        .unwrap()
                        .resultId;
                    let resolved = chain.resolved.contains(&id);
                    let encoded = TEEMLVerifier::disputeResolvedCall::abi_encode_returns(&resolved);
                    return reply(Ok(format!("0x{}", hex::encode(encoded)).into()));
                }
                if input[..4] == TEEMLVerifier::disputeProverWonCall::SELECTOR {
                    let id = TEEMLVerifier::disputeProverWonCall::abi_decode(&input)
                        .unwrap()
                        .resultId;
                    let won = chain.resolved.contains(&id) && !chain.prover_lost.contains(&id);
                    let encoded = TEEMLVerifier::disputeProverWonCall::abi_encode_returns(&won);
                    return reply(Ok(format!("0x{}", hex::encode(encoded)).into()));
                }
                let id = TEEMLVerifier::getResultCall::abi_decode(&input)
                    .unwrap()
                    .resultId;
                let result = chain
                    .results
                    .iter()
                    .find(|(rid, _)| *rid == id)
                    .map(|(_, r)| r.clone())
                    .unwrap_or_else(|| ml_result(0, false, true));
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
        serve_chain(StubChain {
            results,
            estimate_gas_reverts,
            ..empty_chain()
        })
        .await
    }

    async fn serve_chain(chain: StubChain) -> (String, Arc<StubChain>) {
        let chain = Arc::new(chain);
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
        auto_finalize(
            &watcher,
            &chain,
            0,
            &rpc_cb,
            chain_cb,
            &ShutdownState::new(),
        )
        .await;
    }

    /// A shutdown signalled during the scan stops the pass before the next
    /// result, so that the drain starts without waiting for its finalizes.
    #[tokio::test]
    async fn test_auto_finalize_stops_between_results_on_shutdown() {
        let shutdown = Arc::new(ShutdownState::new());
        let (rpc_url, stub) = serve_chain(StubChain {
            results: vec![
                (B256::repeat_byte(1), ml_result(1, false, false)),
                (B256::repeat_byte(2), ml_result(1, false, false)),
            ],
            shutdown_on_logs: Some(shutdown.clone()),
            ..empty_chain()
        })
        .await;
        let watcher = EventWatcher::new(&rpc_url, TEST_CONTRACT.parse().unwrap());
        let chain = chain::ChainClient::new(&rpc_url, TEST_KEY, TEST_CONTRACT).unwrap();
        let rpc_cb = CircuitBreaker::new(CircuitBreakerConfig::default());

        auto_finalize(&watcher, &chain, 0, &rpc_cb, &chain_breaker(), &shutdown).await;

        assert!(shutdown.is_shutting_down());
        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 0);
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

    /// An enclave that answers `/infer`, `/info` and `/attestation` (with an
    /// unsigned development document).
    fn stub_enclave() -> axum::Router {
        use base64::Engine;
        use ciborium::Value;

        let word = |b: u8| format!("0x{}", hex::encode([b; 32]));
        let infer = serde_json::json!({
            "result": "0x01",
            "model_hash": word(0x11),
            "input_hash": word(0x22),
            "result_hash": word(0x33),
            "attestation": format!("0x{}", "00".repeat(65)),
            "enclave_address": TEST_SENDER,
        });
        let info = serde_json::json!({
            "enclave_address": TEST_SENDER,
            "model_hash": word(0x11),
        });

        let sender: Address = TEST_SENDER.parse().unwrap();
        let text = |s: &str| Value::Text(s.to_string());
        let payload = Value::Map(vec![
            (text("module_id"), text("test-mock")),
            (text("timestamp"), Value::Integer(1.into())),
            (text("digest"), text("SHA384")),
            (
                text("pcrs"),
                Value::Map(vec![(Value::Integer(0.into()), Value::Bytes(vec![0; 48]))]),
            ),
            (text("public_key"), Value::Bytes(sender.to_vec())),
            (text("user_data"), Value::Bytes(vec![1; 32])),
            (text("nonce"), Value::Null),
        ]);
        let mut payload_bytes = Vec::new();
        ciborium::into_writer(&payload, &mut payload_bytes).unwrap();
        let cose = Value::Array(vec![
            Value::Bytes(vec![]),
            Value::Map(vec![]),
            Value::Bytes(payload_bytes),
            Value::Bytes(vec![0; 96]),
        ]);
        let mut document = Vec::new();
        ciborium::into_writer(&cose, &mut document).unwrap();
        let attestation = serde_json::json!({
            "document": base64::engine::general_purpose::STANDARD.encode(document),
            "enclave_address": TEST_SENDER,
            "is_nitro": false,
            "pcr0": "00".repeat(48),
        });

        axum::Router::new()
            .route("/health", axum::routing::get(|| async { "ok" }))
            .route(
                "/infer",
                axum::routing::post(move || async move { axum::Json(infer) }),
            )
            .route(
                "/info",
                axum::routing::get(move || async move { axum::Json(info) }),
            )
            .route(
                "/attestation",
                axum::routing::get(move || async move { axum::Json(attestation) }),
            )
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

        let enclave_url = serve_stub(stub_enclave()).await;

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

    // -----------------------------------------------------------------------
    // dry_run in submit, run and register
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dry_run_submit_sends_and_writes_nothing() {
        let (rpc_url, stub) = serve_chain(empty_chain()).await;
        let enclave_url = serve_stub(stub_enclave()).await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = submit_test_config(&rpc_url, &enclave_url, dir.path());
        config.dry_run = true;

        let background = submit(&config, "[1.0, 2.0]", &config.models[0])
            .await
            .unwrap();
        assert!(background.is_empty(), "no follow-up task in dry-run");
        cmd_submit(&config, "[1.0, 2.0]", &config.models[0])
            .await
            .unwrap();

        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 0);
        let written: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert!(written.is_empty(), "no archive and no proof: {:?}", written);
    }

    #[tokio::test]
    async fn test_dry_run_register_sends_nothing() {
        let (rpc_url, stub) = serve_chain(empty_chain()).await;
        let enclave_url = serve_stub(stub_enclave()).await;
        let dir = tempfile::tempdir().unwrap();
        let mut config = submit_test_config(&rpc_url, &enclave_url, dir.path());

        config.dry_run = true;
        cmd_register(&config, None, true).await.unwrap();
        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 0);

        // The same call without dry-run reaches the chain.
        config.dry_run = false;
        cmd_register(&config, None, true).await.unwrap();
        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 1);
    }

    /// `run` in dry-run: neither its submit step nor its watch loop sends anything.
    #[tokio::test]
    async fn test_dry_run_run_sends_nothing() {
        let result_id = B256::repeat_byte(0x55);
        let (rpc_url, stub) = serve_chain(StubChain {
            challenges: vec![result_id],
            ..empty_chain()
        })
        .await;
        let enclave_url = serve_stub(stub_enclave()).await;
        let dir = tempfile::tempdir().unwrap();
        write_proof(dir.path(), result_id);
        let mut config = submit_test_config(&rpc_url, &enclave_url, dir.path());
        config.dry_run = true;

        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::INFO));
        let mut run = std::pin::pin!(cmd_run(&config, "[1.0, 2.0]", 0, &config.models[0]));
        let handled = || {
            logs.contents().contains("[DRY-RUN] Would submit proof")
                || stub.disputes.load(Ordering::SeqCst) > 0
        };
        for _ in 0..600 {
            let step = std::time::Duration::from_millis(50);
            if let Ok(result) = tokio::time::timeout(step, run.as_mut()).await {
                panic!("cmd_run returned: {:?}", result);
            }
            if handled() {
                break;
            }
        }

        assert!(handled(), "logged: {}", logs.contents());
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 0);
        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 0);
        assert!(!dir.path().join("archive").exists());
    }

    // -----------------------------------------------------------------------
    // Watch loop: open challenges, saved state, shutdown, readiness
    // -----------------------------------------------------------------------

    /// Block the saved state of these tests starts from; the stub chain's
    /// latest block is 100.
    const START_BLOCK: u64 = 40;

    fn write_proof(dir: &std::path::Path, result_id: B256) {
        write_proof_text(
            dir,
            result_id,
            r#"{"proof_hex":"0x01","circuit_hash":"0x1111111111111111111111111111111111111111111111111111111111111111","public_inputs_hex":"0x02","gens_hex":"0x03"}"#,
        );
    }

    fn write_proof_text(dir: &std::path::Path, result_id: B256, text: &str) {
        let proofs = dir.join("proofs");
        std::fs::create_dir_all(&proofs).unwrap();
        std::fs::write(
            proofs.join(format!("{}.json", challenge_id(&result_id))),
            text,
        )
        .unwrap();
    }

    /// Wait until `condition` holds; fails the test after 30 s.
    async fn wait_until(what: &str, condition: impl Fn() -> bool) {
        for _ in 0..6000 {
            if condition() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("timed out waiting until {}", what);
    }

    /// A watcher with its own directory, where its state file says that it has
    /// polled up to [`START_BLOCK`] and its proofs directory holds a proof for
    /// every challenge of the stub chain.
    struct WatchTest {
        dir: tempfile::TempDir,
        config: Config,
        stub: Arc<StubChain>,
        shutdown: Arc<ShutdownState>,
        metrics: Arc<metrics::MetricsState>,
        drain_timeout: std::time::Duration,
    }

    impl WatchTest {
        async fn new(chain: StubChain) -> Self {
            let shutdown = chain
                .shutdown_on_logs
                .clone()
                .unwrap_or_else(|| Arc::new(ShutdownState::new()));
            let (rpc_url, stub) = serve_chain(chain).await;
            let dir = tempfile::tempdir().unwrap();
            for id in &stub.challenges {
                write_proof(dir.path(), *id);
            }
            let test = Self {
                config: submit_test_config(&rpc_url, "http://127.0.0.1:1", dir.path()),
                dir,
                stub,
                shutdown,
                metrics: Arc::new(metrics::MetricsState::new()),
                drain_timeout: std::time::Duration::from_secs(60),
            };
            test.save_state(&OperatorState {
                last_polled_block: START_BLOCK,
                ..OperatorState::default()
            });
            test
        }

        /// Run the watch loop next to `script`, until the script has shut it down.
        async fn run(&self, script: impl std::future::Future<Output = ()>) {
            let timing = WatchTiming {
                poll_interval: std::time::Duration::from_millis(10),
                drain_timeout: self.drain_timeout,
            };
            let (watched, ()) = tokio::join!(
                watch(
                    &self.config,
                    0,
                    false,
                    self.shutdown.clone(),
                    self.metrics.clone(),
                    timing,
                ),
                script
            );
            watched.unwrap();
        }

        /// The same watcher after a restart from `state`.
        fn restarted(self, state: &OperatorState) -> Self {
            let test = Self {
                shutdown: Arc::new(ShutdownState::new()),
                metrics: Arc::new(metrics::MetricsState::new()),
                ..self
            };
            test.save_state(state);
            test
        }

        fn save_state(&self, state: &OperatorState) {
            StateStore::new(&self.config.state_file)
                .save(state)
                .unwrap();
        }

        fn saved_state(&self) -> OperatorState {
            StateStore::new(&self.config.state_file)
                .load()
                .unwrap()
                .unwrap()
        }

        fn disputes(&self) -> usize {
            self.stub.disputes.load(Ordering::SeqCst)
        }

        async fn wait_for_disputes(&self, count: usize) {
            wait_until("the disputes have started", || self.disputes() >= count).await;
        }

        /// Wait until the loop has saved its state at least once more.
        async fn wait_for_save(&self) {
            let polls = self.stub.polls.load(Ordering::SeqCst);
            wait_until("the loop has gone round", || {
                self.stub.polls.load(Ordering::SeqCst) >= polls + 3
            })
            .await;
        }
    }

    fn challenges(count: u8) -> Vec<B256> {
        (1..=count).map(B256::repeat_byte).collect()
    }

    fn chain_with_challenges(count: u8) -> StubChain {
        StubChain {
            challenges: challenges(count),
            ..empty_chain()
        }
    }

    /// While the dispute runs the saved state points at the poll that delivered
    /// the challenge and does not have its id; afterwards both are up to date.
    #[tokio::test]
    async fn test_challenge_is_saved_as_processed_when_its_dispute_has_finished() {
        let test = WatchTest::new(chain_with_challenges(1)).await;
        let id = challenge_id(&challenges(1)[0]);

        test.run(async {
            test.wait_for_disputes(1).await;
            test.wait_for_save().await;
            let state = test.saved_state();
            assert!(state.active_disputes.contains_key(&id));
            assert!(!state.processed_event_ids.contains(&id));
            assert_eq!(state.last_polled_block, START_BLOCK);

            test.stub.dispute_gate.add_permits(1);
            wait_until("the id is saved", || {
                test.saved_state().processed_event_ids.contains(&id)
            })
            .await;
            assert_eq!(test.saved_state().last_polled_block, 101);
            test.shutdown.signal_shutdown();
        })
        .await;

        let state = test.saved_state();
        assert!(state.processed_event_ids.contains(&id));
        assert_eq!(state.last_polled_block, 101);
        assert_eq!(test.disputes(), 1, "a failed dispute is not tried again");
    }

    /// A shutdown leaves a running dispute alone and waits for it.
    #[tokio::test]
    async fn test_shutdown_waits_for_running_dispute() {
        let test = WatchTest::new(chain_with_challenges(1)).await;
        let id = challenge_id(&challenges(1)[0]);

        test.run(async {
            test.wait_for_disputes(1).await;
            test.shutdown.signal_shutdown();
            // Time for a cancelled task to end before the dispute can.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            test.stub.dispute_gate.add_permits(1);
        })
        .await;

        // The stub answers with an error, so the dispute ran to its end if it failed.
        let failed = test.metrics.total_disputes_failed.load(Ordering::SeqCst);
        assert_eq!(failed, 1);
        assert_eq!(test.shutdown.in_flight_count(), 0);
        let state = test.saved_state();
        assert!(state.processed_event_ids.contains(&id));
        assert_eq!(state.last_polled_block, 101);
    }

    /// What is still running when the wait ends, or was still waiting for a
    /// slot, stays open in the saved state.
    #[tokio::test]
    async fn test_shutdown_leaves_unfinished_challenges_open() {
        let mut test = WatchTest::new(chain_with_challenges(11)).await;
        test.drain_timeout = std::time::Duration::ZERO;

        test.run(async {
            test.wait_for_disputes(10).await;
            test.shutdown.signal_shutdown();
        })
        .await;

        assert_eq!(test.disputes(), 10);
        let state = test.saved_state();
        assert!(state.processed_event_ids.is_empty());
        assert_eq!(state.last_polled_block, START_BLOCK);
    }

    /// A start from the state saved during a dispute handles it again, one from
    /// the state saved after it does not.
    #[tokio::test]
    async fn test_restart_handles_unfinished_challenge_again() {
        let test = WatchTest::new(chain_with_challenges(1)).await;
        let id = challenge_id(&challenges(1)[0]);

        let unfinished = std::cell::RefCell::new(None);
        test.run(async {
            test.wait_for_disputes(1).await;
            test.wait_for_save().await;
            *unfinished.borrow_mut() = Some(test.saved_state());
            test.shutdown.signal_shutdown();
            test.stub.dispute_gate.add_permits(1);
        })
        .await;
        let finished = test.saved_state();

        let test = test.restarted(&unfinished.take().unwrap());
        test.run(async {
            test.wait_for_disputes(2).await;
            test.stub.dispute_gate.add_permits(1);
            wait_until("the id is saved", || {
                test.saved_state().processed_event_ids.contains(&id)
            })
            .await;
            test.shutdown.signal_shutdown();
        })
        .await;
        assert_eq!(test.disputes(), 2);

        // Also when the finished challenge's block is polled again.
        let replay = OperatorState {
            last_polled_block: START_BLOCK,
            ..finished
        };
        let test = test.restarted(&replay);
        test.run(async {
            test.wait_for_save().await;
            test.shutdown.signal_shutdown();
        })
        .await;
        assert_eq!(test.disputes(), 2);
        assert!(test.dir.path().exists());
    }

    /// With all ten slots busy the eleventh challenge waits, and is started when
    /// one of the ten disputes finishes.
    #[tokio::test]
    async fn test_eleventh_challenge_starts_when_a_slot_is_free() {
        let test = WatchTest::new(chain_with_challenges(11)).await;

        test.run(async {
            test.wait_for_disputes(10).await;
            test.wait_for_save().await;
            assert_eq!(test.disputes(), 10);
            assert_eq!(test.shutdown.in_flight_count(), 10);
            assert_eq!(test.metrics.total_errors.load(Ordering::SeqCst), 1);
            assert_eq!(test.saved_state().last_polled_block, START_BLOCK);

            test.stub.dispute_gate.add_permits(1);
            test.wait_for_disputes(11).await;
            wait_until("the first id is saved", || {
                test.saved_state().processed_event_ids.len() == 1
            })
            .await;
            assert_eq!(test.saved_state().last_polled_block, START_BLOCK);

            test.stub.dispute_gate.add_permits(10);
            wait_until("all ids are saved", || {
                test.saved_state().processed_event_ids.len() == 11
            })
            .await;
            assert_eq!(test.saved_state().last_polled_block, 101);
            test.shutdown.signal_shutdown();
        })
        .await;

        assert_eq!(test.disputes(), 11);
    }

    /// A second event for a challenge that is being handled starts nothing.
    #[tokio::test]
    async fn test_event_for_open_challenge_is_skipped() {
        let id = challenges(1)[0];
        let test = WatchTest::new(StubChain {
            challenges: vec![id, id],
            ..empty_chain()
        })
        .await;
        test.stub.dispute_gate.add_permits(2);

        test.run(async {
            test.wait_for_disputes(1).await;
            test.wait_for_save().await;
            test.shutdown.signal_shutdown();
        })
        .await;

        assert_eq!(test.disputes(), 1);
        assert_eq!(test.metrics.total_challenges.load(Ordering::SeqCst), 1);
    }

    /// Events that a shutdown keeps the loop from looking at are polled again.
    #[tokio::test]
    async fn test_shutdown_before_all_events_saves_the_start_of_the_poll() {
        let test = WatchTest::new(StubChain {
            shutdown_on_logs: Some(Arc::new(ShutdownState::new())),
            ..chain_with_challenges(2)
        })
        .await;

        test.run(async {}).await;

        assert_eq!(test.disputes(), 0);
        let state = test.saved_state();
        assert!(state.processed_event_ids.is_empty());
        assert_eq!(state.last_polled_block, START_BLOCK);
    }

    /// Ready after a successful poll, not because the state file held a block.
    #[tokio::test]
    async fn test_ready_after_a_successful_poll_only() {
        let test = WatchTest::new(empty_chain()).await;
        test.run(async {
            wait_until("ready", || test.metrics.is_ready()).await;
            test.shutdown.signal_shutdown();
        })
        .await;

        let mut test = test.restarted(&OperatorState {
            last_polled_block: 1234,
            ..OperatorState::default()
        });
        test.config.rpc_url = SECRET_RPC_URL.to_string();
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::INFO));
        test.run(async {
            wait_until("two polls have failed", || {
                logs.contents().matches("RPC poll failed").count() >= 2
            })
            .await;
            let last_block = test.metrics.last_block_polled.load(Ordering::SeqCst);
            assert_eq!(last_block, 1234);
            assert!(!test.metrics.is_ready());
            test.shutdown.signal_shutdown();
        })
        .await;
    }

    /// Once a poll has succeeded, later failed polls do not take readiness away.
    #[tokio::test]
    async fn test_ready_stays_after_later_polls_fail() {
        let test = WatchTest::new(empty_chain()).await;
        test.run(async {
            wait_until("ready", || test.metrics.is_ready()).await;
            test.stub.polls_fail.store(true, Ordering::SeqCst);
            let polls = test.stub.polls.load(Ordering::SeqCst);
            wait_until("two polls have failed", || {
                test.stub.polls.load(Ordering::SeqCst) >= polls + 2
            })
            .await;
            assert!(test.metrics.is_ready());
            test.shutdown.signal_shutdown();
        })
        .await;
    }

    /// A challenge seen again after a restart whose dispute was settled before
    /// the stop is recorded as processed without a dispute or a failure.
    #[tokio::test]
    async fn test_restart_does_not_dispute_a_settled_challenge_again() {
        let test = WatchTest::new(StubChain {
            resolved: challenges(1),
            ..chain_with_challenges(1)
        })
        .await;
        let id = challenge_id(&challenges(1)[0]);

        test.run(async {
            wait_until("the id is saved", || {
                test.saved_state().processed_event_ids.contains(&id)
            })
            .await;
            test.shutdown.signal_shutdown();
        })
        .await;

        assert_eq!(test.disputes(), 0);
        let failed = test.metrics.total_disputes_failed.load(Ordering::SeqCst);
        assert_eq!(failed, 0);
        assert_eq!(test.saved_state().last_polled_block, 101);
    }

    /// The default drain timeout, which the deployments' grace periods (75 s)
    /// leave room for.
    #[test]
    fn test_default_drain_timeout_is_sixty_seconds() {
        let timing = WatchTiming::default();
        assert_eq!(timing.drain_timeout, std::time::Duration::from_secs(60));
    }

    // -----------------------------------------------------------------------
    // Dispute tasks that do not return
    // -----------------------------------------------------------------------

    fn dispute_runner(
        dir: &std::path::Path,
        shutdown: Arc<ShutdownState>,
    ) -> (DisputeRunner, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let (finished_tx, finished_rx) = tokio::sync::mpsc::unbounded_channel();
        let chain = chain::ChainClient::new("http://127.0.0.1:1", TEST_KEY, TEST_CONTRACT).unwrap();
        let proofs_dir = dir.to_str().unwrap();
        let proof_mgr =
            ProofManager::with_config("unused", "unused", proofs_dir, 0, 1, 1, None).unwrap();
        let runner = DisputeRunner {
            chain: Arc::new(chain),
            proof_mgr: Arc::new(proof_mgr),
            alert_manager: Arc::new(AlertManager::new(AlertConfig::default())),
            metrics: Arc::new(metrics::MetricsState::new()),
            chain_cb: Arc::new(chain_breaker()),
            pre_verifier: None,
            shutdown,
            slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DISPUTES)),
            finished_tx,
        };
        (runner, finished_rx)
    }

    /// One open challenge, waiting for its task, delivered by the poll from
    /// [`START_BLOCK`].
    fn one_open_challenge() -> (OpenChallenges, String) {
        let result_id = B256::repeat_byte(0x55);
        let mut challenges = OpenChallenges::default();
        assert!(challenges.open(result_id, Address::ZERO, START_BLOCK));
        (challenges, challenge_id(&result_id))
    }

    async fn panicking_dispute() {
        panic!("a bug in the dispute");
    }

    /// A dispute task that panics frees its slot, stops counting as in flight and
    /// reports its challenge, which is then closed and recorded as processed.
    #[tokio::test]
    async fn test_panicking_dispute_is_closed_and_processed() {
        let dir = tempfile::tempdir().unwrap();
        let shutdown = Arc::new(ShutdownState::new());
        let (runner, mut finished_rx) = dispute_runner(dir.path(), shutdown.clone());
        let (mut challenges, id) = one_open_challenge();
        let mut state = OperatorState::default();
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::ERROR));

        runner.start_waiting_with(&mut challenges, |_| panicking_dispute());
        assert_eq!(shutdown.in_flight_count(), 1);
        wait_until("the task has ended", || shutdown.in_flight_count() == 0).await;

        assert_eq!(runner.slots.available_permits(), MAX_CONCURRENT_DISPUTES);
        record_finished(&mut finished_rx, &mut challenges, &mut state);
        assert!(!challenges.is_open(&id));
        assert!(state.processed_event_ids.contains(&id));
        assert_eq!(challenges.resume_block(101), 101);
        assert!(
            logs.contents().contains("panicked"),
            "logged: {}",
            logs.contents()
        );
    }

    /// A dispute task that the runtime drops unfinished frees its slot and stops
    /// counting as in flight, but its challenge stays open: the dispute may not
    /// have been sent.
    #[test]
    fn test_dropped_dispute_task_leaves_its_challenge_open() {
        let dir = tempfile::tempdir().unwrap();
        let shutdown = Arc::new(ShutdownState::new());
        let (runner, mut finished_rx) = dispute_runner(dir.path(), shutdown.clone());
        let (mut challenges, id) = one_open_challenge();
        let mut state = OperatorState::default();
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::WARN));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            runner.start_waiting_with(&mut challenges, |_| std::future::pending());
            tokio::task::yield_now().await;
        });
        assert_eq!(shutdown.in_flight_count(), 1);
        drop(runtime);

        assert_eq!(shutdown.in_flight_count(), 0);
        assert_eq!(runner.slots.available_permits(), MAX_CONCURRENT_DISPUTES);
        record_finished(&mut finished_rx, &mut challenges, &mut state);
        assert!(challenges.is_open(&id));
        assert!(state.processed_event_ids.is_empty());
        assert_eq!(challenges.resume_block(101), START_BLOCK);
        assert!(
            logs.contents().contains("the challenge stays open"),
            "logged: {}",
            logs.contents()
        );
    }

    // -----------------------------------------------------------------------
    // handle_challenge: pre-verification and the chain circuit breaker
    // -----------------------------------------------------------------------

    struct DisputeOutcome {
        /// Chain failures counted by a breaker that started with one.
        chain_failures: u32,
        disputes_failed: u64,
        critical_alerts: Vec<String>,
        /// The `result_id` metadata of each critical alert.
        critical_result_ids: Vec<String>,
        logged: String,
    }

    /// Handle one challenge against `stub` with the files in `dir`.
    async fn handle_one(
        rpc_url: &str,
        dir: &std::path::Path,
        result_id: B256,
        pre_verifier: Option<&Arc<ProofPreVerifier>>,
        dry_run: bool,
    ) -> DisputeOutcome {
        let config = submit_test_config(rpc_url, "http://127.0.0.1:1", dir);
        let chain = chain::ChainClient::new_with_dry_run(rpc_url, TEST_KEY, TEST_CONTRACT, dry_run)
            .unwrap();
        let proof_mgr =
            ProofManager::with_config("unused", "unused", &config.proofs_dir, 0, 1, 1, None)
                .unwrap();
        let alerts = AlertManager::new(AlertConfig::default());
        let metrics = metrics::MetricsState::new();
        let chain_cb = chain_breaker();
        chain_cb.record_failure();
        let logs = LogBuffer::default();
        let _guard = tracing::subscriber::set_default(logs.subscriber(tracing::Level::INFO));

        handle_challenge(
            &chain,
            &proof_mgr,
            result_id,
            &alerts,
            &metrics,
            &chain_cb,
            pre_verifier,
        )
        .await;

        let critical: Vec<_> = alerts
            .get_active_alerts()
            .into_iter()
            .filter(|a| a.severity == AlertSeverity::Critical)
            .collect();
        DisputeOutcome {
            chain_failures: chain_cb.consecutive_failures(),
            disputes_failed: metrics.total_disputes_failed.load(Ordering::SeqCst),
            critical_alerts: critical
                .iter()
                .map(|a| format!("{}: {}", a.alert_type, a.message))
                .collect(),
            critical_result_ids: critical
                .iter()
                .map(|a| a.metadata.get("result_id").cloned().unwrap_or_default())
                .collect(),
            logged: logs.contents(),
        }
    }

    /// Circuit description that no proof verifies against, for [`write_proof`].
    const BAD_CIRCUIT: &str = r#"{"numComputeLayers":0,"numInputLayers":0,"layerTypes":[],"numSumcheckRounds":[],"atomOffsets":[0],"atomTargetLayers":[],"atomCommitIdxs":[],"ptOffsets":[0],"ptData":[],"inputIsCommitted":[],"oracleProductOffsets":[0],"oracleResultIdxs":[],"oracleExprCoeffs":[]}"#;

    fn circuit_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join(format!("{}.json", "11".repeat(32)))
    }

    fn pre_verifier(dir: &std::path::Path) -> Arc<ProofPreVerifier> {
        Arc::new(ProofPreVerifier::new(dir.to_str().unwrap()))
    }

    /// A proof that fails the local check is not submitted, and is reported like
    /// every other failed dispute. The chain had no part in it.
    #[tokio::test]
    async fn test_failed_pre_verification_is_reported_and_not_submitted() {
        let (rpc_url, stub) = serve_chain(empty_chain()).await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        let id = B256::repeat_byte(0x55);
        write_proof(dir.path(), id);
        std::fs::write(circuit_path(dir.path()), BAD_CIRCUIT).unwrap();

        let verifier = pre_verifier(dir.path());
        let outcome = handle_one(&rpc_url, dir.path(), id, Some(&verifier), false).await;

        assert_eq!(stub.disputes.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.disputes_failed, 1);
        assert_eq!(outcome.critical_alerts.len(), 1);
        assert!(
            outcome.critical_alerts[0]
                .starts_with("dispute_failed: Dispute resolution failed: Pre-verification failed"),
            "{:?}",
            outcome.critical_alerts
        );
        assert!(
            outcome.logged.contains("event=\"dispute_failed\""),
            "logged: {}",
            outcome.logged
        );
        assert_eq!(outcome.chain_failures, 1, "not a chain failure");
    }

    /// The pre-verifier runs off the async threads: on a runtime with a single
    /// one, other work goes on while it reads its (slow) circuit description.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_pre_verification_does_not_block_the_async_thread() {
        let (rpc_url, stub) = serve_chain(empty_chain()).await;
        // Answers a dispute that should not be sent, so that the test then fails
        // instead of waiting for ever.
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        let id = B256::repeat_byte(0x55);
        write_proof(dir.path(), id);
        let fifo = circuit_path(dir.path());
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.unwrap().success());

        // Reading the pipe blocks until this thread writes to it: once the rest
        // of the test has run, or after 10 s if it cannot.
        let (others_ran, rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let ran = rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok();
            std::fs::write(&fifo, BAD_CIRCUIT).unwrap();
            ran
        });

        let verifier = pre_verifier(dir.path());
        let (outcome, ()) = tokio::join!(
            handle_one(&rpc_url, dir.path(), id, Some(&verifier), false),
            async { others_ran.send(()).unwrap() }
        );

        assert!(writer.join().unwrap(), "the async thread was blocked");
        assert_eq!(outcome.disputes_failed, 1, "logged: {}", outcome.logged);
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 0);
    }

    /// A proof that cannot be read or decoded is not a chain failure either.
    #[tokio::test]
    async fn test_unreadable_proof_is_not_a_chain_failure() {
        let (rpc_url, stub) = serve_chain(empty_chain()).await;
        stub.dispute_gate.add_permits(2);
        let dir = tempfile::tempdir().unwrap();
        let id = B256::repeat_byte(0x55);

        for text in [
            "not json",
            r#"{"proof_hex":"zz","circuit_hash":"0x11","public_inputs_hex":"","gens_hex":""}"#,
        ] {
            write_proof_text(dir.path(), id, text);
            let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;
            assert_eq!(outcome.disputes_failed, 1);
            assert_eq!(outcome.critical_alerts.len(), 1);
            assert_eq!(outcome.chain_failures, 1, "{}", text);
        }
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 0);
    }

    /// The chain call's own outcome is what the breaker counts.
    #[tokio::test]
    async fn test_chain_call_outcome_reaches_the_breaker() {
        let (rpc_url, stub) = serve_chain(empty_chain()).await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        let id = B256::repeat_byte(0x55);
        write_proof(dir.path(), id);

        let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.chain_failures, 2);

        // The dry-run client stands in for a call that succeeds.
        let outcome = handle_one(&rpc_url, dir.path(), id, None, true).await;
        assert_eq!(outcome.disputes_failed, 0);
        assert_eq!(outcome.chain_failures, 0);
    }

    /// A challenge whose dispute the prover won is not disputed again and not
    /// reported as a failure.
    #[tokio::test]
    async fn test_settled_dispute_is_not_disputed_again() {
        let id = B256::repeat_byte(0x55);
        let (rpc_url, stub) = serve_chain(StubChain {
            resolved: vec![id],
            results: vec![(id, ml_result(0, true, true))],
            ..empty_chain()
        })
        .await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        write_proof(dir.path(), id);

        let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 0);
        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.disputes_failed, 0);
        assert!(outcome.critical_alerts.is_empty());
        assert_eq!(outcome.chain_failures, 1);
        assert!(
            outcome.logged.contains("settled on chain"),
            "{}",
            outcome.logged
        );
    }

    /// Only a resolved dispute counts as settled. A result that reads as
    /// finalized while its dispute reads as unresolved (reads that straddle the
    /// settlement, or nodes at different heights) is disputed, so that a lost
    /// dispute cannot pass for a settled one.
    #[tokio::test]
    async fn test_finalized_result_without_a_resolved_dispute_is_disputed() {
        let id = B256::repeat_byte(0x55);
        let (rpc_url, stub) = serve_chain(StubChain {
            results: vec![(id, ml_result(0, true, true))],
            ..empty_chain()
        })
        .await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        write_proof(dir.path(), id);

        let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 1);
        assert!(
            !outcome.logged.contains("settled on chain"),
            "{}",
            outcome.logged
        );
    }

    /// A dispute that the contract resolved against the prover is not disputed
    /// again, and is reported: a critical alert for its result, a failed
    /// dispute in the metrics and the audit log. It is not a chain failure.
    #[tokio::test]
    async fn test_lost_dispute_is_reported_and_not_disputed() {
        let id = B256::repeat_byte(0x55);
        let (rpc_url, stub) = serve_chain(StubChain {
            resolved: vec![id],
            prover_lost: vec![id],
            results: vec![(id, ml_result(0, true, true))],
            ..empty_chain()
        })
        .await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        write_proof(dir.path(), id);

        let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;

        assert_eq!(stub.disputes.load(Ordering::SeqCst), 0);
        assert_eq!(stub.estimate_gas_calls.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.disputes_failed, 1);
        assert_eq!(
            outcome.critical_alerts,
            ["dispute_lost: Dispute lost: the dispute was resolved against the prover"]
        );
        assert_eq!(outcome.critical_result_ids, [challenge_id(&id)]);
        assert!(
            outcome.logged.contains("ERROR")
                && outcome.logged.contains("is lost")
                && outcome.logged.contains("event=\"dispute_failed\""),
            "logged: {}",
            outcome.logged
        );
        assert!(!outcome.logged.contains("settled on chain"));
        assert_eq!(outcome.chain_failures, 1, "not a chain failure");
    }

    /// A state read that gets no answer does not hold the dispute up: after
    /// [`DISPUTE_STATE_TIMEOUT`] the dispute goes ahead. The clock is paused and
    /// jumps to the timeout once nothing else is left to run.
    #[tokio::test(start_paused = true)]
    async fn test_dispute_goes_ahead_when_its_state_read_hangs() {
        let (rpc_url, stub) = serve_chain(StubChain {
            reads_hang: true,
            ..empty_chain()
        })
        .await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        let id = B256::repeat_byte(0x55);
        write_proof(dir.path(), id);

        // Without a bound of its own the dispute would wait for ever. The test
        // fails instead after 60 s of real time: a second paused-clock timer
        // would be jumped to while the stub's answers are on their way.
        let (gave_up, mut give_up) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let _ = gave_up.send(());
        });
        let started = tokio::time::Instant::now();
        let outcome = tokio::select! {
            outcome = handle_one(&rpc_url, dir.path(), id, None, false) => outcome,
            _ = &mut give_up => panic!("the dispute waited for the state read"),
        };

        assert!(started.elapsed() >= DISPUTE_STATE_TIMEOUT);
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 1);
        assert!(
            outcome
                .logged
                .contains("took over 30 s, disputing it anyway"),
            "logged: {}",
            outcome.logged
        );
    }

    /// A result that reads as not challenged, as on a node that has not seen the
    /// challenge yet, is still disputed: only a resolved dispute means the
    /// challenge needs nothing more.
    #[tokio::test]
    async fn test_result_that_reads_as_not_challenged_is_disputed() {
        let id = B256::repeat_byte(0x55);
        let (rpc_url, stub) = serve_chain(StubChain {
            results: vec![(id, ml_result(0, false, false))],
            ..empty_chain()
        })
        .await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        write_proof(dir.path(), id);

        let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 1);
        assert!(
            !outcome.logged.contains("settled on chain"),
            "{}",
            outcome.logged
        );
    }

    /// When the dispute's state cannot be read the dispute goes ahead.
    #[tokio::test]
    async fn test_dispute_goes_ahead_when_its_state_cannot_be_read() {
        let (rpc_url, stub) = serve_chain(StubChain {
            reads_fail: true,
            ..empty_chain()
        })
        .await;
        stub.dispute_gate.add_permits(1);
        let dir = tempfile::tempdir().unwrap();
        let id = B256::repeat_byte(0x55);
        write_proof(dir.path(), id);

        let outcome = handle_one(&rpc_url, dir.path(), id, None, false).await;
        assert_eq!(stub.disputes.load(Ordering::SeqCst), 1);
        assert!(
            outcome.logged.contains("disputing it anyway"),
            "logged: {}",
            outcome.logged
        );
    }
}
