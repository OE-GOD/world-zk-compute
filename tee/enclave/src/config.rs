//! Configuration for the TEE enclave application.

/// Application configuration, loaded from environment variables.
pub struct Config {
    /// Path to the XGBoost model JSON file.
    pub model_path: String,
    /// HTTP server port.
    pub port: u16,
    /// Optional hex-encoded private key. If not set, a random key is generated.
    pub private_key: Option<String>,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// - `MODEL_PATH` — path to XGBoost model JSON (default: `/app/model/model.json`)
    /// - `PORT` — HTTP port (default: `8080`)
    /// - `ENCLAVE_PRIVATE_KEY` — hex-encoded secp256k1 private key (optional)
    pub fn from_env() -> Self {
        let model_path =
            std::env::var("MODEL_PATH").unwrap_or_else(|_| "/app/model/model.json".to_string());

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

        Config {
            model_path,
            port,
            private_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        // Clear env vars to test defaults
        std::env::remove_var("MODEL_PATH");
        std::env::remove_var("PORT");
        std::env::remove_var("ENCLAVE_PRIVATE_KEY");

        let config = Config::from_env();
        assert_eq!(config.model_path, "/app/model/model.json");
        assert_eq!(config.port, 8080);
        assert!(config.private_key.is_none());
    }
}
