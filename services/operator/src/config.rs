/// Operator service configuration loaded from environment variables.
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
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Required: `OPERATOR_PRIVATE_KEY`, `TEE_VERIFIER_ADDRESS`
    pub fn from_env() -> anyhow::Result<Self> {
        let private_key = std::env::var("OPERATOR_PRIVATE_KEY")
            .map_err(|_| anyhow::anyhow!("OPERATOR_PRIVATE_KEY is required"))?;
        let tee_verifier_address = std::env::var("TEE_VERIFIER_ADDRESS")
            .map_err(|_| anyhow::anyhow!("TEE_VERIFIER_ADDRESS is required"))?;

        Ok(Self {
            rpc_url: std::env::var("OPERATOR_RPC_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8545".to_string()),
            private_key,
            tee_verifier_address,
            enclave_url: std::env::var("ENCLAVE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string()),
            model_path: std::env::var("MODEL_PATH").unwrap_or_else(|_| "./model.json".to_string()),
            proofs_dir: std::env::var("PROOFS_DIR").unwrap_or_else(|_| "./proofs".to_string()),
            prover_stake_wei: std::env::var("PROVER_STAKE")
                .unwrap_or_else(|_| "100000000000000000".to_string()),
            precompute_bin: std::env::var("PRECOMPUTE_BIN")
                .unwrap_or_else(|_| "precompute_proof".to_string()),
            nitro_verification: std::env::var("NITRO_VERIFICATION")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            expected_pcr0: std::env::var("EXPECTED_PCR0").ok(),
            attestation_cache_ttl: std::env::var("ATTESTATION_CACHE_TTL")
                .unwrap_or_else(|_| "300".to_string())
                .parse()
                .unwrap_or(300),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_defaults() {
        std::env::set_var("OPERATOR_PRIVATE_KEY", "0xdeadbeef");
        std::env::set_var("TEE_VERIFIER_ADDRESS", "0x1234");
        let config = Config::from_env().unwrap();
        assert_eq!(config.rpc_url, "http://127.0.0.1:8545");
        assert_eq!(config.enclave_url, "http://127.0.0.1:8080");
        assert_eq!(config.prover_stake_wei, "100000000000000000");
        std::env::remove_var("OPERATOR_PRIVATE_KEY");
        std::env::remove_var("TEE_VERIFIER_ADDRESS");
    }
}
