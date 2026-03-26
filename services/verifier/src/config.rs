use std::env;

/// Service configuration loaded from environment variables.
///
/// ## Environment Variables
///
/// | Variable | Description | Default |
/// |---|---|---|
/// | `VERIFIER_API_KEYS` | Comma-separated list of valid API keys | (none, auth disabled) |
/// | `RATE_LIMIT_RPM` | Requests per minute per key/IP | 100 |
/// | `PORT` | Server listen port | 3000 |
/// | `CIRCUIT_TTL_SECS` | Circuit registration TTL in seconds (0 = no expiry) | 0 |
/// | `VERIFIER_TLS_CERT` | Path to PEM server certificate (enables TLS) | (none, TLS disabled) |
/// | `VERIFIER_TLS_KEY` | Path to PEM server private key (required when cert is set) | (none) |
/// | `VERIFIER_TLS_CLIENT_CA` | Path to PEM CA cert for client verification (enables mTLS) | (none) |
///
/// TLS-related env vars are handled by [`crate::tls::TlsConfig`].
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    /// Allowed API keys. If empty, authentication is disabled (open access).
    pub api_keys: Vec<String>,
    /// Rate limit in requests per minute. Default: 100.
    pub rate_limit_rpm: u32,
    /// Server port. Default: 3000.
    pub port: u16,
    /// Circuit registration TTL in seconds. 0 = no expiry. Default: 0.
    pub circuit_ttl_secs: u64,
}

impl ServiceConfig {
    /// Load configuration from environment variables.
    ///
    /// - `VERIFIER_API_KEYS`: comma-separated list of valid API keys.
    ///   If unset or empty, authentication is disabled.
    /// - `RATE_LIMIT_RPM`: requests per minute per key/IP. Default: 100.
    /// - `PORT`: server listen port. Default: 3000.
    /// - `CIRCUIT_TTL_SECS`: circuit registration TTL in seconds. Default: 0 (no expiry).
    ///
    /// TLS configuration is loaded separately via [`crate::tls::TlsConfig::from_env()`].
    pub fn from_env() -> Self {
        let api_keys = env::var("VERIFIER_API_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let rate_limit_rpm = env::var("RATE_LIMIT_RPM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        let port = env::var("PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);

        let circuit_ttl_secs = env::var("CIRCUIT_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        Self {
            api_keys,
            rate_limit_rpm,
            port,
            circuit_ttl_secs,
        }
    }

    /// Returns true if authentication is enabled (at least one API key configured).
    pub fn auth_enabled(&self) -> bool {
        !self.api_keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        // With no env vars set, we get defaults
        // (Clear any that might be set in the test environment)
        env::remove_var("VERIFIER_API_KEYS");
        env::remove_var("RATE_LIMIT_RPM");
        env::remove_var("PORT");
        env::remove_var("CIRCUIT_TTL_SECS");

        let config = ServiceConfig::from_env();
        assert!(config.api_keys.is_empty());
        assert!(!config.auth_enabled());
        assert_eq!(config.rate_limit_rpm, 100);
        assert_eq!(config.port, 3000);
        assert_eq!(config.circuit_ttl_secs, 0);
    }
}
