//! Request monitoring and processing
//!
//! Now with full optimization:
//! - Parallel processing with nonce management
//! - Job queue with priority
//! - Preflight checks
//! - Caching
//! - Metrics collection
//! - Profitability checking

use crate::cache::ProgramCache;
use crate::config::ProverConfig;
use crate::contracts::{IExecutionEngine, IProgramRegistry};
use crate::fast_prove::{FastProveConfig, FastProver, ProvingStrategy};
use crate::ipfs::{IpfsClient, IpfsConfig};
use crate::metrics::{metrics, Timer};
use crate::nonce::NonceManager;
use crate::prefetch::{InputPrefetcher, PrefetchConfig};
use crate::queue::{JobQueue, QueuedJob};
use alloy::primitives::{B256, U256};
use alloy::providers::fillers::{BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, WalletFiller};
use alloy::providers::Provider;
use alloy::network::{EthereumWallet, Ethereum};
use alloy::transports::http::{Client, Http};
use futures::future::join_all;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

/// Provider type alias with wallet for signing transactions
/// Note: NonceFiller is NOT included - we manage nonces ourselves for parallel safety
pub type SigningProvider = FillProvider<
    JoinFill<
        JoinFill<
            JoinFill<
                JoinFill<
                    alloy::providers::Identity,
                    GasFiller
                >,
                BlobGasFiller
            >,
            ChainIdFiller
        >,
        WalletFiller<EthereumWallet>
    >,
    alloy::providers::RootProvider<Http<Client>>,
    Http<Client>,
    Ethereum
>;

/// Nonce manager type alias with matching transport and network types
pub type SigningNonceManager = NonceManager<SigningProvider, Http<Client>, Ethereum>;

/// Optimized request processor with all features wired together
pub struct OptimizedProcessor {
    /// Job queue for priority ordering
    job_queue: Arc<RwLock<JobQueue>>,
    /// Semaphore for parallel processing
    parallel_semaphore: Arc<Semaphore>,
    /// Program cache
    program_cache: Arc<ProgramCache>,
    /// IPFS client with multi-gateway
    ipfs_client: Arc<IpfsClient>,
    /// Fast prover with preflight
    fast_prover: Arc<FastProver>,
    /// Input prefetcher for reduced latency
    input_prefetcher: Arc<InputPrefetcher>,
    /// Max concurrent jobs
    max_concurrent: usize,
}

impl OptimizedProcessor {
    /// Create a new optimized processor
    pub fn new(
        max_concurrent: usize,
        min_tip_wei: U256,
        cache_size_mb: usize,
        proving_mode: crate::bonsai::ProvingMode,
    ) -> anyhow::Result<Self> {
        // Initialize job queue
        let job_queue = Arc::new(RwLock::new(JobQueue::new(1000, min_tip_wei)));

        // Initialize parallel semaphore
        let parallel_semaphore = Arc::new(Semaphore::new(max_concurrent));

        // Initialize program cache
        let cache_dir = std::path::PathBuf::from("./cache/programs");
        let program_cache = Arc::new(ProgramCache::new(cache_dir, cache_size_mb)?);

        // Initialize IPFS client
        let ipfs_client = Arc::new(IpfsClient::new(IpfsConfig::default()));

        // Initialize fast prover with proving mode (Local, Bonsai, or BonsaiWithFallback)
        let fast_prover = Arc::new(FastProver::with_mode(FastProveConfig::default(), proving_mode));

        // Initialize input prefetcher for reduced latency
        let prefetch_config = PrefetchConfig {
            max_concurrent: max_concurrent * 2, // Prefetch ahead
            max_cache_bytes: cache_size_mb * 1024 * 1024 / 2, // Half of program cache
            ..PrefetchConfig::default()
        };
        let input_prefetcher = Arc::new(InputPrefetcher::new(ipfs_client.clone(), prefetch_config));

        info!(
            "OptimizedProcessor initialized: max_concurrent={}, cache={}MB",
            max_concurrent, cache_size_mb
        );

        Ok(Self {
            job_queue,
            parallel_semaphore,
            program_cache,
            ipfs_client,
            fast_prover,
            input_prefetcher,
            max_concurrent,
        })
    }

    /// Check for pending requests and process them in parallel
    pub async fn check_and_process(
        &self,
        provider: &Arc<SigningProvider>,
        config: &ProverConfig,
        nonce_manager: &Arc<SigningNonceManager>,
    ) -> anyhow::Result<u64> {
        let timer = Timer::start();
        debug!("Checking for pending requests...");

        // Get pending requests from contract
        let engine = IExecutionEngine::new(config.engine_address, provider.clone());

        let pending = engine
            .getPendingRequests(U256::from(0), U256::from(50)) // Fetch more
            .call()
            .await?;

        let request_ids = pending._0;

        if request_ids.is_empty() {
            debug!("No pending requests");
            return Ok(0);
        }

        info!("Found {} pending requests", request_ids.len());

        // Fetch details and add to queue IN PARALLEL
        let fetch_futures: Vec<_> = request_ids
            .iter()
            .map(|&request_id| {
                let provider = provider.clone();
                let config = config.clone();
                let program_cache = self.program_cache.clone();
                let job_queue = self.job_queue.clone();
                async move {
                    fetch_request_details(&provider, &config, request_id, &program_cache, &job_queue).await
                }
            })
            .collect();

        let results = join_all(fetch_futures).await;

        let mut added = 0;
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(true) => added += 1,
                Ok(false) => debug!("Request {} filtered out", request_ids[i]),
                Err(e) => warn!("Failed to fetch request {}: {}", request_ids[i], e),
            }
        }

        info!("Added {} jobs to queue (fetched in parallel)", added);

        // Start prefetching inputs for queued jobs (background, non-blocking)
        {
            let queue = self.job_queue.read().await;
            // Prefetch for all jobs currently in queue
            for scored_job in queue.iter_jobs() {
                self.input_prefetcher.prefetch(scored_job);
            }
        }

        // Log prefetch stats
        let prefetch_stats = self.input_prefetcher.stats().await;
        debug!("{}", prefetch_stats);

        // Process queue in parallel
        let processed = self.process_queue_parallel(provider, config, nonce_manager).await?;

        let elapsed = timer.elapsed();
        info!(
            "Processed {} requests in {:?}",
            processed, elapsed
        );

        Ok(processed)
    }


    /// Process jobs from queue in parallel
    async fn process_queue_parallel(
        &self,
        provider: &Arc<SigningProvider>,
        config: &ProverConfig,
        nonce_manager: &Arc<SigningNonceManager>,
    ) -> anyhow::Result<u64> {
        let mut handles = vec![];
        let mut processed = 0u64;

        // Drain jobs from queue up to max_concurrent
        let jobs: Vec<QueuedJob> = {
            let mut queue = self.job_queue.write().await;
            let mut jobs = vec![];
            while jobs.len() < self.max_concurrent {
                if let Some(job) = queue.pop() {
                    jobs.push(job);
                } else {
                    break;
                }
            }
            jobs
        };

        if jobs.is_empty() {
            return Ok(0);
        }

        info!("Processing {} jobs in parallel (pending nonces: {})",
            jobs.len(), nonce_manager.pending());

        // Spawn parallel tasks
        for job in jobs {
            let provider = provider.clone();
            let config = config.clone();
            let semaphore = self.parallel_semaphore.clone();
            let program_cache = self.program_cache.clone();
            let ipfs_client = self.ipfs_client.clone();
            let fast_prover = self.fast_prover.clone();
            let nonce_mgr = nonce_manager.clone();
            let input_prefetcher = self.input_prefetcher.clone();

            let handle = tokio::spawn(async move {
                // Acquire semaphore permit
                let _permit = semaphore.acquire().await?;

                // Track metrics
                metrics().start_proof();
                let timer = Timer::start();

                let result = process_single_job(
                    &provider,
                    &config,
                    job,
                    &program_cache,
                    &ipfs_client,
                    &fast_prover,
                    &nonce_mgr,
                    &input_prefetcher,
                ).await;

                // Record metrics
                metrics().end_proof();
                match &result {
                    Ok((cycles, bytes)) => {
                        metrics().record_proof_success(timer.elapsed(), *cycles, *bytes);
                    }
                    Err(_) => {
                        metrics().record_proof_failure();
                    }
                }

                result
            });

            handles.push(handle);
        }

        // Wait for all to complete
        for handle in handles {
            match handle.await {
                Ok(Ok(_)) => processed += 1,
                Ok(Err(e)) => warn!("Job failed: {}", e),
                Err(e) => error!("Task panicked: {}", e),
            }
        }

        Ok(processed)
    }
}

/// Fetch request details and add to queue (standalone for parallel execution)
///
/// This function parallelizes the RPC calls to getRequest and getCurrentTip
/// for better performance when fetching multiple requests.
async fn fetch_request_details(
    provider: &Arc<SigningProvider>,
    config: &ProverConfig,
    request_id: U256,
    program_cache: &Arc<ProgramCache>,
    job_queue: &Arc<RwLock<JobQueue>>,
) -> anyhow::Result<bool> {
    let engine = IExecutionEngine::new(config.engine_address, provider.clone());

    // Create call builders (must be bound to avoid temporary lifetime issues)
    let request_call = engine.getRequest(request_id);
    let tip_call = engine.getCurrentTip(request_id);

    // Fetch request details and current tip IN PARALLEL
    let (request_result, tip_result) = tokio::join!(
        request_call.call(),
        tip_call.call()
    );

    let request = request_result?._0;
    let current_tip = tip_result?._0;

    // Check if image ID is allowed
    if !config.is_image_allowed(&request.imageId) {
        return Ok(false);
    }

    // Check tip threshold
    if !config.is_tip_acceptable(current_tip) {
        return Ok(false);
    }

    // Check if program is cached (bonus priority)
    let program_cached = program_cache.get(&request.imageId).is_some();

    // Create queued job
    let job = QueuedJob {
        request_id: request_id.try_into().unwrap_or(0),
        image_id: request.imageId,
        input_hash: request.inputDigest,
        input_url: request.inputUrl.clone(),
        tip: current_tip,
        requester: request.requester,
        expires_at: request.expiresAt.try_into().unwrap_or(u64::MAX),
        queued_at: Instant::now(),
        estimated_cycles: None, // Will be filled by preflight
        program_cached,
        prefetched_input: None, // Will be filled by InputPrefetcher
    };

    // Add to queue
    let mut queue = job_queue.write().await;
    let added = queue.push(job);

    Ok(added)
}

/// Process a single job with all optimizations
async fn process_single_job(
    provider: &Arc<SigningProvider>,
    config: &ProverConfig,
    job: QueuedJob,
    program_cache: &Arc<ProgramCache>,
    ipfs_client: &Arc<IpfsClient>,
    fast_prover: &Arc<FastProver>,
    nonce_manager: &Arc<SigningNonceManager>,
    input_prefetcher: &Arc<InputPrefetcher>,
) -> anyhow::Result<(u64, u64)> {
    let request_id = U256::from(job.request_id);
    info!("Processing job {} (tip: {} wei)", job.request_id, job.tip);

    let engine = IExecutionEngine::new(config.engine_address, provider.clone());

    // Step 1: Fetch program ELF (with caching)
    info!("[Job {}] Fetching program ELF...", job.request_id);
    let fetch_timer = Timer::start();

    let elf = match program_cache.get(&job.image_id) {
        Some(cached) => {
            info!("[Job {}] ELF cache hit!", job.request_id);
            cached
        }
        None => {
            // Fetch from registry (local or on-chain) and cache
            let elf = fetch_program_from_registry(&job.image_id, provider, config).await?;
            program_cache.put(&job.image_id, &elf);
            info!("[Job {}] ELF cached for future use", job.request_id);
            elf
        }
    };
    metrics().record_fetch_time(fetch_timer.elapsed());

    // Step 2: Fetch input (with prefetching support)
    info!("[Job {}] Fetching input...", job.request_id);
    let input_timer = Timer::start();

    // Try to get prefetched input first (instant if available)
    let input = if let Some(prefetched) = input_prefetcher.get_cached(&job.input_url, &job.input_hash).await {
        info!("[Job {}] Input PREFETCH HIT! ({} bytes, saved {:?})",
            job.request_id, prefetched.len(), input_timer.elapsed());
        prefetched
    } else {
        // Fallback to fetching (slower path)
        debug!("[Job {}] Input prefetch miss, fetching now...", job.request_id);
        let input = fetch_input_smart(&job.input_url, ipfs_client).await?;

        // Verify input digest
        let computed_digest = compute_digest(&input);
        if computed_digest != job.input_hash {
            anyhow::bail!(
                "Input digest mismatch: expected {}, got {}",
                job.input_hash,
                computed_digest
            );
        }
        info!("[Job {}] Input fetched and verified ({} bytes in {:?})",
            job.request_id, input.len(), input_timer.elapsed());
        input
    };

    // Step 3: Preflight check
    info!("[Job {}] Running preflight...", job.request_id);
    let preflight = fast_prover.preflight(&elf, &input).await?;

    if preflight.strategy == ProvingStrategy::TooComplex {
        anyhow::bail!("Job {} too complex: {} cycles", job.request_id, preflight.cycles);
    }

    info!(
        "[Job {}] Preflight: {} cycles, strategy: {:?}, est. time: {:?}",
        job.request_id, preflight.cycles, preflight.strategy, preflight.estimated_proof_time
    );

    // Step 4: Profitability check
    if !config.skip_profitability_check {
        let profitability = check_profitability(
            provider,
            &engine,
            request_id,
            job.tip,
            config.min_profit_margin,
        ).await?;

        if !profitability.is_profitable {
            anyhow::bail!(
                "Job {} not profitable: tip={} wei, estimated_cost={} wei, profit_margin={:.1}% (required {:.1}%)",
                job.request_id,
                job.tip,
                profitability.estimated_cost,
                profitability.profit_margin * 100.0,
                config.min_profit_margin * 100.0
            );
        }

        info!(
            "[Job {}] Profitability OK: tip={} wei, cost={} wei, margin={:.1}%",
            job.request_id, job.tip, profitability.estimated_cost, profitability.profit_margin * 100.0
        );
    }

    // Step 5: Claim the request (with retry logic)
    info!("[Job {}] Claiming...", job.request_id);
    let retry_config = RetryConfig::default();

    let claim_result = send_claim_with_retry(
        &engine,
        request_id,
        nonce_manager,
        job.request_id,
        &retry_config,
    ).await?;

    info!("[Job {}] Claimed in tx: {:?}", job.request_id, claim_result.tx_hash);

    // Step 6: Generate proof using fast prover
    info!("[Job {}] Generating proof...", job.request_id);
    let prove_result = fast_prover.prove_fast(&elf, &input).await?;

    info!(
        "[Job {}] Proof generated in {:?} ({} bytes)",
        job.request_id, prove_result.proof_time, prove_result.proof.len()
    );

    // Step 7: Submit proof (with retry logic)
    info!("[Job {}] Submitting proof...", job.request_id);
    let submit_timer = Timer::start();

    let submit_result = send_submit_with_retry(
        &engine,
        request_id,
        prove_result.proof.clone(),
        prove_result.journal.clone(),
        nonce_manager,
        job.request_id,
        &retry_config,
    ).await?;

    metrics().record_submit_time(submit_timer.elapsed());

    info!(
        "[Job {}] Proof submitted in tx: {:?}",
        job.request_id, submit_result.tx_hash
    );

    Ok((prove_result.cycles, input.len() as u64))
}

/// Fetch program ELF from registry
///
/// Lookup order:
/// 1. Local cache directory (./programs/{image_id}.elf)
/// 2. On-chain ProgramRegistry (if configured)
/// 3. Error if not found
async fn fetch_program_from_registry(
    image_id: &B256,
    provider: &Arc<SigningProvider>,
    config: &ProverConfig,
) -> anyhow::Result<Vec<u8>> {
    // First try local cache directory
    let programs_dir = std::path::PathBuf::from("./programs");
    let elf_path = programs_dir.join(format!("{}.elf", hex::encode(image_id)));

    if elf_path.exists() {
        debug!("Loading ELF from local file: {:?}", elf_path);
        let elf = std::fs::read(&elf_path)?;
        return Ok(elf);
    }

    // Try on-chain ProgramRegistry if configured
    if let Some(registry_address) = config.registry_address {
        info!("Fetching program {} from on-chain registry...", image_id);

        let registry = IProgramRegistry::new(registry_address, provider.clone());

        // Query the registry for program details
        let program = match registry.getProgram(*image_id).call().await {
            Ok(result) => result._0,
            Err(e) => {
                warn!("Failed to fetch program from registry: {:?}", e);
                anyhow::bail!(
                    "Program {} not found in registry: {:?}",
                    image_id, e
                );
            }
        };

        // Check if program is active
        if !program.active {
            anyhow::bail!("Program {} is deactivated in registry", image_id);
        }

        let program_url = &program.programUrl;
        if program_url.is_empty() {
            anyhow::bail!("Program {} has no URL in registry", image_id);
        }

        info!("Downloading ELF from: {}", program_url);

        // Download the ELF binary from the URL
        let elf = download_program_elf(program_url).await?;

        // Verify the downloaded ELF matches the image ID
        let computed_image_id = compute_image_id(&elf)?;
        if computed_image_id != *image_id {
            anyhow::bail!(
                "Downloaded ELF image ID mismatch: expected {}, got {}",
                image_id, computed_image_id
            );
        }

        info!("Successfully downloaded and verified ELF ({} bytes)", elf.len());

        // Save to local cache for future use
        if let Err(e) = save_elf_to_cache(&programs_dir, image_id, &elf) {
            warn!("Failed to cache ELF locally: {:?}", e);
        }

        return Ok(elf);
    }

    anyhow::bail!(
        "Program ELF not found for image ID {}. Either place in ./programs/ directory or configure --registry-address.",
        image_id
    )
}

/// Download program ELF from URL (supports HTTP, HTTPS, IPFS)
async fn download_program_elf(url: &str) -> anyhow::Result<Vec<u8>> {
    if url.starts_with("ipfs://") || url.contains("/ipfs/") {
        // Use IPFS client with multi-gateway fallback
        let ipfs_client = IpfsClient::new(IpfsConfig::default());
        ipfs_client.fetch(url).await
    } else if url.starts_with("http://") || url.starts_with("https://") {
        // HTTP(S) download with retry
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120)) // 2 min timeout for large ELFs
            .build()?;

        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("HTTP error downloading ELF: {}", response.status());
        }

        Ok(response.bytes().await?.to_vec())
    } else if url.starts_with("data:") {
        // Data URL (base64 encoded)
        parse_data_url(url)
    } else {
        anyhow::bail!("Unsupported URL scheme for ELF download: {}", url)
    }
}

/// Compute RISC Zero image ID from ELF binary
fn compute_image_id(elf: &[u8]) -> anyhow::Result<B256> {
    let image_id = risc0_zkvm::compute_image_id(elf)?;
    // Convert Digest to B256 via its byte representation
    Ok(B256::from_slice(image_id.as_bytes()))
}

/// Save ELF to local cache directory
fn save_elf_to_cache(
    programs_dir: &std::path::Path,
    image_id: &B256,
    elf: &[u8],
) -> anyhow::Result<()> {
    std::fs::create_dir_all(programs_dir)?;
    let elf_path = programs_dir.join(format!("{}.elf", hex::encode(image_id)));
    std::fs::write(&elf_path, elf)?;
    debug!("Cached ELF to {:?}", elf_path);
    Ok(())
}

/// Smart input fetching with IPFS multi-gateway support
async fn fetch_input_smart(url: &str, ipfs_client: &Arc<IpfsClient>) -> anyhow::Result<Vec<u8>> {
    if IpfsClient::is_ipfs_url(url) {
        // Use multi-gateway IPFS client
        ipfs_client.fetch(url).await
    } else if url.starts_with("http://") || url.starts_with("https://") {
        // HTTP fetch with retry
        let response = reqwest::get(url).await?;
        if !response.status().is_success() {
            anyhow::bail!("HTTP error: {}", response.status());
        }
        Ok(response.bytes().await?.to_vec())
    } else if url.starts_with("data:") {
        // Data URL
        parse_data_url(url)
    } else {
        anyhow::bail!("Unsupported URL scheme: {}", url)
    }
}

/// Parse data URL
fn parse_data_url(url: &str) -> anyhow::Result<Vec<u8>> {
    let parts: Vec<&str> = url.splitn(2, ',').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid data URL format");
    }
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD.decode(parts[1])?;
    Ok(decoded)
}

/// Compute SHA256 digest
fn compute_digest(data: &[u8]) -> B256 {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    B256::from_slice(&hash)
}

// ============================================================================
// Transaction Retry Logic
// ============================================================================

/// Configuration for transaction retries
#[derive(Clone, Debug)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        }
    }
}

/// Check if an error is retryable (transient) or permanent
///
/// Retryable errors include:
/// - Network timeouts
/// - Connection errors
/// - Rate limiting (429)
/// - Server errors (5xx)
///
/// Non-retryable errors include:
/// - Nonce too low (transaction already mined)
/// - Insufficient funds
/// - Contract reverts (execution reverted)
/// - Already claimed/completed
fn is_retryable_error(error: &str) -> bool {
    let error_lower = error.to_lowercase();

    // Non-retryable errors - return false immediately
    let permanent_errors = [
        "nonce too low",
        "insufficient funds",
        "execution reverted",
        "already claimed",
        "already completed",
        "request not found",
        "not pending",
        "invalid signature",
        "gas too low",
        "max fee per gas less than block base fee",
    ];

    for permanent in permanent_errors {
        if error_lower.contains(permanent) {
            return false;
        }
    }

    // Retryable errors
    let transient_errors = [
        "timeout",
        "timed out",
        "connection",
        "network",
        "rate limit",
        "too many requests",
        "429",
        "500",
        "502",
        "503",
        "504",
        "temporarily unavailable",
        "try again",
        "retry",
        "econnreset",
        "econnrefused",
        "socket hang up",
    ];

    for transient in transient_errors {
        if error_lower.contains(transient) {
            return true;
        }
    }

    // Default: don't retry unknown errors
    false
}

/// Result of a transaction with retry
pub struct TransactionResult {
    pub tx_hash: alloy::primitives::TxHash,
    pub attempts: u32,
}

/// Send a claim transaction with retry logic
async fn send_claim_with_retry(
    engine: &IExecutionEngine::IExecutionEngineInstance<Http<Client>, Arc<SigningProvider>>,
    request_id: U256,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
) -> anyhow::Result<TransactionResult> {
    let mut attempts = 0u32;
    let mut delay = config.initial_delay;

    loop {
        attempts += 1;
        let nonce = nonce_manager.next_nonce();
        debug!("[Job {}] Claim attempt {} with nonce {}", job_id, attempts, nonce);

        let result = engine
            .claimExecution(request_id)
            .nonce(nonce)
            .send()
            .await;

        match result {
            Ok(pending_tx) => {
                // Transaction sent, wait for receipt
                match pending_tx.get_receipt().await {
                    Ok(receipt) => {
                        nonce_manager.transaction_completed();
                        if attempts > 1 {
                            info!("[Job {}] Claim succeeded on attempt {}", job_id, attempts);
                        }
                        return Ok(TransactionResult {
                            tx_hash: receipt.transaction_hash,
                            attempts,
                        });
                    }
                    Err(e) => {
                        let error_str = format!("{:?}", e);
                        nonce_manager.handle_nonce_error(&error_str, nonce).await?;

                        if !is_retryable_error(&error_str) || attempts >= config.max_retries {
                            return Err(anyhow::anyhow!(
                                "Claim receipt failed after {} attempts: {}",
                                attempts,
                                error_str
                            ));
                        }

                        warn!(
                            "[Job {}] Claim receipt failed (attempt {}), retrying: {}",
                            job_id, attempts, error_str
                        );
                    }
                }
            }
            Err(e) => {
                let error_str = format!("{:?}", e);
                nonce_manager.handle_nonce_error(&error_str, nonce).await?;

                if !is_retryable_error(&error_str) || attempts >= config.max_retries {
                    return Err(anyhow::anyhow!(
                        "Claim send failed after {} attempts: {}",
                        attempts,
                        error_str
                    ));
                }

                warn!(
                    "[Job {}] Claim send failed (attempt {}), retrying: {}",
                    job_id, attempts, error_str
                );
            }
        }

        // Exponential backoff
        tokio::time::sleep(delay).await;
        delay = Duration::from_secs_f64(
            (delay.as_secs_f64() * config.backoff_multiplier).min(config.max_delay.as_secs_f64())
        );
    }
}

/// Send a submit proof transaction with retry logic
async fn send_submit_with_retry(
    engine: &IExecutionEngine::IExecutionEngineInstance<Http<Client>, Arc<SigningProvider>>,
    request_id: U256,
    proof: Vec<u8>,
    journal: Vec<u8>,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
) -> anyhow::Result<TransactionResult> {
    let mut attempts = 0u32;
    let mut delay = config.initial_delay;

    loop {
        attempts += 1;
        let nonce = nonce_manager.next_nonce();
        debug!("[Job {}] Submit attempt {} with nonce {}", job_id, attempts, nonce);

        let result = engine
            .submitProof(request_id, proof.clone().into(), journal.clone().into())
            .nonce(nonce)
            .send()
            .await;

        match result {
            Ok(pending_tx) => {
                // Transaction sent, wait for receipt
                match pending_tx.get_receipt().await {
                    Ok(receipt) => {
                        nonce_manager.transaction_completed();
                        if attempts > 1 {
                            info!("[Job {}] Submit succeeded on attempt {}", job_id, attempts);
                        }
                        return Ok(TransactionResult {
                            tx_hash: receipt.transaction_hash,
                            attempts,
                        });
                    }
                    Err(e) => {
                        let error_str = format!("{:?}", e);
                        nonce_manager.handle_nonce_error(&error_str, nonce).await?;

                        if !is_retryable_error(&error_str) || attempts >= config.max_retries {
                            return Err(anyhow::anyhow!(
                                "Submit receipt failed after {} attempts: {}",
                                attempts,
                                error_str
                            ));
                        }

                        warn!(
                            "[Job {}] Submit receipt failed (attempt {}), retrying: {}",
                            job_id, attempts, error_str
                        );
                    }
                }
            }
            Err(e) => {
                let error_str = format!("{:?}", e);
                nonce_manager.handle_nonce_error(&error_str, nonce).await?;

                if !is_retryable_error(&error_str) || attempts >= config.max_retries {
                    return Err(anyhow::anyhow!(
                        "Submit send failed after {} attempts: {}",
                        attempts,
                        error_str
                    ));
                }

                warn!(
                    "[Job {}] Submit send failed (attempt {}), retrying: {}",
                    job_id, attempts, error_str
                );
            }
        }

        // Exponential backoff
        tokio::time::sleep(delay).await;
        delay = Duration::from_secs_f64(
            (delay.as_secs_f64() * config.backoff_multiplier).min(config.max_delay.as_secs_f64())
        );
    }
}

// ============================================================================
// Profitability Checking
// ============================================================================

/// Result of profitability check
#[derive(Debug)]
#[allow(dead_code)]
struct ProfitabilityResult {
    /// Whether the job is profitable
    is_profitable: bool,
    /// Estimated total gas cost (claim + submit) in wei
    estimated_cost: U256,
    /// Actual profit margin (tip - cost) / tip
    profit_margin: f64,
    /// Current gas price in wei
    gas_price: u128,
    /// Estimated gas for claim transaction
    claim_gas: u64,
    /// Estimated gas for submit transaction
    submit_gas: u64,
}

/// Check if a job is profitable before claiming
///
/// Estimates gas costs for:
/// 1. claimExecution transaction
/// 2. submitProof transaction (with estimated proof size)
///
/// Returns profitable if: (tip - total_cost) / tip >= min_profit_margin
async fn check_profitability(
    provider: &Arc<SigningProvider>,
    engine: &IExecutionEngine::IExecutionEngineInstance<Http<Client>, Arc<SigningProvider>>,
    request_id: U256,
    tip: U256,
    min_profit_margin: f64,
) -> anyhow::Result<ProfitabilityResult> {
    // Get current gas price
    let gas_price = provider.get_gas_price().await?;

    // Estimate gas for claimExecution
    // If estimation fails, use a conservative default
    let claim_gas = match engine.claimExecution(request_id).estimate_gas().await {
        Ok(gas) => gas,
        Err(e) => {
            warn!("Failed to estimate claim gas, using default: {}", e);
            100_000u64 // Conservative default for claim
        }
    };

    // Estimate gas for submitProof
    // We use a dummy proof for estimation since we don't have the real proof yet
    // Actual proof size is typically 256-512 bytes for STARK, ~300 bytes for Groth16
    let dummy_proof = vec![0u8; 512]; // Conservative estimate
    let dummy_journal = vec![0u8; 64]; // Typical journal size

    let submit_gas = match engine
        .submitProof(request_id, dummy_proof.into(), dummy_journal.into())
        .estimate_gas()
        .await
    {
        Ok(gas) => gas,
        Err(e) => {
            // submitProof estimation often fails because we haven't claimed yet
            // Use a conservative default based on typical proof submission costs
            debug!("Failed to estimate submit gas (expected before claim): {}", e);
            300_000u64 // Conservative default for submit with ~500 byte proof
        }
    };

    // Calculate total estimated cost
    let total_gas = claim_gas + submit_gas;
    let estimated_cost = U256::from(gas_price) * U256::from(total_gas);

    // Calculate profit margin
    // profit_margin = (tip - cost) / tip
    let profit_margin = if tip.is_zero() {
        -1.0 // Definitely not profitable
    } else {
        let tip_f64 = tip.to::<u128>() as f64;
        let cost_f64 = estimated_cost.to::<u128>() as f64;
        (tip_f64 - cost_f64) / tip_f64
    };

    let is_profitable = profit_margin >= min_profit_margin;

    debug!(
        "Profitability check: tip={}, gas_price={}, claim_gas={}, submit_gas={}, total_cost={}, margin={:.1}%",
        tip, gas_price, claim_gas, submit_gas, estimated_cost, profit_margin * 100.0
    );

    Ok(ProfitabilityResult {
        is_profitable,
        estimated_cost,
        profit_margin,
        gas_price,
        claim_gas,
        submit_gas,
    })
}

// ============================================================================
// Legacy function for backwards compatibility
// ============================================================================

/// Check for pending requests and process them (legacy sequential version)
pub async fn check_and_process_requests(
    provider: &Arc<SigningProvider>,
    config: &ProverConfig,
    nonce_manager: &Arc<SigningNonceManager>,
) -> anyhow::Result<u64> {
    // Create processor with defaults
    let processor = OptimizedProcessor::new(
        4,                           // max_concurrent
        config.min_tip_wei,          // min_tip
        256,                         // cache_size_mb
        config.proving_mode.clone(), // proving_mode from config
    )?;

    processor.check_and_process(provider, config, nonce_manager).await
}
