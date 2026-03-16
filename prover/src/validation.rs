//! Input Validation and Sanitization
//!
//! Validates and sanitizes inputs before processing:
//! - Image ID format validation
//! - Input data size limits
//! - URL sanitization with SSRF protection
//! - Address format validation
//!
//! ## Security
//!
//! All external inputs are validated before use to prevent:
//! - Buffer overflow attacks
//! - Injection attacks
//! - Resource exhaustion
//! - Server-Side Request Forgery (SSRF) via private/internal IP ranges

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use thiserror::Error;

/// Compile-time constant: allowed URL schemes for external requests.
/// Only HTTPS and IPFS are permitted. HTTP is intentionally excluded
/// to prevent cleartext data exfiltration. Data URIs are allowed for
/// inline payloads.
const ALLOWED_SCHEMES: &[&str] = &["https", "ipfs", "data"];

/// Maximum URL length (compile-time constant).
const MAX_URL_LENGTH: usize = 2048;

/// Maximum image ID length (compile-time constant).
const MAX_IMAGE_ID_LENGTH: usize = 128;

/// Maximum input size in bytes (compile-time constant).
const MAX_INPUT_SIZE: usize = 100 * 1024 * 1024; // 100 MB

/// Private/reserved hostnames that must always be blocked (compile-time constant).
const BLOCKED_HOSTNAMES: &[&str] = &["localhost", "localhost.localdomain"];

/// Domain suffixes that indicate private/internal hosts (compile-time constant).
const BLOCKED_HOSTNAME_SUFFIXES: &[&str] = &[
    ".local",
    ".internal",
    ".lan",
    ".localhost",
    ".intranet",
    ".corp",
    ".home",
    ".private",
];

/// Validation errors
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ValidationError {
    #[error("Invalid image ID format: {0}")]
    InvalidImageId(String),

    #[error("Invalid address format: {0}")]
    InvalidAddress(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("SSRF blocked: {0}")]
    SsrfBlocked(String),

    #[error("Input too large: {size} bytes (max: {max} bytes)")]
    InputTooLarge { size: usize, max: usize },

    #[error("Input too small: {size} bytes (min: {min} bytes)")]
    InputTooSmall { size: usize, min: usize },

    #[error("Invalid hex string: {0}")]
    InvalidHex(String),

    #[error("Invalid hash format: expected {expected} bytes, got {actual}")]
    InvalidHashLength { expected: usize, actual: usize },

    #[error("Empty input not allowed")]
    EmptyInput,

    #[error("Invalid character in input: {0}")]
    InvalidCharacter(char),

    #[error("Invalid CID format: {0}")]
    InvalidCid(String),
}

/// Validation configuration
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ValidationConfig {
    /// Maximum input size in bytes
    pub max_input_size: usize,
    /// Maximum URL length
    pub max_url_length: usize,
    /// Maximum image ID length
    pub max_image_id_length: usize,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            max_input_size: MAX_INPUT_SIZE,
            max_url_length: MAX_URL_LENGTH,
            max_image_id_length: MAX_IMAGE_ID_LENGTH,
        }
    }
}

/// Check whether an IPv4 address falls within private/reserved ranges.
///
/// Blocked ranges:
/// - `0.0.0.0` (unspecified)
/// - `127.0.0.0/8` (loopback)
/// - `10.0.0.0/8` (private, RFC 1918)
/// - `172.16.0.0/12` (private, RFC 1918)
/// - `192.168.0.0/16` (private, RFC 1918)
/// - `169.254.0.0/16` (link-local, RFC 3927 -- includes AWS metadata)
/// - `100.64.0.0/10` (shared address space, RFC 6598 / CGNAT)
/// - `192.0.0.0/24` (IETF protocol assignments)
/// - `192.0.2.0/24` (TEST-NET-1)
/// - `198.51.100.0/24` (TEST-NET-2)
/// - `203.0.113.0/24` (TEST-NET-3)
/// - `198.18.0.0/15` (benchmarking)
/// - `240.0.0.0/4` (reserved, includes broadcast 255.255.255.255)
fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();

    if ip.is_unspecified() {
        return true;
    }
    if ip.is_loopback() {
        return true;
    }
    // 10.0.0.0/8
    if octets[0] == 10 {
        return true;
    }
    // 172.16.0.0/12
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }
    // 192.168.0.0/16
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    // 169.254.0.0/16 (link-local)
    if ip.is_link_local() {
        return true;
    }
    // 100.64.0.0/10 (CGNAT)
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }
    // 192.0.0.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return true;
    }
    // 192.0.2.0/24 (TEST-NET-1)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // 198.51.100.0/24 (TEST-NET-2)
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // 203.0.113.0/24 (TEST-NET-3)
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }
    // 198.18.0.0/15 (benchmarking)
    if octets[0] == 198 && (18..=19).contains(&octets[1]) {
        return true;
    }
    // 240.0.0.0/4 (reserved, includes broadcast)
    if octets[0] >= 240 {
        return true;
    }

    false
}

/// Check whether an IPv6 address falls within private/reserved ranges.
///
/// Blocked ranges:
/// - `::` (unspecified)
/// - `::1` (loopback)
/// - `fe80::/10` (link-local)
/// - `fc00::/7` (unique local address, RFC 4193)
/// - `::ffff:0:0/96` (IPv4-mapped -- checked via the mapped IPv4 address)
fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    if ip.is_unspecified() {
        return true;
    }
    if ip.is_loopback() {
        return true;
    }

    let segments = ip.segments();

    // fe80::/10
    if segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }
    // fc00::/7
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }
    // IPv4-mapped IPv6: ::ffff:x.x.x.x
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_private_ipv4(&mapped);
    }

    false
}

/// Check whether an IP address (v4 or v6) is private/reserved.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

/// Check whether a hostname is a known private/internal name.
fn is_private_hostname(host: &str) -> bool {
    let lower = host.to_lowercase();

    for blocked in BLOCKED_HOSTNAMES {
        if lower == *blocked {
            return true;
        }
    }

    for suffix in BLOCKED_HOSTNAME_SUFFIXES {
        if lower.ends_with(suffix) {
            return true;
        }
    }

    false
}

/// Extract the host from a URL string without requiring the `url` crate.
/// Returns the host portion (without port, userinfo, or brackets for IPv6).
fn extract_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r)?;

    // Strip path, query, fragment
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.split('?').next().unwrap_or(authority);
    let authority = authority.split('#').next().unwrap_or(authority);

    // Strip userinfo
    let host_port = if let Some((_, hp)) = authority.rsplit_once('@') {
        hp
    } else {
        authority
    };

    if host_port.is_empty() {
        return None;
    }

    // Handle IPv6 bracket notation
    if host_port.starts_with('[') {
        let end = host_port.find(']')?;
        Some(&host_port[1..end])
    } else {
        // Strip port for IPv4/hostname
        Some(host_port.rsplit_once(':').map_or(host_port, |(h, _)| h))
    }
}

/// Input validator
pub struct Validator {
    config: ValidationConfig,
}

impl Validator {
    /// Create a new validator with default config
    pub fn new() -> Self {
        Self {
            config: ValidationConfig::default(),
        }
    }

    /// Validate an Ethereum address (0x-prefixed, 40 hex chars)
    pub fn validate_address(&self, address: &str) -> Result<String, ValidationError> {
        let address = address.trim();

        if address.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        // Must start with 0x
        if !address.starts_with("0x") && !address.starts_with("0X") {
            return Err(ValidationError::InvalidAddress(
                "must start with 0x".to_string(),
            ));
        }

        // Must be 42 characters total (0x + 40 hex)
        if address.len() != 42 {
            return Err(ValidationError::InvalidAddress(format!(
                "expected 42 characters, got {}",
                address.len()
            )));
        }

        // Validate hex characters
        let hex_part = &address[2..];
        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidAddress(
                "contains non-hex characters".to_string(),
            ));
        }

        // Return lowercase normalized address
        Ok(address.to_lowercase())
    }

    /// Validate an image ID (RISC Zero image ID format)
    pub fn validate_image_id(&self, image_id: &str) -> Result<String, ValidationError> {
        let image_id = image_id.trim();

        if image_id.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        if image_id.len() > self.config.max_image_id_length {
            return Err(ValidationError::InvalidImageId(format!(
                "too long: {} chars (max: {})",
                image_id.len(),
                self.config.max_image_id_length
            )));
        }

        // Image ID should be hex (with or without 0x prefix)
        let hex_part = if image_id.starts_with("0x") || image_id.starts_with("0X") {
            &image_id[2..]
        } else {
            image_id
        };

        if !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidImageId(
                "contains non-hex characters".to_string(),
            ));
        }

        // Should be 64 hex chars (32 bytes)
        if hex_part.len() != 64 {
            return Err(ValidationError::InvalidImageId(format!(
                "expected 64 hex chars, got {}",
                hex_part.len()
            )));
        }

        Ok(image_id.to_lowercase())
    }

    /// Validate a URL with SSRF protection.
    ///
    /// Checks:
    /// 1. URL is non-empty and within length limits
    /// 2. URL does not contain control characters
    /// 3. URL scheme is in the compile-time allow list (https, ipfs, data)
    /// 4. If the URL has a host component and it is an IP literal, the IP
    ///    must not be in a private/reserved range
    /// 5. If the host is a hostname, it must not be a known private hostname
    ///    (localhost, *.local, *.internal, etc.)
    pub fn validate_url(&self, url: &str) -> Result<String, ValidationError> {
        let url = url.trim();

        if url.is_empty() {
            return Err(ValidationError::EmptyInput);
        }

        if url.len() > self.config.max_url_length {
            return Err(ValidationError::InvalidUrl(format!(
                "too long: {} chars (max: {})",
                url.len(),
                self.config.max_url_length
            )));
        }

        // Basic URL validation - no control characters
        if url.chars().any(|c| c.is_control()) {
            return Err(ValidationError::InvalidUrl(
                "contains control characters".to_string(),
            ));
        }

        // Check for valid scheme (compile-time constant list)
        let scheme_lower = url
            .split_once(':')
            .map(|(s, _)| s.to_lowercase())
            .unwrap_or_default();

        if !ALLOWED_SCHEMES.contains(&scheme_lower.as_str()) {
            return Err(ValidationError::InvalidUrl(format!(
                "scheme '{}' not allowed. Allowed: {:?}",
                scheme_lower, ALLOWED_SCHEMES
            )));
        }

        // Data URIs and IPFS CIDs do not have network hosts to SSRF-check.
        // Only check URLs with "://" (network-based schemes).
        if url.contains("://") {
            if let Some(host) = extract_host(url) {
                if !host.is_empty() {
                    // Check if host is an IP literal
                    if let Ok(ip) = host.parse::<IpAddr>() {
                        if is_private_ip(&ip) {
                            return Err(ValidationError::SsrfBlocked(format!(
                                "host '{}' is a private/reserved IP address",
                                ip
                            )));
                        }
                    } else {
                        // Host is a hostname -- check against blocklist
                        if is_private_hostname(host) {
                            return Err(ValidationError::SsrfBlocked(format!(
                                "host '{}' is a private/internal hostname",
                                host
                            )));
                        }
                    }
                }
            }
        }

        Ok(url.to_string())
    }
}

impl Default for Validator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Existing tests (updated for new behavior) =====

    #[test]
    fn test_validate_address() {
        let v = Validator::new();

        // Valid addresses
        assert!(v
            .validate_address("0x1234567890abcdef1234567890abcdef12345678")
            .is_ok());
        assert!(v
            .validate_address("0xABCDEF1234567890ABCDEF1234567890ABCDEF12")
            .is_ok());

        // Invalid addresses
        assert!(v.validate_address("").is_err());
        assert!(v
            .validate_address("1234567890abcdef1234567890abcdef12345678")
            .is_err());
        assert!(v.validate_address("0x123").is_err());
        assert!(v
            .validate_address("0xGGGG567890abcdef1234567890abcdef12345678")
            .is_err());
    }

    #[test]
    fn test_validate_image_id() {
        let v = Validator::new();

        // Valid image ID (64 hex chars)
        let valid = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        assert!(v.validate_image_id(valid).is_ok());
        assert!(v.validate_image_id(&format!("0x{}", valid)).is_ok());

        // Invalid
        assert!(v.validate_image_id("").is_err());
        assert!(v.validate_image_id("1234").is_err());
        assert!(v
            .validate_image_id("zzzz567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef")
            .is_err());
    }

    #[test]
    fn test_validate_url_allowed() {
        let v = Validator::new();

        // Valid URLs
        assert!(v.validate_url("https://example.com/data").is_ok());
        assert!(v.validate_url("ipfs://QmTest").is_ok());
        assert!(v
            .validate_url("data:application/octet-stream;base64,dGVzdA==")
            .is_ok());
    }

    #[test]
    fn test_validate_url_scheme_blocked() {
        let v = Validator::new();

        // http is not in allowed schemes (only https)
        assert!(v.validate_url("http://example.com").is_err());
        assert!(v.validate_url("ftp://example.com").is_err());
        assert!(v.validate_url("gopher://evil.com/1").is_err());
    }

    #[test]
    fn test_validate_url_empty() {
        let v = Validator::new();
        assert!(v.validate_url("").is_err());
    }

    // ===== SSRF: IPv4 private ranges =====

    #[test]
    fn test_ssrf_blocks_loopback_127_0_0_1() {
        let v = Validator::new();
        let err = v.validate_url("https://127.0.0.1/path").unwrap_err();
        assert!(
            matches!(err, ValidationError::SsrfBlocked(_)),
            "Expected SsrfBlocked, got: {:?}",
            err
        );
    }

    #[test]
    fn test_ssrf_blocks_loopback_127_x() {
        let v = Validator::new();
        assert!(v.validate_url("https://127.255.0.1/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_10_x() {
        let v = Validator::new();
        let err = v.validate_url("https://10.0.0.1/api").unwrap_err();
        assert!(matches!(err, ValidationError::SsrfBlocked(_)));
    }

    #[test]
    fn test_ssrf_blocks_10_255_x() {
        let v = Validator::new();
        assert!(v.validate_url("https://10.255.255.255/api").is_err());
    }

    #[test]
    fn test_ssrf_blocks_172_16_x() {
        let v = Validator::new();
        assert!(v.validate_url("https://172.16.0.1/api").is_err());
    }

    #[test]
    fn test_ssrf_blocks_172_31_x() {
        let v = Validator::new();
        assert!(v.validate_url("https://172.31.255.255/api").is_err());
    }

    #[test]
    fn test_ssrf_allows_172_32_x() {
        let v = Validator::new();
        // 172.32.x.x is public
        assert!(v.validate_url("https://172.32.0.1/api").is_ok());
    }

    #[test]
    fn test_ssrf_blocks_192_168_x() {
        let v = Validator::new();
        assert!(v.validate_url("https://192.168.1.1/admin").is_err());
    }

    #[test]
    fn test_ssrf_blocks_link_local_169_254() {
        let v = Validator::new();
        // AWS metadata endpoint
        let err = v
            .validate_url("https://169.254.169.254/latest/meta-data/")
            .unwrap_err();
        assert!(matches!(err, ValidationError::SsrfBlocked(_)));
    }

    #[test]
    fn test_ssrf_blocks_broadcast() {
        let v = Validator::new();
        assert!(v.validate_url("https://255.255.255.255/x").is_err());
    }

    #[test]
    fn test_ssrf_blocks_unspecified_0_0_0_0() {
        let v = Validator::new();
        assert!(v.validate_url("https://0.0.0.0/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_cgnat_100_64() {
        let v = Validator::new();
        assert!(v.validate_url("https://100.64.0.1/api").is_err());
    }

    #[test]
    fn test_ssrf_blocks_reserved_240() {
        let v = Validator::new();
        assert!(v.validate_url("https://240.0.0.1/api").is_err());
    }

    // ===== SSRF: IPv6 private ranges =====

    #[test]
    fn test_ssrf_blocks_ipv6_loopback() {
        let v = Validator::new();
        let err = v.validate_url("https://[::1]/path").unwrap_err();
        assert!(matches!(err, ValidationError::SsrfBlocked(_)));
    }

    #[test]
    fn test_ssrf_blocks_ipv6_unspecified() {
        let v = Validator::new();
        assert!(v.validate_url("https://[::]/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_link_local() {
        let v = Validator::new();
        assert!(v.validate_url("https://[fe80::1]/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_unique_local() {
        let v = Validator::new();
        assert!(v.validate_url("https://[fd00::1]/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_ipv4_mapped_ipv6() {
        let v = Validator::new();
        assert!(v.validate_url("https://[::ffff:127.0.0.1]/path").is_err());
    }

    #[test]
    fn test_ssrf_allows_public_ipv6() {
        let v = Validator::new();
        // Google Public DNS IPv6
        assert!(v
            .validate_url("https://[2001:4860:4860::8888]/path")
            .is_ok());
    }

    // ===== SSRF: Private hostnames =====

    #[test]
    fn test_ssrf_blocks_localhost() {
        let v = Validator::new();
        let err = v.validate_url("https://localhost/path").unwrap_err();
        assert!(matches!(err, ValidationError::SsrfBlocked(_)));
    }

    #[test]
    fn test_ssrf_blocks_localhost_with_port() {
        let v = Validator::new();
        assert!(v.validate_url("https://localhost:8080/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_localhost_case_insensitive() {
        let v = Validator::new();
        assert!(v.validate_url("https://LOCALHOST/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_dot_local() {
        let v = Validator::new();
        assert!(v.validate_url("https://myhost.local/api").is_err());
    }

    #[test]
    fn test_ssrf_blocks_dot_internal() {
        let v = Validator::new();
        assert!(v.validate_url("https://metadata.internal/path").is_err());
    }

    #[test]
    fn test_ssrf_blocks_dot_lan() {
        let v = Validator::new();
        assert!(v.validate_url("https://printer.lan/status").is_err());
    }

    // ===== SSRF: Public IPs allowed =====

    #[test]
    fn test_ssrf_allows_public_ipv4() {
        let v = Validator::new();
        assert!(v.validate_url("https://8.8.8.8/dns-query").is_ok());
    }

    #[test]
    fn test_ssrf_allows_public_ipv4_with_port() {
        let v = Validator::new();
        assert!(v.validate_url("https://93.184.216.34:443/page").is_ok());
    }

    #[test]
    fn test_ssrf_allows_public_hostname() {
        let v = Validator::new();
        assert!(v.validate_url("https://api.example.com/v1/data").is_ok());
    }

    // ===== SSRF: Bypass attempt =====

    #[test]
    fn test_ssrf_blocks_userinfo_bypass() {
        let v = Validator::new();
        // Attempt to hide internal IP behind userinfo
        let err = v
            .validate_url("https://evil.com@10.0.0.1/path")
            .unwrap_err();
        assert!(matches!(err, ValidationError::SsrfBlocked(_)));
    }

    #[test]
    fn test_ssrf_blocks_aws_metadata_variant() {
        let v = Validator::new();
        assert!(v
            .validate_url("https://169.254.169.254/latest/meta-data/iam/")
            .is_err());
    }

    #[test]
    fn test_ssrf_blocks_docker_host() {
        let v = Validator::new();
        assert!(v
            .validate_url("https://172.17.0.1:2375/containers/json")
            .is_err());
    }

    // ===== Scheme validation uses compile-time constant =====

    #[test]
    fn test_scheme_is_compile_time_constant() {
        // Verify the allowed schemes list is what we expect
        assert_eq!(ALLOWED_SCHEMES, &["https", "ipfs", "data"]);
    }

    #[test]
    fn test_blocks_file_scheme() {
        let v = Validator::new();
        assert!(v.validate_url("file:///etc/passwd").is_err());
    }

    // ===== Helper function direct tests =====

    #[test]
    fn test_is_private_ip_direct() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_private_ip(&"169.254.169.254".parse().unwrap()));
        assert!(is_private_ip(&"::1".parse().unwrap()));

        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(
            &"2001:4860:4860::8888".parse().unwrap()
        ));
    }

    #[test]
    fn test_is_private_hostname_direct() {
        assert!(is_private_hostname("localhost"));
        assert!(is_private_hostname("LOCALHOST"));
        assert!(is_private_hostname("db.internal"));
        assert!(is_private_hostname("server.local"));

        assert!(!is_private_hostname("example.com"));
        assert!(!is_private_hostname("api.worldcoin.org"));
    }

    #[test]
    fn test_extract_host() {
        assert_eq!(extract_host("https://example.com/path"), Some("example.com"));
        assert_eq!(extract_host("https://10.0.0.1:8080/path"), Some("10.0.0.1"));
        assert_eq!(extract_host("https://[::1]:443/path"), Some("::1"));
        assert_eq!(extract_host("https://user:pass@example.com/path"), Some("example.com"));
        assert_eq!(extract_host("ipfs://QmHash/path"), Some("QmHash"));
    }

    // ===== Data URIs and IPFS are allowed (no host to SSRF-check) =====

    #[test]
    fn test_data_uri_allowed() {
        let v = Validator::new();
        assert!(v
            .validate_url("data:application/octet-stream;base64,dGVzdA==")
            .is_ok());
    }

    #[test]
    fn test_ipfs_allowed() {
        let v = Validator::new();
        assert!(v.validate_url("ipfs://QmTestHash123").is_ok());
    }
}
