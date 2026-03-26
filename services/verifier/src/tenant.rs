//! Multi-tenant API key management.
//!
//! Supports multiple API keys with per-tenant rate limits and usage tracking.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// A registered tenant with API access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    /// Unique tenant identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// API key (hashed for storage, plaintext in memory).
    pub api_key: String,
    /// Maximum requests per minute (0 = unlimited).
    pub rate_limit: u32,
    /// Whether the tenant is active.
    pub active: bool,
    /// Total requests made.
    #[serde(default)]
    pub total_requests: u64,
    /// Created timestamp (Unix seconds).
    #[serde(default)]
    pub created_at: u64,
}

/// Thread-safe tenant store.
pub struct TenantStore {
    tenants: RwLock<HashMap<String, Tenant>>,
}

impl TenantStore {
    pub fn new() -> Self {
        Self {
            tenants: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new tenant. Returns the generated API key.
    pub fn register(&self, id: &str, name: &str, rate_limit: u32) -> Result<String, String> {
        let mut tenants = self.tenants.write().map_err(|e| e.to_string())?;
        if tenants.contains_key(id) {
            return Err(format!("tenant '{}' already exists", id));
        }

        let api_key = generate_api_key();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        tenants.insert(
            id.to_string(),
            Tenant {
                id: id.to_string(),
                name: name.to_string(),
                api_key: api_key.clone(),
                rate_limit,
                active: true,
                total_requests: 0,
                created_at: now,
            },
        );

        Ok(api_key)
    }

    /// Look up a tenant by API key. Returns None if not found or inactive.
    pub fn authenticate(&self, api_key: &str) -> Option<Tenant> {
        let tenants = self.tenants.read().ok()?;
        tenants
            .values()
            .find(|t| t.api_key == api_key && t.active)
            .cloned()
    }

    /// Increment request count for a tenant.
    pub fn record_request(&self, tenant_id: &str) {
        if let Ok(mut tenants) = self.tenants.write() {
            if let Some(t) = tenants.get_mut(tenant_id) {
                t.total_requests += 1;
            }
        }
    }

    /// Deactivate a tenant (soft delete).
    pub fn deactivate(&self, tenant_id: &str) -> Result<(), String> {
        let mut tenants = self.tenants.write().map_err(|e| e.to_string())?;
        match tenants.get_mut(tenant_id) {
            Some(t) => {
                t.active = false;
                Ok(())
            }
            None => Err(format!("tenant '{}' not found", tenant_id)),
        }
    }

    /// List all tenants (for admin).
    pub fn list(&self) -> Vec<Tenant> {
        self.tenants
            .read()
            .map(|t| t.values().cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for TenantStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a random API key.
fn generate_api_key() -> String {
    use std::time::UNIX_EPOCH;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seed = now.as_nanos();
    // Simple hash-based key generation (not cryptographically secure — use
    // a proper CSPRNG in production)
    let mut h: u128 = seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    format!("zk_{:032x}", h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_authenticate() {
        let store = TenantStore::new();
        let key = store.register("acme", "Acme Corp", 100).unwrap();
        assert!(key.starts_with("zk_"));

        let tenant = store.authenticate(&key).unwrap();
        assert_eq!(tenant.id, "acme");
        assert_eq!(tenant.name, "Acme Corp");
        assert!(tenant.active);
    }

    #[test]
    fn test_duplicate_register_fails() {
        let store = TenantStore::new();
        store.register("acme", "Acme", 100).unwrap();
        assert!(store.register("acme", "Acme2", 50).is_err());
    }

    #[test]
    fn test_deactivate() {
        let store = TenantStore::new();
        let key = store.register("acme", "Acme", 100).unwrap();
        assert!(store.authenticate(&key).is_some());

        store.deactivate("acme").unwrap();
        assert!(store.authenticate(&key).is_none());
    }

    #[test]
    fn test_record_request() {
        let store = TenantStore::new();
        store.register("acme", "Acme", 100).unwrap();
        store.record_request("acme");
        store.record_request("acme");

        let tenants = store.list();
        assert_eq!(tenants[0].total_requests, 2);
    }

    #[test]
    fn test_unknown_key_returns_none() {
        let store = TenantStore::new();
        assert!(store.authenticate("nonexistent").is_none());
    }
}
