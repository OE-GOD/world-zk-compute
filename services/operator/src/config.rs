use alloy::primitives::keccak256;
use serde::{Deserialize, Serialize};

/// Configuration for a single model in the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelConfig {
    /// Human-readable name for the model (used as key for --model flag).
    pub name: String,
    /// Path to the model file on disk.
    pub path: String,
    /// Model format: "auto", "xgboost", or "lightgbm".
    #[serde(default = "default_model_format")]
    pub model_format: String,
    /// Optional keccak256 hash of the model file for integrity verification.
    /// If provided, the operator validates it on startup.
    #[serde(default)]
    pub model_hash: Option<String>,
}

fn default_model_format() -> String {
    "auto".to_string()
}

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
    pub max_proof_retries: Option<u32>,
    pub proof_retry_delay_secs: Option<u64>,
    /// Webhook URL for dispute notifications (optional).
    /// When set, the operator sends JSON payloads to this URL on
    /// challenge, proof submission, and dispute resolution events.
    pub webhook_url: Option<String>,
    /// Multi-model registry. Each entry is a `[[models]]` table in TOML.
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// Dry-run mode: simulate the full flow without sending on-chain transactions.
    pub dry_run: Option<bool>,
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
    /// Legacy single model path. Kept for backward compatibility.
    /// Prefer using `models` registry and `get_model()` instead.
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
    /// Maximum number of proof generation retries on failure (0 = no retries).
    pub max_proof_retries: u32,
    /// Base delay in seconds between proof generation retries (exponential backoff).
    pub proof_retry_delay_secs: u64,
    /// Webhook URL for dispute notifications (optional).
    /// When set, the operator POSTs JSON payloads to this URL on
    /// challenge, proof submission, and dispute resolution events.
    /// Compatible with Slack incoming webhooks.
    pub webhook_url: Option<String>,
    /// Multi-model registry. Always contains at least one entry (the default model).
    pub models: Vec<ModelConfig>,
    /// Dry-run mode: simulate the full flow without sending on-chain transactions.
    /// Useful for testing operator configuration against a real chain without spending gas.
    #[allow(dead_code)]
    pub dry_run: bool,
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

        let max_proof_retries = std::env::var("MAX_PROOF_RETRIES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .or(file_cfg.max_proof_retries)
            .unwrap_or(3);

        let proof_retry_delay_secs = std::env::var("PROOF_RETRY_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.proof_retry_delay_secs)
            .unwrap_or(10);

        let webhook_url = std::env::var("WEBHOOK_URL").ok().or(file_cfg.webhook_url);

        let dry_run = std::env::var("DRY_RUN")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .or(file_cfg.dry_run)
            .unwrap_or(false);

        // Build the model registry.
        // Priority: [[models]] section in TOML > MODEL_PATH env var / model_path config.
        // If [[models]] is empty, create a single default entry from model_path.
        let models = if file_cfg.models.is_empty() {
            vec![ModelConfig {
                name: "default".to_string(),
                path: model_path.clone(),
                model_format: "auto".to_string(),
                model_hash: None,
            }]
        } else {
            file_cfg.models
        };

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
            max_proof_retries,
            proof_retry_delay_secs,
            webhook_url,
            models,
            dry_run,
        })
    }

    /// Look up a model by name. If `name` is None, returns the first model.
    /// Returns an error if no model matches or the registry is empty.
    pub fn get_model(&self, name: Option<&str>) -> anyhow::Result<&ModelConfig> {
        match name {
            Some(n) => self.models.iter().find(|m| m.name == n).ok_or_else(|| {
                let available: Vec<&str> = self.models.iter().map(|m| m.name.as_str()).collect();
                anyhow::anyhow!("Model '{}' not found. Available models: {:?}", n, available)
            }),
            None => self
                .models
                .first()
                .ok_or_else(|| anyhow::anyhow!("No models configured")),
        }
    }

    /// Validate all model entries: check that model files exist and verify
    /// hashes when `model_hash` is provided.
    ///
    /// Call this on startup to catch configuration errors early.
    #[allow(dead_code)]
    pub fn validate_models(&self) -> anyhow::Result<()> {
        for model in &self.models {
            // Check file exists
            let path = std::path::Path::new(&model.path);
            if !path.exists() {
                anyhow::bail!("Model '{}': file not found at '{}'", model.name, model.path);
            }

            // Validate model_format
            match model.model_format.as_str() {
                "auto" | "xgboost" | "lightgbm" => {}
                other => {
                    anyhow::bail!(
                        "Model '{}': invalid model_format '{}'. Expected one of: auto, xgboost, lightgbm",
                        model.name,
                        other
                    );
                }
            }

            // Validate hash if provided
            if let Some(ref expected_hash) = model.model_hash {
                let file_bytes = std::fs::read(&model.path).map_err(|e| {
                    anyhow::anyhow!(
                        "Model '{}': failed to read file '{}' for hash verification: {}",
                        model.name,
                        model.path,
                        e
                    )
                })?;
                let actual_hash = format!("0x{}", hex::encode(keccak256(&file_bytes)));
                let normalized_expected = if expected_hash.starts_with("0x") {
                    expected_hash.to_lowercase()
                } else {
                    format!("0x{}", expected_hash.to_lowercase())
                };
                if actual_hash != normalized_expected {
                    anyhow::bail!(
                        "Model '{}': hash mismatch. Expected {}, got {}",
                        model.name,
                        normalized_expected,
                        actual_hash
                    );
                }
            }
        }
        Ok(())
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
            "MAX_PROOF_RETRIES",
            "PROOF_RETRY_DELAY_SECS",
            "WEBHOOK_URL",
            "DRY_RUN",
        ] {
            std::env::remove_var(var);
        }
    }

    /// Helper to create a temporary model file and return its path.
    fn create_temp_model_file(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
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
max_proof_retries = 5
proof_retry_delay_secs = 20
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
        assert_eq!(cf.max_proof_retries, Some(5));
        assert_eq!(cf.proof_retry_delay_secs, Some(20));
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
        assert_eq!(config.max_proof_retries, 3);
        assert_eq!(config.proof_retry_delay_secs, 10);

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
max_proof_retries = 5
proof_retry_delay_secs = 20
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
        assert_eq!(config.max_proof_retries, 5);
        assert_eq!(config.proof_retry_delay_secs, 20);

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

    #[test]
    fn test_retry_config_env_var_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xtomlkey"
tee_verifier_address = "0xtomladdr"
max_proof_retries = 2
proof_retry_delay_secs = 5
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        // Env vars should override TOML values
        std::env::set_var("MAX_PROOF_RETRIES", "7");
        std::env::set_var("PROOF_RETRY_DELAY_SECS", "30");

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.max_proof_retries, 7);
        assert_eq!(config.proof_retry_delay_secs, 30);

        clear_all_env_vars();
    }

    // ======================== Multi-model config tests ========================

    #[test]
    fn test_model_config_deserialize() {
        let toml_str = r#"
[[models]]
name = "xgboost-v1"
path = "/app/model/model.json"
model_format = "xgboost"
model_hash = "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"

[[models]]
name = "lightgbm-v1"
path = "/app/model/lgbm.json"
model_format = "lightgbm"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.models.len(), 2);
        assert_eq!(cf.models[0].name, "xgboost-v1");
        assert_eq!(cf.models[0].path, "/app/model/model.json");
        assert_eq!(cf.models[0].model_format, "xgboost");
        assert!(cf.models[0].model_hash.is_some());
        assert_eq!(cf.models[1].name, "lightgbm-v1");
        assert_eq!(cf.models[1].path, "/app/model/lgbm.json");
        assert_eq!(cf.models[1].model_format, "lightgbm");
        assert!(cf.models[1].model_hash.is_none());
    }

    #[test]
    fn test_model_config_default_format() {
        let toml_str = r#"
[[models]]
name = "my-model"
path = "/app/model.json"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.models.len(), 1);
        assert_eq!(cf.models[0].model_format, "auto");
    }

    #[test]
    fn test_no_models_section_creates_default_from_model_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xaddr");
        std::env::set_var("MODEL_PATH", "/custom/model.json");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.models[0].name, "default");
        assert_eq!(config.models[0].path, "/custom/model.json");
        assert_eq!(config.models[0].model_format, "auto");
        assert!(config.models[0].model_hash.is_none());

        clear_all_env_vars();
    }

    #[test]
    fn test_models_section_overrides_model_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"
model_path = "/old/model.json"

[[models]]
name = "xgboost-v1"
path = "/new/model-v1.json"

[[models]]
name = "xgboost-v2"
path = "/new/model-v2.json"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        // When [[models]] section exists, it takes precedence
        assert_eq!(config.models.len(), 2);
        assert_eq!(config.models[0].name, "xgboost-v1");
        assert_eq!(config.models[1].name, "xgboost-v2");
        // Legacy model_path is still populated for backward compat
        assert_eq!(config.model_path, "/old/model.json");

        clear_all_env_vars();
    }

    #[test]
    fn test_get_model_by_name() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "model-a"
path = "/app/a.json"

[[models]]
name = "model-b"
path = "/app/b.json"
model_format = "lightgbm"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();

        // Select by name
        let model_a = config.get_model(Some("model-a")).unwrap();
        assert_eq!(model_a.path, "/app/a.json");

        let model_b = config.get_model(Some("model-b")).unwrap();
        assert_eq!(model_b.path, "/app/b.json");
        assert_eq!(model_b.model_format, "lightgbm");

        // Default (None) returns first
        let default = config.get_model(None).unwrap();
        assert_eq!(default.name, "model-a");

        // Not found
        let result = config.get_model(Some("nonexistent"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("nonexistent"));
        assert!(err_msg.contains("model-a"));
        assert!(err_msg.contains("model-b"));

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_models_file_exists() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let model_file = create_temp_model_file(r#"{"model": "data"}"#);
        let model_path = model_file.path().to_str().unwrap().to_string();

        let toml_str = format!(
            r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "valid-model"
path = "{}"
"#,
            model_path
        );
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert!(config.validate_models().is_ok());

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_models_file_not_found() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "missing-model"
path = "/nonexistent/model-file-abc123.json"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        let result = config.validate_models();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing-model"),
            "Error should mention model name, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("file not found"),
            "Error should mention file not found, got: {}",
            err_msg
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_models_invalid_format() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let model_file = create_temp_model_file(r#"{"model": "data"}"#);
        let model_path = model_file.path().to_str().unwrap().to_string();

        let toml_str = format!(
            r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "bad-format"
path = "{}"
model_format = "tensorflow"
"#,
            model_path
        );
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        let result = config.validate_models();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid model_format"),
            "Error should mention invalid format, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("tensorflow"),
            "Error should mention the bad format, got: {}",
            err_msg
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_models_hash_match() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let content = r#"{"trees": [1,2,3]}"#;
        let model_file = create_temp_model_file(content);
        let model_path = model_file.path().to_str().unwrap().to_string();

        // Compute the actual keccak256 hash of the content
        let actual_hash = format!("0x{}", hex::encode(keccak256(content.as_bytes())));

        let toml_str = format!(
            r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "hashed-model"
path = "{}"
model_hash = "{}"
"#,
            model_path, actual_hash
        );
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert!(config.validate_models().is_ok());

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_models_hash_mismatch() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let content = r#"{"trees": [1,2,3]}"#;
        let model_file = create_temp_model_file(content);
        let model_path = model_file.path().to_str().unwrap().to_string();

        let wrong_hash = "0x0000000000000000000000000000000000000000000000000000000000000000";

        let toml_str = format!(
            r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "bad-hash"
path = "{}"
model_hash = "{}"
"#,
            model_path, wrong_hash
        );
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        let result = config.validate_models();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("hash mismatch"),
            "Error should mention hash mismatch, got: {}",
            err_msg
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_models_hash_without_0x_prefix() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let content = r#"{"test": true}"#;
        let model_file = create_temp_model_file(content);
        let model_path = model_file.path().to_str().unwrap().to_string();

        // Hash without 0x prefix should also work
        let actual_hash = hex::encode(keccak256(content.as_bytes()));

        let toml_str = format!(
            r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "no-prefix"
path = "{}"
model_hash = "{}"
"#,
            model_path, actual_hash
        );
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert!(config.validate_models().is_ok());

        clear_all_env_vars();
    }

    #[test]
    fn test_backward_compat_model_path_env_var_no_models_section() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        // When MODEL_PATH is set via env var and no [[models]] section exists,
        // a default model entry should be created from it.
        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xaddr");
        std::env::set_var("MODEL_PATH", "/env/model.json");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.models[0].name, "default");
        assert_eq!(config.models[0].path, "/env/model.json");

        clear_all_env_vars();
    }

    #[test]
    fn test_backward_compat_model_path_config_file_no_models_section() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        // When model_path is set in TOML but no [[models]] section,
        // a default model entry should be created from model_path.
        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"
model_path = "/toml/model.json"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.models[0].name, "default");
        assert_eq!(config.models[0].path, "/toml/model.json");

        clear_all_env_vars();
    }

    #[test]
    fn test_multiple_models_with_all_fields() {
        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"

[[models]]
name = "xgboost-iris"
path = "/models/iris.json"
model_format = "xgboost"
model_hash = "0xabcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234"

[[models]]
name = "lgbm-credit"
path = "/models/credit.json"
model_format = "lightgbm"

[[models]]
name = "auto-detect"
path = "/models/auto.json"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.models.len(), 3);
        assert_eq!(cf.models[0].name, "xgboost-iris");
        assert_eq!(cf.models[0].model_format, "xgboost");
        assert!(cf.models[0].model_hash.is_some());
        assert_eq!(cf.models[1].name, "lgbm-credit");
        assert_eq!(cf.models[1].model_format, "lightgbm");
        assert!(cf.models[1].model_hash.is_none());
        assert_eq!(cf.models[2].name, "auto-detect");
        assert_eq!(cf.models[2].model_format, "auto"); // default
    }

    // ======================== Webhook config tests ========================

    #[test]
    fn test_webhook_url_from_toml() {
        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"
webhook_url = "https://hooks.slack.com/services/T00/B00/XXXX"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(
            cf.webhook_url.as_deref(),
            Some("https://hooks.slack.com/services/T00/B00/XXXX")
        );
    }

    #[test]
    fn test_webhook_url_default_none() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xaddr");

        let config = Config::from_env(None).unwrap();
        assert!(config.webhook_url.is_none());

        clear_all_env_vars();
    }

    #[test]
    fn test_webhook_url_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xaddr");
        std::env::set_var("WEBHOOK_URL", "https://hooks.example.com/webhook");

        let config = Config::from_env(None).unwrap();
        assert_eq!(
            config.webhook_url.as_deref(),
            Some("https://hooks.example.com/webhook")
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_webhook_url_env_overrides_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"
webhook_url = "https://toml-webhook.example.com"
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        std::env::set_var("WEBHOOK_URL", "https://env-webhook.example.com");

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(
            config.webhook_url.as_deref(),
            Some("https://env-webhook.example.com")
        );

        clear_all_env_vars();
    }

    // ======================== Dry-run config tests ========================

    #[test]
    fn test_dry_run_default_false() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xaddr");

        let config = Config::from_env(None).unwrap();
        assert!(!config.dry_run);

        clear_all_env_vars();
    }

    #[test]
    fn test_dry_run_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xkey");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0xaddr");
        std::env::set_var("DRY_RUN", "true");

        let config = Config::from_env(None).unwrap();
        assert!(config.dry_run);

        clear_all_env_vars();
    }

    #[test]
    fn test_dry_run_from_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"
dry_run = true
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert!(config.dry_run);

        clear_all_env_vars();
    }

    #[test]
    fn test_dry_run_env_overrides_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "0xkey"
tee_verifier_address = "0xaddr"
dry_run = true
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        std::env::set_var("DRY_RUN", "false");

        let config = Config::from_env(Some(&path)).unwrap();
        assert!(!config.dry_run);

        clear_all_env_vars();
    }
}
