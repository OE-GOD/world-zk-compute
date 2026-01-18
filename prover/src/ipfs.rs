//! IPFS Integration for Decentralized Input Storage
//!
//! Supports fetching inputs from IPFS via multiple gateways for resilience.
//!
//! ## Usage
//!
//! ```rust
//! let client = IpfsClient::new(IpfsConfig::default());
//! let data = client.fetch("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG").await?;
//! ```
//!
//! ## Gateway Fallback
//!
//! The client tries multiple gateways in order:
//! 1. Cloudflare IPFS (fastest, most reliable)
//! 2. ipfs.io (official)
//! 3. dweb.link (fallback)
//! 4. Custom gateway (if configured)

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Default IPFS gateways (ordered by reliability)
pub const DEFAULT_GATEWAYS: &[&str] = &[
    "https://cloudflare-ipfs.com/ipfs/",
    "https://ipfs.io/ipfs/",
    "https://dweb.link/ipfs/",
    "https://gateway.pinata.cloud/ipfs/",
];

/// IPFS client configuration
#[derive(Clone, Debug)]
pub struct IpfsConfig {
    /// Gateway URLs to try (in order)
    pub gateways: Vec<String>,
    /// Timeout per gateway attempt
    pub timeout: Duration,
    /// Maximum retries per gateway
    pub max_retries: usize,
    /// Maximum content size (bytes)
    pub max_size: usize,
}

impl Default for IpfsConfig {
    fn default() -> Self {
        Self {
            gateways: DEFAULT_GATEWAYS.iter().map(|s| s.to_string()).collect(),
            timeout: Duration::from_secs(30),
            max_retries: 2,
            max_size: 100 * 1024 * 1024, // 100 MB
        }
    }
}

impl IpfsConfig {
    /// Create config with custom gateway
    pub fn with_gateway(mut self, gateway: &str) -> Self {
        // Add custom gateway as first option
        self.gateways.insert(0, gateway.to_string());
        self
    }

    /// Create config for local IPFS node
    pub fn local_node() -> Self {
        Self {
            gateways: vec!["http://localhost:8080/ipfs/".to_string()],
            timeout: Duration::from_secs(60),
            max_retries: 1,
            max_size: 500 * 1024 * 1024, // 500 MB for local
        }
    }
}

/// IPFS client for fetching content
pub struct IpfsClient {
    http: Client,
    config: IpfsConfig,
}

impl IpfsClient {
    /// Create a new IPFS client
    pub fn new(config: IpfsConfig) -> Self {
        let http = Client::builder()
            .timeout(config.timeout)
            .pool_max_idle_per_host(4)
            .build()
            .expect("Failed to create HTTP client");

        Self { http, config }
    }

    /// Fetch content by CID
    pub async fn fetch(&self, cid: &str) -> Result<Vec<u8>> {
        // Normalize CID (remove ipfs:// prefix if present)
        let cid = Self::normalize_cid(cid);

        info!("Fetching IPFS content: {}", cid);

        let mut last_error = None;

        for gateway in &self.config.gateways {
            match self.fetch_from_gateway(gateway, &cid).await {
                Ok(data) => {
                    info!("Successfully fetched {} bytes from {}", data.len(), gateway);
                    return Ok(data);
                }
                Err(e) => {
                    warn!("Gateway {} failed: {}", gateway, e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("No gateways configured")))
    }

    /// Fetch from a specific gateway with retries
    async fn fetch_from_gateway(&self, gateway: &str, cid: &str) -> Result<Vec<u8>> {
        let url = format!("{}{}", gateway, cid);
        debug!("Trying gateway: {}", url);

        let mut last_error = None;

        for attempt in 0..self.config.max_retries {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            }

            match self.do_fetch(&url).await {
                Ok(data) => return Ok(data),
                Err(e) => {
                    debug!("Attempt {} failed: {}", attempt + 1, e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Unknown error")))
    }

    /// Perform the actual HTTP fetch
    async fn do_fetch(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.http
            .get(url)
            .header("Accept", "application/octet-stream")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("HTTP {}: {}", response.status(), url));
        }

        // Check content length
        if let Some(len) = response.content_length() {
            if len as usize > self.config.max_size {
                return Err(anyhow!(
                    "Content too large: {} bytes (max: {})",
                    len,
                    self.config.max_size
                ));
            }
        }

        let bytes = response.bytes().await?;

        if bytes.len() > self.config.max_size {
            return Err(anyhow!(
                "Downloaded content too large: {} bytes",
                bytes.len()
            ));
        }

        Ok(bytes.to_vec())
    }

    /// Normalize CID by removing common prefixes
    fn normalize_cid(cid: &str) -> String {
        cid.trim()
            .trim_start_matches("ipfs://")
            .trim_start_matches("/ipfs/")
            .to_string()
    }

    /// Check if a string looks like an IPFS CID
    pub fn is_ipfs_url(url: &str) -> bool {
        url.starts_with("ipfs://")
            || url.starts_with("/ipfs/")
            || url.starts_with("Qm") // CIDv0
            || url.starts_with("bafy") // CIDv1
    }

    /// Fetch JSON content and deserialize
    pub async fn fetch_json<T: serde::de::DeserializeOwned>(&self, cid: &str) -> Result<T> {
        let data = self.fetch(cid).await?;
        let value: T = serde_json::from_slice(&data)?;
        Ok(value)
    }
}

/// Helper to resolve input URLs (supports both HTTP and IPFS)
pub async fn resolve_input_url(url: &str, ipfs_client: Option<&IpfsClient>) -> Result<Vec<u8>> {
    if IpfsClient::is_ipfs_url(url) {
        match ipfs_client {
            Some(client) => client.fetch(url).await,
            None => {
                // Use default client for one-off fetch
                let client = IpfsClient::new(IpfsConfig::default());
                client.fetch(url).await
            }
        }
    } else {
        // Regular HTTP fetch
        let response = reqwest::get(url).await?;
        if !response.status().is_success() {
            return Err(anyhow!("HTTP error: {}", response.status()));
        }
        Ok(response.bytes().await?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_cid() {
        assert_eq!(
            IpfsClient::normalize_cid("ipfs://QmTest123"),
            "QmTest123"
        );
        assert_eq!(
            IpfsClient::normalize_cid("/ipfs/QmTest123"),
            "QmTest123"
        );
        assert_eq!(
            IpfsClient::normalize_cid("QmTest123"),
            "QmTest123"
        );
    }

    #[test]
    fn test_is_ipfs_url() {
        assert!(IpfsClient::is_ipfs_url("ipfs://QmTest"));
        assert!(IpfsClient::is_ipfs_url("/ipfs/QmTest"));
        assert!(IpfsClient::is_ipfs_url("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"));
        assert!(IpfsClient::is_ipfs_url("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"));
        assert!(!IpfsClient::is_ipfs_url("https://example.com/file"));
    }

    #[test]
    fn test_config_defaults() {
        let config = IpfsConfig::default();
        assert_eq!(config.gateways.len(), 4);
        assert_eq!(config.max_retries, 2);
    }
}
