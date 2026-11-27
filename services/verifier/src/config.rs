use std::env;
use std::ffi::OsString;
use std::fmt::Display;
use std::str::FromStr;

/// Service configuration loaded from environment variables.
///
/// ## Environment Variables
///
/// | Variable | Description | Default |
/// |---|---|---|
/// | `VERIFIER_API_KEYS` | Comma-separated list of valid API keys (legacy); at least one key if non-empty | (none, auth disabled) |
/// | `RATE_LIMIT_RPM` | Default requests per minute per key/IP (whole number) | 100 |
/// | `PORT` | Server listen port (0 to 65535) | 3000 |
/// | `CIRCUIT_TTL_SECS` | Circuit registration TTL in seconds (whole number, 0 = no expiry) | 0 |
/// | `VERIFIER_ADMIN_KEY` | Admin API key for tenant management | (none, admin disabled) |
/// | `VERIFIER_TENANT_FILE` | Path to tenant JSON file for persistence (see below) | (none, in-memory) |
/// | `VERIFIER_TLS_CERT` | Path to PEM server certificate (enables TLS; required when key or client CA is set) | (none, TLS disabled) |
/// | `VERIFIER_TLS_KEY` | Path to PEM server private key (required when cert is set) | (none) |
/// | `VERIFIER_TLS_CLIENT_CA` | Path to PEM CA cert for client verification (enables mTLS) | (none) |
///
/// An unset or empty variable keeps the default. `RATE_LIMIT_RPM`, `PORT` or
/// `CIRCUIT_TTL_SECS` set to anything else that does not parse, a non-empty
/// `VERIFIER_API_KEYS` that holds no key, and a `VERIFIER_API_KEYS`,
/// `VERIFIER_ADMIN_KEY` or `VERIFIER_TENANT_FILE` that is not valid UTF-8, stop
/// start-up. Spaces around a number and units (`60s`) do not parse.
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
    ///
    /// Returns `Err` if a numeric variable is set, non-empty and does not parse,
    /// if `VERIFIER_API_KEYS` is non-empty but holds no key, or if
    /// `VERIFIER_API_KEYS`, `VERIFIER_ADMIN_KEY` or `VERIFIER_TENANT_FILE` is
    /// not valid UTF-8.
    pub fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| env::var_os(name))
    }

    /// Build the configuration from variable values supplied by `var`
    /// (`None` = not set).
    pub(crate) fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, String> {
        let api_keys = parse_api_keys(parse_text(&var, "VERIFIER_API_KEYS")?)?;
        let rate_limit_rpm = parse_number(&var, "RATE_LIMIT_RPM", 100)?;
        let port = parse_number(&var, "PORT", 3000)?;
        let circuit_ttl_secs = parse_number(&var, "CIRCUIT_TTL_SECS", 0)?;
        let admin_key = parse_text(&var, "VERIFIER_ADMIN_KEY")?;
        let tenant_file = parse_text(&var, "VERIFIER_TENANT_FILE")?;

        Ok(Self {
            api_keys,
            rate_limit_rpm,
            port,
            circuit_ttl_secs,
            admin_key,
            tenant_file,
        })
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

/// The text in the variable `name`, or an empty string if it is unset. The
/// value can be a secret, so the error does not repeat it.
fn parse_text(var: impl Fn(&str) -> Option<OsString>, name: &str) -> Result<String, String> {
    var(name)
        .unwrap_or_default()
        .into_string()
        .map_err(|_| format!("{name} is not valid UTF-8"))
}

/// Split `VERIFIER_API_KEYS` into keys. The value is a secret, so the error
/// does not repeat it.
fn parse_api_keys(value: String) -> Result<Vec<String>, String> {
    let keys: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if keys.is_empty() && !value.is_empty() {
        return Err("VERIFIER_API_KEYS is set but holds no key".into());
    }
    Ok(keys)
}

/// The number in the variable `name`, or `default` if it is unset or empty.
fn parse_number<T>(
    var: impl Fn(&str) -> Option<OsString>,
    name: &str,
    default: T,
) -> Result<T, String>
where
    T: FromStr,
    T::Err: Display,
{
    let value = var(name).unwrap_or_default();
    if value.is_empty() {
        return Ok(default);
    }
    let value = value.to_string_lossy();
    value
        .parse()
        .map_err(|e| format!("{name} has an invalid value {value:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the configuration with only the variable `name` set.
    fn with_var(name: &str, value: impl Into<OsString>) -> Result<ServiceConfig, String> {
        let value = value.into();
        ServiceConfig::from_vars(|n| (n == name).then(|| value.clone()))
    }

    #[test]
    fn test_default_config() {
        // With no env vars set, we get defaults
        let config = ServiceConfig::from_vars(|_| None).unwrap();
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
    fn test_empty_variables_keep_the_defaults() {
        let config = ServiceConfig::from_vars(|_| Some(OsString::new())).unwrap();
        assert!(!config.auth_enabled());
        assert_eq!(config.rate_limit_rpm, 100);
        assert_eq!(config.port, 3000);
        assert_eq!(config.circuit_ttl_secs, 0);
    }

    #[test]
    fn test_set_variables_are_used() {
        assert_eq!(with_var("RATE_LIMIT_RPM", "7").unwrap().rate_limit_rpm, 7);
        assert_eq!(with_var("PORT", "65535").unwrap().port, 65535);
        let config = with_var("CIRCUIT_TTL_SECS", "60").unwrap();
        assert_eq!(config.circuit_ttl_secs, 60);
        let config = with_var("VERIFIER_API_KEYS", " a ,, b,").unwrap();
        assert_eq!(config.api_keys, ["a", "b"]);
    }

    #[test]
    fn test_unparseable_number_is_an_error() {
        for (name, value) in [
            ("RATE_LIMIT_RPM", "lots"),
            ("RATE_LIMIT_RPM", "-1"),
            ("RATE_LIMIT_RPM", "4294967296"),
            ("RATE_LIMIT_RPM", " "),
            ("PORT", "abc"),
            ("PORT", "65536"),
            ("PORT", " 3000"),
            ("CIRCUIT_TTL_SECS", "60s"),
            ("CIRCUIT_TTL_SECS", "1.5"),
        ] {
            let err = with_var(name, value).expect_err(name);
            assert!(
                err.starts_with(&format!("{name} has an invalid value {value:?}: ")),
                "{err}"
            );
        }
    }

    #[test]
    fn test_api_keys_without_a_key_is_an_error() {
        for value in [" , ", ",", " "] {
            let err = with_var("VERIFIER_API_KEYS", value).expect_err(value);
            assert_eq!(err, "VERIFIER_API_KEYS is set but holds no key");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_non_utf8_value_is_an_error() {
        use std::os::unix::ffi::OsStringExt;
        let value = || OsString::from_vec(vec![b'1', 0xff]);
        let err = with_var("PORT", value()).expect_err("PORT");
        assert!(err.starts_with("PORT has an invalid value "), "{err}");
        for name in [
            "VERIFIER_API_KEYS",
            "VERIFIER_ADMIN_KEY",
            "VERIFIER_TENANT_FILE",
        ] {
            let err = with_var(name, value()).expect_err(name);
            assert_eq!(err, format!("{name} is not valid UTF-8"));
        }
    }

    #[test]
    fn test_admin_key_and_tenant_file_are_read() {
        let config = with_var("VERIFIER_ADMIN_KEY", "admin-secret").unwrap();
        assert_eq!(config.admin_key, "admin-secret");
        let config = with_var("VERIFIER_TENANT_FILE", "/data/tenants.json").unwrap();
        assert_eq!(config.tenant_file, "/data/tenants.json");
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
