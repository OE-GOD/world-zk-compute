use std::env;

/// Service configuration loaded from environment variables.
///
/// ## Environment Variables
///
/// | Variable | Description | Default |
/// |---|---|---|
/// | `VERIFIER_API_KEYS` | Comma-separated list of valid API keys (legacy) | (none, auth disabled) |
/// | `RATE_LIMIT_RPM` | Default requests per minute per key/IP | 100 |
/// | `PORT` | Server listen port | 3000 |
/// | `CIRCUIT_TTL_SECS` | Circuit registration TTL in seconds (0 = no expiry) | 0 |
/// | `VERIFIER_ADMIN_KEY` | Admin API key for tenant management | (none, admin disabled) |
/// | `VERIFIER_TENANT_FILE` | Path to tenant JSON file for persistence (see below) | (none, in-memory) |
/// | `VERIFIER_TLS_CERT` | Path to PEM server certificate (enables TLS) | (none, TLS disabled) |
/// | `VERIFIER_TLS_KEY` | Path to PEM server private key (required when cert is set) | (none) |
/// | `VERIFIER_TLS_CLIENT_CA` | Path to PEM CA cert for client verification (enables mTLS) | (none) |
///
/// `VERIFIER_TENANT_FILE`: a missing file starts with no tenants. A file that
/// exists but cannot be read or parsed stops start-up; that includes an empty
/// file, so a pre-created one must contain `{"tenants":[]}`. Each save writes a
/// temporary file next to it (mode 0600) and renames it over the tenant file,
/// so the directory must be writable by the service.
///
/// TLS-related env vars are handled by [`crate::tls::TlsConfig`].
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    /// Allowed API keys (legacy). If empty, authentication is disabled (open access)
    /// unless tenants are configured.
    pub api_keys: Vec<String>,
    /// Default rate limit in requests per minute. Default: 100.
    /// Per-tenant rate limits override this.
    pub rate_limit_rpm: u32,
    /// Server port. Default: 3000.
    pub port: u16,
    /// Circuit registration TTL in seconds. 0 = no expiry. Default: 0.
    pub circuit_ttl_secs: u64,
    /// Admin API key for tenant management endpoints.
    /// If empty, admin endpoints are disabled.
    pub admin_key: String,
    /// Path to tenant JSON file for persistence.
    /// If empty, tenants are stored in memory only.
    pub tenant_file: String,
}

impl ServiceConfig {
    /// Load configuration from environment variables.
    ///
    /// - `VERIFIER_API_KEYS`: comma-separated list of valid API keys (legacy mode).
    ///   If unset or empty, authentication is disabled (unless tenants exist).
    /// - `RATE_LIMIT_RPM`: default requests per minute per key/IP. Default: 100.
    /// - `PORT`: server listen port. Default: 3000.
    /// - `CIRCUIT_TTL_SECS`: circuit registration TTL in seconds. Default: 0 (no expiry).
    /// - `VERIFIER_ADMIN_KEY`: admin key for /admin/* endpoints.
    /// - `VERIFIER_TENANT_FILE`: path to JSON file for tenant persistence.
    ///
    /// TLS configuration is loaded separately via [`crate::tls::TlsConfig::from_env()`].
    pub fn from_env() -> Self {
        Self::from_vars(|name| env::var(name).ok())
    }

    /// Build the configuration from variable values supplied by `var`
    /// (`None` = not set).
    pub(crate) fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let api_keys = var("VERIFIER_API_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let rate_limit_rpm = var("RATE_LIMIT_RPM")
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        let port = var("PORT").and_then(|v| v.parse().ok()).unwrap_or(3000);

        let circuit_ttl_secs = var("CIRCUIT_TTL_SECS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let admin_key = var("VERIFIER_ADMIN_KEY").unwrap_or_default();

        let tenant_file = var("VERIFIER_TENANT_FILE").unwrap_or_default();

        Self {
            api_keys,
            rate_limit_rpm,
            port,
            circuit_ttl_secs,
            admin_key,
            tenant_file,
        }
    }

    /// Returns true if legacy API key authentication is enabled (at least one key configured).
    pub fn auth_enabled(&self) -> bool {
        !self.api_keys.is_empty()
    }

    /// Returns true if the admin API is enabled (admin key is set).
    pub fn admin_enabled(&self) -> bool {
        !self.admin_key.is_empty()
    }

    /// Returns true if file-based tenant persistence is configured.
    pub fn tenant_persistence_enabled(&self) -> bool {
        !self.tenant_file.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        // With no env vars set, we get defaults
        let config = ServiceConfig::from_vars(|_| None);
        assert!(config.api_keys.is_empty());
        assert!(!config.auth_enabled());
        assert_eq!(config.rate_limit_rpm, 100);
        assert_eq!(config.port, 3000);
        assert_eq!(config.circuit_ttl_secs, 0);
        assert!(config.admin_key.is_empty());
        assert!(!config.admin_enabled());
        assert!(config.tenant_file.is_empty());
        assert!(!config.tenant_persistence_enabled());
    }

    #[test]
    fn test_admin_config() {
        // Test admin config directly (env var manipulation is unsafe in parallel tests)
        let config = ServiceConfig {
            api_keys: vec![],
            rate_limit_rpm: 100,
            port: 3000,
            circuit_ttl_secs: 0,
            admin_key: "super-secret-admin".to_string(),
            tenant_file: "/data/tenants.json".to_string(),
        };
        assert_eq!(config.admin_key, "super-secret-admin");
        assert!(config.admin_enabled());
        assert_eq!(config.tenant_file, "/data/tenants.json");
        assert!(config.tenant_persistence_enabled());
    }

    #[test]
    fn test_admin_disabled() {
        let config = ServiceConfig {
            api_keys: vec![],
            rate_limit_rpm: 100,
            port: 3000,
            circuit_ttl_secs: 0,
            admin_key: String::new(),
            tenant_file: String::new(),
        };
        assert!(!config.admin_enabled());
        assert!(!config.tenant_persistence_enabled());
    }
}
