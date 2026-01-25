//! Configuration File Support
//!
//! Load prover configuration from TOML files with:
//! - Environment variable overrides
//! - Default values
//! - Validation
//!
//! ## Example Configuration
//!
//! ```toml
//! [prover]
//! private_key = "0x..."
//! rpc_url = "https://worldchain-mainnet.g.alchemy.com/v2/..."
//!
//! [proving]
//! mode = "local"
//! max_concurrent = 4
//!
//! [api]
//! enabled = true
//! port = 9090
//! ```

use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{info, warn};

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Prover identity and connection settings
    pub prover: ProverSection,
    /// Proving engine settings
    pub proving: ProvingSection,
    /// API server settings
    pub api: ApiSection,
    /// Cluster settings
    pub cluster: ClusterSection,
    /// Recovery settings
    pub recovery: RecoverySection,
    /// Logging settings
    pub logging: LoggingSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            prover: ProverSection::default(),
            proving: ProvingSection::default(),
            api: ApiSection::default(),
            cluster: ClusterSection::default(),
            recovery: RecoverySection::default(),
            logging: LoggingSection::default(),
        }
    }
}

/// Prover identity and connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProverSection {
    /// Private key for signing transactions (env: PRIVATE_KEY)
    #[serde(skip_serializing)]
    pub private_key: Option<String>,
    /// RPC URL for World Chain (env: RPC_URL)
    pub rpc_url: String,
    /// Execution Engine contract address
    pub contract_address: Option<String>,
    /// Chain ID
    pub chain_id: u64,
    /// Minimum tip to accept (in wei)
    pub min_tip_wei: u64,
    /// Programs to accept (empty = all)
    pub allowed_programs: Vec<String>,
}

impl Default for ProverSection {
    fn default() -> Self {
        Self {
            private_key: None,
            rpc_url: "https://worldchain-mainnet.g.alchemy.com/v2/demo".to_string(),
            contract_address: None,
            chain_id: 480,
            min_tip_wei: 0,
            allowed_programs: vec![],
        }
    }
}

/// Proving engine settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvingSection {
    /// Proving mode: local, bonsai, hybrid
    pub mode: String,
    /// Bonsai API key (env: BONSAI_API_KEY)
    #[serde(skip_serializing)]
    pub bonsai_api_key: Option<String>,
    /// Maximum concurrent proofs
    pub max_concurrent: usize,
    /// Enable GPU acceleration
    pub gpu_enabled: bool,
    /// Proof cache directory
    pub cache_dir: String,
    /// Maximum cache size in MB
    pub cache_size_mb: u64,
    /// Enable SNARK wrapping for cheaper verification
    pub snark_enabled: bool,
}

impl Default for ProvingSection {
    fn default() -> Self {
        Self {
            mode: "local".to_string(),
            bonsai_api_key: None,
            max_concurrent: 4,
            gpu_enabled: true,
            cache_dir: "./data/cache".to_string(),
            cache_size_mb: 1024,
            snark_enabled: true,
        }
    }
}

/// API server settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiSection {
    /// Enable HTTP API server
    pub enabled: bool,
    /// Host to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
}

impl Default for ApiSection {
    fn default() -> Self {
        Self {
            enabled: true,
            host: "0.0.0.0".to_string(),
            port: 9090,
        }
    }
}

/// Cluster settings for multi-prover mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterSection {
    /// Enable cluster mode
    pub enabled: bool,
    /// This prover's ID (auto-generated if not set)
    pub prover_id: Option<String>,
    /// Address to listen for cluster communication
    pub listen_addr: String,
    /// Seed peers to connect to
    pub seed_peers: Vec<String>,
    /// Heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,
    /// Peer timeout in seconds
    pub peer_timeout_secs: u64,
}

impl Default for ClusterSection {
    fn default() -> Self {
        Self {
            enabled: false,
            prover_id: None,
            listen_addr: "0.0.0.0:9091".to_string(),
            seed_peers: vec![],
            heartbeat_interval_secs: 5,
            peer_timeout_secs: 30,
        }
    }
}

/// Recovery settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoverySection {
    /// Path to recovery file
    pub recovery_file: String,
    /// Stale claim timeout in seconds
    pub stale_timeout_secs: u64,
    /// Auto-resume incomplete jobs on startup
    pub auto_resume: bool,
}

impl Default for RecoverySection {
    fn default() -> Self {
        Self {
            recovery_file: "./data/recovery.json".to_string(),
            stale_timeout_secs: 1800, // 30 minutes
            auto_resume: true,
        }
    }
}

/// Logging settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingSection {
    /// Log level: trace, debug, info, warn, error
    pub level: String,
    /// Output format: text, json
    pub format: String,
    /// Log file path (optional)
    pub file: Option<String>,
}

impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "text".to_string(),
            file: None,
        }
    }
}

impl Config {
    /// Load configuration from a TOML file
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&content)?;

        // Apply environment variable overrides
        config.apply_env_overrides();

        info!("Loaded configuration from {}", path.display());
        Ok(config)
    }

    /// Load configuration from file or use defaults
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        let mut config = if let Some(path) = path {
            Self::from_file(path)?
        } else {
            // Try default paths
            let default_paths = ["prover.toml", "config/prover.toml", "/etc/world-zk-prover/config.toml"];

            let mut loaded = None;
            for path in default_paths {
                if Path::new(path).exists() {
                    match Self::from_file(path) {
                        Ok(cfg) => {
                            loaded = Some(cfg);
                            break;
                        }
                        Err(e) => warn!("Failed to load {}: {}", path, e),
                    }
                }
            }

            loaded.unwrap_or_default()
        };

        // Always apply env overrides
        config.apply_env_overrides();

        Ok(config)
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&mut self) {
        // Private key
        if let Ok(key) = std::env::var("PRIVATE_KEY") {
            self.prover.private_key = Some(key);
        }

        // RPC URL
        if let Ok(url) = std::env::var("RPC_URL") {
            self.prover.rpc_url = url;
        }

        // Contract address
        if let Ok(addr) = std::env::var("CONTRACT_ADDRESS") {
            self.prover.contract_address = Some(addr);
        }

        // Bonsai API key
        if let Ok(key) = std::env::var("BONSAI_API_KEY") {
            self.proving.bonsai_api_key = Some(key);
        }

        // Log level
        if let Ok(level) = std::env::var("RUST_LOG") {
            self.logging.level = level;
        }

        // API port
        if let Ok(port) = std::env::var("API_PORT") {
            if let Ok(p) = port.parse() {
                self.api.port = p;
            }
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> anyhow::Result<()> {
        // Check private key
        if self.prover.private_key.is_none() {
            anyhow::bail!("Private key is required (set PRIVATE_KEY env var or in config)");
        }

        // Check RPC URL format
        if !self.prover.rpc_url.starts_with("http://") && !self.prover.rpc_url.starts_with("https://") {
            anyhow::bail!("RPC URL must start with http:// or https://");
        }

        // Check proving mode
        let valid_modes = ["local", "bonsai", "hybrid"];
        if !valid_modes.contains(&self.proving.mode.as_str()) {
            anyhow::bail!("Invalid proving mode: {}. Must be one of: {:?}", self.proving.mode, valid_modes);
        }

        // Check Bonsai key if using Bonsai
        if (self.proving.mode == "bonsai" || self.proving.mode == "hybrid")
            && self.proving.bonsai_api_key.is_none()
        {
            anyhow::bail!("Bonsai API key required for {} mode", self.proving.mode);
        }

        // Check log level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.logging.level.to_lowercase().as_str()) {
            anyhow::bail!("Invalid log level: {}. Must be one of: {:?}", self.logging.level, valid_levels);
        }

        Ok(())
    }

    /// Generate a sample configuration file
    pub fn sample() -> String {
        let config = Config::default();
        let mut sample = toml::to_string_pretty(&config).unwrap();

        // Add comments
        let header = r#"# World ZK Compute Prover Configuration
#
# Environment variables take precedence over config file values.
# Required env vars: PRIVATE_KEY
# Optional env vars: RPC_URL, BONSAI_API_KEY, CONTRACT_ADDRESS, RUST_LOG, API_PORT

"#;

        format!("{}{}", header, sample)
    }

    /// Save configuration to file
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.prover.chain_id, 480);
        assert_eq!(config.proving.mode, "local");
        assert!(config.api.enabled);
    }

    #[test]
    fn test_config_from_toml() {
        let toml = r#"
[prover]
rpc_url = "https://test.rpc.io"
chain_id = 123

[proving]
mode = "bonsai"
max_concurrent = 8

[api]
port = 8080
"#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.prover.rpc_url, "https://test.rpc.io");
        assert_eq!(config.prover.chain_id, 123);
        assert_eq!(config.proving.mode, "bonsai");
        assert_eq!(config.proving.max_concurrent, 8);
        assert_eq!(config.api.port, 8080);
    }

    #[test]
    fn test_config_file_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::default();
        config.save(&path).unwrap();

        let loaded = Config::from_file(&path).unwrap();
        assert_eq!(loaded.prover.chain_id, config.prover.chain_id);
        assert_eq!(loaded.proving.mode, config.proving.mode);
    }

    #[test]
    fn test_validation_missing_key() {
        let config = Config::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_invalid_mode() {
        let mut config = Config::default();
        config.prover.private_key = Some("0x123".to_string());
        config.proving.mode = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_bonsai_without_key() {
        let mut config = Config::default();
        config.prover.private_key = Some("0x123".to_string());
        config.proving.mode = "bonsai".to_string();
        config.proving.bonsai_api_key = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_success() {
        let mut config = Config::default();
        config.prover.private_key = Some("0x123".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sample_config() {
        let sample = Config::sample();
        assert!(sample.contains("[prover]"));
        assert!(sample.contains("[proving]"));
        assert!(sample.contains("[api]"));
    }

    #[test]
    fn test_partial_config() {
        // Test that missing sections use defaults
        let toml = r#"
[prover]
rpc_url = "https://custom.rpc.io"
"#;

        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.prover.rpc_url, "https://custom.rpc.io");
        // Other sections should have defaults
        assert_eq!(config.proving.mode, "local");
        assert!(config.api.enabled);
    }
}
