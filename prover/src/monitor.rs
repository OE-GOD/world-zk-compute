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
use crate::concurrency::ConcurrencyManager;
use crate::config::ProverConfig;
use crate::contracts::{IExecutionEngine, IProgramRegistry};
use crate::fast_prove::{FastProveConfig, FastProver, ProvingStrategy};
use crate::gpu_manager::GpuDeviceManager;
use crate::input_decomposer::{DecomposerConfig, InputDecomposer, StrategyRegistry};
use crate::ipfs::{IpfsClient, IpfsConfig};
use crate::metrics::{metrics, Timer};
use crate::multi_vm::MultiVmProver;
use crate::nonce::NonceManager;
use crate::prefetch::{InputPrefetcher, PrefetchConfig};
use crate::private_input;
use crate::proof_cache::{CachedProof, ProofCache, ProofCacheKey};
use crate::prove_metrics::{pipeline_metrics, ProveStage, ProveTimeline};
use crate::queue::{JobQueue, QueuedJob};
use crate::recursive_wrapper;
use crate::redact;
use crate::xgboost_decomp::XGBoostDecompStrategy;
use alloy::contract::{CallBuilder, CallDecoder};
use alloy::eips::BlockNumberOrTag;
use alloy::network::{Ethereum, EthereumWallet};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::fillers::{
    BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, WalletFiller,
};
use alloy::providers::{PendingTransactionError, Provider, ProviderBuilder, WatchTxError};
use alloy::rpc::types::Filter;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolEvent;
use futures::future::join_all;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Provider type alias with wallet for signing transactions
/// Note: NonceFiller is NOT included - we manage nonces ourselves for parallel safety
pub type SigningProvider = FillProvider<
    JoinFill<
        JoinFill<
            JoinFill<JoinFill<alloy::providers::Identity, GasFiller>, BlobGasFiller>,
            ChainIdFiller,
        >,
        WalletFiller<EthereumWallet>,
    >,
    alloy::providers::RootProvider,
    Ethereum,
>;

/// Maximum time to connect to the RPC endpoint
pub const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time an RPC call may take
pub const RPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Build the provider that signs transactions with `signer`. A connection or
/// a call that takes longer than its timeout fails.
pub fn signing_provider(
    rpc_url: reqwest::Url,
    signer: PrivateKeySigner,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> anyhow::Result<SigningProvider> {
    let client = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()?;

    Ok(ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(GasFiller)
        .filler(BlobGasFiller::default())
        .filler(ChainIdFiller::default())
        .wallet(EthereumWallet::from(signer))
        .connect_reqwest(client, rpc_url))
}

/// Nonce manager type alias with matching transport and network types
pub type SigningNonceManager = NonceManager<SigningProvider>;

/// Optimized request processor with all features wired together
pub struct OptimizedProcessor {
    /// Job queue for priority ordering
    job_queue: Arc<RwLock<JobQueue>>,
    /// GPU-aware concurrency manager (replaces bare semaphore)
    concurrency: Arc<ConcurrencyManager>,
    /// Program cache
    program_cache: Arc<ProgramCache>,
    /// IPFS client with multi-gateway
    ipfs_client: Arc<IpfsClient>,
    /// Fast prover with preflight
    fast_prover: Arc<FastProver>,
    /// Input prefetcher for reduced latency
    input_prefetcher: Arc<InputPrefetcher>,
    /// Proof cache for instant results on repeated jobs
    proof_cache: Arc<ProofCache>,
    /// Multi-VM router (risc0 / SP1)
    #[allow(dead_code)]
    multi_vm: Arc<MultiVmProver>,
    /// Strategy registry for input decomposition
    strategy_registry: Arc<StrategyRegistry>,
    /// Max concurrent jobs
    max_concurrent: usize,
    /// Enable SNARK (Groth16) proof generation
    use_snark: bool,
    /// Wallet signer for private input authentication
    signer: Option<Arc<PrivateKeySigner>>,
}

impl OptimizedProcessor {
    /// Create a new optimized processor with explicit GPU/CPU concurrency config.
    ///
    /// - `max_gpu_concurrent`: GPU concurrency slots (0 = auto-detect from GPU count)
    /// - `max_cpu_concurrent`: CPU concurrency slots (0 = auto-detect from CPU count - 1)
    /// - `queue_size`: maximum number of queued jobs (at least 1)
    #[allow(clippy::too_many_arguments)]
    pub fn with_gpu_config(
        max_concurrent: usize,
        min_tip_wei: U256,
        cache_size_mb: usize,
        proving_mode: crate::bonsai::ProvingMode,
        use_snark: bool,
        max_gpu_concurrent: usize,
        max_cpu_concurrent: usize,
        queue_size: usize,
    ) -> anyhow::Result<Self> {
        Self::with_fast_prove_config(
            max_concurrent,
            min_tip_wei,
            cache_size_mb,
            proving_mode,
            use_snark,
            max_gpu_concurrent,
            max_cpu_concurrent,
            queue_size,
            FastProveConfig::default(),
        )
    }

    /// [`with_gpu_config`](Self::with_gpu_config) with the fast prover's
    /// configuration given by the caller.
    #[allow(clippy::too_many_arguments)]
    fn with_fast_prove_config(
        max_concurrent: usize,
        min_tip_wei: U256,
        cache_size_mb: usize,
        proving_mode: crate::bonsai::ProvingMode,
        use_snark: bool,
        max_gpu_concurrent: usize,
        max_cpu_concurrent: usize,
        queue_size: usize,
        fast_prove_config: FastProveConfig,
    ) -> anyhow::Result<Self> {
        if queue_size == 0 {
            anyhow::bail!("Queue size must be at least 1");
        }

        // Initialize job queue
        let job_queue = Arc::new(RwLock::new(JobQueue::new(queue_size, min_tip_wei)));

        // Initialize multi-GPU device manager
        let gpu_manager = if max_gpu_concurrent > 0 {
            let backend = crate::gpu_optimize::GpuBackend::detect();
            Arc::new(GpuDeviceManager::with_device_count(
                backend,
                max_gpu_concurrent,
            ))
        } else {
            Arc::new(GpuDeviceManager::detect())
        };

        // Initialize GPU-aware concurrency manager
        let concurrency = Arc::new(ConcurrencyManager::new(gpu_manager, max_cpu_concurrent));

        // Initialize program cache
        let cache_dir = std::path::PathBuf::from("./cache/programs");
        let program_cache = Arc::new(ProgramCache::new(cache_dir, cache_size_mb)?);

        // Initialize IPFS client
        let ipfs_client = Arc::new(IpfsClient::new(IpfsConfig::default())?);

        // Initialize multi-VM router
        let multi_vm = Arc::new(MultiVmProver::new(proving_mode.clone()));

        // Initialize fast prover with multi-VM router
        let fast_prover = Arc::new(FastProver::with_multi_vm(
            fast_prove_config,
            proving_mode,
            use_snark,
            multi_vm.clone(),
        ));

        // Initialize strategy registry for input decomposition
        let mut strategy_registry = StrategyRegistry::new();

        // Register XGBoost decomposition strategy
        let xgboost_image_id = {
            let bytes =
                hex::decode("d6b60ae7d1f27aec34d247fad9c4700be237938cec515af03c0451f2ca8aefe4")
                    .map_err(|e| anyhow::anyhow!("Invalid xgboost image ID hex: {}", e))?;
            B256::from_slice(&bytes)
        };
        strategy_registry.register(
            xgboost_image_id,
            std::sync::Arc::new(XGBoostDecompStrategy::new()),
        );
        let strategy_registry = Arc::new(strategy_registry);

        // Initialize input prefetcher for reduced latency
        let prefetch_config = PrefetchConfig {
            max_concurrent: max_concurrent * 2,               // Prefetch ahead
            max_cache_bytes: cache_size_mb * 1024 * 1024 / 2, // Half of program cache
            ..PrefetchConfig::default()
        };
        let input_prefetcher = Arc::new(InputPrefetcher::new(ipfs_client.clone(), prefetch_config));

        // Initialize proof cache for instant results on repeated jobs
        let proof_cache = Arc::new(ProofCache::with_defaults());

        info!(
            "OptimizedProcessor initialized: max_concurrent={}, cache={}MB",
            max_concurrent, cache_size_mb
        );

        Ok(Self {
            job_queue,
            concurrency,
            program_cache,
            ipfs_client,
            fast_prover,
            input_prefetcher,
            proof_cache,
            multi_vm,
            strategy_registry,
            max_concurrent,
            use_snark,
            signer: None,
        })
    }

    /// Set the wallet signer for private input authentication.
    pub fn set_signer(&mut self, signer: Arc<PrivateKeySigner>) {
        self.signer = Some(signer);
    }

    /// Get the concurrency manager (for metrics/health reporting).
    pub fn concurrency(&self) -> &Arc<ConcurrencyManager> {
        &self.concurrency
    }

    /// Start the periodic cleanup of expired entries in this processor's proof cache.
    pub fn start_proof_cache_cleanup(&self, interval: Duration) -> tokio::task::JoinHandle<()> {
        self.proof_cache.start_cleanup_task(interval)
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

        let request_ids = pending;

        if request_ids.is_empty() {
            info!("No pending requests (engine: {:?})", config.engine_address);
            return Ok(0);
        }

        info!("Found {} pending requests", request_ids.len());

        let added = self
            .queue_new_requests(provider, config, &request_ids)
            .await;
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
        let processed = self
            .process_queue_parallel(provider, config, nonce_manager)
            .await?;

        let elapsed = timer.elapsed();
        info!("Processed {} requests in {:?}", processed, elapsed);

        Ok(processed)
    }

    /// Add pending requests to the job queue and return how many were added.
    ///
    /// Requests whose input URL is not found are skipped: they are still
    /// pending in the next cycle and are looked up again then.
    async fn queue_new_requests(
        &self,
        provider: &Arc<SigningProvider>,
        config: &ProverConfig,
        request_ids: &[U256],
    ) -> usize {
        // Batch-fetch inputUrls from events for all pending requests in a single RPC call
        // (inputUrl is no longer stored on-chain, only emitted in the ExecutionRequested event)
        let input_urls =
            match batch_fetch_input_urls(provider, config.engine_address, request_ids).await {
                Ok(input_urls) => input_urls,
                Err(e) => {
                    warn!(
                        "Failed to batch-fetch inputUrls from events, skipping {} requests: {}",
                        request_ids.len(),
                        redact::redact_urls(&e.to_string())
                    );
                    return 0;
                }
            };

        // Fetch details and add to queue IN PARALLEL
        let mut found_ids = Vec::new();
        let mut fetch_futures = Vec::new();
        for &request_id in request_ids {
            let Some(meta) = input_urls.get(&request_id).cloned() else {
                warn!(
                    "No ExecutionRequested event found for request {}, skipping it",
                    request_id
                );
                continue;
            };
            found_ids.push(request_id);

            let provider = provider.clone();
            let config = config.clone();
            let program_cache = self.program_cache.clone();
            let job_queue = self.job_queue.clone();
            fetch_futures.push(async move {
                fetch_request_details(
                    &provider,
                    &config,
                    request_id,
                    meta.url,
                    meta.input_type,
                    &program_cache,
                    &job_queue,
                )
                .await
            });
        }

        let results = join_all(fetch_futures).await;

        let mut added = 0;
        for (request_id, result) in found_ids.into_iter().zip(results) {
            match result {
                Ok(true) => added += 1,
                Ok(false) => debug!("Request {} filtered out", request_id),
                // An RPC error names the URL of the node
                Err(e) => warn!(
                    "Failed to fetch request {}: {}",
                    request_id,
                    redact::redact_urls(&e.to_string())
                ),
            }
        }
        added
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

        info!("Processing {} jobs in parallel", jobs.len());

        // Spawn parallel tasks
        for job in jobs {
            let provider = provider.clone();
            let config = config.clone();
            let concurrency = self.concurrency.clone();
            let program_cache = self.program_cache.clone();
            let ipfs_client = self.ipfs_client.clone();
            let fast_prover = self.fast_prover.clone();
            let nonce_mgr = nonce_manager.clone();
            let input_prefetcher = self.input_prefetcher.clone();
            let proof_cache = self.proof_cache.clone();
            let strategy_registry = self.strategy_registry.clone();
            let use_snark = self.use_snark;
            let signer = self.signer.clone();

            let handle = tokio::spawn(async move {
                // Classify job and acquire from appropriate pool (GPU or CPU)
                let resource = concurrency.classify_job(
                    job.estimated_cycles.unwrap_or(10_000_000),
                    0, // Memory not known at this stage
                );
                let _guard = concurrency.acquire(resource).await?;

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
                    &proof_cache,
                    &strategy_registry,
                    use_snark,
                    &signer,
                )
                .await;

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
                // A job error can be an RPC error, which names the URL of
                // the node
                Ok(Err(e)) => warn!("Job failed: {}", redact::redact_urls(&e.to_string())),
                Err(e) => error!("Task panicked: {}", redact::redact_urls(&e.to_string())),
            }
        }

        Ok(processed)
    }
}

/// Input metadata extracted from ExecutionRequested events
#[derive(Clone, Debug)]
struct EventInputMeta {
    /// Input URL from the event
    url: String,
    /// Input type (0 = Public, 1 = Private)
    input_type: u8,
}

/// Batch-fetch inputUrls and inputTypes from ExecutionRequested events for multiple request IDs.
///
/// Performs a single `get_logs` RPC call with all request IDs over the whole
/// chain, returning a map of request_id → EventInputMeta. Much more efficient
/// than one query per request.
async fn batch_fetch_input_urls(
    provider: &Arc<SigningProvider>,
    engine_address: Address,
    request_ids: &[U256],
) -> anyhow::Result<std::collections::HashMap<U256, EventInputMeta>> {
    use std::collections::HashMap;

    let mut result: HashMap<U256, EventInputMeta> = HashMap::new();

    // Build topic1 filter with all request IDs
    let topic_values: Vec<B256> = request_ids
        .iter()
        .map(|id| B256::from(id.to_be_bytes::<32>()))
        .collect();

    // Without a block range the node searches only the latest block
    let filter = Filter::new()
        .from_block(BlockNumberOrTag::Earliest)
        .to_block(BlockNumberOrTag::Latest)
        .address(engine_address)
        .event_signature(IExecutionEngine::ExecutionRequested::SIGNATURE_HASH)
        .topic1(topic_values);

    for log in provider.get_logs(&filter).await? {
        if let Ok(decoded) = log.log_decode::<IExecutionEngine::ExecutionRequested>() {
            let req_id = decoded.inner.data.requestId;
            let meta = EventInputMeta {
                url: decoded.inner.data.inputUrl.clone(),
                input_type: decoded.inner.data.inputType,
            };
            result.insert(req_id, meta);
        }
    }
    debug!("Batch-fetched {} inputUrls from events", result.len());

    Ok(result)
}

/// Fetch request details and add to queue (standalone for parallel execution)
///
/// This function parallelizes the RPC calls to getRequest and getCurrentTip
/// for better performance when fetching multiple requests.
async fn fetch_request_details(
    provider: &Arc<SigningProvider>,
    config: &ProverConfig,
    request_id: U256,
    input_url: String,
    input_type: u8,
    program_cache: &Arc<ProgramCache>,
    job_queue: &Arc<RwLock<JobQueue>>,
) -> anyhow::Result<bool> {
    let engine = IExecutionEngine::new(config.engine_address, provider.clone());

    // Create call builders (must be bound to avoid temporary lifetime issues)
    let request_call = engine.getRequest(request_id);
    let tip_call = engine.getCurrentTip(request_id);

    // Fetch request details and current tip IN PARALLEL
    let (request_result, tip_result) = tokio::join!(request_call.call(), tip_call.call());

    let request = request_result?;
    let current_tip = tip_result?;

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
    let request_id_u64: u64 = request_id
        .try_into()
        .map_err(|_| anyhow::anyhow!("Request ID {} overflows u64", request_id))?;
    let expires_at_u64: u64 = request.expiresAt.to::<u64>();

    let job = QueuedJob {
        request_id: request_id_u64,
        image_id: request.imageId,
        input_hash: request.inputDigest,
        input_url,
        input_type,
        tip: current_tip,
        requester: request.requester,
        expires_at: expires_at_u64,
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
#[allow(clippy::too_many_arguments)]
async fn process_single_job(
    provider: &Arc<SigningProvider>,
    config: &ProverConfig,
    job: QueuedJob,
    program_cache: &Arc<ProgramCache>,
    ipfs_client: &Arc<IpfsClient>,
    fast_prover: &Arc<FastProver>,
    nonce_manager: &Arc<SigningNonceManager>,
    input_prefetcher: &Arc<InputPrefetcher>,
    proof_cache: &Arc<ProofCache>,
    strategy_registry: &Arc<StrategyRegistry>,
    use_snark: bool,
    signer: &Option<Arc<PrivateKeySigner>>,
) -> anyhow::Result<(u64, u64)> {
    let request_id = U256::from(job.request_id);
    info!("Processing job {} (tip: {} wei)", job.request_id, job.tip);

    // Start pipeline timeline for stage-level instrumentation
    let mut timeline = ProveTimeline::start(job.request_id);

    let engine = IExecutionEngine::new(config.engine_address, provider.clone());

    // Step 1: Fetch program ELF (with caching)
    timeline.enter_stage(ProveStage::Fetch);
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
    // For private inputs, we must claim first before the auth server releases data
    let is_private = job.input_type == 1;

    // If private input, claim first (auth server checks on-chain claim)
    let early_claim_result = if is_private {
        info!(
            "[Job {}] Private input: claiming before fetch (auth server requires on-chain claim)",
            job.request_id
        );
        let retry_config = RetryConfig::default();
        let claim_result = send_claim_with_retry(
            &engine,
            request_id,
            nonce_manager,
            job.request_id,
            &retry_config,
        )
        .await?;
        info!(
            "[Job {}] Claimed in tx: {:?}",
            job.request_id, claim_result.tx_hash
        );
        Some(claim_result)
    } else {
        None
    };

    info!("[Job {}] Fetching input...", job.request_id);
    let input_timer = Timer::start();

    let input = if is_private {
        // Private input: fetch from auth server with signed request
        let signer_ref = signer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Signer required for private input jobs"))?;

        let input =
            private_input::fetch_private_input(&job.input_url, job.request_id, signer_ref).await?;

        // Verify input digest (same validation as public inputs)
        let computed_digest = compute_digest(&input);
        if computed_digest != job.input_hash {
            anyhow::bail!(
                "Private input digest mismatch: expected {}, got {}",
                job.input_hash,
                computed_digest
            );
        }
        info!(
            "[Job {}] Private input fetched and verified ({} bytes in {:?})",
            job.request_id,
            input.len(),
            input_timer.elapsed()
        );
        input
    } else if let Some(prefetched) = input_prefetcher
        .get_cached(&job.input_url, &job.input_hash)
        .await
    {
        // Try to get prefetched input first (instant if available)
        info!(
            "[Job {}] Input PREFETCH HIT! ({} bytes, saved {:?})",
            job.request_id,
            prefetched.len(),
            input_timer.elapsed()
        );
        prefetched
    } else {
        // Fallback to fetching (slower path)
        debug!(
            "[Job {}] Input prefetch miss, fetching now...",
            job.request_id
        );
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
        info!(
            "[Job {}] Input fetched and verified ({} bytes in {:?})",
            job.request_id,
            input.len(),
            input_timer.elapsed()
        );
        input
    };

    // Step 3: Check proof cache (instant if cached!)
    timeline.enter_stage(ProveStage::Preflight);
    let cache_key = ProofCacheKey::new(job.image_id, job.input_hash, config.use_snark);

    if let Some(cached_proof) = proof_cache.get(&cache_key).await {
        info!(
            "[Job {}] PROOF CACHE HIT! Using cached proof ({} bytes, {} cycles)",
            job.request_id,
            cached_proof.proof.len(),
            cached_proof.cycles
        );

        // Fast path: claim and submit with cached proof
        let retry_config = RetryConfig::default();

        // Claim (skip if already claimed for private inputs)
        claim_unless_claimed(
            &early_claim_result,
            " (cached path)",
            &engine,
            request_id,
            nonce_manager,
            job.request_id,
            &retry_config,
        )
        .await?;

        // Submit cached proof
        timeline.enter_stage(ProveStage::Submission);
        info!("[Job {}] Submitting cached proof...", job.request_id);
        let submit_timer = Timer::start();

        let submit_result = send_submit_with_retry(
            &engine,
            request_id,
            cached_proof.proof.clone(),
            cached_proof.journal.clone(),
            nonce_manager,
            job.request_id,
            &retry_config,
        )
        .await?;

        metrics().record_submit_time(submit_timer.elapsed());
        info!(
            "[Job {}] Cached proof submitted in tx: {:?} (saved proving time!)",
            job.request_id, submit_result.tx_hash
        );

        let finished = timeline.finish(true);
        info!("[Job {}] {}", job.request_id, finished);
        pipeline_metrics().record(&finished);

        return Ok((cached_proof.cycles, input.len() as u64));
    }

    // Step 4: Preflight check (cache miss path)
    timeline.enter_stage(ProveStage::Preflight);
    info!("[Job {}] Running preflight...", job.request_id);
    let preflight = fast_prover.preflight(&elf, &input).await?;

    if preflight.strategy == ProvingStrategy::TooComplex {
        anyhow::bail!(
            "Job {} too complex: {} cycles",
            job.request_id,
            preflight.cycles
        );
    }

    timeline.enter_stage_with_meta(
        ProveStage::Strategy,
        &format!("{:?}, {} cycles", preflight.strategy, preflight.cycles),
    );
    info!(
        "[Job {}] Preflight: {} cycles, strategy: {:?}, est. time: {:?}",
        job.request_id, preflight.cycles, preflight.strategy, preflight.estimated_proof_time
    );

    // Step 4b: Check for input decomposition opportunity
    // If a decomposition strategy is registered for this image ID and the input
    // is decomposable with enough cycles, use decomposed proving with recursive
    // verification wrapper.
    let decomp_threshold_cycles = 20_000_000; // 20M cycles minimum for decomposition
    if let Some(strategy) = strategy_registry.get(&job.image_id) {
        if preflight.cycles > decomp_threshold_cycles && strategy.can_decompose(&input) {
            info!(
                "[Job {}] Input decomposition available ({} strategy, {} items)",
                job.request_id,
                strategy.name(),
                strategy.item_count(&input).unwrap_or(0),
            );

            let decomposer = InputDecomposer::new(
                DecomposerConfig::default(),
                config.proving_mode.clone(),
                use_snark,
            );

            // Use receipt-preserving path for recursive wrapping
            match decomposer
                .prove_decomposed_with_receipts(&elf, strategy.as_ref(), &input)
                .await
            {
                Ok(decomp_result) => {
                    info!(
                        "[Job {}] Decomposed proving complete: {} sub-jobs, {:.1}x speedup, {:?}",
                        job.request_id,
                        decomp_result.sub_job_count,
                        decomp_result.speedup,
                        decomp_result.total_time,
                    );

                    // Try recursive wrapping for trustless verification.
                    // Returns Some((seal, journal)) if we have a valid proof to submit,
                    // or None if we should fall through to standard proving.
                    let decomp_proof = match try_recursive_wrap(
                        &job.image_id,
                        &decomp_result,
                        strategy.as_ref(),
                        use_snark,
                    )
                    .await
                    {
                        Ok(wrap_result) => {
                            info!(
                                "[Job {}] Recursive wrapper proof complete: {:?}",
                                job.request_id, wrap_result.wrap_time,
                            );
                            Some((wrap_result.first_sub_seal, wrap_result.merged_journal))
                        }
                        Err(e) if decomp_result.sub_job_count == 1 => {
                            // Single sub-job: the first seal/journal is a complete proof
                            warn!(
                                "[Job {}] Recursive wrapping failed but only 1 sub-job, using directly: {}",
                                job.request_id, e
                            );
                            let seal = decomp_result.seals.into_iter().next().unwrap_or_default();
                            let journal = decomp_result
                                .journals
                                .into_iter()
                                .next()
                                .unwrap_or_default();
                            Some((seal, journal))
                        }
                        Err(e) => {
                            // Multiple sub-jobs but wrapper failed: each seal only proves
                            // a subset of data. Submitting any single sub-proof with the
                            // merged journal would fail verification (proof/journal mismatch).
                            // Fall through to standard (non-decomposed) proving.
                            warn!(
                                "[Job {}] Recursive wrapping failed for {} sub-jobs, \
                                 cannot submit partial proof. Falling back to standard proving: {}",
                                job.request_id, decomp_result.sub_job_count, e
                            );
                            None
                        }
                    };

                    // Only submit if we have a valid proof from decomposition
                    if let Some((seal, journal)) = decomp_proof {
                        let retry_config = RetryConfig::default();
                        // Claim (skip if already claimed for private inputs)
                        claim_unless_claimed(
                            &early_claim_result,
                            " (decomposed path)",
                            &engine,
                            request_id,
                            nonce_manager,
                            job.request_id,
                            &retry_config,
                        )
                        .await?;

                        timeline.enter_stage(ProveStage::Submission);
                        info!("[Job {}] Submitting decomposed proof...", job.request_id);
                        let submit_timer = Timer::start();

                        let submit_result = send_submit_with_retry(
                            &engine,
                            request_id,
                            seal,
                            journal,
                            nonce_manager,
                            job.request_id,
                            &retry_config,
                        )
                        .await?;

                        metrics().record_submit_time(submit_timer.elapsed());
                        info!(
                            "[Job {}] Decomposed proof submitted in tx: {:?}",
                            job.request_id, submit_result.tx_hash
                        );

                        let finished = timeline.finish(true);
                        info!("[Job {}] {}", job.request_id, finished);
                        pipeline_metrics().record(&finished);

                        return Ok((decomp_result.total_cycles, input.len() as u64));
                    }
                    // else: fall through to standard proving below
                }
                Err(e) => {
                    warn!(
                        "[Job {}] Decomposed proving failed, falling back to standard: {}",
                        job.request_id, e
                    );
                    // Fall through to standard proving
                }
            }
        }
    }

    // Step 5: Profitability check
    if !config.skip_profitability_check {
        let profitability = check_profitability(
            provider,
            &engine,
            request_id,
            job.tip,
            config.min_profit_margin,
        )
        .await?;

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
            job.request_id,
            job.tip,
            profitability.estimated_cost,
            profitability.profit_margin * 100.0
        );
    }

    // Step 6: Claim the request (skip if already claimed for private inputs)
    let retry_config = RetryConfig::default();
    claim_unless_claimed(
        &early_claim_result,
        "",
        &engine,
        request_id,
        nonce_manager,
        job.request_id,
        &retry_config,
    )
    .await?;

    // Step 7: Generate proof using fast prover
    timeline.enter_stage(ProveStage::Proving);
    info!("[Job {}] Generating proof...", job.request_id);
    let prove_timer = Timer::start();
    let prove_result = fast_prover.prove_fast(&elf, &input).await?;

    info!(
        "[Job {}] Proof generated in {:?} ({} bytes)",
        job.request_id,
        prove_result.proof_time,
        prove_result.proof.len()
    );

    let proof = CachedProof::new(
        prove_result.proof,
        prove_result.journal,
        prove_result.cycles,
        prove_timer.elapsed(),
    );

    // Step 8: Submit proof (with retry logic) and cache it for future use
    timeline.enter_stage(ProveStage::Submission);
    info!("[Job {}] Submitting proof...", job.request_id);
    let submit_timer = Timer::start();

    let submit_result = submit_and_cache(
        &engine,
        request_id,
        proof,
        nonce_manager,
        job.request_id,
        &retry_config,
        proof_cache,
        &cache_key,
    )
    .await?;

    metrics().record_submit_time(submit_timer.elapsed());

    info!(
        "[Job {}] Proof submitted in tx: {:?}",
        job.request_id, submit_result.tx_hash
    );

    // Record pipeline timeline
    let finished = timeline.finish(true);
    info!("[Job {}] {}", job.request_id, finished);
    pipeline_metrics().record(&finished);

    Ok((prove_result.cycles, input.len() as u64))
}

/// Try to recursively wrap decomposed sub-proofs into a single verified proof.
///
/// Loads the wrapper ELF from disk. If not available, returns an error so the
/// caller can fall back to using the first sub-proof seal.
async fn try_recursive_wrap(
    image_id: &B256,
    decomp_result: &crate::input_decomposer::DecomposedReceiptResult,
    strategy: &dyn crate::input_decomposer::DecompositionStrategy,
    use_snark: bool,
) -> anyhow::Result<recursive_wrapper::RecursiveWrapResult> {
    let wrapper = recursive_wrapper::try_load_wrapper(use_snark)
        .ok_or_else(|| anyhow::anyhow!("Recursive wrapper ELF not available"))?;

    let mut inner_image_id = [0u8; 32];
    inner_image_id.copy_from_slice(image_id.as_slice());

    // Clone receipts for the wrapper (it takes ownership)
    let receipts = decomp_result.receipts.to_vec();

    wrapper.wrap(inner_image_id, receipts, strategy).await
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
            Ok(result) => result,
            Err(e) => {
                // The Debug text names the URL of the node
                let error = redact::redact_urls(&format!("{:?}", e));
                warn!("Failed to fetch program from registry: {}", error);
                anyhow::bail!("Program {} not found in registry: {}", image_id, error);
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
                image_id,
                computed_image_id
            );
        }

        info!(
            "Successfully downloaded and verified ELF ({} bytes)",
            elf.len()
        );

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
    if IpfsClient::is_ipfs_url(url) {
        // Use IPFS client with multi-gateway fallback
        let ipfs_client = IpfsClient::new(IpfsConfig::default())?;
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
    crate::cache::write_atomic(&elf_path, elf)?;
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
    /// Maximum time to wait for the receipt of a sent transaction
    pub receipt_timeout: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            receipt_timeout: Duration::from_secs(300),
        }
    }
}

/// How the HTTP client failed, for an error behind which there is one
#[derive(Debug, PartialEq)]
enum HttpClientFailure {
    /// No connection to the node was made, so nothing was sent
    NotConnected,
    /// The request may have reached the node: a request timeout, or a
    /// connection that broke after it was made
    AfterConnecting,
}

/// How the HTTP client failed, if `error` or an error behind it comes from
/// it. None for an error the node answered with, or an HTTP status.
fn http_client_failure(error: &(dyn std::error::Error + 'static)) -> Option<HttpClientFailure> {
    let mut next = Some(error);
    while let Some(error) = next {
        if let Some(http) = error.downcast_ref::<reqwest::Error>() {
            // A connect timeout is a timeout and a connect error
            return Some(if http.is_connect() {
                HttpClientFailure::NotConnected
            } else {
                HttpClientFailure::AfterConnecting
            });
        }
        next = error.source();
    }
    None
}

/// Check if an error is retryable (transient) or permanent, by its text. For
/// errors that do not come from the HTTP client (see [`http_client_failure`]).
///
/// Retryable errors include:
/// - Network timeouts
/// - Connection errors
/// - Rate limiting (429)
/// - Server errors (5xx)
/// - Nonce too low (the next attempt has a nonce re-read from the chain)
///
/// Non-retryable errors include:
/// - Insufficient funds
/// - Contract reverts (execution reverted)
/// - Already claimed/completed
fn is_retryable_error(error: &str) -> bool {
    let error_lower = error.to_lowercase();

    // Non-retryable errors - return false immediately
    let permanent_errors = [
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
        "nonce too low",
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
    #[allow(dead_code)]
    pub attempts: u32,
}

/// Claim the request unless `early_claim` shows that this job has already
/// claimed it (private inputs are claimed before the input is fetched).
/// `path` is appended to the log line to tell the claim sites apart.
async fn claim_unless_claimed(
    early_claim: &Option<TransactionResult>,
    path: &str,
    engine: &IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>>,
    request_id: U256,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
) -> anyhow::Result<()> {
    if early_claim.is_some() {
        debug!(
            "[Job {}] Already claimed (private input early claim)",
            job_id
        );
        return Ok(());
    }

    info!("[Job {}] Claiming{}...", job_id, path);
    let claim_result =
        send_claim_with_retry(engine, request_id, nonce_manager, job_id, config).await?;
    info!("[Job {}] Claimed in tx: {:?}", job_id, claim_result.tx_hash);
    Ok(())
}

/// Send a claim transaction with retry logic
async fn send_claim_with_retry(
    engine: &IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>>,
    request_id: U256,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
) -> anyhow::Result<TransactionResult> {
    let call = engine.claimExecution(request_id);
    send_with_retry("Claim", call, nonce_manager, job_id, config).await
}

/// Send a submit proof transaction with retry logic
async fn send_submit_with_retry(
    engine: &IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>>,
    request_id: U256,
    proof: Vec<u8>,
    journal: Vec<u8>,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
) -> anyhow::Result<TransactionResult> {
    let call = engine.submitProof(request_id, proof.into(), journal.into());
    send_with_retry("Submit", call, nonce_manager, job_id, config).await
}

/// Submit a proof and, once the submission succeeded, put it into the proof cache
#[allow(clippy::too_many_arguments)]
async fn submit_and_cache(
    engine: &IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>>,
    request_id: U256,
    proof: CachedProof,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
    proof_cache: &ProofCache,
    cache_key: &ProofCacheKey,
) -> anyhow::Result<TransactionResult> {
    let result = send_submit_with_retry(
        engine,
        request_id,
        proof.proof.clone(),
        proof.journal.clone(),
        nonce_manager,
        job_id,
        config,
    )
    .await?;

    proof_cache.put(cache_key, proof).await;
    Ok(result)
}

/// Time between two reads of a receipt after a read failed
const RECEIPT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Wait for the receipt of a transaction the node has accepted, for at most
/// `receipt_timeout` from now. None when there is no receipt by then.
///
/// A failed read of the receipt does not mean that the transaction failed,
/// and sending it again could put it on the chain twice, so the receipt of
/// the same hash is read again until the time is up.
async fn wait_for_receipt(
    pending_tx: alloy::providers::PendingTransactionBuilder<Ethereum>,
    kind: &str,
    job_id: u64,
    receipt_timeout: Duration,
) -> Option<alloy::rpc::types::TransactionReceipt> {
    let sent_at = Instant::now();
    let tx_hash = *pending_tx.tx_hash();
    let provider = pending_tx.provider().clone();

    let watched = tokio::time::timeout(
        receipt_timeout,
        pending_tx.with_timeout(Some(receipt_timeout)).get_receipt(),
    )
    .await;
    // The error of the last read; None when the receipt was not there yet
    let mut error = match watched {
        Ok(Ok(receipt)) => return Some(receipt),
        Err(_) | Ok(Err(PendingTransactionError::TxWatcher(WatchTxError::Timeout))) => return None,
        Ok(Err(e)) => Some(format!("{:?}", e)),
    };

    loop {
        let left = receipt_timeout.saturating_sub(sent_at.elapsed());
        if left.is_zero() {
            return None;
        }
        if let Some(error) = &error {
            warn!(
                "[Job {}] {} tx {:?}: reading the receipt failed, reading it again: {}",
                job_id,
                kind,
                tx_hash,
                redact::redact_urls(error)
            );
        }
        tokio::time::sleep(RECEIPT_POLL_INTERVAL.min(left)).await;

        let left = receipt_timeout.saturating_sub(sent_at.elapsed());
        match tokio::time::timeout(left, provider.get_transaction_receipt(tx_hash)).await {
            Ok(Ok(Some(receipt))) => return Some(receipt),
            Ok(Ok(None)) => error = None,
            Ok(Err(e)) => error = Some(format!("{:?}", e)),
            Err(_) => return None,
        }
    }
}

/// Send a contract call as a transaction and wait for its receipt, with retry
/// logic. `kind` names the transaction in log lines and errors.
async fn send_with_retry<D: CallDecoder + Clone>(
    kind: &str,
    call: CallBuilder<&Arc<SigningProvider>, D>,
    nonce_manager: &Arc<SigningNonceManager>,
    job_id: u64,
    config: &RetryConfig,
) -> anyhow::Result<TransactionResult> {
    let mut attempts = 0u32;
    let mut delay = config.initial_delay;

    loop {
        attempts += 1;

        // Estimates the gas, signs and broadcasts
        let result = nonce_manager
            .send(|nonce| {
                debug!(
                    "[Job {}] {} attempt {} with nonce {}",
                    job_id, kind, attempts, nonce
                );
                let call = call.clone().nonce(nonce);
                async move { call.send().await }
            })
            .await;

        let (error_str, retryable) = match result {
            Ok(pending_tx) => {
                let tx_hash = *pending_tx.tx_hash();
                match wait_for_receipt(pending_tx, kind, job_id, config.receipt_timeout).await {
                    Some(receipt) => {
                        if !receipt.status() {
                            return Err(anyhow::anyhow!(
                                "{} tx {:?} reverted on-chain",
                                kind,
                                receipt.transaction_hash
                            ));
                        }
                        if attempts > 1 {
                            info!(
                                "[Job {}] {} succeeded on attempt {}",
                                job_id, kind, attempts
                            );
                        }
                        return Ok(TransactionResult {
                            tx_hash: receipt.transaction_hash,
                            attempts,
                        });
                    }
                    None => {
                        // If the transaction was dropped, its nonce is free again
                        nonce_manager.sync_from_chain().await;
                        return Err(anyhow::anyhow!(
                            "{} tx {:?} has no receipt after {:?}",
                            kind,
                            tx_hash,
                            config.receipt_timeout
                        ));
                    }
                }
            }
            Err(e) => {
                // The Debug text names the URL of the node, so it is redacted
                let error_str = redact::redact_urls(&format!("{:?}", e));
                let retryable = match http_client_failure(&e) {
                    Some(HttpClientFailure::NotConnected) => true,
                    // The node may have accepted the transaction before its
                    // answer was lost, and a retry would send a second one.
                    // The text is not looked at: it says "timeout", and the
                    // port in the URL can read like "502".
                    Some(HttpClientFailure::AfterConnecting) => false,
                    None => is_retryable_error(&error_str),
                };
                (error_str, retryable)
            }
        };

        if !retryable || attempts >= config.max_retries {
            return Err(anyhow::anyhow!(
                "{} send failed after {} attempts: {}",
                kind,
                attempts,
                error_str
            ));
        }

        warn!(
            "[Job {}] {} send failed (attempt {}), retrying: {}",
            job_id, kind, attempts, error_str
        );

        // Exponential backoff
        tokio::time::sleep(delay).await;
        delay = Duration::from_secs_f64(
            (delay.as_secs_f64() * config.backoff_multiplier).min(config.max_delay.as_secs_f64()),
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
    engine: &IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>>,
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
            warn!(
                "Failed to estimate claim gas, using default: {}",
                redact::redact_urls(&e.to_string())
            );
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
            debug!(
                "Failed to estimate submit gas (expected before claim): {}",
                redact::redact_urls(&e.to_string())
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bonsai::ProvingMode;
    use crate::test_support::{serve_http, MockChain, MockNode};

    /// Retries without a noticeable delay
    fn fast_retries() -> RetryConfig {
        RetryConfig {
            initial_delay: Duration::from_millis(1),
            receipt_timeout: Duration::from_secs(30),
            ..Default::default()
        }
    }

    /// Claim request `id` with `config`
    async fn claim(
        node: &MockNode,
        id: u64,
        config: &RetryConfig,
    ) -> anyhow::Result<TransactionResult> {
        send_claim_with_retry(
            &node.engine(),
            U256::from(id),
            &node.nonce_manager,
            id,
            config,
        )
        .await
    }

    #[tokio::test]
    async fn test_claim_with_successful_receipt() {
        let node = MockNode::start(MockChain::default()).await;

        let result = claim(&node, 7, &RetryConfig::default()).await.unwrap();

        assert_eq!(result.tx_hash, node.chain().transactions[0].hash);
        assert_eq!(node.transactions_sent(), 1);
        assert_eq!(node.nonce_manager.current().await, 1);
    }

    #[tokio::test]
    async fn test_reverted_claim_is_an_error() {
        let node = MockNode::start(MockChain {
            receipt_status: Some("0x0"),
            ..Default::default()
        })
        .await;

        let result = claim(&node, 7, &RetryConfig::default()).await;

        let err = result.err().expect("reverted claim reported as success");
        assert!(err.to_string().contains("reverted on-chain"), "{err}");
        // The transaction was mined: its nonce is used and it is not sent again
        assert_eq!(node.transactions_sent(), 1);
        assert_eq!(node.nonce_manager.current().await, 1);
    }

    #[tokio::test]
    async fn test_reverted_submit_is_an_error() {
        let node = MockNode::start(MockChain {
            receipt_status: Some("0x0"),
            ..Default::default()
        })
        .await;

        let result = send_submit_with_retry(
            &node.engine(),
            U256::from(7),
            vec![1, 2, 3],
            vec![4, 5, 6],
            &node.nonce_manager,
            7,
            &RetryConfig::default(),
        )
        .await;

        let err = result.err().expect("reverted submit reported as success");
        assert!(err.to_string().contains("reverted on-chain"), "{err}");
        assert_eq!(node.transactions_sent(), 1);
        assert_eq!(node.nonce_manager.current().await, 1);
    }

    /// A claim that reverts at gas estimation sends nothing and is not
    /// retried, and its nonce goes to the next claim.
    #[tokio::test]
    async fn test_revert_at_estimation_uses_no_nonce() {
        let node = MockNode::start(MockChain {
            reverting_requests: vec![U256::from(7)],
            ..Default::default()
        })
        .await;

        let err = claim(&node, 7, &fast_retries())
            .await
            .err()
            .expect("reverting claim reported as success");
        assert!(err.to_string().contains("after 1 attempts"), "{err}");
        assert!(err.to_string().contains("execution reverted"), "{err}");
        assert_eq!(node.transactions_sent(), 0);
        assert_eq!(node.nonce_manager.current().await, 0);

        claim(&node, 8, &fast_retries()).await.unwrap();
        assert_eq!(node.accepted_nonces(), [0]);
    }

    #[tokio::test]
    async fn test_submit_revert_at_estimation_uses_no_nonce() {
        let node = MockNode::start(MockChain {
            reverting_requests: vec![U256::from(7)],
            ..Default::default()
        })
        .await;

        for (id, reverts) in [(7, true), (8, false)] {
            let result = send_submit_with_retry(
                &node.engine(),
                U256::from(id),
                vec![1],
                vec![2],
                &node.nonce_manager,
                id,
                &fast_retries(),
            )
            .await;
            assert_eq!(result.is_err(), reverts);
        }
        assert_eq!(node.accepted_nonces(), [0]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_claims_use_consecutive_nonces() {
        let node = Arc::new(
            MockNode::start(MockChain {
                mined_count: 5,
                pending_count: 5,
                reverting_requests: [2, 3, 9].map(U256::from).to_vec(),
                ..Default::default()
            })
            .await,
        );

        let claims = (0..10).map(|id| {
            let node = node.clone();
            tokio::spawn(async move { claim(&node, id, &fast_retries()).await.is_ok() })
        });
        let succeeded: Vec<bool> = join_all(claims)
            .await
            .into_iter()
            .map(|result| result.unwrap())
            .collect();

        let expected: Vec<bool> = (0..10).map(|id| ![2, 3, 9].contains(&id)).collect();
        assert_eq!(succeeded, expected);
        // Seven transactions, without gap or duplicate
        assert_eq!(node.transactions_sent(), 7);
        assert_eq!(node.accepted_nonces(), [5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(node.nonce_manager.current().await, 12);
    }

    /// A transaction whose receipt never comes ends with an error, and the
    /// nonce of a dropped transaction goes to the next one.
    #[tokio::test]
    async fn test_missing_receipt_is_an_error_after_the_timeout() {
        let node = MockNode::start(MockChain {
            drops_transactions: true,
            ..Default::default()
        })
        .await;
        let config = RetryConfig {
            receipt_timeout: Duration::from_millis(200),
            ..fast_retries()
        };

        let result = tokio::time::timeout(Duration::from_secs(30), claim(&node, 7, &config))
            .await
            .expect("still waiting for the receipt");
        let err = result.err().expect("claim without receipt succeeded");
        let tx_hash = node.chain().transactions[0].hash;
        assert!(err.to_string().contains(&format!("{tx_hash:?}")), "{err}");
        // Not sent a second time
        assert_eq!(node.transactions_sent(), 1);

        node.chain().drops_transactions = false;
        claim(&node, 8, &fast_retries()).await.unwrap();
        assert_eq!(node.accepted_nonces(), [0, 0]);
    }

    /// A transaction that is still in the node's pool when the wait for its
    /// receipt ends keeps its nonce.
    #[tokio::test]
    async fn test_pending_transaction_keeps_its_nonce_after_the_timeout() {
        let node = MockNode::start(MockChain {
            receipt_status: None,
            ..Default::default()
        })
        .await;
        let config = RetryConfig {
            receipt_timeout: Duration::from_millis(200),
            ..fast_retries()
        };

        assert!(claim(&node, 7, &config).await.is_err());

        node.chain().receipt_status = Some("0x1");
        claim(&node, 8, &fast_retries()).await.unwrap();
        assert_eq!(node.accepted_nonces(), [0, 1]);
    }

    #[tokio::test]
    async fn test_nonce_too_low_is_retried_with_the_reread_nonce() {
        let node = MockNode::start(MockChain::default()).await;

        // Another user of the account sent three transactions
        node.chain().mined_count = 3;
        node.chain().pending_count = 3;

        let result = claim(&node, 7, &fast_retries()).await.unwrap();

        assert_eq!(result.attempts, 2);
        assert_eq!(node.transactions_sent(), 2);
        assert_eq!(node.accepted_nonces(), [3]);
    }

    /// The proof is cached once its submission succeeded, not before.
    #[tokio::test]
    async fn test_proof_is_cached_only_after_its_submit_succeeded() {
        // Memory-only cache, so the test writes no proof files
        let cache = ProofCache::new(10, None, Duration::from_secs(3600));
        let key = ProofCacheKey::new(B256::repeat_byte(1), B256::repeat_byte(2), false);
        let proof = CachedProof::new(vec![1], vec![2], 3, Duration::ZERO);

        // Reverts at estimation, reverts on-chain, succeeds
        for (reverting_requests, receipt_status, cached) in [
            (vec![U256::from(7)], "0x1", 0),
            (vec![], "0x0", 0),
            (vec![], "0x1", 1),
        ] {
            let node = MockNode::start(MockChain {
                reverting_requests,
                receipt_status: Some(receipt_status),
                ..Default::default()
            })
            .await;
            let result = submit_and_cache(
                &node.engine(),
                U256::from(7),
                proof.clone(),
                &node.nonce_manager,
                7,
                &fast_retries(),
                &cache,
                &key,
            )
            .await;

            assert_eq!(result.is_ok(), cached == 1);
            assert_eq!(cache.memory_len().await, cached);
        }
        assert_eq!(cache.get(&key).await.unwrap().proof, [1]);
    }

    #[tokio::test]
    async fn test_rpc_call_fails_after_the_request_timeout() {
        // Accepts connections (the kernel does) and never answers
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let provider = signing_provider(
            format!("http://{addr}").parse().unwrap(),
            PrivateKeySigner::random(),
            Duration::from_secs(10),
            Duration::from_millis(200),
        )
        .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(20), provider.get_chain_id()).await;
        assert!(result.expect("the call is still waiting").is_err());
    }

    #[test]
    fn test_default_timeouts() {
        assert_eq!(
            RetryConfig::default().receipt_timeout,
            Duration::from_secs(300)
        );
        assert_eq!(RPC_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(RPC_REQUEST_TIMEOUT, Duration::from_secs(30));
        assert_eq!(RECEIPT_POLL_INTERVAL, Duration::from_secs(2));
    }

    /// Claim request 7 through the node at `url`. The nonces come from `node`.
    async fn claim_through(
        node: &MockNode,
        url: String,
        config: &RetryConfig,
    ) -> anyhow::Result<TransactionResult> {
        let provider = signing_provider(
            url.parse().unwrap(),
            PrivateKeySigner::random(),
            Duration::from_secs(10),
            Duration::from_millis(200),
        )
        .unwrap();
        let engine = IExecutionEngine::new(Address::repeat_byte(0xee), Arc::new(provider));

        let claim = send_claim_with_retry(&engine, U256::from(7), &node.nonce_manager, 7, config);
        tokio::time::timeout(Duration::from_secs(30), claim)
            .await
            .expect("the claim is still waiting")
    }

    /// Claim request 7 through a node at `path` that accepts connections (the
    /// kernel does) and never answers. The nonces come from `node`.
    async fn claim_through_silent_node(
        node: &MockNode,
        path: &str,
        config: &RetryConfig,
    ) -> anyhow::Result<TransactionResult> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        claim_through(node, format!("http://{addr}{path}"), config).await
    }

    /// A port on which nothing listens
    async fn refusing_addr() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    }

    /// The node accepts the transaction and answers after the request
    /// timeout. Sending the call again would put a second transaction on
    /// the chain.
    #[tokio::test]
    async fn test_request_timeout_at_send_is_not_retried() {
        let node = MockNode::start_with(
            MockChain {
                late_methods: vec!["eth_sendRawTransaction"],
                ..Default::default()
            },
            "",
            Duration::from_millis(500),
        )
        .await;

        let result =
            tokio::time::timeout(Duration::from_secs(30), claim(&node, 7, &fast_retries()))
                .await
                .expect("the claim is still waiting");

        let err = result.err().expect("claim without an answer succeeded");
        assert!(
            err.to_string().contains("send failed after 1 attempts"),
            "{err}"
        );
        assert_eq!(node.transactions_sent(), 1);
        assert_eq!(node.accepted_nonces(), vec![0]);
        // The pending count was read again: the accepted transaction keeps
        // its nonce
        assert_eq!(node.nonce_manager.current().await, 1);
    }

    #[tokio::test]
    async fn test_refused_connection_at_send_is_retried() {
        let node = MockNode::start(MockChain::default()).await;
        let addr = refusing_addr().await;

        let err = claim_through(&node, format!("http://{addr}"), &fast_retries())
            .await
            .err()
            .expect("claim through a refusing node succeeded");

        assert!(
            err.to_string().contains("send failed after 3 attempts"),
            "{err}"
        );
        assert_eq!(node.nonce_manager.current().await, 0);
    }

    #[tokio::test]
    async fn test_send_error_does_not_show_the_url_path() {
        let node = MockNode::start(MockChain::default()).await;
        let config = RetryConfig {
            max_retries: 1,
            ..fast_retries()
        };

        let err = claim_through_silent_node(&node, "/v2/SECRETKEY", &config)
            .await
            .err()
            .expect("claim through a silent node succeeded");

        let text = format!("{err:#}");
        assert!(text.contains("send failed after 1 attempts"), "{text}");
        assert!(text.contains("127.0.0.1"), "{text}");
        assert!(!text.contains("SECRETKEY"), "{text}");
    }

    #[tokio::test]
    async fn test_http_client_failure_tells_a_refused_connection_from_a_timeout() {
        let provider_at = |addr: std::net::SocketAddr| {
            signing_provider(
                format!("http://{addr}").parse().unwrap(),
                PrivateKeySigner::random(),
                Duration::from_secs(10),
                Duration::from_millis(200),
            )
            .unwrap()
        };

        let refused = provider_at(refusing_addr().await);
        let err = refused.get_chain_id().await.unwrap_err();
        assert_eq!(
            http_client_failure(&err),
            Some(HttpClientFailure::NotConnected),
            "{err:?}"
        );

        // Accepts connections (the kernel does) and never answers
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = provider_at(listener.local_addr().unwrap());
        let err = silent.get_chain_id().await.unwrap_err();
        assert_eq!(
            http_client_failure(&err),
            Some(HttpClientFailure::AfterConnecting),
            "{err:?}"
        );

        // A node's error answer is not one
        let node = MockNode::start(MockChain::default()).await;
        node.chain().fail_transaction_count = true;
        let err = node
            .provider
            .get_transaction_count(Address::ZERO)
            .await
            .unwrap_err();
        assert_eq!(http_client_failure(&err), None, "{err:?}");
    }

    /// A failed read of the receipt is followed by another read of the same
    /// transaction's receipt, not by a second transaction.
    #[tokio::test]
    async fn test_failed_receipt_read_does_not_send_the_transaction_again() {
        let node = MockNode::start(MockChain {
            failing_receipt_reads: 1,
            ..Default::default()
        })
        .await;

        let result =
            tokio::time::timeout(Duration::from_secs(30), claim(&node, 7, &fast_retries()))
                .await
                .expect("still waiting for the receipt")
                .unwrap();

        assert_eq!(result.attempts, 1);
        assert_eq!(result.tx_hash, node.chain().transactions[0].hash);
        assert_eq!(node.transactions_sent(), 1);
        assert_eq!(node.nonce_manager.current().await, 1);
    }

    /// Receipt reads that fail until the timeout end with the error of a
    /// missing receipt, still without a second transaction.
    #[tokio::test]
    async fn test_failing_receipt_reads_end_at_the_timeout() {
        let node = MockNode::start(MockChain {
            failing_receipt_reads: usize::MAX,
            ..Default::default()
        })
        .await;
        let config = RetryConfig {
            receipt_timeout: Duration::from_millis(500),
            ..fast_retries()
        };

        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(30), claim(&node, 7, &config))
            .await
            .expect("still waiting for the receipt");

        let err = result.err().expect("claim without receipt succeeded");
        assert!(err.to_string().contains("has no receipt after"), "{err}");
        // Ends at the timeout, not after a whole poll interval
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(node.transactions_sent(), 1);
    }

    /// A processor and a node on which requests 7 and 8 are pending. Only
    /// request 7 has an event, in a block before the latest one.
    async fn processor_with_pending_requests(
        fail_get_logs: bool,
    ) -> (OptimizedProcessor, MockNode, ProverConfig) {
        let node = MockNode::start(MockChain {
            events: vec![(
                40,
                IExecutionEngine::ExecutionRequested {
                    requestId: U256::from(7),
                    requester: Address::repeat_byte(0x33),
                    imageId: B256::repeat_byte(0x44),
                    inputDigest: B256::repeat_byte(0x55),
                    inputUrl: "https://input.test/7".to_string(),
                    inputType: 1,
                    tip: U256::from(1_000_000_000_000_000u64),
                    expiresAt: U256::from(u32::MAX),
                },
            )],
            fail_get_logs,
            ..Default::default()
        })
        .await;
        let config = ProverConfig {
            engine_address: *node.engine().address(),
            ..Default::default()
        };
        (test_processor(10).unwrap(), node, config)
    }

    #[tokio::test]
    async fn test_requests_are_queued_with_the_url_of_an_earlier_block() {
        let (processor, node, config) = processor_with_pending_requests(false).await;

        let added = processor
            .queue_new_requests(&node.provider, &config, &[7, 8].map(U256::from))
            .await;

        // Request 8 has no event: it is not queued with an empty URL
        assert_eq!(added, 1);
        let queue = processor.job_queue.read().await;
        let jobs: Vec<_> = queue.iter_jobs().collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].request_id, 7);
        assert_eq!(jobs[0].input_url, "https://input.test/7");
        assert_eq!(jobs[0].input_type, 1);
    }

    #[tokio::test]
    async fn test_request_without_event_is_looked_up_again() {
        let (processor, node, config) = processor_with_pending_requests(false).await;
        let pending = [U256::from(8)];

        let added = processor
            .queue_new_requests(&node.provider, &config, &pending)
            .await;
        assert_eq!(added, 0);

        // The node now knows the event
        let (block, mut event) = node.chain().events[0].clone();
        event.requestId = U256::from(8);
        event.inputUrl = "https://input.test/8".to_string();
        node.chain().events.push((block, event));
        let added = processor
            .queue_new_requests(&node.provider, &config, &pending)
            .await;
        assert_eq!(added, 1);
        let queue = processor.job_queue.read().await;
        assert_eq!(
            queue.iter_jobs().next().unwrap().input_url,
            "https://input.test/8"
        );
    }

    #[tokio::test]
    async fn test_failed_url_lookup_queues_nothing() {
        let (processor, node, config) = processor_with_pending_requests(true).await;

        let added = processor
            .queue_new_requests(&node.provider, &config, &[7, 8].map(U256::from))
            .await;

        assert_eq!(added, 0);
        assert_eq!(processor.job_queue.read().await.iter_jobs().count(), 0);

        // The next cycle finds it
        node.chain().fail_get_logs = false;
        let added = processor
            .queue_new_requests(&node.provider, &config, &[7, 8].map(U256::from))
            .await;
        assert_eq!(added, 1);
    }

    /// Log lines written on this thread, for a subscriber of [`capture_logs`]
    #[derive(Clone, Default)]
    struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl LogBuffer {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// Capture the log lines of this thread until the guard is dropped
    fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
        let logs = LogBuffer::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    #[tokio::test]
    async fn test_failed_request_lookup_does_not_log_the_url_path() {
        let node = MockNode::start_with(
            MockChain {
                events: vec![(
                    40,
                    IExecutionEngine::ExecutionRequested {
                        requestId: U256::from(7),
                        requester: Address::repeat_byte(0x33),
                        imageId: B256::repeat_byte(0x44),
                        inputDigest: B256::repeat_byte(0x55),
                        inputUrl: "https://input.test/7".to_string(),
                        inputType: 0,
                        tip: U256::from(1_000_000_000_000_000u64),
                        expiresAt: U256::from(u32::MAX),
                    },
                )],
                // getRequest and getCurrentTip run into the request timeout
                late_methods: vec!["eth_call"],
                ..Default::default()
            },
            "/v2/SECRETKEY",
            Duration::from_millis(500),
        )
        .await;
        let config = ProverConfig {
            engine_address: *node.engine().address(),
            ..Default::default()
        };
        let processor = test_processor(10).unwrap();

        let (logs, _guard) = capture_logs();
        let added = processor
            .queue_new_requests(&node.provider, &config, &[U256::from(7)])
            .await;

        assert_eq!(added, 0);
        let text = logs.text();
        assert!(text.contains("Failed to fetch request 7"), "{text}");
        assert!(text.contains("127.0.0.1"), "{text}");
        assert!(!text.contains("SECRETKEY"), "{text}");
    }

    #[tokio::test]
    async fn test_claim_is_skipped_after_early_claim() {
        let node = MockNode::start(MockChain::default()).await;
        let engine = node.engine();
        let retry_config = RetryConfig::default();

        // A private-input job that claimed before fetching its input
        let early_claim = Some(TransactionResult {
            tx_hash: B256::repeat_byte(0x11),
            attempts: 1,
        });
        for path in [" (cached path)", " (decomposed path)", ""] {
            claim_unless_claimed(
                &early_claim,
                path,
                &engine,
                U256::from(7),
                &node.nonce_manager,
                7,
                &retry_config,
            )
            .await
            .unwrap();
        }
        assert_eq!(node.transactions_sent(), 0);
        assert_eq!(node.nonce_manager.current().await, 0);

        // A job without an early claim claims now
        claim_unless_claimed(
            &None,
            "",
            &engine,
            U256::from(7),
            &node.nonce_manager,
            7,
            &retry_config,
        )
        .await
        .unwrap();
        assert_eq!(node.transactions_sent(), 1);
    }

    /// A job holds its GPU device through the concurrency manager for as long
    /// as it runs. With the processor's own wiring and one GPU slot,
    /// prove_fast must not wait for a device on top of that.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_prove_fast_does_not_wait_for_device_held_by_job() {
        use crate::concurrency::JobResource;

        // Preflight is off so that the invalid ELF below is not refused
        // before prove_fast reaches the proving step.
        let processor = OptimizedProcessor::with_fast_prove_config(
            2,
            U256::ZERO,
            1,
            ProvingMode::Local,
            false,
            1,
            0,
            10,
            FastProveConfig {
                preflight_enabled: false,
                ..Default::default()
            },
        )
        .unwrap();

        // What the task of each job does before it calls process_single_job,
        // and with it prove_fast
        let resource = processor.concurrency.classify_job(10_000_000, 0);
        assert_eq!(resource, JobResource::Gpu);
        let _guard = processor.concurrency.acquire(resource).await.unwrap();

        // The ELF is not a valid program, so proving fails; the point is that
        // prove_fast gets that far while the job holds the only device.
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            processor.fast_prover.prove_fast(b"not an elf", &[]),
        )
        .await;
        assert!(result.is_ok(), "prove_fast is waiting for a GPU device");
        assert!(result.unwrap().is_err());
    }

    fn test_processor(queue_size: usize) -> anyhow::Result<OptimizedProcessor> {
        OptimizedProcessor::with_gpu_config(
            2,
            U256::ZERO,
            1,
            ProvingMode::Local,
            false,
            0,
            0,
            queue_size,
        )
    }

    fn test_job(request_id: u64, tip_wei: u64) -> QueuedJob {
        QueuedJob {
            request_id,
            image_id: B256::ZERO,
            input_hash: B256::ZERO,
            input_url: String::new(),
            input_type: 0,
            tip: U256::from(tip_wei),
            requester: Address::ZERO,
            expires_at: u64::MAX,
            queued_at: Instant::now(),
            estimated_cycles: None,
            program_cached: false,
            prefetched_input: None,
        }
    }

    #[tokio::test]
    async fn test_queue_size_limits_job_queue() {
        let processor = test_processor(2).unwrap();

        let mut queue = processor.job_queue.write().await;
        assert!(queue.push(test_job(1, 3_000_000_000_000_000)));
        assert!(queue.push(test_job(2, 2_000_000_000_000_000)));
        // The queue is full and this job is worse than both
        assert!(!queue.push(test_job(3, 1_000_000_000_000_000)));

        assert_eq!(queue.iter_jobs().count(), 2);
    }

    #[test]
    fn test_queue_size_zero_is_rejected() {
        let err = test_processor(0).err().expect("queue size 0 accepted");
        assert!(err.to_string().contains("Queue size"), "{err}");
    }

    #[tokio::test]
    async fn test_cleanup_task_runs_on_processor_proof_cache() {
        let mut processor = test_processor(10).unwrap();
        // Memory-only cache, so the test writes no proof files
        processor.proof_cache = Arc::new(ProofCache::new(10, None, Duration::from_secs(3600)));

        let mut expired = CachedProof::new(vec![1], vec![2], 1, Duration::ZERO);
        expired.generated_at = 0;
        let key = ProofCacheKey::new(B256::repeat_byte(1), B256::repeat_byte(2), false);
        processor.proof_cache.put(&key, expired).await;
        assert_eq!(processor.proof_cache.memory_len().await, 1);

        let cleanup = processor.start_proof_cache_cleanup(Duration::from_millis(10));
        for _ in 0..200 {
            if processor.proof_cache.memory_len().await == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cleanup.abort();

        assert_eq!(processor.proof_cache.memory_len().await, 0);
    }

    #[tokio::test]
    async fn test_program_download_from_http_gateway_url() {
        let cid = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
        let content_path = format!("/ipfs/{cid}");
        let addr = serve_http(move |request| {
            if request.path == content_path {
                (200, b"program bytes".to_vec())
            } else {
                (404, Vec::new())
            }
        })
        .await;

        // An http(s) URL of a gateway is fetched as it is, not handed to the
        // IPFS client as if it were a CID
        let elf = download_program_elf(&format!("http://{addr}/ipfs/{cid}"))
            .await
            .unwrap();
        assert_eq!(elf, b"program bytes");
    }

    #[test]
    fn test_save_elf_replaces_file_instead_of_overwriting_it() {
        let dir = tempfile::tempdir().unwrap();
        let image_id = B256::repeat_byte(0x42);
        let elf_path = dir.path().join(format!("{}.elf", hex::encode(image_id)));

        // An older file for the same image ID, also reachable under a second name
        std::fs::write(&elf_path, b"old contents").unwrap();
        let old_file = dir.path().join("old-file");
        std::fs::hard_link(&elf_path, &old_file).unwrap();

        save_elf_to_cache(dir.path(), &image_id, b"new contents").unwrap();

        // The ELF path points to a new, complete file; the old file was not
        // written to, so a reader of it never sees a partial ELF.
        assert_eq!(std::fs::read(&elf_path).unwrap(), b"new contents");
        assert_eq!(std::fs::read(&old_file).unwrap(), b"old contents");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
