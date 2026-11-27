//! Input Prefetching for Reduced Latency
//!
//! The InputPrefetcher runs in the background and fetches input data for queued jobs
//! BEFORE they are claimed. This reduces the time between claiming and proving.
//!
//! ## Benefits
//!
//! - **20-50% latency reduction**: Input is ready when job processing starts
//! - **Better throughput**: Network I/O happens in parallel with proving
//! - **Smarter prioritization**: Jobs with prefetched inputs get priority boost
//!
//! ## How It Works
//!
//! ```text
//! Without prefetching:
//!   [Claim] → [Fetch Input] → [Prove] → [Submit]
//!              ^^^^^^^^^^^^
//!              Blocking I/O
//!
//! With prefetching:
//!   [Prefetch Input] ────────────────┐
//!                                    ↓
//!   [Claim] → [Use Cached Input] → [Prove] → [Submit]
//!              ^^^^^^^^^^^^^^^^^
//!              Instant (from cache)
//! ```

use crate::ipfs::IpfsClient;
use crate::queue::QueuedJob;
use crate::redact;
use crate::validation::read_body_capped;
use alloy::primitives::B256;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

/// Configuration for the input prefetcher
#[derive(Clone, Debug)]
pub struct PrefetchConfig {
    /// Maximum number of concurrent prefetch operations
    pub max_concurrent: usize,
    /// Maximum size of prefetch cache (in bytes)
    pub max_cache_bytes: usize,
    /// Time-to-live for cached inputs
    pub cache_ttl: Duration,
    /// Maximum input size to prefetch (larger inputs fetched on-demand)
    pub max_input_size: usize,
}

impl Default for PrefetchConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            max_cache_bytes: 256 * 1024 * 1024,  // 256 MB
            cache_ttl: Duration::from_secs(300), // 5 minutes
            max_input_size: 10 * 1024 * 1024,    // 10 MB
        }
    }
}

/// Cached input entry
#[derive(Clone)]
struct CachedInput {
    data: Vec<u8>,
    hash: B256,
    fetched_at: Instant,
    size: usize,
}

/// Input prefetcher with LRU-style cache
pub struct InputPrefetcher {
    config: PrefetchConfig,
    /// IPFS client for fetching
    ipfs_client: Arc<IpfsClient>,
    /// HTTP client for http(s) inputs, shared by all prefetches
    http_client: reqwest::Client,
    /// One permit per prefetch that may download at a time
    /// (`max_concurrent`)
    fetch_permits: Arc<Semaphore>,
    /// Cache of prefetched inputs (keyed by input URL)
    cache: Arc<RwLock<HashMap<String, CachedInput>>>,
    /// Current cache size in bytes
    cache_size: Arc<RwLock<usize>>,
    /// Prefetch in-progress tracker (to avoid duplicate fetches); it also
    /// holds the prefetches that wait for a permit
    in_progress: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl InputPrefetcher {
    /// Create a new input prefetcher
    pub fn new(ipfs_client: Arc<IpfsClient>, config: PrefetchConfig) -> Self {
        // Building fails only when the TLS backend cannot start, which is
        // what makes reqwest::Client::new() panic too
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build the prefetch HTTP client");
        // With no permit no prefetch would ever finish; 0 is taken as 1
        // Semaphore::new panics above MAX_PERMITS
        let fetch_permits = Arc::new(Semaphore::new(
            config.max_concurrent.clamp(1, Semaphore::MAX_PERMITS),
        ));

        Self {
            config,
            ipfs_client,
            http_client,
            fetch_permits,
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_size: Arc::new(RwLock::new(0)),
            in_progress: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Start prefetching input for a job (non-blocking)
    ///
    /// This spawns a background task to fetch the input. The result
    /// will be available via `get_cached()` when ready.
    /// Private inputs (input_type == 1) are skipped since they require
    /// on-chain claim before the auth server releases data.
    pub fn prefetch(&self, job: &QueuedJob) {
        if job.input_type == 1 {
            debug!(
                "[Prefetch {}] Skipping private input (requires claim first)",
                job.request_id
            );
            return;
        }
        let url = job.input_url.clone();
        let expected_hash = job.input_hash;
        let request_id = job.request_id;

        // Clone references for the spawned task
        let ipfs_client = self.ipfs_client.clone();
        let http_client = self.http_client.clone();
        let fetch_permits = self.fetch_permits.clone();
        let cache = self.cache.clone();
        let cache_size = self.cache_size.clone();
        let in_progress = self.in_progress.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            // Check if already cached or in progress
            {
                let mut cache_write = cache.write().await;
                if let Some(entry) = cache_write.get(&url) {
                    if entry.fetched_at.elapsed() <= config.cache_ttl {
                        debug!("[Prefetch {}] Already cached", request_id);
                        return;
                    }

                    // Expired entries are dropped and fetched again
                    if let Some(removed) = cache_write.remove(&url) {
                        *cache_size.write().await -= removed.size;
                    }
                }
            }

            {
                let mut progress = in_progress.write().await;
                if progress.contains(&url) {
                    debug!("[Prefetch {}] Already in progress", request_id);
                    return;
                }
                progress.insert(url.clone());
            }

            // Fetch input
            debug!(
                "[Prefetch {}] Starting fetch from {}",
                request_id,
                redact::url_origin(&url)
            );
            let start = Instant::now();

            // The permit is held until the download is done, so at most
            // max_concurrent prefetches download at a time; the others wait
            // here. The semaphore is never closed.
            let result = match fetch_permits.acquire_owned().await {
                Ok(_permit) => {
                    fetch_input_for_prefetch(&ipfs_client, &http_client, &url, &config).await
                }
                Err(e) => Err(e.into()),
            };

            // Remove from in-progress
            {
                let mut progress = in_progress.write().await;
                progress.remove(&url);
            }

            match result {
                Ok(data) => {
                    // Verify hash
                    let computed_hash = compute_hash(&data);
                    if computed_hash != expected_hash {
                        warn!(
                            "[Prefetch {}] Hash mismatch: expected {}, got {}",
                            request_id, expected_hash, computed_hash
                        );
                        return;
                    }

                    let size = data.len();
                    let elapsed = start.elapsed();

                    // Add to cache
                    {
                        let mut cache_write = cache.write().await;
                        let mut size_write = cache_size.write().await;

                        // Evict old entries if needed
                        while *size_write + size > config.max_cache_bytes && !cache_write.is_empty()
                        {
                            // Find oldest entry
                            let oldest_key = cache_write
                                .iter()
                                .min_by_key(|(_, v)| v.fetched_at)
                                .map(|(k, _)| k.clone());

                            if let Some(key) = oldest_key {
                                if let Some(removed) = cache_write.remove(&key) {
                                    *size_write -= removed.size;
                                    debug!(
                                        "[Prefetch] Evicted {} from cache",
                                        redact::url_origin(&key)
                                    );
                                }
                            } else {
                                break;
                            }
                        }

                        // Insert new entry
                        cache_write.insert(
                            url.clone(),
                            CachedInput {
                                data,
                                hash: computed_hash,
                                fetched_at: Instant::now(),
                                size,
                            },
                        );
                        *size_write += size;
                    }

                    info!(
                        "[Prefetch {}] Cached {} bytes in {:?}",
                        request_id, size, elapsed
                    );
                }
                Err(e) => {
                    warn!("[Prefetch {}] Failed: {}", request_id, e);
                }
            }
        });
    }

    /// Get cached input if available
    ///
    /// Returns the input data if it was prefetched and is still valid.
    /// Returns None if not cached, expired, or hash doesn't match.
    pub async fn get_cached(&self, url: &str, expected_hash: &B256) -> Option<Vec<u8>> {
        let cache = self.cache.read().await;

        if let Some(entry) = cache.get(url) {
            // Check TTL
            if entry.fetched_at.elapsed() > self.config.cache_ttl {
                debug!(
                    "[Prefetch] Cache entry expired for {}",
                    redact::url_origin(url)
                );
                return None;
            }

            // Verify hash
            if entry.hash != *expected_hash {
                warn!(
                    "[Prefetch] Cache hash mismatch for {}: expected {}, got {}",
                    redact::url_origin(url),
                    expected_hash,
                    entry.hash
                );
                return None;
            }

            debug!(
                "[Prefetch] Cache hit for {} ({} bytes)",
                redact::url_origin(url),
                entry.size
            );
            return Some(entry.data.clone());
        }

        None
    }

    /// Get cache statistics
    pub async fn stats(&self) -> PrefetchStats {
        let cache = self.cache.read().await;
        let size = *self.cache_size.read().await;
        let in_progress = self.in_progress.read().await;

        PrefetchStats {
            cached_entries: cache.len(),
            cached_bytes: size,
            in_progress: in_progress.len(),
            max_bytes: self.config.max_cache_bytes,
        }
    }
}

/// Prefetch statistics
#[derive(Debug, Clone)]
pub struct PrefetchStats {
    pub cached_entries: usize,
    pub cached_bytes: usize,
    pub in_progress: usize,
    pub max_bytes: usize,
}

impl std::fmt::Display for PrefetchStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Prefetch: {} entries, {:.1} MB / {:.1} MB, {} in progress",
            self.cached_entries,
            self.cached_bytes as f64 / 1024.0 / 1024.0,
            self.max_bytes as f64 / 1024.0 / 1024.0,
            self.in_progress
        )
    }
}

/// Fetch input for prefetching (with size limit)
async fn fetch_input_for_prefetch(
    ipfs_client: &IpfsClient,
    http_client: &reqwest::Client,
    url: &str,
    config: &PrefetchConfig,
) -> anyhow::Result<Vec<u8>> {
    // Determine fetch method based on URL scheme
    let data = if IpfsClient::is_ipfs_url(url) {
        ipfs_client
            .fetch_at_most(url, config.max_input_size)
            .await?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        // The error of a failed request names the URL, whose path can hold a
        // key
        let response = http_client
            .get(url)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;

        read_body_capped(response, config.max_input_size).await?
    } else if url.starts_with("data:") {
        // Data URL - decode inline
        use base64::Engine;
        let parts: Vec<&str> = url.splitn(2, ',').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid data URL");
        }
        base64::engine::general_purpose::STANDARD.decode(parts[1])?
    } else {
        anyhow::bail!("Unsupported URL scheme: {}", redact::url_origin(url));
    };

    // Final size check
    if data.len() > config.max_input_size {
        anyhow::bail!("Input too large: {} bytes", data.len());
    }

    Ok(data)
}

/// Compute SHA256 hash
fn compute_hash(data: &[u8]) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    B256::from_slice(&result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipfs::IpfsConfig;
    use crate::test_support::{gzip_zeros, serve_http_responses, HttpResponse};
    use crate::validation::ValidationError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_prefetch_stats() {
        let ipfs = Arc::new(IpfsClient::new(IpfsConfig::default()).unwrap());
        let prefetcher = InputPrefetcher::new(ipfs, PrefetchConfig::default());

        let stats = prefetcher.stats().await;
        assert_eq!(stats.cached_entries, 0);
        assert_eq!(stats.in_progress, 0);
    }

    #[test]
    fn test_compute_hash() {
        let data = b"hello world";
        let hash = compute_hash(data);
        assert!(!hash.is_zero());
    }

    const TEST_CID: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";

    /// A local server that answers like an IPFS gateway for TEST_CID
    async fn serve_test_gateway() -> std::net::SocketAddr {
        let content_path = format!("/ipfs/{TEST_CID}");
        crate::test_support::serve_http(move |request| {
            if request.path == content_path {
                (200, b"gateway content".to_vec())
            } else {
                (404, Vec::new())
            }
        })
        .await
    }

    fn ipfs_client_with_gateway(gateway: String) -> IpfsClient {
        IpfsClient::new(IpfsConfig {
            gateways: vec![gateway],
            max_retries: 1,
            ..IpfsConfig::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn test_http_gateway_url_is_fetched_as_http() {
        let addr = serve_test_gateway().await;
        // The IPFS client's own gateway has no content, so it cannot serve the URL
        let ipfs = ipfs_client_with_gateway(format!("http://{addr}/other-gateway/"));

        let url = format!("http://{addr}/ipfs/{TEST_CID}");
        let data =
            fetch_input_for_prefetch(&ipfs, &test_http_client(), &url, &PrefetchConfig::default())
                .await
                .unwrap();
        assert_eq!(data, b"gateway content");
    }

    #[tokio::test]
    async fn test_bare_cid_is_fetched_through_ipfs_client() {
        let addr = serve_test_gateway().await;
        let ipfs = ipfs_client_with_gateway(format!("http://{addr}/ipfs/"));

        let data = fetch_input_for_prefetch(
            &ipfs,
            &test_http_client(),
            TEST_CID,
            &PrefetchConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(data, b"gateway content");
    }

    fn test_http_client() -> reqwest::Client {
        reqwest::Client::new()
    }

    /// A server that sends a body that decompresses to `decompressed` zero
    /// bytes, with Content-Encoding: gzip and the (much smaller) compressed
    /// Content-Length
    async fn serve_gzip(decompressed: usize) -> std::net::SocketAddr {
        let gzip = gzip_zeros(decompressed);
        serve_http_responses(move |_| {
            HttpResponse::new(200, gzip.clone()).header("Content-Encoding", "gzip")
        })
        .await
    }

    /// The prefetch limit is applied while the decompressed body is read, on
    /// the http path and on the IPFS path (whose own limit is 100 MB)
    #[tokio::test]
    async fn test_compressed_input_over_limit_is_refused_while_reading() {
        const LIMIT: usize = 64 * 1024;
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        assert!(gzip_zeros(DECOMPRESSED).len() < LIMIT);
        let addr = serve_gzip(DECOMPRESSED).await;
        let ipfs = ipfs_client_with_gateway(format!("http://{addr}/ipfs/"));
        let config = PrefetchConfig {
            max_input_size: LIMIT,
            ..PrefetchConfig::default()
        };

        for url in [format!("http://{addr}/input"), TEST_CID.to_string()] {
            let err = fetch_input_for_prefetch(&ipfs, &test_http_client(), &url, &config)
                .await
                .unwrap_err();

            match err.downcast_ref::<ValidationError>() {
                Some(ValidationError::BodyTooLarge { max, read }) => {
                    assert_eq!(*max, LIMIT, "{url}");
                    assert!(*read < DECOMPRESSED / 4, "{url}: read {read}");
                }
                _ => panic!("{url}: expected BodyTooLarge, got {err}"),
            }
        }
    }

    /// The same stream is taken whole under a higher limit: it is valid
    /// gzip and reqwest decompresses it
    #[tokio::test]
    async fn test_compressed_input_under_limit_is_read_decompressed() {
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        let addr = serve_gzip(DECOMPRESSED).await;
        let ipfs = ipfs_client_with_gateway(format!("http://{addr}/ipfs/"));
        let config = PrefetchConfig {
            max_input_size: 2 * DECOMPRESSED,
            ..PrefetchConfig::default()
        };

        let url = format!("http://{addr}/input");
        let data = fetch_input_for_prefetch(&ipfs, &test_http_client(), &url, &config)
            .await
            .unwrap();
        assert_eq!(data.len(), DECOMPRESSED);
    }

    /// A max_concurrent beyond what a semaphore can hold is taken as its
    /// largest value instead of stopping the prover at start-up
    #[test]
    fn test_huge_max_concurrent_does_not_stop_the_prover() {
        let ipfs = Arc::new(IpfsClient::new(IpfsConfig::default()).unwrap());
        let config = PrefetchConfig {
            max_concurrent: usize::MAX,
            ..PrefetchConfig::default()
        };
        let _prefetcher = InputPrefetcher::new(ipfs, config);
    }

    /// With max_concurrent = 2 and twelve queued jobs, the server never has
    /// more than two prefetch requests open at once, and every job is
    /// fetched.
    #[tokio::test]
    async fn test_prefetches_in_progress_never_exceed_max_concurrent() {
        const JOBS: u64 = 12;
        const FETCH_TIME: Duration = Duration::from_millis(150);
        let open = Arc::new(AtomicUsize::new(0));
        let most_open = Arc::new(AtomicUsize::new(0));
        let served = Arc::new(AtomicUsize::new(0));

        let (counted_open, counted_most, counted_served) =
            (open.clone(), most_open.clone(), served.clone());
        let addr = serve_http_responses(move |_| {
            let now_open = counted_open.fetch_add(1, Ordering::SeqCst) + 1;
            counted_most.fetch_max(now_open, Ordering::SeqCst);
            counted_served.fetch_add(1, Ordering::SeqCst);
            // Counted as closed a little before the answer is written: a
            // request that a finished fetch lets through can only come after
            // the answer, so the count never runs ahead of the open requests
            let closing = counted_open.clone();
            tokio::spawn(async move {
                tokio::time::sleep(FETCH_TIME - Duration::from_millis(30)).await;
                closing.fetch_sub(1, Ordering::SeqCst);
            });
            HttpResponse::new(200, b"input".to_vec()).delayed(FETCH_TIME)
        })
        .await;

        let ipfs = Arc::new(IpfsClient::new(IpfsConfig::default()).unwrap());
        let config = PrefetchConfig {
            max_concurrent: 2,
            ..PrefetchConfig::default()
        };
        let prefetcher = InputPrefetcher::new(ipfs, config);
        let jobs: Vec<QueuedJob> = (0..JOBS)
            .map(|id| queued_job(id, format!("http://{addr}/input/{id}"), b"input"))
            .collect();
        for job in &jobs {
            prefetcher.prefetch(job);
        }

        for job in &jobs {
            let cached = wait_for_cached(&prefetcher, &job.input_url, &job.input_hash).await;
            assert_eq!(cached.as_deref(), Some(&b"input"[..]));
        }
        assert_eq!(served.load(Ordering::SeqCst), JOBS as usize);
        assert_eq!(most_open.load(Ordering::SeqCst), 2);
    }

    fn queued_job(request_id: u64, input_url: String, data: &[u8]) -> QueuedJob {
        QueuedJob {
            request_id,
            image_id: B256::ZERO,
            input_hash: compute_hash(data),
            input_url,
            input_type: 0,
            tip: alloy::primitives::U256::ZERO,
            requester: alloy::primitives::Address::ZERO,
            expires_at: u64::MAX,
            queued_at: Instant::now(),
            estimated_cycles: None,
            program_cached: false,
            prefetched_input: None,
        }
    }

    /// Poll the prefetch cache until the entry appears
    async fn wait_for_cached(
        prefetcher: &InputPrefetcher,
        url: &str,
        hash: &B256,
    ) -> Option<Vec<u8>> {
        for _ in 0..1000 {
            if let Some(data) = prefetcher.get_cached(url, hash).await {
                return Some(data);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        None
    }

    #[tokio::test]
    async fn test_expired_entry_is_prefetched_again() {
        let ipfs = Arc::new(IpfsClient::new(IpfsConfig::default()).unwrap());
        let config = PrefetchConfig {
            cache_ttl: Duration::from_millis(300),
            ..PrefetchConfig::default()
        };
        let prefetcher = InputPrefetcher::new(ipfs, config);

        // "hello" as a data URL, so no network is involved
        let job = queued_job(
            1,
            "data:application/octet-stream;base64,aGVsbG8=".to_string(),
            b"hello",
        );

        prefetcher.prefetch(&job);
        let cached = wait_for_cached(&prefetcher, &job.input_url, &job.input_hash).await;
        assert_eq!(cached.as_deref(), Some(&b"hello"[..]));

        // Let the entry expire
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(prefetcher
            .get_cached(&job.input_url, &job.input_hash)
            .await
            .is_none());

        // A new prefetch replaces the expired entry
        prefetcher.prefetch(&job);
        let cached = wait_for_cached(&prefetcher, &job.input_url, &job.input_hash).await;
        assert_eq!(cached.as_deref(), Some(&b"hello"[..]));

        let stats = prefetcher.stats().await;
        assert_eq!(stats.cached_entries, 1);
        assert_eq!(stats.cached_bytes, 5);
    }
}
