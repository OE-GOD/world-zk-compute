//! Configuration for the TEE enclave application.

use crate::model::ModelFormat;

/// Application configuration, loaded from environment variables.
pub struct Config {
    /// Path to the model JSON file (XGBoost or LightGBM).
    pub model_path: String,
    /// Model format: auto-detect, xgboost, or lightgbm.
    pub model_format: ModelFormat,
    /// HTTP server port.
    pub port: u16,
    /// Optional hex-encoded private key. If not set, a random key is generated.
    pub private_key: Option<String>,
    /// Whether to use real AWS Nitro attestation.
    /// When true and no private key is set, a random key is generated and
    /// bound to the enclave image via Nitro attestation.
    pub nitro_enabled: bool,
    /// Chain ID for replay protection in attestation signing.
    /// Default: 1 (Ethereum mainnet). Can be overridden per-request.
    pub chain_id: u64,
    /// Optional admin API key for protected endpoints (e.g. model hot-reload).
    /// If not set, admin endpoints return 403.
    pub admin_api_key: Option<String>,
    /// Maximum inference requests per minute (rate limiting). Default: 120.
    pub max_requests_per_minute: u64,
    /// Whether the background watchdog health-monitor is enabled. Default: true.
    pub watchdog_enabled: bool,
    /// Optional expected SHA-256 hex hash of the model file.
    /// If set, the enclave will reject any model whose hash does not match.
    pub expected_model_hash: Option<String>,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// - `MODEL_PATH` — path to model JSON file (default: `/app/model/model.json`)
    /// - `MODEL_FORMAT` — model format: `auto`, `xgboost`, or `lightgbm` (default: `auto`)
    /// - `PORT` — HTTP port (default: `8080`)
    /// - `ENCLAVE_PRIVATE_KEY` — hex-encoded secp256k1 private key (optional)
    /// - `NITRO_ENABLED` — set to `true` to enable AWS Nitro attestation (default: `false`)
    /// - `CHAIN_ID` — chain ID for attestation replay protection (default: `1`)
    /// - `EXPECTED_MODEL_HASH` — optional SHA-256 hex hash; model rejected if mismatch
    pub fn from_env() -> Self {
        let model_path =
            std::env::var("MODEL_PATH").unwrap_or_else(|_| "/app/model/model.json".to_string());

        let model_format = std::env::var("MODEL_FORMAT")
            .ok()
            .and_then(|s| ModelFormat::from_str(&s))
            .unwrap_or_default();

        let port = std::env::var("PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8080u16);

        let private_key = std::env::var("ENCLAVE_PRIVATE_KEY").ok().and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let nitro_enabled = std::env::var("NITRO_ENABLED")
            .map(|s| s.eq_ignore_ascii_case("true") || s == "1")
            .unwrap_or(false);

        let chain_id = std::env::var("CHAIN_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1u64);

        let admin_api_key = std::env::var("ADMIN_API_KEY").ok().and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        let max_requests_per_minute = std::env::var("MAX_REQUESTS_PER_MINUTE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120u64);

        let watchdog_enabled = std::env::var("WATCHDOG_ENABLED")
            .map(|s| !s.eq_ignore_ascii_case("false") && s != "0")
            .unwrap_or(true);

        let expected_model_hash = std::env::var("EXPECTED_MODEL_HASH").ok().and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        });

        Config {
            model_path,
            model_format,
            port,
            private_key,
            nitro_enabled,
            chain_id,
            admin_api_key,
            max_requests_per_minute,
            watchdog_enabled,
            expected_model_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_defaults() {
        // Clear env vars to test defaults
        std::env::remove_var("MODEL_PATH");
        std::env::remove_var("MODEL_FORMAT");
        std::env::remove_var("PORT");
        std::env::remove_var("ENCLAVE_PRIVATE_KEY");
        std::env::remove_var("NITRO_ENABLED");
        std::env::remove_var("CHAIN_ID");
        std::env::remove_var("ADMIN_API_KEY");
        std::env::remove_var("MAX_REQUESTS_PER_MINUTE");
        std::env::remove_var("WATCHDOG_ENABLED");
        std::env::remove_var("EXPECTED_MODEL_HASH");

        let config = Config::from_env();
        assert_eq!(config.model_path, "/app/model/model.json");
        assert_eq!(config.model_format, ModelFormat::Auto);
        assert_eq!(config.port, 8080);
        assert!(config.private_key.is_none());
        assert!(!config.nitro_enabled);
        assert_eq!(config.chain_id, 1); // default is Ethereum mainnet
        assert!(config.admin_api_key.is_none());
        assert_eq!(config.max_requests_per_minute, 120);
        assert!(config.watchdog_enabled); // default is true
        assert!(config.expected_model_hash.is_none());
    }

    #[test]
    #[serial]
    fn test_chain_id_from_env() {
        std::env::set_var("CHAIN_ID", "11155111");
        let config = Config::from_env();
        assert_eq!(config.chain_id, 11155111); // Sepolia
        std::env::remove_var("CHAIN_ID");
    }

    #[test]
    #[serial]
    fn test_admin_api_key_from_env() {
        std::env::set_var("ADMIN_API_KEY", "my-secret-key");
        let config = Config::from_env();
        assert_eq!(config.admin_api_key.as_deref(), Some("my-secret-key"));
        std::env::remove_var("ADMIN_API_KEY");

        // Empty string should be treated as None
        std::env::set_var("ADMIN_API_KEY", "");
        let config = Config::from_env();
        assert!(config.admin_api_key.is_none());
        std::env::remove_var("ADMIN_API_KEY");

        // Whitespace-only should be treated as None
        std::env::set_var("ADMIN_API_KEY", "   ");
        let config = Config::from_env();
        assert!(config.admin_api_key.is_none());
        std::env::remove_var("ADMIN_API_KEY");
    }

    #[test]
    #[serial]
    fn test_watchdog_enabled_from_env() {
        // Default is true (when env var not set)
        std::env::remove_var("WATCHDOG_ENABLED");
        let config = Config::from_env();
        assert!(config.watchdog_enabled);

        // Explicitly "true"
        std::env::set_var("WATCHDOG_ENABLED", "true");
        let config = Config::from_env();
        assert!(config.watchdog_enabled);
        std::env::remove_var("WATCHDOG_ENABLED");

        // Explicitly "false"
        std::env::set_var("WATCHDOG_ENABLED", "false");
        let config = Config::from_env();
        assert!(!config.watchdog_enabled);
        std::env::remove_var("WATCHDOG_ENABLED");

        // "0" disables
        std::env::set_var("WATCHDOG_ENABLED", "0");
        let config = Config::from_env();
        assert!(!config.watchdog_enabled);
        std::env::remove_var("WATCHDOG_ENABLED");

        // "1" enables
        std::env::set_var("WATCHDOG_ENABLED", "1");
        let config = Config::from_env();
        assert!(config.watchdog_enabled);
        std::env::remove_var("WATCHDOG_ENABLED");
    }

    #[test]
    #[serial]
    fn test_model_format_from_env() {
        std::env::set_var("MODEL_FORMAT", "lightgbm");
        let config = Config::from_env();
        assert_eq!(config.model_format, ModelFormat::Lightgbm);
        std::env::remove_var("MODEL_FORMAT");

        std::env::set_var("MODEL_FORMAT", "xgboost");
        let config = Config::from_env();
        assert_eq!(config.model_format, ModelFormat::Xgboost);
        std::env::remove_var("MODEL_FORMAT");

        // Invalid format falls back to Auto
        std::env::set_var("MODEL_FORMAT", "invalid_format");
        let config = Config::from_env();
        assert_eq!(config.model_format, ModelFormat::Auto);
        std::env::remove_var("MODEL_FORMAT");
    }

    #[test]
    #[serial]
    fn test_expected_model_hash_from_env() {
        let hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        std::env::set_var("EXPECTED_MODEL_HASH", hash);
        let config = Config::from_env();
        assert_eq!(config.expected_model_hash.as_deref(), Some(hash));
        std::env::remove_var("EXPECTED_MODEL_HASH");

        // Empty string should be treated as None
        std::env::set_var("EXPECTED_MODEL_HASH", "");
        let config = Config::from_env();
        assert!(config.expected_model_hash.is_none());
        std::env::remove_var("EXPECTED_MODEL_HASH");

        // Whitespace-only should be treated as None
        std::env::set_var("EXPECTED_MODEL_HASH", "   ");
        let config = Config::from_env();
        assert!(config.expected_model_hash.is_none());
        std::env::remove_var("EXPECTED_MODEL_HASH");

        // With 0x prefix
        std::env::set_var("EXPECTED_MODEL_HASH", format!("0x{}", hash));
        let config = Config::from_env();
        assert_eq!(
            config.expected_model_hash.as_deref(),
            Some(format!("0x{}", hash).as_str())
        );
        std::env::remove_var("EXPECTED_MODEL_HASH");
    }
}
