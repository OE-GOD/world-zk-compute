use alloy::primitives::keccak256;
use secrecy::{ExposeSecret, SecretString};
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
    pub attestation_freshness_secs: Option<u64>,
    pub max_proof_retries: Option<u32>,
    pub proof_retry_delay_secs: Option<u64>,
    /// Webhook URL for dispute notifications (optional).
    /// When set, the operator sends JSON payloads to this URL on
    /// challenge, proof submission, and dispute resolution events.
    pub webhook_url: Option<String>,
    /// Generic retry: base delay in milliseconds between retries.
    pub retry_base_delay_ms: Option<u64>,
    /// Generic retry: maximum delay in milliseconds (caps exponential backoff).
    pub retry_max_delay_ms: Option<u64>,
    /// Multi-model registry. Each entry is a `[[models]]` table in TOML.
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// Dry-run mode: simulate the full flow without sending on-chain transactions.
    pub dry_run: Option<bool>,
    /// Path to the operator state file for crash recovery.
    /// Default: `./operator-state.json`.
    pub state_file: Option<String>,
    /// Multiple TEEMLVerifier contract addresses to watch.
    /// In TOML: `contracts = ["0x...", "0x..."]`
    /// When set, takes precedence over `tee_verifier_address` for multi-contract watching.
    #[serde(default)]
    pub contracts: Vec<String>,
    /// Total request timeout for enclave HTTP requests (seconds).
    /// Default: 30.
    pub enclave_timeout_secs: Option<u64>,
    /// Total request timeout for prover HTTP requests (seconds).
    /// Default: 300 (proof generation can take several minutes).
    pub prover_timeout_secs: Option<u64>,
    /// Time in seconds before an unacknowledged critical alert is escalated.
    /// Default: 900 (15 minutes). A value of 0 means immediate escalation,
    /// which is almost certainly unintended.
    /// Env var: `ESCALATION_TIMEOUT_SECS`.
    pub escalation_timeout_secs: Option<u64>,
    /// URL for a remote prover service (optional).
    pub prover_url: Option<String>,
    /// Port for the health/metrics HTTP server. Default: 9090.
    pub metrics_port: Option<u16>,
    /// Generic retry: maximum number of attempts. Default: same as max_proof_retries.
    pub retry_max_attempts: Option<u32>,
    /// Prover-specific retry: maximum number of retries. Default: 3.
    pub prover_max_retries: Option<u32>,
    /// Prover-specific retry: base delay in seconds between retries. Default: 5.
    pub prover_retry_delay_secs: Option<u64>,
    /// Prover-specific retry: maximum delay in seconds (backoff cap). Default: 300.
    pub prover_retry_max_delay_secs: Option<u64>,
    /// Bind address for the metrics HTTP server. Default: "127.0.0.1".
    pub metrics_bind_addr: Option<String>,
    /// Directory containing circuit description JSON files for local pre-verification.
    pub circuit_desc_dir: Option<String>,
    /// Base directory for the append-only proof archive.
    pub proof_archive_dir: Option<String>,
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
    /// Private key for signing on-chain transactions.
    /// Wrapped in `SecretString` to prevent accidental logging and ensure
    /// the value is zeroized on drop.
    pub private_key: SecretString,
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
    /// Maximum age in seconds for a valid attestation document (default: 600 = 10 min).
    pub attestation_freshness_secs: u64,
    /// Maximum number of proof generation retries on failure (0 = no retries).
    pub max_proof_retries: u32,
    /// Base delay in seconds between proof generation retries (exponential backoff).
    pub proof_retry_delay_secs: u64,
    /// Webhook URL for dispute notifications (optional).
    /// When set, the operator POSTs JSON payloads to this URL on
    /// challenge, proof submission, and dispute resolution events.
    /// Compatible with Slack incoming webhooks.
    pub webhook_url: Option<String>,
    /// Generic retry: base delay in milliseconds between retries (default: 1000).
    /// Env var: `RETRY_BASE_DELAY_MS`.
    pub retry_base_delay_ms: u64,
    /// Generic retry: maximum delay in milliseconds (default: 30000).
    /// Caps exponential backoff so retries do not wait indefinitely.
    /// Env var: `RETRY_MAX_DELAY_MS`.
    pub retry_max_delay_ms: u64,
    /// Multi-model registry. Always contains at least one entry (the default model).
    pub models: Vec<ModelConfig>,
    /// Path to the operator state file for crash recovery.
    /// The watcher persists its progress here after each poll cycle so that it
    /// can resume from the same block after a restart.
    pub state_file: String,
    /// All TEEMLVerifier contract addresses to watch.
    /// Always contains at least one address (the primary `tee_verifier_address`).
    /// Built from: `CONTRACT_ADDRESSES` env var (comma-separated),
    /// or `contracts` array in TOML, falling back to `tee_verifier_address`.
    pub contract_addresses: Vec<String>,
    /// Total request timeout for enclave HTTP requests (seconds).
    /// Env var: `ENCLAVE_TIMEOUT_SECS`. Default: 30.
    pub enclave_timeout_secs: u64,
    /// Total request timeout for prover HTTP requests (seconds).
    /// Proof generation can take several minutes, so default is generous.
    /// Env var: `PROVER_TIMEOUT_SECS`. Default: 300.
    pub prover_timeout_secs: u64,
    /// Time in seconds before an unacknowledged critical alert is escalated.
    /// A value of 0 means immediate escalation, which is almost certainly
    /// unintended (likely a misconfiguration). Default: 900 (15 minutes).
    /// Env var: `ESCALATION_TIMEOUT_SECS`.
    pub escalation_timeout_secs: u64,
    /// URL for a remote prover service (optional).
    /// When set via config, provides a default for `ProverMode::Http`.
    /// Env var: `PROVER_URL`.
    pub prover_url: Option<String>,
    /// Port for the health/metrics HTTP server.
    /// Used as default when `--metrics-port` CLI flag is not provided.
    /// Env var: `METRICS_PORT`. Default: 9090.
    pub metrics_port: u16,
    /// Bind address for the metrics HTTP server.
    /// Env var: `METRICS_BIND_ADDR`. Default: "127.0.0.1".
    pub metrics_bind_addr: String,
    /// Dry-run mode: simulate the full flow without sending on-chain transactions.
    /// Used as default when `--dry-run` CLI flag is not provided.
    /// Env var: `DRY_RUN`. Default: false.
    pub dry_run: bool,
    /// Generic retry: maximum number of attempts.
    /// Env var: `RETRY_MAX_ATTEMPTS`. Default: same as `max_proof_retries`.
    pub retry_max_attempts: u32,
    /// Prover-specific retry: maximum number of retries (default: 3).
    /// Env var: `PROVER_MAX_RETRIES`.
    pub prover_max_retries: u32,
    /// Prover-specific retry: base delay in seconds between retries (default: 5).
    /// Env var: `PROVER_RETRY_DELAY_SECS`.
    pub prover_retry_delay_secs: u64,
    /// Prover-specific retry: maximum delay in seconds for exponential backoff cap (default: 300).
    /// Env var: `PROVER_RETRY_MAX_DELAY_SECS`.
    pub prover_retry_max_delay_secs: u64,
    /// Directory containing circuit description JSON files for local pre-verification.
    /// Files should be named `<circuit_hash_hex>.json`.
    /// Env var: `CIRCUIT_DESC_DIR`. Default: "" (disabled).
    pub circuit_desc_dir: String,
    /// Base directory for the append-only proof archive.
    /// Env var: `PROOF_ARCHIVE_DIR`. Default: "" (disabled).
    pub proof_archive_dir: String,
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
        let private_key: SecretString = std::env::var("OPERATOR_PRIVATE_KEY")
            .ok()
            .or(file_cfg.private_key)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "OPERATOR_PRIVATE_KEY is required (set env var or private_key in config file)"
                )
            })?
            .into();

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

        let attestation_freshness_secs = std::env::var("ATTESTATION_FRESHNESS_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.attestation_freshness_secs)
            .unwrap_or(600);

        if attestation_freshness_secs > 1800 {
            tracing::warn!(
                "ATTESTATION_FRESHNESS_SECS={} exceeds 30 minutes — consider clock skew risks",
                attestation_freshness_secs
            );
        }

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

        // Generic retry parameters.
        // RETRY_MAX_ATTEMPTS overrides max_proof_retries when set.
        let retry_max_attempts = std::env::var("RETRY_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .or(file_cfg.retry_max_attempts)
            .unwrap_or(max_proof_retries);

        let retry_base_delay_ms = std::env::var("RETRY_BASE_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.retry_base_delay_ms)
            .unwrap_or(1000);

        let retry_max_delay_ms = std::env::var("RETRY_MAX_DELAY_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.retry_max_delay_ms)
            .unwrap_or(30000);

        let state_file = std::env::var("STATE_FILE")
            .ok()
            .or(file_cfg.state_file)
            .unwrap_or_else(|| "./operator-state.json".to_string());

        let enclave_timeout_secs = std::env::var("ENCLAVE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.enclave_timeout_secs)
            .unwrap_or(30);

        let prover_timeout_secs = std::env::var("PROVER_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.prover_timeout_secs)
            .unwrap_or(300);

        // Prover-specific retry parameters.
        let prover_max_retries = std::env::var("PROVER_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .or(file_cfg.prover_max_retries)
            .unwrap_or(3);

        let prover_retry_delay_secs = std::env::var("PROVER_RETRY_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.prover_retry_delay_secs)
            .unwrap_or(5);

        let prover_retry_max_delay_secs = std::env::var("PROVER_RETRY_MAX_DELAY_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.prover_retry_max_delay_secs)
            .unwrap_or(300);

        let escalation_timeout_secs = std::env::var("ESCALATION_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .or(file_cfg.escalation_timeout_secs)
            .unwrap_or(900);

        // Build the contract addresses list.
        // Priority: CONTRACT_ADDRESSES env var > contracts array in TOML > tee_verifier_address.
        let contract_addresses = if let Ok(addrs_str) = std::env::var("CONTRACT_ADDRESSES") {
            // Comma-separated list of addresses from env var
            let parsed: Vec<String> = addrs_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if parsed.is_empty() {
                vec![tee_verifier_address.clone()]
            } else {
                parsed
            }
        } else if !file_cfg.contracts.is_empty() {
            // Array from TOML config
            file_cfg.contracts
        } else {
            // Fall back to single tee_verifier_address
            vec![tee_verifier_address.clone()]
        };

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

        let dry_run = std::env::var("DRY_RUN")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .or(file_cfg.dry_run)
            .unwrap_or(false);

        let metrics_port = std::env::var("METRICS_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .or(file_cfg.metrics_port)
            .unwrap_or(9090);

        let metrics_bind_addr = std::env::var("METRICS_BIND_ADDR")
            .ok()
            .or(file_cfg.metrics_bind_addr)
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let prover_url = std::env::var("PROVER_URL").ok().or(file_cfg.prover_url);

        let circuit_desc_dir = std::env::var("CIRCUIT_DESC_DIR")
            .ok()
            .or_else(|| file_cfg.circuit_desc_dir.clone())
            .unwrap_or_default();

        let proof_archive_dir = std::env::var("PROOF_ARCHIVE_DIR")
            .ok()
            .or_else(|| file_cfg.proof_archive_dir.clone())
            .unwrap_or_default();

        let config = Self {
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
            attestation_freshness_secs,
            max_proof_retries,
            proof_retry_delay_secs,
            webhook_url,
            retry_max_attempts,
            retry_base_delay_ms,
            retry_max_delay_ms,
            models,
            state_file,
            contract_addresses,
            enclave_timeout_secs,
            prover_timeout_secs,
            prover_max_retries,
            prover_retry_delay_secs,
            prover_retry_max_delay_secs,
            escalation_timeout_secs,
            dry_run,
            metrics_port,
            metrics_bind_addr,
            prover_url,
            circuit_desc_dir,
            proof_archive_dir,
        };

        // Run validation during config loading so issues are caught early.
        // Hard errors are returned immediately; soft warnings are emitted via tracing::warn.
        if let Err(validation_errors) = config.validate() {
            // Log all issues before returning the combined error.
            for msg in &validation_errors {
                tracing::error!(msg = %msg, "Config validation error");
            }
            return Err(anyhow::anyhow!(
                "Configuration validation failed:\n  - {}",
                validation_errors.join("\n  - ")
            ));
        }

        Ok(config)
    }

    /// Returns the primary contract address (the first one) for backward compatibility.
    #[allow(dead_code)]
    pub fn contract_address(&self) -> &str {
        &self.contract_addresses[0]
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

    /// Validate all configuration values for correctness.
    ///
    /// Checks:
    /// - RPC URL starts with `http://`, `https://`, `ws://`, or `wss://`
    /// - RPC URL uses `https://` or `wss://` for non-localhost targets (warns on `http://`)
    /// - Private key is valid hex (64 hex chars or `0x`-prefixed 66 chars)
    /// - Contract addresses are valid Ethereum addresses (`0x` + 40 hex chars)
    /// - `prover_max_retries` <= 10 (warns if higher)
    /// - `retry_max_attempts` <= 10 (warns if higher, errors above 100)
    /// - Prover retry backoff cap >= base delay
    /// - `enclave_url` is a valid URL with scheme and host
    /// - `escalation_timeout_secs` > 0 (warns if 0 -- immediate escalation is likely unintended)
    /// - Numeric config values are in reasonable ranges
    /// - URL fields have valid schemes
    ///
    /// Returns `Ok(())` if valid, or `Err(Vec<String>)` with all validation errors.
    /// Soft issues are emitted via `tracing::warn!` and do not cause a hard failure.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors: Vec<String> = Vec::new();

        // 1. RPC URL must have a valid scheme
        if !self.rpc_url.starts_with("http://")
            && !self.rpc_url.starts_with("https://")
            && !self.rpc_url.starts_with("ws://")
            && !self.rpc_url.starts_with("wss://")
        {
            errors.push(format!(
                "rpc_url '{}' is invalid: must start with http://, https://, ws://, or wss://",
                self.rpc_url
            ));
        }

        // 1b. Warn when using unencrypted transport for non-localhost RPC URLs.
        // This strongly suggests a mainnet or testnet endpoint where HTTPS should be used.
        if (self.rpc_url.starts_with("http://") || self.rpc_url.starts_with("ws://"))
            && !self.rpc_url.contains("127.0.0.1")
            && !self.rpc_url.contains("localhost")
        {
            tracing::warn!(
                rpc_url = %self.rpc_url,
                "RPC URL uses unencrypted transport (http:// or ws://). \
                 Use https:// or wss:// for mainnet and testnet endpoints to prevent \
                 private key and transaction data from being intercepted."
            );
        }

        // 2. Private key must be valid hex (64 chars or 0x-prefixed 66 chars)
        {
            let pk = self.private_key.expose_secret();
            if !pk.is_empty() {
                let stripped = pk.strip_prefix("0x").unwrap_or(pk);
                if stripped.len() != 64 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
                    errors.push(
                        "private_key is invalid: must be 64 hex characters \
                         (or 0x-prefixed 66 characters)"
                            .to_string(),
                    );
                }
            }
        }

        // 3. Validate contract addresses (all must be 0x + 40 hex chars)
        Self::validate_eth_address(
            &self.tee_verifier_address,
            "tee_verifier_address",
            &mut errors,
        );
        for (i, addr) in self.contract_addresses.iter().enumerate() {
            Self::validate_eth_address(addr, &format!("contract_addresses[{}]", i), &mut errors);
        }

        // 4. Prover retry parameter warnings (soft) and errors (hard).
        if self.prover_max_retries > 10 {
            tracing::warn!(
                prover_max_retries = self.prover_max_retries,
                "prover_max_retries is greater than 10. High retry counts can delay \
                 dispute resolution and waste resources. Consider reducing it."
            );
        }

        if self.prover_retry_max_delay_secs < self.prover_retry_delay_secs {
            errors.push(format!(
                "prover_retry_max_delay_secs ({}) must be >= prover_retry_delay_secs ({})",
                self.prover_retry_max_delay_secs, self.prover_retry_delay_secs
            ));
        }

        if self.prover_retry_delay_secs == 0 {
            errors.push(
                "prover_retry_delay_secs must be > 0 (a zero delay creates a tight retry loop)"
                    .to_string(),
            );
        }

        // 6. Numeric range checks
        if self.attestation_cache_ttl > 86400 {
            errors.push(format!(
                "attestation_cache_ttl {} exceeds maximum of 86400 (24 hours)",
                self.attestation_cache_ttl
            ));
        }

        if self.attestation_freshness_secs == 0 || self.attestation_freshness_secs > 3600 {
            errors.push(format!(
                "attestation_freshness_secs {} is out of range (1-3600)",
                self.attestation_freshness_secs
            ));
        }

        if self.max_proof_retries > 100 {
            errors.push(format!(
                "max_proof_retries {} exceeds maximum of 100",
                self.max_proof_retries
            ));
        }

        if self.proof_retry_delay_secs > 3600 {
            errors.push(format!(
                "proof_retry_delay_secs {} exceeds maximum of 3600 (1 hour)",
                self.proof_retry_delay_secs
            ));
        }

        // Retry parameter range checks
        if self.retry_max_attempts > 100 {
            errors.push(format!(
                "retry_max_attempts {} exceeds maximum of 100",
                self.retry_max_attempts
            ));
        } else if self.retry_max_attempts > 10 {
            tracing::warn!(
                retry_max_attempts = self.retry_max_attempts,
                "retry_max_attempts is greater than 10. High retry counts can delay \
                 error recovery and waste resources. Consider reducing it."
            );
        }

        if self.retry_base_delay_ms == 0 || self.retry_base_delay_ms > 60_000 {
            errors.push(format!(
                "retry_base_delay_ms {} is out of range (1-60000)",
                self.retry_base_delay_ms
            ));
        }

        if self.retry_max_delay_ms < self.retry_base_delay_ms {
            errors.push(format!(
                "retry_max_delay_ms ({}) must be >= retry_base_delay_ms ({})",
                self.retry_max_delay_ms, self.retry_base_delay_ms
            ));
        }

        if self.retry_max_delay_ms > 600_000 {
            errors.push(format!(
                "retry_max_delay_ms {} exceeds maximum of 600000 (10 minutes)",
                self.retry_max_delay_ms
            ));
        }

        // Enclave URL: must have valid scheme and a host component.
        if !self.enclave_url.starts_with("http://") && !self.enclave_url.starts_with("https://") {
            errors.push(format!(
                "enclave_url '{}' is invalid: must start with http:// or https://",
                self.enclave_url
            ));
        } else {
            // Verify the URL has a host (not just a bare scheme like "http://")
            let after_scheme = if self.enclave_url.starts_with("https://") {
                &self.enclave_url[8..]
            } else {
                &self.enclave_url[7..]
            };
            if after_scheme.is_empty() || after_scheme == "/" {
                errors.push(format!(
                    "enclave_url '{}' is missing a host (e.g., http://127.0.0.1:8080)",
                    self.enclave_url
                ));
            }
        }

        // Webhook URL: scheme check + SSRF protection (if set)
        if let Some(ref url) = self.webhook_url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                errors.push(format!(
                    "webhook_url '{}' is invalid: must start with http:// or https://",
                    url
                ));
            }
            if let Err(e) = crate::ssrf::validate_url_ssrf(url) {
                errors.push(format!("webhook_url SSRF check failed: {}", e));
            }
        }

        // Prover URL: SSRF protection (if set)
        if let Some(ref url) = self.prover_url {
            if let Err(e) = crate::ssrf::validate_url_ssrf(url) {
                errors.push(format!("prover_url SSRF check failed: {}", e));
            }
        }

        // Metrics port: 0 is valid (disabled / OS-assigned), but 1-1023 is
        // the privileged port range and almost certainly unintended.
        if self.metrics_port > 0 && self.metrics_port < 1024 {
            errors.push(format!(
                "metrics_port {} is in the privileged range (1-1023). \
                 Use a port >= 1024, or 0 to disable.",
                self.metrics_port
            ));
        }

        // Escalation timeout: 0 means immediate escalation, which is almost
        // certainly a misconfiguration (every critical alert would trigger
        // immediate re-notification on every check cycle).
        if self.escalation_timeout_secs == 0 {
            tracing::warn!(
                "escalation_timeout_secs is 0 (immediate escalation). This means every \
                 unacknowledged critical alert will be re-escalated on every check cycle. \
                 This is likely unintended -- consider setting it to at least 60 seconds."
            );
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate a single Ethereum address (0x + 40 hex chars).
    fn validate_eth_address(addr: &str, field_name: &str, errors: &mut Vec<String>) {
        if !addr.starts_with("0x") {
            errors.push(format!(
                "{} '{}' is invalid: must start with '0x'",
                field_name, addr
            ));
            return;
        }
        let hex_part = &addr[2..];
        if hex_part.len() != 40 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push(format!(
                "{} '{}' is invalid: must be '0x' followed by exactly 40 hex characters",
                field_name, addr
            ));
        }
    }

    /// Pre-flight validation for command handlers (Submit, Watch, Run).
    ///
    /// Runs full `validate()` plus command-specific checks. Collects all
    /// problems before returning so the operator sees every issue at once.
    pub fn validate_for_command(&self, command: &str) -> Result<(), Vec<String>> {
        // Start with full structural validation
        let mut errors = match self.validate() {
            Ok(()) => Vec::new(),
            Err(errs) => errs,
        };

        // rpc_url: warn if using the default localhost value
        if self.rpc_url == "http://127.0.0.1:8545" && std::env::var("OPERATOR_RPC_URL").is_err() {
            tracing::warn!(
                "OPERATOR_RPC_URL is not set -- using default '{}'. \
                 This is unsuitable for production.",
                self.rpc_url
            );
        }

        // private_key: should not be empty (from_env already requires it,
        // but guard against future changes)
        if self.private_key.expose_secret().is_empty() {
            errors.push(
                "OPERATOR_PRIVATE_KEY is required but empty. \
                 Set the env var or private_key in the config file."
                    .to_string(),
            );
        }

        // tee_verifier_address: should not be empty
        if self.tee_verifier_address.is_empty() {
            errors.push(
                "TEE_VERIFIER_ADDRESS is required but empty. \
                 Set the env var or tee_verifier_address in the config file."
                    .to_string(),
            );
        }

        // Command-specific checks
        match command {
            "submit" | "run" => {
                // enclave_url should be reachable for submit/run
                if self.enclave_url == "http://127.0.0.1:8080"
                    && std::env::var("ENCLAVE_URL").is_err()
                {
                    tracing::warn!(
                        "ENCLAVE_URL is not set -- using default '{}'. \
                         This is unsuitable for production.",
                        self.enclave_url
                    );
                }
            }
            _ => {}
        }

        if !errors.is_empty() {
            for msg in &errors {
                tracing::error!("{}", msg);
            }
            return Err(errors);
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
            "ATTESTATION_FRESHNESS_SECS",
            "PROVER_URL",
            "METRICS_PORT",
            "MAX_PROOF_RETRIES",
            "PROOF_RETRY_DELAY_SECS",
            "WEBHOOK_URL",
            "RETRY_MAX_ATTEMPTS",
            "RETRY_BASE_DELAY_MS",
            "RETRY_MAX_DELAY_MS",
            "DRY_RUN",
            "STATE_FILE",
            "CONTRACT_ADDRESSES",
            "ENCLAVE_TIMEOUT_SECS",
            "PROVER_TIMEOUT_SECS",
            "PROVER_MAX_RETRIES",
            "PROVER_RETRY_DELAY_SECS",
            "PROVER_RETRY_MAX_DELAY_SECS",
            "ESCALATION_TIMEOUT_SECS",
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

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

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
private_key = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
tee_verifier_address = "0x3333333333333333333333333333333333333333"
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
        assert_eq!(
            config.private_key.expose_secret(),
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        );
        assert_eq!(
            config.tee_verifier_address,
            "0x3333333333333333333333333333333333333333"
        );
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
private_key = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
tee_verifier_address = "0x3333333333333333333333333333333333333333"
nitro_verification = false
attestation_cache_ttl = 900
metrics_port = 8888
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        // Set env vars that should override TOML values
        std::env::set_var("OPERATOR_RPC_URL", "https://from-env.example.com");
        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x2222222222222222222222222222222222222222",
        );
        std::env::set_var("NITRO_VERIFICATION", "true");
        std::env::set_var("ATTESTATION_CACHE_TTL", "120");
        std::env::set_var("METRICS_PORT", "7777");

        let config = Config::from_env(Some(&path)).unwrap();

        // Env vars should win
        assert_eq!(config.rpc_url, "https://from-env.example.com");
        assert_eq!(
            config.private_key.expose_secret(),
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            config.tee_verifier_address,
            "0x2222222222222222222222222222222222222222"
        );
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
private_key = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
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
private_key = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
tee_verifier_address = "0x3333333333333333333333333333333333333333"
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

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"

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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
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

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

        let config = Config::from_env(None).unwrap();
        assert!(config.webhook_url.is_none());

        clear_all_env_vars();
    }

    #[test]
    fn test_webhook_url_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
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

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

        let config = Config::from_env(None).unwrap();
        assert!(!config.dry_run);

        clear_all_env_vars();
    }

    #[test]
    fn test_dry_run_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
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
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
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

    // ======================== Multi-contract config tests ========================

    #[test]
    fn test_single_contract_address_backward_compat() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        // When only TEE_VERIFIER_ADDRESS is set (no CONTRACT_ADDRESSES),
        // contract_addresses should contain just that one address.
        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.contract_addresses.len(), 1);
        assert_eq!(
            config.contract_addresses[0],
            "0x1111111111111111111111111111111111111111"
        );
        // contract_address() helper returns the same
        assert_eq!(
            config.contract_address(),
            "0x1111111111111111111111111111111111111111"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_contract_addresses_env_var_comma_separated() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        std::env::set_var(
            "CONTRACT_ADDRESSES",
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.contract_addresses.len(), 2);
        assert_eq!(
            config.contract_addresses[0],
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            config.contract_addresses[1],
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        // contract_address() returns the first
        assert_eq!(
            config.contract_address(),
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        // tee_verifier_address is still the original one
        assert_eq!(
            config.tee_verifier_address,
            "0x1111111111111111111111111111111111111111"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_contract_addresses_env_var_with_spaces() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        // Extra spaces around addresses should be trimmed
        std::env::set_var(
            "CONTRACT_ADDRESSES",
            " 0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa , 0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb , 0xcccccccccccccccccccccccccccccccccccccccc ",
        );

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.contract_addresses.len(), 3);
        assert_eq!(
            config.contract_addresses[0],
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            config.contract_addresses[1],
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            config.contract_addresses[2],
            "0xcccccccccccccccccccccccccccccccccccccccc"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_contract_addresses_env_var_empty_falls_back() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        // Empty CONTRACT_ADDRESSES should fall back to tee_verifier_address
        std::env::set_var("CONTRACT_ADDRESSES", "");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.contract_addresses.len(), 1);
        assert_eq!(
            config.contract_addresses[0],
            "0x1111111111111111111111111111111111111111"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_contracts_toml_array() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
contracts = [
    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "0xcccccccccccccccccccccccccccccccccccccccc"
]
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.contract_addresses.len(), 3);
        assert_eq!(
            config.contract_addresses[0],
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            config.contract_addresses[1],
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            config.contract_addresses[2],
            "0xcccccccccccccccccccccccccccccccccccccccc"
        );
        // Legacy field still populated
        assert_eq!(
            config.tee_verifier_address,
            "0x1111111111111111111111111111111111111111"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_contract_addresses_env_overrides_toml_contracts() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
contracts = [
    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
]
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        // Env var should override TOML contracts array
        std::env::set_var(
            "CONTRACT_ADDRESSES",
            "0xdddddddddddddddddddddddddddddddddddddddd",
        );

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.contract_addresses.len(), 1);
        assert_eq!(
            config.contract_addresses[0],
            "0xdddddddddddddddddddddddddddddddddddddddd"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_contracts_toml_empty_falls_back_to_single() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        // Empty contracts array in TOML should fall back to tee_verifier_address
        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
contracts = []
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.contract_addresses.len(), 1);
        assert_eq!(
            config.contract_addresses[0],
            "0x1111111111111111111111111111111111111111"
        );

        clear_all_env_vars();
    }

    #[test]
    fn test_config_file_contracts_deserialization() {
        let toml_str = r#"
contracts = [
    "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
]
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.contracts.len(), 2);
        assert_eq!(
            cf.contracts[0],
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            cf.contracts[1],
            "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }

    #[test]
    fn test_config_file_no_contracts_defaults_empty() {
        let toml_str = r#"
rpc_url = "https://rpc.example.com"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert!(cf.contracts.is_empty());
    }

    // ======================== HTTP timeout config tests ========================

    #[test]
    fn test_timeout_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.enclave_timeout_secs, 30);
        assert_eq!(config.prover_timeout_secs, 300);

        clear_all_env_vars();
    }

    #[test]
    fn test_timeout_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        std::env::set_var("ENCLAVE_TIMEOUT_SECS", "60");
        std::env::set_var("PROVER_TIMEOUT_SECS", "600");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.enclave_timeout_secs, 60);
        assert_eq!(config.prover_timeout_secs, 600);

        clear_all_env_vars();
    }

    #[test]
    fn test_timeout_from_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
enclave_timeout_secs = 45
prover_timeout_secs = 500
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.enclave_timeout_secs, 45);
        assert_eq!(config.prover_timeout_secs, 500);

        clear_all_env_vars();
    }

    #[test]
    fn test_timeout_env_overrides_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
enclave_timeout_secs = 45
prover_timeout_secs = 500
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        std::env::set_var("ENCLAVE_TIMEOUT_SECS", "10");
        std::env::set_var("PROVER_TIMEOUT_SECS", "120");

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.enclave_timeout_secs, 10);
        assert_eq!(config.prover_timeout_secs, 120);

        clear_all_env_vars();
    }

    #[test]
    fn test_timeout_invalid_env_uses_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        std::env::set_var("ENCLAVE_TIMEOUT_SECS", "not_a_number");
        std::env::set_var("PROVER_TIMEOUT_SECS", "");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.enclave_timeout_secs, 30);
        assert_eq!(config.prover_timeout_secs, 300);

        clear_all_env_vars();
    }

    #[test]
    fn test_timeout_config_file_deserialization() {
        let toml_str = r#"
enclave_timeout_secs = 15
prover_timeout_secs = 900
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.enclave_timeout_secs, Some(15));
        assert_eq!(cf.prover_timeout_secs, Some(900));
    }

    #[test]
    fn test_timeout_config_file_missing_defaults_none() {
        let toml_str = r#"
rpc_url = "https://rpc.example.com"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert!(cf.enclave_timeout_secs.is_none());
        assert!(cf.prover_timeout_secs.is_none());
    }

    // ======================== validate() tests ========================

    /// Helper: build a Config with valid defaults for validation tests.
    /// This bypasses `from_env` so we can test `validate()` in isolation
    /// without touching environment variables.
    fn make_valid_config() -> Config {
        Config {
            rpc_url: "https://rpc.example.com".to_string(),
            private_key: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
                .to_string()
                .into(),
            tee_verifier_address: "0x1111111111111111111111111111111111111111".to_string(),
            enclave_url: "http://127.0.0.1:8080".to_string(),
            model_path: "./model.json".to_string(),
            proofs_dir: "./proofs".to_string(),
            prover_stake_wei: "100000000000000000".to_string(),
            precompute_bin: "precompute_proof".to_string(),
            nitro_verification: false,
            expected_pcr0: None,
            attestation_cache_ttl: 300,
            attestation_freshness_secs: 600,
            max_proof_retries: 3,
            proof_retry_delay_secs: 10,
            webhook_url: None,
            retry_max_attempts: 3,
            retry_base_delay_ms: 1000,
            retry_max_delay_ms: 30000,
            models: vec![ModelConfig {
                name: "default".to_string(),
                path: "./model.json".to_string(),
                model_format: "auto".to_string(),
                model_hash: None,
            }],
            state_file: "./operator-state.json".to_string(),
            contract_addresses: vec!["0x1111111111111111111111111111111111111111".to_string()],
            enclave_timeout_secs: 30,
            prover_timeout_secs: 300,
            prover_max_retries: 3,
            prover_retry_delay_secs: 5,
            prover_retry_max_delay_secs: 300,
            escalation_timeout_secs: 900,
            prover_url: None,
            metrics_port: 9090,
            metrics_bind_addr: "127.0.0.1".to_string(),
            dry_run: false,
            circuit_desc_dir: String::new(),
            proof_archive_dir: String::new(),
        }
    }

    #[test]
    fn test_validate_valid_config() {
        let config = make_valid_config();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_config_with_0x_private_key() {
        let mut config = make_valid_config();
        config.private_key =
            "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string().into();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_rpc_url_no_scheme() {
        let mut config = make_valid_config();
        config.rpc_url = "rpc.example.com".to_string();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("rpc_url")));
    }

    #[test]
    fn test_validate_invalid_rpc_url_ftp() {
        let mut config = make_valid_config();
        config.rpc_url = "ftp://rpc.example.com".to_string();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("rpc_url")));
    }

    #[test]
    fn test_validate_valid_rpc_url_ws() {
        let mut config = make_valid_config();
        config.rpc_url = "ws://127.0.0.1:8545".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_rpc_url_wss() {
        let mut config = make_valid_config();
        config.rpc_url = "wss://rpc.example.com".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_private_key_too_short() {
        let mut config = make_valid_config();
        config.private_key = "abcdef".to_string().into();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("private_key")));
    }

    #[test]
    fn test_validate_invalid_private_key_non_hex() {
        let mut config = make_valid_config();
        config.private_key =
            "zzzzzz1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_string().into();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("private_key")));
    }

    #[test]
    fn test_validate_invalid_tee_verifier_address_no_0x() {
        let mut config = make_valid_config();
        config.tee_verifier_address = "1111111111111111111111111111111111111111".to_string();
        config.contract_addresses = vec![config.tee_verifier_address.clone()];
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("tee_verifier_address")));
    }

    #[test]
    fn test_validate_invalid_tee_verifier_address_too_short() {
        let mut config = make_valid_config();
        config.tee_verifier_address = "0x1234".to_string();
        config.contract_addresses = vec![config.tee_verifier_address.clone()];
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("tee_verifier_address")));
    }

    #[test]
    fn test_validate_invalid_contract_address() {
        let mut config = make_valid_config();
        config.contract_addresses = vec![
            "0x1111111111111111111111111111111111111111".to_string(),
            "not-an-address".to_string(),
        ];
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("contract_addresses[1]")));
    }

    #[test]
    fn test_validate_invalid_enclave_url() {
        let mut config = make_valid_config();
        config.enclave_url = "ftp://enclave:8080".to_string();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("enclave_url")));
    }

    #[test]
    fn test_validate_enclave_url_missing_host() {
        let mut config = make_valid_config();
        config.enclave_url = "http://".to_string();
        let errors = config.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("enclave_url") && e.contains("missing a host")),
            "Expected error about missing host, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_enclave_url_bare_scheme_with_slash() {
        let mut config = make_valid_config();
        config.enclave_url = "https:///".to_string();
        // "https:///" has an empty host followed by a path slash -- should fail
        // after_scheme = "/" which is caught as missing host
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("enclave_url")),
            "Expected enclave_url error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_valid_enclave_url_with_port() {
        let mut config = make_valid_config();
        config.enclave_url = "https://enclave.internal:8443".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_webhook_url() {
        let mut config = make_valid_config();
        config.webhook_url = Some("not-a-url".to_string());
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("webhook_url")));
    }

    // -- metrics_port validation --

    #[test]
    fn test_validate_metrics_port_zero_is_valid() {
        // 0 = disabled / OS-assigned; should not trigger an error.
        let mut config = make_valid_config();
        config.metrics_port = 0;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_metrics_port_privileged_range() {
        let mut config = make_valid_config();
        config.metrics_port = 80;
        let errors = config.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("metrics_port") && e.contains("privileged")),
            "Expected metrics_port privileged error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_metrics_port_valid_range() {
        let mut config = make_valid_config();
        config.metrics_port = 1024;
        assert!(config.validate().is_ok());

        config.metrics_port = 65535;
        assert!(config.validate().is_ok());
    }

    // -- escalation_timeout_secs validation --

    #[test]
    fn test_validate_escalation_timeout_zero_warns_but_valid() {
        // escalation_timeout_secs = 0 triggers a tracing::warn but is NOT a hard error.
        let mut config = make_valid_config();
        config.escalation_timeout_secs = 0;
        // Should still pass (warning only, no error).
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_escalation_timeout_nonzero_is_valid() {
        let mut config = make_valid_config();
        config.escalation_timeout_secs = 60;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_escalation_timeout_default_from_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.escalation_timeout_secs, 900);

        clear_all_env_vars();
    }

    #[test]
    fn test_escalation_timeout_from_env_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        std::env::set_var("ESCALATION_TIMEOUT_SECS", "120");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.escalation_timeout_secs, 120);

        clear_all_env_vars();
    }

    #[test]
    fn test_escalation_timeout_from_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
escalation_timeout_secs = 300
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.escalation_timeout_secs, 300);

        clear_all_env_vars();
    }

    // -- retry_max_attempts warning --

    #[test]
    fn test_validate_retry_max_attempts_above_10_warns_but_valid() {
        // retry_max_attempts between 11-100 produces a warning but is valid.
        let mut config = make_valid_config();
        config.retry_max_attempts = 15;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_retry_max_attempts_above_100_errors() {
        let mut config = make_valid_config();
        config.retry_max_attempts = 200;
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("retry_max_attempts")));
    }

    #[test]
    fn test_validate_retry_base_delay_zero_errors() {
        let mut config = make_valid_config();
        config.retry_base_delay_ms = 0;
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("retry_base_delay_ms")));
    }

    #[test]
    fn test_validate_retry_max_delay_less_than_base() {
        let mut config = make_valid_config();
        config.retry_base_delay_ms = 5000;
        config.retry_max_delay_ms = 1000;
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("retry_max_delay_ms")));
    }

    // -- prover retry validation --

    #[test]
    fn test_validate_prover_retry_delay_zero_errors() {
        let mut config = make_valid_config();
        config.prover_retry_delay_secs = 0;
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("prover_retry_delay_secs")),
            "Expected prover_retry_delay_secs error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_prover_retry_max_delay_less_than_base_errors() {
        let mut config = make_valid_config();
        config.prover_retry_delay_secs = 10;
        config.prover_retry_max_delay_secs = 5;
        let errors = config.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("prover_retry_max_delay_secs")),
            "Expected prover_retry_max_delay_secs error, got: {:?}",
            errors
        );
    }

    // -- multiple errors at once --

    #[test]
    fn test_validate_multiple_errors() {
        let mut config = make_valid_config();
        config.rpc_url = "bad-url".to_string();
        config.private_key = "short".to_string().into();
        config.tee_verifier_address = "bad-addr".to_string();
        config.contract_addresses = vec!["bad-addr".to_string()];
        let errors = config.validate().unwrap_err();
        // Should collect multiple errors
        assert!(
            errors.len() >= 3,
            "Expected at least 3 errors, got {}: {:?}",
            errors.len(),
            errors
        );
    }

    // -- validate_for_command --

    #[test]
    fn test_validate_for_command_calls_validate() {
        let mut config = make_valid_config();
        config.rpc_url = "bad-url".to_string();
        let result = config.validate_for_command("submit");
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("rpc_url")));
    }

    // ======================== Prover retry env var tests ========================

    #[test]
    fn test_prover_retry_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.prover_max_retries, 3);
        assert_eq!(config.prover_retry_delay_secs, 5);
        assert_eq!(config.prover_retry_max_delay_secs, 300);

        clear_all_env_vars();
    }

    #[test]
    fn test_prover_retry_from_env_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        std::env::set_var("PROVER_MAX_RETRIES", "5");
        std::env::set_var("PROVER_RETRY_DELAY_SECS", "10");
        std::env::set_var("PROVER_RETRY_MAX_DELAY_SECS", "600");

        let config = Config::from_env(None).unwrap();
        assert_eq!(config.prover_max_retries, 5);
        assert_eq!(config.prover_retry_delay_secs, 10);
        assert_eq!(config.prover_retry_max_delay_secs, 600);

        clear_all_env_vars();
    }

    #[test]
    fn test_prover_retry_from_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
prover_max_retries = 7
prover_retry_delay_secs = 15
prover_retry_max_delay_secs = 900
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.prover_max_retries, 7);
        assert_eq!(config.prover_retry_delay_secs, 15);
        assert_eq!(config.prover_retry_max_delay_secs, 900);

        clear_all_env_vars();
    }

    #[test]
    fn test_prover_retry_env_overrides_toml() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        let toml_str = r#"
private_key = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
tee_verifier_address = "0x1111111111111111111111111111111111111111"
prover_max_retries = 2
prover_retry_delay_secs = 3
prover_retry_max_delay_secs = 100
"#;
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(toml_str.as_bytes()).unwrap();
        let path = tmpfile.path().to_str().unwrap().to_string();

        std::env::set_var("PROVER_MAX_RETRIES", "8");
        std::env::set_var("PROVER_RETRY_DELAY_SECS", "20");
        std::env::set_var("PROVER_RETRY_MAX_DELAY_SECS", "1200");

        let config = Config::from_env(Some(&path)).unwrap();
        assert_eq!(config.prover_max_retries, 8);
        assert_eq!(config.prover_retry_delay_secs, 20);
        assert_eq!(config.prover_retry_max_delay_secs, 1200);

        clear_all_env_vars();
    }

    #[test]
    fn test_prover_retry_invalid_env_uses_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_all_env_vars();

        std::env::set_var(
            "OPERATOR_PRIVATE_KEY",
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        );
        std::env::set_var(
            "TEE_VERIFIER_ADDRESS",
            "0x1111111111111111111111111111111111111111",
        );
        std::env::set_var("PROVER_MAX_RETRIES", "not_a_number");
        std::env::set_var("PROVER_RETRY_DELAY_SECS", "");
        std::env::set_var("PROVER_RETRY_MAX_DELAY_SECS", "xyz");

        let config = Config::from_env(None).unwrap();
        // Invalid values should fall back to defaults
        assert_eq!(config.prover_max_retries, 3);
        assert_eq!(config.prover_retry_delay_secs, 5);
        assert_eq!(config.prover_retry_max_delay_secs, 300);

        clear_all_env_vars();
    }

    #[test]
    fn test_validate_prover_max_retries_above_10_warns_but_valid() {
        // prover_max_retries > 10 produces a warning but is not a hard error.
        let mut config = make_valid_config();
        config.prover_max_retries = 15;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_rpc_url_http_non_localhost_warns_but_valid() {
        // Using http:// for a non-localhost URL produces a warning but is valid.
        let mut config = make_valid_config();
        config.rpc_url = "http://rpc.mainnet.example.com".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_rpc_url_http_localhost_no_warning() {
        // Using http:// for localhost should not produce a warning.
        let mut config = make_valid_config();
        config.rpc_url = "http://127.0.0.1:8545".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_prover_retry_config_file_deserialization() {
        let toml_str = r#"
prover_max_retries = 4
prover_retry_delay_secs = 8
prover_retry_max_delay_secs = 500
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert_eq!(cf.prover_max_retries, Some(4));
        assert_eq!(cf.prover_retry_delay_secs, Some(8));
        assert_eq!(cf.prover_retry_max_delay_secs, Some(500));
    }

    #[test]
    fn test_prover_retry_config_file_missing_defaults_none() {
        let toml_str = r#"
rpc_url = "https://rpc.example.com"
"#;
        let cf = ConfigFile::from_toml_str(toml_str).unwrap();
        assert!(cf.prover_max_retries.is_none());
        assert!(cf.prover_retry_delay_secs.is_none());
        assert!(cf.prover_retry_max_delay_secs.is_none());
    }

    // ======================== SSRF protection tests ========================

    #[test]
    fn test_validate_prover_url_ssrf_blocks_private_ip() {
        let mut config = make_valid_config();
        config.prover_url = Some("http://10.0.0.1:8080/prove".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("prover_url") && e.contains("SSRF")),
            "Expected SSRF error for prover_url, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_prover_url_ssrf_blocks_localhost() {
        let mut config = make_valid_config();
        config.prover_url = Some("http://localhost:8080/prove".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("prover_url") && e.contains("SSRF")),
            "Expected SSRF error for prover_url with localhost, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_prover_url_ssrf_blocks_aws_metadata() {
        let mut config = make_valid_config();
        config.prover_url = Some("http://169.254.169.254/latest/meta-data/".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("prover_url") && e.contains("SSRF")),
            "Expected SSRF error for AWS metadata endpoint, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_prover_url_ssrf_blocks_ipv6_loopback() {
        let mut config = make_valid_config();
        config.prover_url = Some("http://[::1]:8080/prove".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("prover_url") && e.contains("SSRF")),
            "Expected SSRF error for IPv6 loopback, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_prover_url_ssrf_allows_public_host() {
        let mut config = make_valid_config();
        config.prover_url = Some("https://prover.example.com/prove".to_string());
        // Should not produce any SSRF-related errors
        assert!(
            config.validate().is_ok(),
            "Public prover URL should pass SSRF validation"
        );
    }

    #[test]
    fn test_validate_webhook_url_ssrf_blocks_private_ip() {
        let mut config = make_valid_config();
        config.webhook_url = Some("https://192.168.1.1/webhook".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("webhook_url") && e.contains("SSRF")),
            "Expected SSRF error for webhook_url, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_webhook_url_ssrf_blocks_internal_hostname() {
        let mut config = make_valid_config();
        config.webhook_url = Some("https://hooks.internal/notify".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("webhook_url") && e.contains("SSRF")),
            "Expected SSRF error for .internal hostname, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_webhook_url_ssrf_allows_public_slack() {
        let mut config = make_valid_config();
        config.webhook_url = Some("https://hooks.slack.com/services/T00/B00/XXXX".to_string());
        assert!(
            config.validate().is_ok(),
            "Public Slack webhook URL should pass SSRF validation"
        );
    }

    #[test]
    fn test_validate_prover_url_none_passes() {
        let mut config = make_valid_config();
        config.prover_url = None;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_prover_url_ssrf_blocks_172_16_x() {
        let mut config = make_valid_config();
        config.prover_url = Some("http://172.16.0.1:3000/prove".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("SSRF")),
            "Expected SSRF error for 172.16.x, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_validate_prover_url_ssrf_blocks_dot_local() {
        let mut config = make_valid_config();
        config.prover_url = Some("http://myprover.local:8080/prove".to_string());
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("SSRF")),
            "Expected SSRF error for .local hostname, got: {:?}",
            errors
        );
    }
}
