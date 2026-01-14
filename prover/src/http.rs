//! Optimized HTTP client with connection pooling
//!
//! Provides a shared, reusable HTTP client with:

#![allow(dead_code)]
//! - Connection pooling (reuse TCP connections)
//! - Keep-alive connections
//! - Automatic retry with backoff
//! - Timeout configuration
//! - Compression support

use anyhow::Result;
use reqwest::{Client, ClientBuilder};
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, warn};

/// Global shared HTTP client
static SHARED_CLIENT: OnceLock<Client> = OnceLock::new();

/// HTTP client configuration
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Connection timeout
    pub connect_timeout_secs: u64,
    /// Request timeout
    pub request_timeout_secs: u64,
    /// Maximum idle connections per host
    pub pool_idle_per_host: usize,
    /// Idle connection timeout
    pub pool_idle_timeout_secs: u64,
    /// Enable gzip/brotli compression
    pub enable_compression: bool,
    /// Maximum retries for failed requests
    pub max_retries: u32,
    /// Base delay between retries (doubles each retry)
    pub retry_base_delay_ms: u64,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 10,
            request_timeout_secs: 300, // 5 min for large downloads
            pool_idle_per_host: 10,
            pool_idle_timeout_secs: 90,
            enable_compression: true,
            max_retries: 3,
            retry_base_delay_ms: 100,
        }
    }
}

/// Get the shared HTTP client (creates if needed)
pub fn shared_client() -> &'static Client {
    SHARED_CLIENT.get_or_init(|| {
        create_client(&HttpConfig::default()).expect("Failed to create HTTP client")
    })
}

/// Create a new HTTP client with configuration
pub fn create_client(config: &HttpConfig) -> Result<Client> {
    let mut builder = ClientBuilder::new()
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout_secs))
        .pool_max_idle_per_host(config.pool_idle_per_host)
        .tcp_keepalive(Duration::from_secs(60))
        .tcp_nodelay(true);

    if config.enable_compression {
        builder = builder.gzip(true).brotli(true);
    }

    let client = builder.build()?;
    Ok(client)
}

/// Fetch data from URL with retry
pub async fn fetch_with_retry(url: &str, config: &HttpConfig) -> Result<Vec<u8>> {
    let client = shared_client();
    let mut last_error = None;

    for attempt in 0..=config.max_retries {
        if attempt > 0 {
            let delay = Duration::from_millis(
                config.retry_base_delay_ms * 2u64.pow(attempt - 1)
            );
            debug!("Retry {} after {:?}", attempt, delay);
            tokio::time::sleep(delay).await;
        }

        match client.get(url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    let bytes = response.bytes().await?;
                    return Ok(bytes.to_vec());
                } else if response.status().is_server_error() {
                    // Retry on 5xx errors
                    last_error = Some(anyhow::anyhow!(
                        "Server error: {} {}",
                        response.status(),
                        response.status().canonical_reason().unwrap_or("")
                    ));
                    warn!("Server error, will retry: {}", response.status());
                } else {
                    // Don't retry on 4xx errors
                    anyhow::bail!(
                        "HTTP error: {} {}",
                        response.status(),
                        response.status().canonical_reason().unwrap_or("")
                    );
                }
            }
            Err(e) => {
                last_error = Some(e.into());
                warn!("Request failed, will retry: {:?}", last_error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error")))
}

/// Prefetch multiple URLs in parallel
pub async fn prefetch_parallel(urls: &[String]) -> Vec<Result<Vec<u8>>> {
    let futures: Vec<_> = urls.iter()
        .map(|url| {
            let url = url.clone();
            async move {
                fetch_with_retry(&url, &HttpConfig::default()).await
            }
        })
        .collect();

    futures::future::join_all(futures).await
}

/// HTTP client statistics
#[derive(Debug, Clone, Default)]
pub struct HttpStats {
    pub requests_made: u64,
    pub bytes_downloaded: u64,
    pub retries: u64,
    pub failures: u64,
}

impl std::fmt::Display for HttpStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HTTP: {} requests, {:.2} MB downloaded, {} retries, {} failures",
            self.requests_made,
            self.bytes_downloaded as f64 / 1024.0 / 1024.0,
            self.retries,
            self.failures
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HttpConfig::default();
        assert_eq!(config.max_retries, 3);
        assert!(config.enable_compression);
    }

    #[test]
    fn test_shared_client() {
        let client1 = shared_client();
        let client2 = shared_client();
        // Should return the same instance
        assert!(std::ptr::eq(client1, client2));
    }
}
