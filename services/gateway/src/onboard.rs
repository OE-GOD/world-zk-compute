//! Self-service tenant onboarding.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
pub struct OnboardRequest {
    pub company_name: String,
    pub email: String,
    #[serde(default = "default_plan")]
    pub plan: String,
}

fn default_plan() -> String {
    "free".to_string()
}

#[derive(Serialize)]
pub struct OnboardResponse {
    pub tenant_id: String,
    pub api_key: String,
    pub plan: String,
    pub rate_limit: u32,
}

/// Rate limits by plan.
pub fn rate_limit_for_plan(plan: &str) -> u32 {
    match plan {
        "free" => 60,
        "starter" => 600,
        "pro" => 6000,
        "enterprise" => 0, // unlimited
        _ => 60,
    }
}

/// Generate a tenant ID from company name.
pub fn generate_tenant_id(company_name: &str) -> String {
    let slug: String = company_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let hash = format!("{:x}", Sha256::digest(company_name.as_bytes()));
    format!("{}-{}", &slug[..slug.len().min(20)], &hash[..8])
}

/// Generate an API key.
pub fn generate_api_key() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let hash = Sha256::digest(seed.to_le_bytes());
    format!("zk_{}", hex::encode(&hash[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_tenant_id() {
        let id = generate_tenant_id("Acme Bank");
        assert!(id.starts_with("acme-bank-"));
        assert!(id.len() > 10);
    }

    #[test]
    fn test_generate_api_key() {
        let key = generate_api_key();
        assert!(key.starts_with("zk_"));
        assert_eq!(key.len(), 3 + 32); // "zk_" + 32 hex chars
    }

    #[test]
    fn test_rate_limits() {
        assert_eq!(rate_limit_for_plan("free"), 60);
        assert_eq!(rate_limit_for_plan("pro"), 6000);
        assert_eq!(rate_limit_for_plan("enterprise"), 0);
        assert_eq!(rate_limit_for_plan("unknown"), 60);
    }
}
