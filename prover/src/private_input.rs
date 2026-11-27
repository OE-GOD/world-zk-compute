//! Private Input Support
//!
//! Auth client for fetching private inputs from an auth server.
//! The prover authenticates with a wallet-signed request, and the auth server
//! verifies the prover has claimed the job on-chain before releasing data.

use crate::redact;
use crate::validation::{read_body_capped, Validator};
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Input type enum matching the Solidity uint8 values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum InputType {
    /// Public input (default) - fetched from inputUrl directly
    Public = 0,
    /// Private input - fetched via authenticated request to auth server
    Private = 1,
}

impl From<u8> for InputType {
    fn from(v: u8) -> Self {
        match v {
            1 => InputType::Private,
            _ => InputType::Public,
        }
    }
}

/// Authentication request sent to the private input server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    /// The on-chain request ID
    pub request_id: u64,
    /// The prover's Ethereum address
    pub prover_address: String,
    /// Unix timestamp of the request
    pub timestamp: u64,
    /// EIP-191 signature of the authentication message
    pub signature: String,
}

/// Build the EIP-191 message for private input authentication
pub fn build_auth_message(request_id: u64, prover_address: &Address, timestamp: u64) -> String {
    format!(
        "world-zk-compute:fetch-input:{}:{}:{}",
        request_id,
        prover_address.to_checksum(None),
        timestamp,
    )
}

/// Largest private input taken from an auth server. `fetch_private_input`
/// is not given the prover's configuration, so the limit is the prefetcher's
/// default for public inputs (`PrefetchConfig::max_input_size`).
pub const MAX_PRIVATE_INPUT_SIZE: usize = 10 * 1024 * 1024; // 10 MiB

/// Most of the body of an error answer that is read, for the error text
const MAX_ERROR_BODY_SIZE: usize = 1024;

/// Check the URL a private input is requested from with the crate's SSRF
/// rules ([`Validator::validate_url`]), and require https: the request
/// carries a signature over the prover's address, and ipfs and data URLs,
/// which the validator also takes, cannot be posted to.
fn check_auth_url(url: &str) -> anyhow::Result<()> {
    Validator::new().validate_url(url)?;
    match url::Url::parse(url)?.scheme() {
        "https" => Ok(()),
        scheme => anyhow::bail!("auth server URL must use https, not {scheme}"),
    }
}

/// The URL a private input is fetched from: `{auth_url}/inputs/{request_id}`,
/// checked. The URL the request goes to is the one checked. The prover also
/// checks it before it claims a private job, so that an input the fetch would
/// refuse does not cost a claim.
pub fn input_url(auth_url: &str, request_id: u64) -> anyhow::Result<String> {
    let url = format!("{}/inputs/{}", auth_url.trim_end_matches('/'), request_id);
    check_auth_url(&url)?;
    Ok(url)
}

/// HTTP client for auth servers
fn auth_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        // A redirect would send the request to a host that was never checked
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Fetch private input from an auth server
///
/// Signs an EIP-191 message and POSTs to the auth server endpoint.
/// The auth server verifies the signature and checks on-chain that
/// the prover has claimed the job before releasing the input data.
pub async fn fetch_private_input(
    auth_url: &str,
    request_id: u64,
    signer: &PrivateKeySigner,
) -> anyhow::Result<Vec<u8>> {
    let url = input_url(auth_url, request_id)?;

    let prover_address = signer.address();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("system clock error: {e}"))?
        .as_secs();

    // Build and sign the auth message
    let message = build_auth_message(request_id, &prover_address, timestamp);
    debug!(
        "[Private Input {}] Signing auth message: {}",
        request_id, message
    );

    let signature = signer.sign_message(message.as_bytes()).await?;
    let sig_hex = format!("0x{}", hex::encode(signature.as_bytes()));

    let auth_request = AuthRequest {
        request_id,
        prover_address: prover_address.to_checksum(None),
        timestamp,
        signature: sig_hex,
    };

    info!(
        "[Private Input {}] Fetching from auth server: {}",
        request_id,
        redact::url_origin(&url)
    );

    let client = auth_client()?;
    let data = post_auth_request(&client, &url, &auth_request, MAX_PRIVATE_INPUT_SIZE).await?;
    info!(
        "[Private Input {}] Received {} bytes from auth server",
        request_id,
        data.len()
    );

    Ok(data)
}

/// POST `auth_request` to `url` and read the input it answers with, refusing
/// more than `max_size` bytes. A redirect answer is an error.
async fn post_auth_request(
    client: &reqwest::Client,
    url: &str,
    auth_request: &AuthRequest,
    max_size: usize,
) -> anyhow::Result<Vec<u8>> {
    // The error of a failed request names the URL, whose path can hold a key
    let response = client
        .post(url)
        .json(auth_request)
        .send()
        .await
        .map_err(reqwest::Error::without_url)?;

    if !response.status().is_success() {
        let status = response.status();
        // Quoted with escapes: the server chooses the text, and line breaks or
        // terminal codes in it must not make lines of the prover's log
        let body = match read_body_capped(response, MAX_ERROR_BODY_SIZE).await {
            Ok(body) => format!("{:?}", String::from_utf8_lossy(&body)),
            Err(_) => "body not shown".to_string(),
        };
        anyhow::bail!(
            "Auth server returned {} for request {}: {}",
            status,
            auth_request.request_id,
            body
        );
    }

    read_body_capped(response, max_size).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{gzip_zeros, serve_http, serve_http_responses, HttpResponse};
    use crate::validation::ValidationError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_input_type_from_u8() {
        assert_eq!(InputType::from(0), InputType::Public);
        assert_eq!(InputType::from(1), InputType::Private);
        assert_eq!(InputType::from(2), InputType::Public); // Unknown defaults to public
    }

    #[test]
    fn test_build_auth_message() {
        let address: Address = "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"
            .parse()
            .unwrap();
        let msg = build_auth_message(42, &address, 1700000000);
        assert!(msg.starts_with("world-zk-compute:fetch-input:42:"));
        assert!(msg.contains("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"));
        assert!(msg.ends_with(":1700000000"));
    }

    #[test]
    fn test_auth_url_accepts_public_https() {
        assert!(check_auth_url("https://api.example.com/inputs/7").is_ok());
    }

    #[test]
    fn test_auth_url_refuses_http() {
        assert!(check_auth_url("http://api.example.com/inputs/7").is_err());
    }

    #[test]
    fn test_auth_url_refuses_private_ip() {
        assert!(check_auth_url("https://192.168.1.1/inputs").is_err());
        assert!(check_auth_url("https://10.0.0.1/inputs").is_err());
        assert!(check_auth_url("https://127.0.0.1/inputs").is_err());
    }

    #[test]
    fn test_auth_url_refuses_localhost() {
        assert!(check_auth_url("https://localhost/inputs").is_err());
    }

    #[test]
    fn test_auth_url_refuses_bad_scheme() {
        assert!(check_auth_url("ftp://example.com/inputs").is_err());
        assert!(check_auth_url("file:///etc/passwd").is_err());
        assert!(check_auth_url("ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG").is_err());
        assert!(check_auth_url("data:application/octet-stream;base64,dGVzdA==").is_err());
    }

    #[test]
    fn test_auth_url_refuses_cloud_metadata() {
        assert!(check_auth_url("https://metadata.google.internal/computeMetadata").is_err());
    }

    /// The internal addresses the old check let through: IPv6 literals,
    /// 0.0.0.0, CGNAT and a trailing-dot localhost. Each is refused by the
    /// SSRF rules, not only for being http, and before any request is made.
    #[tokio::test]
    async fn test_fetch_refuses_ipv6_unspecified_cgnat_and_localhost_dot() {
        let signer = PrivateKeySigner::random();
        for auth_url in [
            "[::1]:8545",
            "[::ffff:127.0.0.1]:8545",
            "[::ffff:169.254.169.254]/latest/meta-data",
            "[fd00::1]",
            "[fe80::1]",
            "0.0.0.0:8545",
            "100.64.0.1",
            "localhost.:8545",
        ] {
            for scheme in ["https", "http"] {
                let auth_url = format!("{scheme}://{auth_url}/");
                let err = fetch_private_input(&auth_url, 7, &signer)
                    .await
                    .unwrap_err();
                if scheme == "https" {
                    assert!(
                        matches!(
                            err.downcast_ref::<ValidationError>(),
                            Some(ValidationError::SsrfBlocked(_))
                        ),
                        "{auth_url}: {err}"
                    );
                }
            }
        }
    }

    fn test_auth_request() -> AuthRequest {
        AuthRequest {
            request_id: 7,
            prover_address: "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045".to_string(),
            timestamp: 1700000000,
            signature: "0x00".to_string(),
        }
    }

    /// A redirect answer is an error and its target gets no request: it
    /// could be an address the URL check refuses.
    #[tokio::test]
    async fn test_redirect_is_not_followed() {
        let target_requests = Arc::new(AtomicUsize::new(0));
        let counted = target_requests.clone();
        let target = serve_http(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            (200, b"internal-secret".to_vec())
        })
        .await;
        let location = format!("http://{target}/internal-admin");
        let auth_server = serve_http_responses(move |_| {
            HttpResponse::new(307, Vec::new()).header("Location", &location)
        })
        .await;

        let client = auth_client().unwrap();
        let err = post_auth_request(
            &client,
            &format!("http://{auth_server}/inputs/7"),
            &test_auth_request(),
            MAX_PRIVATE_INPUT_SIZE,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("307"), "{err}");
        assert_eq!(target_requests.load(Ordering::SeqCst), 0);
    }

    /// The text of an error answer goes into the error quoted, with its line
    /// breaks and terminal codes escaped, so it cannot add lines to the log
    #[tokio::test]
    async fn test_error_answer_text_is_quoted() {
        let auth_server =
            serve_http(|_| (403, b"denied\nINFO forged line \x1b[31mred".to_vec())).await;

        let client = auth_client().unwrap();
        let err = post_auth_request(
            &client,
            &format!("http://{auth_server}/inputs/7"),
            &test_auth_request(),
            MAX_PRIVATE_INPUT_SIZE,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("403"), "{err}");
        assert!(
            err.contains(r#""denied\nINFO forged line \u{1b}[31mred""#),
            "{err}"
        );
        assert!(!err.contains('\n') && !err.contains('\x1b'), "{err}");
    }

    /// A body that decompresses to more than the limit is refused soon after
    /// the limit, though its Content-Length is under the limit
    #[tokio::test]
    async fn test_body_over_limit_is_refused_without_reading_all_of_it() {
        const LIMIT: usize = 64 * 1024;
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        let gzip = gzip_zeros(DECOMPRESSED);
        assert!(gzip.len() < LIMIT);
        let auth_server = serve_http_responses(move |_| {
            HttpResponse::new(200, gzip.clone()).header("Content-Encoding", "gzip")
        })
        .await;

        let client = auth_client().unwrap();
        let err = post_auth_request(
            &client,
            &format!("http://{auth_server}/inputs/7"),
            &test_auth_request(),
            LIMIT,
        )
        .await
        .unwrap_err();

        match err.downcast_ref::<ValidationError>() {
            Some(ValidationError::BodyTooLarge { max, read }) => {
                assert_eq!(*max, LIMIT);
                assert!(*read > LIMIT && *read < DECOMPRESSED / 4, "read {read}");
            }
            _ => panic!("expected BodyTooLarge, got {err}"),
        }
    }

    /// The same body is taken whole when the limit is above its size: the
    /// gzip stream is valid and reqwest decompresses it
    #[tokio::test]
    async fn test_body_under_limit_is_read_decompressed() {
        const DECOMPRESSED: usize = 8 * 1024 * 1024;
        let gzip = gzip_zeros(DECOMPRESSED);
        let auth_server = serve_http_responses(move |_| {
            HttpResponse::new(200, gzip.clone()).header("Content-Encoding", "gzip")
        })
        .await;

        let client = auth_client().unwrap();
        let data = post_auth_request(
            &client,
            &format!("http://{auth_server}/inputs/7"),
            &test_auth_request(),
            MAX_PRIVATE_INPUT_SIZE,
        )
        .await
        .unwrap();

        assert_eq!(data.len(), DECOMPRESSED);
        assert!(data.iter().all(|&b| b == 0));
    }

    #[tokio::test]
    async fn test_body_with_content_length_over_limit_is_refused() {
        let auth_server = serve_http(|_| (200, vec![1u8; 4096])).await;

        let client = auth_client().unwrap();
        let err = post_auth_request(
            &client,
            &format!("http://{auth_server}/inputs/7"),
            &test_auth_request(),
            1024,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(
                err.downcast_ref::<ValidationError>(),
                Some(ValidationError::BodyTooLarge { max: 1024, .. })
            ),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_auth_request_signing() {
        // Generate a random signer for testing
        let signer = PrivateKeySigner::random();
        let address = signer.address();
        let timestamp = 1700000000u64;
        let request_id = 42u64;

        let message = build_auth_message(request_id, &address, timestamp);
        let signature = signer.sign_message(message.as_bytes()).await.unwrap();

        // Verify the signature recovers to the correct address
        let recovered = signature
            .recover_address_from_msg(message.as_bytes())
            .unwrap();
        assert_eq!(recovered, address);
    }
}
