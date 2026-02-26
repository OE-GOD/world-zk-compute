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
use crate::xgboost_decomp::XGBoostDecompStrategy;
use alloy::network::{Ethereum, EthereumWallet};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::fillers::{
    BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, WalletFiller,
};
use alloy::providers::Provider;
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
    /// Create a new optimized processor
    #[allow(dead_code)]
    pub fn new(
        max_concurrent: usize,
        min_tip_wei: U256,
        cache_size_mb: usize,
        proving_mode: crate::bonsai::ProvingMode,
        use_snark: bool,
    ) -> anyhow::Result<Self> {
        Self::with_gpu_config(
            max_concurrent,
            min_tip_wei,
            cache_size_mb,
            proving_mode,
            use_snark,
            0,
            0,
        )
    }

    /// Create a new optimized processor with explicit GPU/CPU concurrency config.
    ///
    /// - `max_gpu_concurrent`: GPU concurrency slots (0 = auto-detect from GPU count)
    /// - `max_cpu_concurrent`: CPU concurrency slots (0 = auto-detect from CPU count - 1)
    pub fn with_gpu_config(
        max_concurrent: usize,
        min_tip_wei: U256,
        cache_size_mb: usize,
        proving_mode: crate::bonsai::ProvingMode,
        use_snark: bool,
        max_gpu_concurrent: usize,
        max_cpu_concurrent: usize,
    ) -> anyhow::Result<Self> {
        // Initialize job queue
        let job_queue = Arc::new(RwLock::new(JobQueue::new(1000, min_tip_wei)));

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
        let concurrency = Arc::new(ConcurrencyManager::new(
            gpu_manager.clone(),
            max_cpu_concurrent,
        ));

        // Initialize program cache
        let cache_dir = std::path::PathBuf::from("./cache/programs");
        let program_cache = Arc::new(ProgramCache::new(cache_dir, cache_size_mb)?);

        // Initialize IPFS client
        let ipfs_client = Arc::new(IpfsClient::new(IpfsConfig::default()));

        // Initialize multi-VM router
        let multi_vm = Arc::new(MultiVmProver::new(proving_mode.clone()));

        // Initialize fast prover with multi-VM router and GPU manager
        let fast_prover = Arc::new(FastProver::with_gpu_manager(
            FastProveConfig::default(),
            proving_mode,
            use_snark,
            multi_vm.clone(),
            gpu_manager,
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

        // Batch-fetch inputUrls from events for all pending requests in a single RPC call
        // (inputUrl is no longer stored on-chain, only emitted in the ExecutionRequested event)
        let input_urls =
            batch_fetch_input_urls(provider, config.engine_address, &request_ids).await;

        // Fetch details and add to queue IN PARALLEL
        let fetch_futures: Vec<_> = request_ids
            .iter()
            .map(|&request_id| {
                let provider = provider.clone();
                let config = config.clone();
                let program_cache = self.program_cache.clone();
                let job_queue = self.job_queue.clone();
                let meta = input_urls.get(&request_id).cloned().unwrap_or_default();
                async move {
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
        let processed = self
            .process_queue_parallel(provider, config, nonce_manager)
            .await?;

        let elapsed = timer.elapsed();
        info!("Processed {} requests in {:?}", processed, elapsed);

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

        info!(
            "Processing {} jobs in parallel (pending nonces: {})",
            jobs.len(),
            nonce_manager.pending()
        );

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
                Ok(Err(e)) => warn!("Job failed: {}", e),
                Err(e) => error!("Task panicked: {}", e),
            }
        }

        Ok(processed)
    }
}

/// Input metadata extracted from ExecutionRequested events
#[derive(Clone, Debug, Default)]
struct EventInputMeta {
    /// Input URL from the event
    url: String,
    /// Input type (0 = Public, 1 = Private)
    input_type: u8,
}

/// Batch-fetch inputUrls and inputTypes from ExecutionRequested events for multiple request IDs.
///
/// Performs a single `get_logs` RPC call with all request IDs, returning a map
/// of request_id → EventInputMeta. Much more efficient than one query per request.
async fn batch_fetch_input_urls(
    provider: &Arc<SigningProvider>,
    engine_address: Address,
    request_ids: &[U256],
) -> std::collections::HashMap<U256, EventInputMeta> {
    use std::collections::HashMap;

    let mut result: HashMap<U256, EventInputMeta> = HashMap::new();

    // Build topic1 filter with all request IDs
    let topic_values: Vec<B256> = request_ids
        .iter()
        .map(|id| B256::from(id.to_be_bytes::<32>()))
        .collect();

    let filter = Filter::new()
        .address(engine_address)
        .event_signature(IExecutionEngine::ExecutionRequested::SIGNATURE_HASH)
        .topic1(topic_values);

    match provider.get_logs(&filter).await {
        Ok(logs) => {
            for log in logs {
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
        }
        Err(e) => {
            warn!("Failed to batch-fetch inputUrls from events: {}", e);
        }
    }

    result
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

        // Claim
        info!("[Job {}] Claiming (cached path)...", job.request_id);
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
                        info!("[Job {}] Claiming (decomposed path)...", job.request_id);
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
    if early_claim_result.is_none() {
        info!("[Job {}] Claiming...", job.request_id);

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
    } else {
        debug!(
            "[Job {}] Already claimed (private input early claim)",
            job.request_id
        );
    }

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

    // Step 8: Cache the proof for future use
    let cached = CachedProof::new(
        prove_result.proof.clone(),
        prove_result.journal.clone(),
        prove_result.cycles,
        prove_timer.elapsed(),
    );
    proof_cache.put(&cache_key, cached).await;

    // Step 9: Submit proof (with retry logic)
    timeline.enter_stage(ProveStage::Submission);
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
                warn!("Failed to fetch program from registry: {:?}", e);
                anyhow::bail!("Program {} not found in registry: {:?}", image_id, e);
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
    #[allow(dead_code)]
    pub attempts: u32,
}

/// Send a claim transaction with retry logic
async fn send_claim_with_retry(
    engine: &IExecutionEngine::IExecutionEngineInstance<Arc<SigningProvider>>,
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
        debug!(
            "[Job {}] Claim attempt {} with nonce {}",
            job_id, attempts, nonce
        );

        let result = engine.claimExecution(request_id).nonce(nonce).send().await;

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
            (delay.as_secs_f64() * config.backoff_multiplier).min(config.max_delay.as_secs_f64()),
        );
    }
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
    let mut attempts = 0u32;
    let mut delay = config.initial_delay;

    loop {
        attempts += 1;
        let nonce = nonce_manager.next_nonce();
        debug!(
            "[Job {}] Submit attempt {} with nonce {}",
            job_id, attempts, nonce
        );

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
            debug!(
                "Failed to estimate submit gas (expected before claim): {}",
                e
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
