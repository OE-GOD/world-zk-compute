//! TLS/mTLS configuration for the verifier API.
//!
//! When TLS is enabled via environment variables, the server binds with
//! `axum_server::tls_rustls` for encrypted connections.
//!
//! ## Environment Variables
//!
//! - `TLS_CERT_PATH`: Path to PEM certificate file
//! - `TLS_KEY_PATH`: Path to PEM private key file
//! - `TLS_CA_PATH`: Path to CA certificate for client verification (mTLS)
//!
//! When `TLS_CA_PATH` is set, the server requires client certificates
//! signed by the specified CA (mutual TLS).

use std::path::PathBuf;

/// TLS configuration parsed from environment.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to server certificate (PEM).
    pub cert_path: PathBuf,
    /// Path to server private key (PEM).
    pub key_path: PathBuf,
    /// Optional CA cert for mutual TLS client verification.
    pub ca_path: Option<PathBuf>,
}

impl TlsConfig {
    /// Load TLS config from environment variables.
    ///
    /// Returns `None` if `TLS_CERT_PATH` is not set (TLS disabled).
    /// Returns `Err` if cert path is set but key path is missing.
    pub fn from_env() -> Result<Option<Self>, String> {
        let cert_path = match std::env::var("TLS_CERT_PATH") {
            Ok(p) if !p.is_empty() => PathBuf::from(p),
            _ => return Ok(None),
        };

        let key_path = std::env::var("TLS_KEY_PATH")
            .map(PathBuf::from)
            .map_err(|_| "TLS_CERT_PATH set but TLS_KEY_PATH is missing".to_string())?;

        let ca_path = std::env::var("TLS_CA_PATH").ok().map(PathBuf::from);

        Ok(Some(Self {
            cert_path,
            key_path,
            ca_path,
        }))
    }

    /// Whether mutual TLS (client certificate verification) is enabled.
    pub fn is_mtls(&self) -> bool {
        self.ca_path.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_disabled_by_default() {
        // Clear env vars for this test
        std::env::remove_var("TLS_CERT_PATH");
        std::env::remove_var("TLS_KEY_PATH");
        std::env::remove_var("TLS_CA_PATH");

        let config = TlsConfig::from_env().unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn test_tls_config_parsing() {
        let config = TlsConfig {
            cert_path: PathBuf::from("/certs/server.pem"),
            key_path: PathBuf::from("/certs/server-key.pem"),
            ca_path: None,
        };
        assert!(!config.is_mtls());
    }

    #[test]
    fn test_mtls_config() {
        let config = TlsConfig {
            cert_path: PathBuf::from("/certs/server.pem"),
            key_path: PathBuf::from("/certs/server-key.pem"),
            ca_path: Some(PathBuf::from("/certs/ca.pem")),
        };
        assert!(config.is_mtls());
    }
}
