//! Private Input Support
//!
//! Auth client for fetching private inputs from an auth server.
//! The prover authenticates with a wallet-signed request, and the auth server
//! verifies the prover has claimed the job on-chain before releasing data.

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
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

/// Validate a URL to prevent SSRF attacks.
///
/// Rejects non-HTTP(S) schemes and URLs pointing to private/loopback IPs.
pub fn validate_url(url: &str) -> anyhow::Result<()> {
    let parsed: reqwest::Url =
        url.parse().map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;

    match parsed.scheme() {
        "https" | "http" => {}
        scheme => anyhow::bail!("unsupported URL scheme: {scheme}"),
    }

    if let Some(host) = parsed.host_str() {
        if host == "localhost" || host == "metadata.google.internal" {
            anyhow::bail!("URL host is not allowed: {host}");
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if ip.is_loopback()
                || matches!(ip, IpAddr::V4(v4) if v4.is_private())
                || matches!(ip, IpAddr::V4(v4) if v4.is_link_local())
            {
                anyhow::bail!("URL points to private/loopback address: {ip}");
            }
        }
    } else {
        anyhow::bail!("URL has no host");
    }

    Ok(())
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
    validate_url(auth_url)?;

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

    // POST to auth server
    let url = format!("{}/inputs/{}", auth_url.trim_end_matches('/'), request_id);
    info!(
        "[Private Input {}] Fetching from auth server: {}",
        request_id, url
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let response = client.post(&url).json(&auth_request).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "no body".to_string());
        anyhow::bail!(
            "Auth server returned {} for request {}: {}",
            status,
            request_id,
            body
        );
    }

    let data = response.bytes().await?.to_vec();
    info!(
        "[Private Input {}] Received {} bytes from auth server",
        request_id,
        data.len()
    );

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_validate_url_accepts_https() {
        assert!(validate_url("https://api.example.com/inputs").is_ok());
    }

    #[test]
    fn test_validate_url_accepts_http() {
        assert!(validate_url("http://api.example.com/inputs").is_ok());
    }

    #[test]
    fn test_validate_url_rejects_private_ip() {
        assert!(validate_url("https://192.168.1.1/inputs").is_err());
        assert!(validate_url("https://10.0.0.1/inputs").is_err());
        assert!(validate_url("https://127.0.0.1/inputs").is_err());
    }

    #[test]
    fn test_validate_url_rejects_localhost() {
        assert!(validate_url("https://localhost/inputs").is_err());
    }

    #[test]
    fn test_validate_url_rejects_bad_scheme() {
        assert!(validate_url("ftp://example.com/inputs").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_validate_url_rejects_cloud_metadata() {
        assert!(validate_url("http://metadata.google.internal/computeMetadata").is_err());
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
