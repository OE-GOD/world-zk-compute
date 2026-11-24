//! TLS/mTLS configuration for the verifier API.
//!
//! When TLS is enabled via environment variables, the server binds with
//! `axum_server::tls_rustls` for encrypted connections. Optional mutual TLS
//! (mTLS) requires clients to present certificates signed by a trusted CA.
//!
//! ## Environment Variables
//!
//! - `VERIFIER_TLS_CERT`: Path to PEM-encoded server certificate file (required for TLS)
//! - `VERIFIER_TLS_KEY`: Path to PEM-encoded server private key file (required for TLS)
//! - `VERIFIER_TLS_CLIENT_CA`: Path to PEM-encoded CA certificate for client verification (mTLS)
//!
//! When `VERIFIER_TLS_CLIENT_CA` is set, the server requires client certificates
//! signed by the specified CA (mutual TLS).

use axum_server::tls_rustls::RustlsConfig;
use std::path::PathBuf;

/// TLS configuration parsed from environment variables.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to PEM-encoded server certificate.
    pub cert_path: PathBuf,
    /// Path to PEM-encoded server private key.
    pub key_path: PathBuf,
    /// Optional path to PEM-encoded CA certificate for mutual TLS client verification.
    pub client_ca_path: Option<PathBuf>,
}

impl TlsConfig {
    /// Load TLS configuration from environment variables.
    ///
    /// Returns `Ok(None)` if `VERIFIER_TLS_CERT` is not set (TLS disabled).
    /// Returns `Err` if cert path is set but key path is missing.
    pub fn from_env() -> Result<Option<Self>, String> {
        Self::from_vars(
            std::env::var("VERIFIER_TLS_CERT").ok(),
            std::env::var("VERIFIER_TLS_KEY").ok(),
            std::env::var("VERIFIER_TLS_CLIENT_CA").ok(),
        )
    }

    /// Build the TLS configuration from the values of `VERIFIER_TLS_CERT`,
    /// `VERIFIER_TLS_KEY` and `VERIFIER_TLS_CLIENT_CA` (`None` = not set).
    fn from_vars(
        cert: Option<String>,
        key: Option<String>,
        client_ca: Option<String>,
    ) -> Result<Option<Self>, String> {
        let cert_path = match cert {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => return Ok(None),
        };

        let key_path = key.map(PathBuf::from).ok_or_else(|| {
            "VERIFIER_TLS_CERT is set but VERIFIER_TLS_KEY is missing".to_string()
        })?;

        if key_path.as_os_str().is_empty() {
            return Err("VERIFIER_TLS_KEY must not be empty when VERIFIER_TLS_CERT is set".into());
        }

        let client_ca_path = client_ca.filter(|p| !p.is_empty()).map(PathBuf::from);

        Ok(Some(Self {
            cert_path,
            key_path,
            client_ca_path,
        }))
    }

    /// Whether mutual TLS (client certificate verification) is enabled.
    pub fn is_mtls(&self) -> bool {
        self.client_ca_path.is_some()
    }

    /// Build a `RustlsConfig` from the configured certificate and key files.
    ///
    /// For basic TLS, this loads the server cert + key. For mTLS, it also
    /// configures client certificate verification using the specified CA.
    pub async fn into_rustls_config(self) -> Result<RustlsConfig, Box<dyn std::error::Error>> {
        if let Some(ca_path) = &self.client_ca_path {
            // mTLS: build a custom rustls ServerConfig with client cert verification
            let config = build_mtls_server_config(&self.cert_path, &self.key_path, ca_path)?;
            Ok(RustlsConfig::from_config(config))
        } else {
            // Standard TLS: just load cert + key
            let config = RustlsConfig::from_pem_file(&self.cert_path, &self.key_path).await?;
            Ok(config)
        }
    }
}

/// Build a rustls `ServerConfig` with mutual TLS client certificate verification.
fn build_mtls_server_config(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    ca_path: &std::path::Path,
) -> Result<std::sync::Arc<rustls::ServerConfig>, Box<dyn std::error::Error>> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use std::io::BufReader;

    // Load server certificate chain
    let cert_file = std::fs::File::open(cert_path)
        .map_err(|e| format!("failed to open cert file {:?}: {}", cert_path, e))?;
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut BufReader::new(cert_file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("failed to parse cert PEM: {}", e))?;

    if certs.is_empty() {
        return Err("no certificates found in cert file".into());
    }

    // Load server private key
    let key_file = std::fs::File::open(key_path)
        .map_err(|e| format!("failed to open key file {:?}: {}", key_path, e))?;
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut BufReader::new(key_file))
        .map_err(|e| format!("failed to parse key PEM: {}", e))?
        .ok_or("no private key found in key file")?;

    // Load CA certificate(s) for client verification
    let ca_file = std::fs::File::open(ca_path)
        .map_err(|e| format!("failed to open CA file {:?}: {}", ca_path, e))?;
    let ca_certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut BufReader::new(ca_file))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("failed to parse CA PEM: {}", e))?;

    if ca_certs.is_empty() {
        return Err("no CA certificates found in CA file".into());
    }

    let mut root_store = rustls::RootCertStore::empty();
    for cert in ca_certs {
        root_store.add(cert)?;
    }

    let client_verifier = rustls::server::WebPkiClientVerifier::builder(root_store.into())
        .build()
        .map_err(|e| format!("failed to build client verifier: {}", e))?;

    let server_config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .map_err(|e| format!("failed to build server config: {}", e))?;

    Ok(std::sync::Arc::new(server_config))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install a default CryptoProvider for rustls.
    ///
    /// When rustls cannot auto-detect which provider to use, we explicitly
    /// install `aws_lc_rs` as the default (the only crypto backend enabled
    /// in this crate's dependency tree).
    fn ensure_crypto_provider() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }

    #[test]
    fn test_tls_disabled_by_default() {
        let config = TlsConfig::from_vars(None, None, None).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn test_tls_requires_key_when_cert_set() {
        let result = TlsConfig::from_vars(Some("/certs/server.pem".into()), None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("VERIFIER_TLS_KEY is missing"));
    }

    #[test]
    fn test_tls_config_without_mtls() {
        let config = TlsConfig {
            cert_path: PathBuf::from("/certs/server.pem"),
            key_path: PathBuf::from("/certs/server-key.pem"),
            client_ca_path: None,
        };
        assert!(!config.is_mtls());
    }

    #[test]
    fn test_mtls_config() {
        let config = TlsConfig {
            cert_path: PathBuf::from("/certs/server.pem"),
            key_path: PathBuf::from("/certs/server-key.pem"),
            client_ca_path: Some(PathBuf::from("/certs/ca.pem")),
        };
        assert!(config.is_mtls());
    }

    #[test]
    fn test_empty_cert_path_means_disabled() {
        let config = TlsConfig::from_vars(Some(String::new()), None, None).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn test_empty_client_ca_path_means_no_mtls() {
        let config = TlsConfig::from_vars(
            Some("/certs/server.pem".into()),
            Some("/certs/server-key.pem".into()),
            Some(String::new()),
        )
        .unwrap()
        .unwrap();
        assert!(!config.is_mtls());
    }

    #[tokio::test]
    async fn test_rustls_config_basic_tls() {
        ensure_crypto_provider();

        // Generate self-signed cert for testing
        let params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap();
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert2 = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&key_pair)
            .unwrap();

        let dir = std::env::temp_dir().join("verifier-tls-test");
        std::fs::create_dir_all(&dir).unwrap();

        let cert_path = dir.join("server.pem");
        let key_path = dir.join("server-key.pem");

        std::fs::write(&cert_path, cert2.pem()).unwrap();
        std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();

        let tls_config = TlsConfig {
            cert_path,
            key_path,
            client_ca_path: None,
        };

        let result = tls_config.into_rustls_config().await;
        assert!(
            result.is_ok(),
            "failed to build rustls config: {:?}",
            result.err()
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
        drop(cert); // suppress unused warning
    }

    #[tokio::test]
    async fn test_rustls_config_mtls() {
        ensure_crypto_provider();

        let dir = std::env::temp_dir().join("verifier-mtls-test");
        std::fs::create_dir_all(&dir).unwrap();

        // Generate CA cert
        let mut ca_params = rcgen::CertificateParams::new(vec!["Test CA".to_string()]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();

        // Generate server cert
        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .self_signed(&server_key)
            .unwrap();

        let cert_path = dir.join("server.pem");
        let key_path = dir.join("server-key.pem");
        let ca_path = dir.join("ca.pem");

        std::fs::write(&cert_path, server_cert.pem()).unwrap();
        std::fs::write(&key_path, server_key.serialize_pem()).unwrap();
        std::fs::write(&ca_path, ca_cert.pem()).unwrap();

        let tls_config = TlsConfig {
            cert_path,
            key_path,
            client_ca_path: Some(ca_path),
        };

        assert!(tls_config.is_mtls());
        let result = tls_config.into_rustls_config().await;
        assert!(
            result.is_ok(),
            "failed to build mTLS config: {:?}",
            result.err()
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_rustls_config_bad_cert_path() {
        let tls_config = TlsConfig {
            cert_path: PathBuf::from("/nonexistent/server.pem"),
            key_path: PathBuf::from("/nonexistent/server-key.pem"),
            client_ca_path: None,
        };

        let result = tls_config.into_rustls_config().await;
        assert!(result.is_err());
    }
}
