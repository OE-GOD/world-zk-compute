use serde::Deserialize;

/// Represents a TOML configuration file with all optional fields.
///
/// Fields set here serve as defaults; environment variables always
/// take precedence over values from the config file.
#[derive(Deserialize, Default, Debug)]
pub struct ConfigFile {
    pub rpc_url: Option<String>,
    pub private_key: Option<String>,
    pub tee_verifier_address: Option<String>,
    pub enclave_url: Option<String>,
    pub model_path: Option<String>,
    pub proofs_dir: Option<String>,
    pub prover_stake: Option<String>,
    pub precompute_bin: Option<String>,
    pub nitro_verification: Option<bool>,
    pub expected_pcr0: Option<String>,
    pub attestation_cache_ttl: Option<u64>,
    pub prover_url: Option<String>,
    pub metrics_port: Option<u16>,
}

impl ConfigFile {
    /// Parse a TOML string into a ConfigFile.
    pub fn from_toml_str(s: &str) -> anyhow::Result<Self> {
        let cf: ConfigFile =
            toml::from_str(s).map_err(|e| anyhow::anyhow!("Failed to parse TOML: {}", e))?;
        Ok(cf)
    }

    /// Read and parse a TOML file from disk.
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file '{}': {}", path, e))?;
        Self::from_toml_str(&contents)
    }
}

/// Operator service configuration loaded from environment variables,
/// optionally overlaid on top of a TOML config file.
#[derive(Debug)]
pub struct Config {
    pub rpc_url: String,
    pub private_key: String,
    pub tee_verifier_address: String,
    pub enclave_url: String,
    pub model_path: String,
    pub proofs_dir: String,
    pub prover_stake_wei: String,
    pub precompute_bin: String,
    /// Whether to verify Nitro attestation before operations.
    pub nitro_verification: bool,
    /// Expected PCR0 value (optional, for strict verification).
    pub expected_pcr0: Option<String>,
    /// Attestation cache TTL in seconds.
    pub attestation_cache_ttl: u64,
    /// URL of the warm prover HTTP service (optional).
    /// Not yet consumed by all code paths; kept for forward compatibility.
    #[allow(dead_code)]
    pub prover_url: Option<String>,
    /// Port for the health/metrics HTTP server.
    /// Available for use by callers; cmd_watch currently takes this via CLI arg.
    #[allow(dead_code)]
    pub metrics_port: u16,
}

impl Config {
    /// Load configuration with priority: env vars > config file > defaults.
    ///
    /// When `config_file` is `Some(path)`, the TOML file is read first as a
    /// base layer of defaults. Environment variables then override any values
    /// found in the file. When `config_file` is `None`, behavior is identical
    /// to the original env-var-only loading.
    ///
    /// Required fields (`private_key`, `tee_verifier_address`) must be present
    /// in either the config file or the corresponding env var.
    pub fn from_env(config_file: Option<&str>) -> anyhow::Result<Self> {
        let file_cfg = match config_file {
            Some(path) => ConfigFile::from_file(path)?,
            None => ConfigFile::default(),
        };

        // Helper: env var wins, then config file, then default.
        let private_key = std::env::var("OPERATOR_PRIVATE_KEY")
            .ok()
            .or(file_cfg.private_key)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "OPERATOR_PRIVATE_KEY is required (set env var or private_key in config file)"
                )
            })?;

        let tee_verifier_address = std::env::var("TEE_VERIFIER_ADDRESS")
            .ok()
            .or(file_cfg.tee_verifier_address)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "TEE_VERIFIER_ADDRESS is required (set env var or tee_verifier_address in config file)"
                )
            })?;

        let rpc_url = std::env::var("OPERATOR_RPC_URL")
            .ok()
            .or(file_cfg.rpc_url)
            .unwrap_or_else(|| "http://127.0.0.1:8545".to_string());

        let enclave_url = std::env::var("ENCLAVE_URL")
            .ok()
            .or(file_cfg.enclave_url)
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

        let model_path = std::env::var("MODEL_PATH")
            .ok()
            .or(file_cfg.model_path)
            .unwrap_or_else(|| "./model.json".to_string());

        let proofs_dir = std::env::var("PROOFS_DIR")
            .ok()
            .or(file_cfg.proofs_dir)
            .unwrap_or_else(|| "./proofs".to_string());

        let prover_stake_wei = std::env::var("PROVER_STAKE")
            .ok()
            .or(file_cfg.prover_stake)
            .unwrap_or_else(|| "100000000000000000".to_string());

        let precompute_bin = std::env::var("PRECOMPUTE_BIN")
            .ok()
            .or(file_cfg.precompute_bin)
            .unwrap_or_else(|| "precompute_proof".to_string());

        let nitro_verification = std::env::var("NITRO_VERIFICATION")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .or(file_cfg.nitro_verification)
            .unwrap_or(false);

        let expected_pcr0 = std::env::var("EXPECTED_PCR0")
            .ok()
            .or(file_cfg.expected_pcr0);

        let attestation_cache_ttl = std::env::var("ATTESTATION_CACHE_TTL")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.attestation_cache_ttl)
            .unwrap_or(300);

        let prover_url = std::env::var("PROVER_URL").ok().or(file_cfg.prover_url);

        let metrics_port = std::env::var("METRICS_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .or(file_cfg.metrics_port)
            .unwrap_or(9090);

        Ok(Self {
            rpc_url,
            private_key,
            tee_verifier_address,
            enclave_url,
            model_path,
            proofs_dir,
            prover_stake_wei,
            precompute_bin,
            nitro_verification,
            expected_pcr0,
            attestation_cache_ttl,
            prover_url,
            metrics_port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    /// Serialize tests that manipulate environment variables so they
    /// do not race against each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Clear all operator-related env vars to ensure a clean state.
    fn clear_all_env_vars() {
        for var in [
            "OPERATOR_PRIVATE_KEY",
            "TEE_VERIFIER_ADDRESS",
            "OPERATOR_RPC_URL",
            "ENCLAVE_URL",
            "MODEL_PATH",
            "PROOFS_DIR",
            "PROVER_STAKE",
            "PRECOMPUTE_BIN",
            "NITRO_VERIFICATION",
            "EXPECTED_PCR0",
            "ATTESTATION_CACHE_TTL",
            "PROVER_URL",
            "METRICS_PORT",
        ] {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn test_config_file_from_toml_str() {
        let toml_str = r#"
rpc_url = "https://rpc.example.com"
private_key = "0xabcdef"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
enclave_url = "http://enclave:8080"
model_path = "/data/model.json"
proofs_dir = "/data/proofs"
prover_stake = "200000000000000000"
precompute_bin = "/usr/local/bin/precompute_proof"
nitro_verification = true
expected_pcr0 = "aabbccdd"
attestation_cache_ttl = 600
prover_url = "http://prover:3000"
metrics_port = 9091
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.rpc_url.as_deref(), Some("https://rpc.example.com"));
        assert_eq!(cf.private_key.as_deref(), Some("0xabcdef"));
        assert_eq!(
            cf.tee_verifier_address.as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(cf.enclave_url.as_deref(), Some("http://enclave:8080"));
        assert_eq!(cf.model_path.as_deref(), Some("/data/model.json"));
        assert_eq!(cf.proofs_dir.as_deref(), Some("/data/proofs"));
        assert_eq!(cf.prover_stake.as_deref(), Some("200000000000000000"));
        assert_eq!(
            cf.precompute_bin.as_deref(),
            Some("/usr/local/bin/precompute_proof")
        );
        assert_eq!(cf.nitro_verification, Some(true));
        assert_eq!(cf.expected_pcr0.as_deref(), Some("aabbccdd"));
        assert_eq!(cf.attestation_cache_ttl, Some(600));
        assert_eq!(cf.prover_url.as_deref(), Some("http://prover:3000"));
        assert_eq!(cf.metrics_port, Some(9091));
    }

    #[test]
    fn test_config_file_partial_toml() {
        let toml_str = r#"
rpc_url = "https://rpc.example.com"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.rpc_url.as_deref(), Some("https://rpc.example.com"));
        assert!(cf.private_key.is_none());
        assert!(cf.tee_verifier_address.is_none());
        assert!(cf.nitro_verification.is_none());
    }

    #[test]
    fn test_config_file_empty_toml() {
        let cf = ConfigFile::from_toml_str("").unwrap();
        assert!(cf.rpc_url.is_none());
        assert!(cf.private_key.is_none());
    }

    #[test]
    fn test_from_env_no_config_file_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xdeadbeef");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0x1234");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.rpc_url, "http://127.0.0.1:8545");
        assert_eq!(config.enclave_url, "http://127.0.0.1:8080");
        assert_eq!(config.prover_stake_wei, "100000000000000000");
        assert_eq!(config.precompute_bin, "precompute_proof");
        assert!(!config.nitro_verification);
        assert!(config.expected_pcr0.is_none());
        assert_eq!(config.attestation_cache_ttl, 300);
        assert!(config.prover_url.is_none());
        assert_eq!(config.metrics_port, 9090);

        clear_all_env_vars();
    }

    #[test]
    fn test_from_env_with_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
rpc_url = "https://from-toml.example.com"
private_key = "0xtomlkey"
tee_verifier_address = "0xtomladdr"
enclave_url = "http://toml-enclave:9999"
model_path = "/toml/model.json"
proofs_dir = "/toml/proofs"
prover_stake = "500000000000000000"
precompute_bin = "/toml/bin"
nitro_verification = true
expected_pcr0 = "tomlpcr0"
attestation_cache_ttl = 900
prover_url = "http://toml-prover:4000"
metrics_port = 8888
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.rpc_url, "https://from-toml.example.com");
        assert_eq!(config.private_key, "0xtomlkey");
        assert_eq!(config.tee_verifier_address, "0xtomladdr");
        assert_eq!(config.enclave_url, "http://toml-enclave:9999");
        assert_eq!(config.model_path, "/toml/model.json");
        assert_eq!(config.proofs_dir, "/toml/proofs");
        assert_eq!(config.prover_stake_wei, "500000000000000000");
        assert_eq!(config.precompute_bin, "/toml/bin");
        assert!(config.nitro_verification);
        assert_eq!(config.expected_pcr0.as_deref(), Some("tomlpcr0"));
        assert_eq!(config.attestation_cache_ttl, 900);
        assert_eq!(
            config.prover_url.as_deref(),
            Some("http://toml-prover:4000")
        );
        assert_eq!(config.metrics_port, 8888);

        clear_all_env_vars();
    }

    #[test]
    fn test_env_var_overrides_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
rpc_url = "https://from-toml.example.com"
private_key = "0xtomlkey"
tee_verifier_address = "0xtomladdr"
nitro_verification = false
attestation_cache_ttl = 900
metrics_port = 8888
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        // Set env vars that should override TOML values
        std::env::set_var("OPERATOR_RPC_URL", "https://from-env.example.com");
        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xenvkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xenvaddr");
        std::env::set_var("NITRO_VERIFICATION", "true");
        std::env::set_var("ATTESTATION_CACHE_TTL", "120");
        std::env::set_var("METRICS_PORT", "7777");

        let config = Config::from_env(Some(&path)).unwrap();

        // Env vars should win
        assert_eq!(config.rpc_url, "https://from-env.example.com");
        assert_eq!(config.private_key, "0xenvkey");
        assert_eq!(config.tee_verifier_address, "0xenvaddr");
        assert!(config.nitro_verification);
        assert_eq!(config.attestation_cache_ttl, 120);
        assert_eq!(config.metrics_port, 7777);

        clear_all_env_vars();
    }

    #[test]
    fn test_missing_required_fields_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let result = Config::from_env(None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("OPERATOR_PRIVATE_KEY"),
            "Error should mention OPERATOR_PRIVATE_KEY, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_missing_tee_verifier_address_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        // Config file has private_key but not tee_verifier_address
        let toml_str = r#"
private_key = "0xsomekey"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let result = Config::from_env(Some(&path));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("TEE_VERIFIER_ADDRESS"),
            "Error should mention TEE_VERIFIER_ADDRESS, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_config_file_not_found_error() {
        let result = Config::from_env(Some("/nonexistent/path/config.toml"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to read config file"),
            "Error should mention file read failure, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_invalid_toml_error() {
        let result = ConfigFile::from_toml_str("this is not valid toml = = =");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to parse TOML"),
            "Error should mention TOML parse failure, got: {}",
            err_msg
        );
    }
}
