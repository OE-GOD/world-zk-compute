//! SSRF (Server-Side Request Forgery) Protection
//!
//! Validates URLs to prevent requests to internal/private network resources.
//! This is critical security hardening for any service that fetches URLs
//! derived from on-chain events or user input, as an attacker can craft
//! events pointing to internal services (e.g., AWS metadata at
//! 169.254.169.254, internal APIs on 10.x.x.x, etc.).
//!
//! ## Blocked Ranges
//!
//! - IPv4 loopback: `127.0.0.0/8`
//! - IPv4 private: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
//! - IPv4 link-local: `169.254.0.0/16`
//! - IPv4 broadcast: `255.255.255.255`
//! - IPv4 unspecified: `0.0.0.0`
//! - IPv6 loopback: `::1`
//! - IPv6 link-local: `fe80::/10`
//! - IPv6 unique local: `fc00::/7`
//! - IPv6 unspecified: `::`
//! - Private hostnames: `localhost`, `*.local`, `*.internal`, `*.lan`
//!
//! ## Design
//!
//! All blocklists are compile-time constants to prevent runtime configuration
//! bypass attacks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Allowed URL schemes (compile-time constant).
/// Only HTTP and HTTPS are permitted for outbound requests.
const ALLOWED_SCHEMES: &[&str] = &["http", "https"];

/// Private/reserved hostnames that must always be blocked.
/// These are checked case-insensitively.
const BLOCKED_HOSTNAMES: &[&str] = &["localhost", "localhost.localdomain"];

/// Domain suffixes that indicate private/internal hosts.
/// Checked case-insensitively.
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

/// Maximum URL length to prevent resource exhaustion.
const MAX_URL_LENGTH: usize = 2048;

/// Errors returned by SSRF URL validation.
#[derive(Debug, Clone, PartialEq)]
pub enum SsrfError {
    /// URL is empty.
    EmptyUrl,
    /// URL exceeds maximum length.
    TooLong { len: usize, max: usize },
    /// URL scheme is not in the allow list.
    DisallowedScheme(String),
    /// URL has no host component.
    MissingHost,
    /// Host resolves to or is a private/reserved IP address.
    PrivateIp(String),
    /// Host is a known private/internal hostname.
    PrivateHostname(String),
    /// URL failed to parse.
    ParseError(String),
    /// URL contains control characters.
    ControlCharacters,
}

impl std::fmt::Display for SsrfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SsrfError::EmptyUrl => write!(f, "URL is empty"),
            SsrfError::TooLong { len, max } => {
                write!(f, "URL too long: {} chars (max: {})", len, max)
            }
            SsrfError::DisallowedScheme(scheme) => {
                write!(
                    f,
                    "URL scheme '{}' is not allowed; permitted schemes: {:?}",
                    scheme, ALLOWED_SCHEMES
                )
            }
            SsrfError::MissingHost => write!(f, "URL has no host component"),
            SsrfError::PrivateIp(ip) => {
                write!(f, "URL host '{}' resolves to a private/reserved IP", ip)
            }
            SsrfError::PrivateHostname(host) => {
                write!(f, "URL host '{}' is a private/internal hostname", host)
            }
            SsrfError::ParseError(msg) => write!(f, "URL parse error: {}", msg),
            SsrfError::ControlCharacters => write!(f, "URL contains control characters"),
        }
    }
}

impl std::error::Error for SsrfError {}

/// Check whether an IPv4 address falls within private/reserved ranges.
///
/// Blocked ranges:
/// - `0.0.0.0` (unspecified)
/// - `127.0.0.0/8` (loopback)
/// - `10.0.0.0/8` (private, RFC 1918)
/// - `172.16.0.0/12` (private, RFC 1918)
/// - `192.168.0.0/16` (private, RFC 1918)
/// - `169.254.0.0/16` (link-local, RFC 3927)
/// - `100.64.0.0/10` (shared address space, RFC 6598 / CGNAT)
/// - `192.0.0.0/24` (IETF protocol assignments)
/// - `192.0.2.0/24` (TEST-NET-1)
/// - `198.51.100.0/24` (TEST-NET-2)
/// - `203.0.113.0/24` (TEST-NET-3)
/// - `198.18.0.0/15` (benchmarking)
/// - `240.0.0.0/4` (reserved)
/// - `255.255.255.255` (broadcast)
fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();

    // Unspecified: 0.0.0.0
    if ip.is_unspecified() {
        return true;
    }

    // Loopback: 127.0.0.0/8
    if ip.is_loopback() {
        return true;
    }

    // Private RFC 1918: 10.0.0.0/8
    if octets[0] == 10 {
        return true;
    }

    // Private RFC 1918: 172.16.0.0/12
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }

    // Private RFC 1918: 192.168.0.0/16
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }

    // Link-local: 169.254.0.0/16 (includes AWS metadata endpoint 169.254.169.254)
    if ip.is_link_local() {
        return true;
    }

    // Shared address space (CGNAT): 100.64.0.0/10
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }

    // IETF protocol assignments: 192.0.0.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return true;
    }

    // Documentation (TEST-NET-1): 192.0.2.0/24
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }

    // Documentation (TEST-NET-2): 198.51.100.0/24
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }

    // Documentation (TEST-NET-3): 203.0.113.0/24
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }

    // Benchmarking: 198.18.0.0/15
    if octets[0] == 198 && (18..=19).contains(&octets[1]) {
        return true;
    }

    // Reserved: 240.0.0.0/4
    if octets[0] >= 240 {
        return true;
    }

    // Broadcast: 255.255.255.255 (already covered by 240.0.0.0/4 above,
    // but explicit for clarity)
    if ip.is_broadcast() {
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
/// - `::ffff:0:0/96` (IPv4-mapped — checked via the mapped IPv4 address)
fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    // Unspecified: ::
    if ip.is_unspecified() {
        return true;
    }

    // Loopback: ::1
    if ip.is_loopback() {
        return true;
    }

    let segments = ip.segments();

    // Link-local: fe80::/10
    if segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }

    // Unique local: fc00::/7
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }

    // IPv4-mapped IPv6: ::ffff:x.x.x.x — check the embedded IPv4 address
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

    // Exact match against blocked hostnames
    for blocked in BLOCKED_HOSTNAMES {
        if lower == *blocked {
            return true;
        }
    }

    // Suffix match against blocked domain suffixes
    for suffix in BLOCKED_HOSTNAME_SUFFIXES {
        if lower.ends_with(suffix) {
            return true;
        }
    }

    false
}

/// Validate a URL for SSRF safety.
///
/// This function checks:
/// 1. URL is non-empty and within length limits
/// 2. URL does not contain control characters
/// 3. URL scheme is in the allow list (http, https only)
/// 4. URL has a host component
/// 5. Host is not a known private/internal hostname
/// 6. If the host is an IP literal, it is not in a private/reserved range
///
/// For hostname-based URLs, this performs a static check against known
/// private hostnames. For full protection against DNS rebinding attacks,
/// the actual HTTP client should also validate the resolved IP at
/// connection time.
///
/// Returns `Ok(())` on success, or `Err(SsrfError)` describing the violation.
pub fn validate_url_ssrf(url: &str) -> Result<(), SsrfError> {
    let url = url.trim();

    // 1. Empty check
    if url.is_empty() {
        return Err(SsrfError::EmptyUrl);
    }

    // 2. Length check
    if url.len() > MAX_URL_LENGTH {
        return Err(SsrfError::TooLong {
            len: url.len(),
            max: MAX_URL_LENGTH,
        });
    }

    // 3. Control characters
    if url.chars().any(|c| c.is_control()) {
        return Err(SsrfError::ControlCharacters);
    }

    // 4. Parse the URL to extract scheme and host.
    //    We use a simple manual parser to avoid adding the `url` crate dependency.
    let (scheme, rest) = url.split_once("://").ok_or_else(|| {
        SsrfError::ParseError("missing '://' separator".to_string())
    })?;

    // 5. Scheme check
    let scheme_lower = scheme.to_lowercase();
    if !ALLOWED_SCHEMES.contains(&scheme_lower.as_str()) {
        return Err(SsrfError::DisallowedScheme(scheme_lower));
    }

    // 6. Extract host (strip path, query, fragment, and userinfo)
    // Remove path/query/fragment
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.split('?').next().unwrap_or(authority);
    let authority = authority.split('#').next().unwrap_or(authority);

    // Remove userinfo (user:pass@)
    let host_port = if let Some((_userinfo, hp)) = authority.rsplit_once('@') {
        hp
    } else {
        authority
    };

    if host_port.is_empty() {
        return Err(SsrfError::MissingHost);
    }

    // Extract just the host, handling IPv6 bracket notation [::1]:port
    let host = if host_port.starts_with('[') {
        // IPv6 literal: [addr]:port or [addr]
        let end_bracket = host_port
            .find(']')
            .ok_or_else(|| SsrfError::ParseError("unclosed IPv6 bracket".to_string()))?;
        &host_port[1..end_bracket]
    } else {
        // IPv4 or hostname: host:port or host
        host_port.rsplit_once(':').map_or(host_port, |(h, _)| h)
    };

    if host.is_empty() {
        return Err(SsrfError::MissingHost);
    }

    // 7. Check if host is an IP literal
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(SsrfError::PrivateIp(ip.to_string()));
        }
    } else {
        // Host is a hostname -- check against private hostname blocklist
        if is_private_hostname(host) {
            return Err(SsrfError::PrivateHostname(host.to_string()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Allowed URLs =====

    #[test]
    fn test_allows_public_https_url() {
        assert!(validate_url_ssrf("https://example.com/data").is_ok());
    }

    #[test]
    fn test_allows_public_http_url() {
        assert!(validate_url_ssrf("http://example.com/path?q=1").is_ok());
    }

    #[test]
    fn test_allows_public_ipv4() {
        assert!(validate_url_ssrf("https://8.8.8.8/dns-query").is_ok());
    }

    #[test]
    fn test_allows_public_ipv4_with_port() {
        assert!(validate_url_ssrf("https://93.184.216.34:443/page").is_ok());
    }

    #[test]
    fn test_allows_public_ipv6() {
        // 2001:4860:4860::8888 is Google Public DNS
        assert!(validate_url_ssrf("https://[2001:4860:4860::8888]/path").is_ok());
    }

    #[test]
    fn test_allows_public_hostname_with_port() {
        assert!(validate_url_ssrf("https://api.example.com:8443/v1/data").is_ok());
    }

    // ===== Blocked: schemes =====

    #[test]
    fn test_blocks_ftp_scheme() {
        let err = validate_url_ssrf("ftp://example.com/file").unwrap_err();
        assert!(matches!(err, SsrfError::DisallowedScheme(_)));
    }

    #[test]
    fn test_blocks_file_scheme() {
        let err = validate_url_ssrf("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, SsrfError::DisallowedScheme(_)));
    }

    #[test]
    fn test_blocks_gopher_scheme() {
        let err = validate_url_ssrf("gopher://evil.com/1").unwrap_err();
        assert!(matches!(err, SsrfError::DisallowedScheme(_)));
    }

    #[test]
    fn test_blocks_data_scheme() {
        let err =
            validate_url_ssrf("data:text/html,<script>alert(1)</script>").unwrap_err();
        // data: has no "://" so it will fail as a parse error
        assert!(
            matches!(err, SsrfError::ParseError(_)),
            "Expected ParseError, got: {:?}",
            err
        );
    }

    #[test]
    fn test_blocks_javascript_scheme() {
        let err = validate_url_ssrf("javascript:alert(1)").unwrap_err();
        assert!(matches!(err, SsrfError::ParseError(_)));
    }

    // ===== Blocked: IPv4 private ranges =====

    #[test]
    fn test_blocks_loopback_127_0_0_1() {
        let err = validate_url_ssrf("https://127.0.0.1/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_loopback_127_x() {
        let err = validate_url_ssrf("https://127.255.0.1/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_10_x() {
        let err = validate_url_ssrf("https://10.0.0.1/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_10_255_x() {
        let err = validate_url_ssrf("https://10.255.255.255/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_172_16_x() {
        let err = validate_url_ssrf("https://172.16.0.1/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_172_31_x() {
        let err = validate_url_ssrf("https://172.31.255.255/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_allows_172_32_x() {
        // 172.32.x.x is public (outside the 172.16-31 range)
        assert!(validate_url_ssrf("https://172.32.0.1/api").is_ok());
    }

    #[test]
    fn test_blocks_192_168_x() {
        let err = validate_url_ssrf("https://192.168.1.1/admin").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_link_local_169_254() {
        // AWS metadata endpoint
        let err =
            validate_url_ssrf("https://169.254.169.254/latest/meta-data/").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_link_local_169_254_other() {
        let err = validate_url_ssrf("https://169.254.0.1/something").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_broadcast() {
        let err = validate_url_ssrf("https://255.255.255.255/x").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_unspecified_0_0_0_0() {
        let err = validate_url_ssrf("https://0.0.0.0/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_cgnat_100_64() {
        let err = validate_url_ssrf("https://100.64.0.1/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_cgnat_100_127() {
        let err = validate_url_ssrf("https://100.127.255.255/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_allows_100_128() {
        // 100.128.x.x is outside CGNAT range
        assert!(validate_url_ssrf("https://100.128.0.1/api").is_ok());
    }

    #[test]
    fn test_blocks_reserved_240() {
        let err = validate_url_ssrf("https://240.0.0.1/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    // ===== Blocked: IPv6 private ranges =====

    #[test]
    fn test_blocks_ipv6_loopback() {
        let err = validate_url_ssrf("https://[::1]/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_ipv6_unspecified() {
        let err = validate_url_ssrf("https://[::]/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_ipv6_link_local() {
        let err = validate_url_ssrf("https://[fe80::1]/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_ipv6_unique_local() {
        let err = validate_url_ssrf("https://[fd00::1]/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_ipv4_mapped_ipv6_private() {
        // ::ffff:127.0.0.1 is an IPv4-mapped IPv6 address
        let err = validate_url_ssrf("https://[::ffff:127.0.0.1]/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_ipv4_mapped_ipv6_10_x() {
        let err = validate_url_ssrf("https://[::ffff:10.0.0.1]/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    // ===== Blocked: private hostnames =====

    #[test]
    fn test_blocks_localhost() {
        let err = validate_url_ssrf("https://localhost/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_localhost_with_port() {
        let err = validate_url_ssrf("https://localhost:8080/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_localhost_case_insensitive() {
        let err = validate_url_ssrf("https://LOCALHOST/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_localhost_localdomain() {
        let err = validate_url_ssrf("https://localhost.localdomain/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_dot_local_suffix() {
        let err = validate_url_ssrf("https://myhost.local/api").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_dot_internal_suffix() {
        let err = validate_url_ssrf("https://metadata.internal/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_dot_lan_suffix() {
        let err = validate_url_ssrf("https://printer.lan/status").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    #[test]
    fn test_blocks_dot_localhost_suffix() {
        let err = validate_url_ssrf("https://anything.localhost/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateHostname(_)));
    }

    // ===== Edge cases =====

    #[test]
    fn test_empty_url() {
        let err = validate_url_ssrf("").unwrap_err();
        assert!(matches!(err, SsrfError::EmptyUrl));
    }

    #[test]
    fn test_whitespace_only() {
        let err = validate_url_ssrf("   ").unwrap_err();
        assert!(matches!(err, SsrfError::EmptyUrl));
    }

    #[test]
    fn test_url_too_long() {
        let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
        let err = validate_url_ssrf(&long_url).unwrap_err();
        assert!(matches!(err, SsrfError::TooLong { .. }));
    }

    #[test]
    fn test_control_characters() {
        let err = validate_url_ssrf("https://example.com/path\x00").unwrap_err();
        assert!(matches!(err, SsrfError::ControlCharacters));
    }

    #[test]
    fn test_no_scheme() {
        let err = validate_url_ssrf("example.com/path").unwrap_err();
        assert!(matches!(err, SsrfError::ParseError(_)));
    }

    #[test]
    fn test_scheme_only() {
        let err = validate_url_ssrf("https://").unwrap_err();
        assert!(matches!(err, SsrfError::MissingHost));
    }

    #[test]
    fn test_ipv4_with_port() {
        let err = validate_url_ssrf("https://127.0.0.1:8080/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_ipv6_with_port() {
        let err = validate_url_ssrf("https://[::1]:443/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_userinfo_bypass_attempt() {
        // Attempt to hide IP in userinfo: https://evil.com@10.0.0.1/
        let err = validate_url_ssrf("https://evil.com@10.0.0.1/path").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    // ===== Helper function tests =====

    #[test]
    fn test_is_private_ip_public_v4() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(!is_private_ip(&ip));
    }

    #[test]
    fn test_is_private_ip_loopback_v4() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(is_private_ip(&ip));
    }

    #[test]
    fn test_is_private_ip_public_v6() {
        let ip: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(!is_private_ip(&ip));
    }

    #[test]
    fn test_is_private_ip_loopback_v6() {
        let ip: IpAddr = "::1".parse().unwrap();
        assert!(is_private_ip(&ip));
    }

    #[test]
    fn test_is_private_hostname_false_for_public() {
        assert!(!is_private_hostname("example.com"));
        assert!(!is_private_hostname("api.worldcoin.org"));
    }

    #[test]
    fn test_is_private_hostname_true_for_localhost() {
        assert!(is_private_hostname("localhost"));
        assert!(is_private_hostname("LOCALHOST"));
    }

    #[test]
    fn test_is_private_hostname_true_for_suffixes() {
        assert!(is_private_hostname("db.internal"));
        assert!(is_private_hostname("server.local"));
        assert!(is_private_hostname("myhost.lan"));
    }

    // ===== SSRF bypass attempt tests =====

    #[test]
    fn test_blocks_aws_metadata_http() {
        let err =
            validate_url_ssrf("http://169.254.169.254/latest/meta-data/iam/").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_gce_metadata() {
        // GCE metadata uses 169.254.169.254 with Metadata-Flavor header
        let err =
            validate_url_ssrf("http://169.254.169.254/computeMetadata/v1/").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_azure_metadata() {
        // Azure metadata: 169.254.169.254
        let err = validate_url_ssrf(
            "http://169.254.169.254/metadata/instance?api-version=2021-02-01",
        )
        .unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }

    #[test]
    fn test_blocks_docker_host() {
        // Docker default bridge: 172.17.0.1
        let err = validate_url_ssrf("http://172.17.0.1:2375/containers/json").unwrap_err();
        assert!(matches!(err, SsrfError::PrivateIp(_)));
    }
}
