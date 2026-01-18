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
use crate::contracts::IExecutionEngine;
use crate::fast_prove::{FastProveConfig, FastProver, ProvingStrategy};
use crate::ipfs::{IpfsClient, IpfsConfig};
use crate::metrics::{metrics, Timer};
use crate::nonce::NonceManager;
use crate::queue::{JobQueue, QueuedJob};
use alloy::primitives::{B256, U256};
use alloy::providers::fillers::{BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, WalletFiller};
use alloy::providers::Provider;
use alloy::network::{EthereumWallet, Ethereum};
use alloy::transports::http::{Client, Http};
use futures::future::join_all;
use std::sync::Arc;
use std::time::Instant;
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
            // Fetch from registry and cache
            let elf = fetch_program_from_registry(&job.image_id).await?;
            program_cache.put(&job.image_id, &elf);
            info!("[Job {}] ELF cached for future use", job.request_id);
            elf
        }
    };
    metrics().record_fetch_time(fetch_timer.elapsed());

    // Step 2: Fetch input (with IPFS multi-gateway support)
    info!("[Job {}] Fetching input...", job.request_id);
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
    info!("[Job {}] Input verified ({} bytes)", job.request_id, input.len());

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

    // Step 5: Claim the request (with managed nonce for parallel safety)
    info!("[Job {}] Claiming...", job.request_id);
    let claim_nonce = nonce_manager.next_nonce();
    debug!("[Job {}] Using nonce {} for claim", job.request_id, claim_nonce);

    let claim_result = engine
        .claimExecution(request_id)
        .nonce(claim_nonce)
        .send()
        .await;

    let claim_tx = match claim_result {
        Ok(tx) => tx,
        Err(e) => {
            let error_str = format!("{:?}", e);
            nonce_manager.handle_nonce_error(&error_str, claim_nonce).await?;
            return Err(e.into());
        }
    };

    let claim_receipt = match claim_tx.get_receipt().await {
        Ok(receipt) => {
            nonce_manager.transaction_completed();
            receipt
        }
        Err(e) => {
            let error_str = format!("{:?}", e);
            nonce_manager.handle_nonce_error(&error_str, claim_nonce).await?;
            return Err(e.into());
        }
    };
    info!("[Job {}] Claimed in tx: {:?}", job.request_id, claim_receipt.transaction_hash);

    // Step 6: Generate proof using fast prover
    info!("[Job {}] Generating proof...", job.request_id);
    let prove_result = fast_prover.prove_fast(&elf, &input).await?;

    info!(
        "[Job {}] Proof generated in {:?} ({} bytes)",
        job.request_id, prove_result.proof_time, prove_result.proof.len()
    );

    // Step 7: Submit proof (with managed nonce for parallel safety)
    info!("[Job {}] Submitting proof...", job.request_id);
    let submit_timer = Timer::start();

    let submit_nonce = nonce_manager.next_nonce();
    debug!("[Job {}] Using nonce {} for submit", job.request_id, submit_nonce);

    // Use actual journal from proving result
    let submit_result = engine
        .submitProof(request_id, prove_result.proof.clone().into(), prove_result.journal.clone().into())
        .nonce(submit_nonce)
        .send()
        .await;

    let submit_tx = match submit_result {
        Ok(tx) => tx,
        Err(e) => {
            let error_str = format!("{:?}", e);
            nonce_manager.handle_nonce_error(&error_str, submit_nonce).await?;
            return Err(e.into());
        }
    };

    let submit_receipt = match submit_tx.get_receipt().await {
        Ok(receipt) => {
            nonce_manager.transaction_completed();
            receipt
        }
        Err(e) => {
            let error_str = format!("{:?}", e);
            nonce_manager.handle_nonce_error(&error_str, submit_nonce).await?;
            return Err(e.into());
        }
    };

    metrics().record_submit_time(submit_timer.elapsed());

    info!(
        "[Job {}] Proof submitted in tx: {:?}",
        job.request_id, submit_receipt.transaction_hash
    );

    Ok((prove_result.cycles, input.len() as u64))
}

/// Fetch program ELF from registry
async fn fetch_program_from_registry(
    image_id: &B256,
) -> anyhow::Result<Vec<u8>> {
    // First try local cache directory
    let programs_dir = std::path::PathBuf::from("./programs");
    let elf_path = programs_dir.join(format!("{}.elf", hex::encode(image_id)));

    if elf_path.exists() {
        debug!("Loading ELF from local file: {:?}", elf_path);
        let elf = std::fs::read(&elf_path)?;
        return Ok(elf);
    }

    // TODO: Query ProgramRegistry contract for URL
    // let registry = IProgramRegistry::new(registry_address, provider.clone());
    // let program = registry.getProgram(*image_id).call().await?;
    // let url = program.url;
    // let elf = reqwest::get(&url).await?.bytes().await?.to_vec();

    anyhow::bail!(
        "Program ELF not found for image ID {}. Place in ./programs/ directory.",
        image_id
    )
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
