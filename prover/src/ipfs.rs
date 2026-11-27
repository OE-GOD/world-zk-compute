//! IPFS Integration for Decentralized Input Storage
//!
//! Supports fetching inputs from IPFS via multiple gateways for resilience.
//!
//! ## Usage
//!
//! ```rust
//! let client = IpfsClient::new(IpfsConfig::default())?;
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

use crate::redact;
use crate::validation::{read_body_capped, ValidationError};
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

/// IPFS client for fetching content
pub struct IpfsClient {
    http: Client,
    config: IpfsConfig,
}

/// Whether a fetch failed because the content is over the size limit, which
/// no other gateway or attempt can change
fn is_too_large(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<ValidationError>(),
        Some(ValidationError::BodyTooLarge { .. })
    )
}

impl IpfsClient {
    /// Create a new IPFS client
    pub fn new(config: IpfsConfig) -> Result<Self> {
        let http = Client::builder()
            .timeout(config.timeout)
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create IPFS HTTP client: {}", e))?;

        Ok(Self { http, config })
    }

    /// Fetch content by CID
    pub async fn fetch(&self, cid: &str) -> Result<Vec<u8>> {
        self.fetch_at_most(cid, self.config.max_size).await
    }

    /// [`Self::fetch`] with a size limit of the caller's: content over
    /// `max_size` bytes, or over the client's own limit, is refused
    pub async fn fetch_at_most(&self, cid: &str, max_size: usize) -> Result<Vec<u8>> {
        // Normalize CID (remove ipfs:// prefix if present)
        let cid = Self::normalize_cid(cid);
        Self::check_cid_path(&cid)?;
        let max_size = max_size.min(self.config.max_size);

        info!("Fetching IPFS content: {}", cid);

        let mut last_error = None;

        for gateway in &self.config.gateways {
            match self.fetch_from_gateway(gateway, &cid, max_size).await {
                Ok(data) => {
                    info!(
                        "Successfully fetched {} bytes from {}",
                        data.len(),
                        redact::url_origin(gateway)
                    );
                    return Ok(data);
                }
                // The size belongs to the content, not to the gateway
                Err(e) if is_too_large(&e) => return Err(e),
                Err(e) => {
                    warn!("Gateway {} failed: {}", redact::url_origin(gateway), e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("No gateways configured")))
    }

    /// Fetch from a specific gateway with retries
    async fn fetch_from_gateway(
        &self,
        gateway: &str,
        cid: &str,
        max_size: usize,
    ) -> Result<Vec<u8>> {
        let url = format!("{}{}", gateway, cid);
        debug!("Trying gateway: {}", redact::url_origin(gateway));

        let mut last_error = None;

        for attempt in 0..self.config.max_retries {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            }

            match self.do_fetch(&url, max_size).await {
                Ok(data) => return Ok(data),
                Err(e) if is_too_large(&e) => return Err(e),
                Err(e) => {
                    debug!("Attempt {} failed: {}", attempt + 1, e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Unknown error")))
    }

    /// Perform the actual HTTP fetch
    async fn do_fetch(&self, url: &str, max_size: usize) -> Result<Vec<u8>> {
        // The error of a failed request names the URL; the gateway's path can
        // hold a key
        let response = self
            .http
            .get(url)
            .header("Accept", "application/octet-stream")
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP {}: {}",
                response.status(),
                redact::url_origin(url)
            ));
        }

        read_body_capped(response, max_size).await
    }

    /// Check that a normalised CID path names IPFS content: a CID, then
    /// sub-path segments. The path goes behind the gateway's /ipfs/ prefix as
    /// written, so a '..' (also percent-encoded), '?', '#' or '\' in it
    /// could address another path on the gateway host.
    fn check_cid_path(path: &str) -> Result<()> {
        let invalid = |reason: &str| -> anyhow::Error {
            ValidationError::InvalidCid(format!("{path:?}: {reason}")).into()
        };

        let mut segments = path.split('/');
        let cid = segments.next().unwrap_or_default();
        if !Self::is_cid(cid) {
            return Err(invalid(
                "does not start with a CIDv0 (Qm...) or a base32 CIDv1 (b...)",
            ));
        }
        for segment in segments {
            let allowed =
                |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-');
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || !segment.bytes().all(allowed)
            {
                return Err(invalid(
                    "a sub-path segment is empty, '.' or '..', or has a character other than A-Z, a-z, 0-9, '.', '_', '~' and '-'",
                ));
            }
        }

        Ok(())
    }

    /// A CIDv0 ("Qm" and 44 more base58btc characters) or a CIDv1 in
    /// lower-case base32 ("b" and at least 49 more characters of a-z, 2-7)
    fn is_cid(s: &str) -> bool {
        const BASE58BTC: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let v0 = s.len() == 46 && s.starts_with("Qm") && s.bytes().all(|b| BASE58BTC.contains(&b));
        let v1 = s.len() >= 50
            && s.starts_with('b')
            && s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'));
        v0 || v1
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{gzip_zeros, serve_http, serve_http_responses, HttpResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const CID_V0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
    const CID_V1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    fn client_with_gateway(gateway: String, max_size: usize) -> IpfsClient {
        IpfsClient::new(IpfsConfig {
            gateways: vec![gateway],
            max_retries: 1,
            max_size,
            ..IpfsConfig::default()
        })
        .unwrap()
    }

    /// A gateway that answers every path with its own text, and counts the
    /// requests
    async fn echo_gateway() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = requests.clone();
        let addr = serve_http(move |request| {
            counted.fetch_add(1, Ordering::SeqCst);
            (200, request.path.clone().into_bytes())
        })
        .await;
        (addr, requests)
    }

    /// The finding's case: "../" walked out of the gateway's /ipfs/ prefix
    /// to the node's API. It and its encoded forms are refused before any
    /// request is sent.
    #[tokio::test]
    async fn test_path_that_is_not_a_cid_is_refused_without_a_request() {
        let (addr, requests) = echo_gateway().await;
        let client = client_with_gateway(format!("http://{addr}/ipfs/"), 1024);

        for path in [
            "ipfs://../api/v0/pin/ls?x=".to_string(),
            "ipfs://%2e%2e/api/v0/pin/ls".to_string(),
            "ipfs://%2E%2E/api/v0/pin/ls".to_string(),
            "/ipfs/../../api/v0/pin/ls".to_string(),
            "QmTest123".to_string(),
            format!("{CID_V0}/../../api/v0/pin/ls"),
            format!("{CID_V0}/%2e%2e/%2e%2e/api/v0/pin/ls"),
            format!("{CID_V0}/..%2f..%2fapi"),
            format!("{CID_V0}/./file"),
            format!("{CID_V0}?x="),
            format!("{CID_V0}/file?x="),
            format!("{CID_V0}#x"),
            format!("{CID_V0}\\..\\api"),
            format!("{CID_V0}//api"),
            format!("{CID_V0}/"),
            format!("{CID_V0}0"),
            CID_V1.to_uppercase(),
            format!("{CID_V1}/dir/../../api"),
        ] {
            let err = client.fetch(&path).await.unwrap_err();
            assert!(
                matches!(
                    err.downcast_ref::<ValidationError>(),
                    Some(ValidationError::InvalidCid(_))
                ),
                "{path}: {err}"
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_cid_v0_cid_v1_and_sub_path_are_fetched_below_the_prefix() {
        let (addr, requests) = echo_gateway().await;
        let client = client_with_gateway(format!("http://{addr}/ipfs/"), 1024);

        for (path, requested) in [
            (format!("ipfs://{CID_V0}"), format!("/ipfs/{CID_V0}")),
            (format!("/ipfs/{CID_V1}"), format!("/ipfs/{CID_V1}")),
            (
                format!("ipfs://{CID_V1}/dir/file-1_v2.~bin"),
                format!("/ipfs/{CID_V1}/dir/file-1_v2.~bin"),
            ),
        ] {
            let data = client.fetch(&path).await.unwrap();
            assert_eq!(data, requested.into_bytes(), "{path}");
        }
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    /// A gateway that sends `gzip` with Content-Encoding: gzip
    async fn gzip_gateway(gzip: Vec<u8>) -> std::net::SocketAddr {
        serve_http_responses(move |_| {
            HttpResponse::new(200, gzip.clone()).header("Content-Encoding", "gzip")
        })
        .await
    }

    /// Content over the limit is read once: another attempt or gateway would
    /// only read the same content again
    #[tokio::test]
    async fn test_content_over_limit_is_read_once() {
        const LIMIT: usize = 64 * 1024;
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = requests.clone();
        let gzip = gzip_zeros(8 * 1024 * 1024);
        let addr = serve_http_responses(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            HttpResponse::new(200, gzip.clone()).header("Content-Encoding", "gzip")
        })
        .await;
        let gateway = format!("http://{addr}/ipfs/");
        let client = IpfsClient::new(IpfsConfig {
            gateways: vec![gateway.clone(), gateway],
            max_retries: 2,
            max_size: LIMIT,
            ..IpfsConfig::default()
        })
        .unwrap();

        let err = client.fetch(CID_V0).await.unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<ValidationError>(),
                Some(ValidationError::BodyTooLarge { .. })
            ),
            "{err}"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    /// The finding's reproducer: a small compressed body, with a
    /// Content-Length under the limit, that decompresses to far more. The
    /// read stops soon after the limit.
    #[tokio::test]
    async fn test_compressed_body_over_limit_is_refused_while_reading() {
        const LIMIT: usize = 64 * 1024;
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        let gzip = gzip_zeros(DECOMPRESSED);
        assert!(gzip.len() < LIMIT);
        let addr = gzip_gateway(gzip).await;
        let client = client_with_gateway(format!("http://{addr}/ipfs/"), LIMIT);

        let err = client.fetch(CID_V0).await.unwrap_err();

        match err.downcast_ref::<ValidationError>() {
            Some(ValidationError::BodyTooLarge { max, read }) => {
                assert_eq!(*max, LIMIT);
                assert!(*read > LIMIT && *read < DECOMPRESSED / 4, "read {read}");
            }
            _ => panic!("expected BodyTooLarge, got {err}"),
        }
    }

    /// A caller's lower limit is applied while reading, not after
    #[tokio::test]
    async fn test_fetch_at_most_applies_the_lower_limit_while_reading() {
        const LIMIT: usize = 64 * 1024;
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        let addr = gzip_gateway(gzip_zeros(DECOMPRESSED)).await;
        let client = client_with_gateway(format!("http://{addr}/ipfs/"), 100 * 1024 * 1024);

        let err = client.fetch_at_most(CID_V0, LIMIT).await.unwrap_err();

        match err.downcast_ref::<ValidationError>() {
            Some(ValidationError::BodyTooLarge { max, read }) => {
                assert_eq!(*max, LIMIT);
                assert!(*read < DECOMPRESSED / 4, "read {read}");
            }
            _ => panic!("expected BodyTooLarge, got {err}"),
        }
    }

    /// The same stream is read whole under a higher limit: it is valid gzip
    /// and reqwest decompresses it
    #[tokio::test]
    async fn test_compressed_body_under_limit_is_read_decompressed() {
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        let addr = gzip_gateway(gzip_zeros(DECOMPRESSED)).await;
        let client = client_with_gateway(format!("http://{addr}/ipfs/"), 2 * DECOMPRESSED);

        let data = client.fetch(CID_V0).await.unwrap();

        assert_eq!(data.len(), DECOMPRESSED);
        assert!(data.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_normalize_cid() {
        assert_eq!(IpfsClient::normalize_cid("ipfs://QmTest123"), "QmTest123");
        assert_eq!(IpfsClient::normalize_cid("/ipfs/QmTest123"), "QmTest123");
        assert_eq!(IpfsClient::normalize_cid("QmTest123"), "QmTest123");
    }

    #[test]
    fn test_is_ipfs_url() {
        assert!(IpfsClient::is_ipfs_url("ipfs://QmTest"));
        assert!(IpfsClient::is_ipfs_url("/ipfs/QmTest"));
        assert!(IpfsClient::is_ipfs_url(
            "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"
        ));
        assert!(IpfsClient::is_ipfs_url(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ));
        assert!(!IpfsClient::is_ipfs_url("https://example.com/file"));
    }

    #[test]
    fn test_config_defaults() {
        let config = IpfsConfig::default();
        assert_eq!(config.gateways.len(), 4);
        assert_eq!(config.max_retries, 2);
    }
}
