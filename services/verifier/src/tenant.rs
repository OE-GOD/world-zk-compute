//! Multi-tenant API key management with file-based persistence.
//!
//! Supports multiple API keys with per-tenant rate limits and usage tracking.
//! Tenant data is stored in a JSON file for persistence across restarts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// A registered tenant with API access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    /// Unique tenant identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// API key (hex string prefixed with `zk_`).
    pub api_key: String,
    /// Maximum requests per minute for this tenant.
    pub rate_limit_rpm: u32,
    /// Whether the tenant is active (false = revoked).
    pub active: bool,
    /// Total requests made by this tenant.
    #[serde(default)]
    pub total_requests: u64,
    /// Created timestamp (ISO 8601 string).
    pub created_at: String,
    /// Last request timestamp (ISO 8601 string), if any.
    #[serde(default)]
    pub last_request_at: Option<String>,
}

/// Serializable form of the tenant store (for JSON persistence).
#[derive(Debug, Serialize, Deserialize)]
struct TenantFile {
    tenants: Vec<Tenant>,
}

/// Thread-safe tenant store with optional file persistence.
pub struct TenantStore {
    /// Map from API key -> Tenant.
    tenants_by_key: RwLock<HashMap<String, Tenant>>,
    /// Map from tenant ID -> API key (for lookups by ID).
    id_to_key: RwLock<HashMap<String, String>>,
    /// File path for persistence (None = in-memory only).
    file_path: Option<PathBuf>,
}

impl TenantStore {
    /// Create an empty in-memory tenant store.
    pub fn new() -> Self {
        Self {
            tenants_by_key: RwLock::new(HashMap::new()),
            id_to_key: RwLock::new(HashMap::new()),
            file_path: None,
        }
    }

    /// Load tenants from a JSON file. If the file doesn't exist, starts empty.
    pub fn load(path: &str) -> Self {
        let file_path = PathBuf::from(path);
        let store = Self {
            tenants_by_key: RwLock::new(HashMap::new()),
            id_to_key: RwLock::new(HashMap::new()),
            file_path: Some(file_path.clone()),
        };

        if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(contents) => match serde_json::from_str::<TenantFile>(&contents) {
                    Ok(data) => {
                        let mut by_key = store.tenants_by_key.write().unwrap();
                        let mut id_map = store.id_to_key.write().unwrap();
                        for tenant in data.tenants {
                            id_map.insert(tenant.id.clone(), tenant.api_key.clone());
                            by_key.insert(tenant.api_key.clone(), tenant);
                        }
                        drop(by_key);
                        drop(id_map);
                        tracing::info!(
                            "Loaded {} tenants from {}",
                            store.tenants_by_key.read().unwrap().len(),
                            path
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse tenant file {}: {}", path, e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read tenant file {}: {}", path, e);
                }
            }
        } else {
            tracing::info!("Tenant file {} not found, starting with empty store", path);
        }

        store
    }

    /// Look up a tenant by API key. Returns None if not found or inactive.
    pub fn get_by_key(&self, api_key: &str) -> Option<Tenant> {
        let tenants = self.tenants_by_key.read().ok()?;
        tenants.get(api_key).filter(|t| t.active).cloned()
    }

    /// Look up a tenant by ID (regardless of active status).
    pub fn get_by_id(&self, tenant_id: &str) -> Option<Tenant> {
        let id_map = self.id_to_key.read().ok()?;
        let api_key = id_map.get(tenant_id)?;
        let tenants = self.tenants_by_key.read().ok()?;
        tenants.get(api_key).cloned()
    }

    /// Create a new tenant, returning the generated API key.
    pub fn create(&self, name: &str, rate_limit_rpm: u32) -> Result<Tenant, String> {
        let id = generate_tenant_id(name);
        let api_key = generate_api_key();
        let now = now_iso8601();

        let tenant = Tenant {
            id: id.clone(),
            name: name.to_string(),
            api_key: api_key.clone(),
            rate_limit_rpm,
            active: true,
            total_requests: 0,
            created_at: now,
            last_request_at: None,
        };

        {
            let mut tenants = self.tenants_by_key.write().map_err(|e| e.to_string())?;
            let mut id_map = self.id_to_key.write().map_err(|e| e.to_string())?;

            if id_map.contains_key(&id) {
                return Err(format!("tenant '{}' already exists", id));
            }

            id_map.insert(id.clone(), api_key.clone());
            tenants.insert(api_key, tenant.clone());
        }

        self.save_to_file();
        Ok(tenant)
    }

    /// Create a tenant with a specific ID. Used for admin-specified IDs.
    pub fn create_with_id(
        &self,
        id: &str,
        name: &str,
        rate_limit_rpm: u32,
    ) -> Result<Tenant, String> {
        let api_key = generate_api_key();
        let now = now_iso8601();

        let tenant = Tenant {
            id: id.to_string(),
            name: name.to_string(),
            api_key: api_key.clone(),
            rate_limit_rpm,
            active: true,
            total_requests: 0,
            created_at: now,
            last_request_at: None,
        };

        {
            let mut tenants = self.tenants_by_key.write().map_err(|e| e.to_string())?;
            let mut id_map = self.id_to_key.write().map_err(|e| e.to_string())?;

            if id_map.contains_key(id) {
                return Err(format!("tenant '{}' already exists", id));
            }

            id_map.insert(id.to_string(), api_key.clone());
            tenants.insert(api_key, tenant.clone());
        }

        self.save_to_file();
        Ok(tenant)
    }

    /// Revoke (deactivate) a tenant by ID. Returns false if not found.
    pub fn revoke(&self, tenant_id: &str) -> bool {
        let result = {
            let id_map = match self.id_to_key.read() {
                Ok(m) => m,
                Err(_) => return false,
            };
            let api_key = match id_map.get(tenant_id) {
                Some(k) => k.clone(),
                None => return false,
            };
            drop(id_map);

            let mut tenants = match self.tenants_by_key.write() {
                Ok(t) => t,
                Err(_) => return false,
            };
            match tenants.get_mut(&api_key) {
                Some(t) => {
                    t.active = false;
                    true
                }
                None => false,
            }
        };

        if result {
            self.save_to_file();
        }
        result
    }

    /// Revoke (deactivate) a tenant by API key. Returns false if not found.
    #[allow(dead_code)]
    pub fn revoke_by_key(&self, api_key: &str) -> bool {
        let result = {
            let mut tenants = match self.tenants_by_key.write() {
                Ok(t) => t,
                Err(_) => return false,
            };
            match tenants.get_mut(api_key) {
                Some(t) => {
                    t.active = false;
                    true
                }
                None => false,
            }
        };

        if result {
            self.save_to_file();
        }
        result
    }

    /// Record a request for a tenant (by API key). Updates total count and last_request_at.
    pub fn record_request(&self, api_key: &str) {
        if let Ok(mut tenants) = self.tenants_by_key.write() {
            if let Some(t) = tenants.get_mut(api_key) {
                t.total_requests += 1;
                t.last_request_at = Some(now_iso8601());
            }
        }
        // Don't save on every request (too expensive); save periodically or on shutdown.
    }

    /// List all tenants (for admin use).
    pub fn list(&self) -> Vec<Tenant> {
        self.tenants_by_key
            .read()
            .map(|t| t.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get usage stats for a tenant by ID.
    pub fn usage(&self, tenant_id: &str) -> Option<TenantUsage> {
        let tenant = self.get_by_id(tenant_id)?;
        Some(TenantUsage {
            tenant_id: tenant.id,
            name: tenant.name,
            total_requests: tenant.total_requests,
            last_request_at: tenant.last_request_at,
            rate_limit_rpm: tenant.rate_limit_rpm,
            active: tenant.active,
        })
    }

    /// Persist current state to file (if a file path is configured).
    pub fn save_to_file(&self) {
        let file_path = match &self.file_path {
            Some(p) => p.clone(),
            None => return,
        };

        let tenants: Vec<Tenant> = match self.tenants_by_key.read() {
            Ok(t) => t.values().cloned().collect(),
            Err(_) => return,
        };

        let data = TenantFile { tenants };
        match serde_json::to_string_pretty(&data) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&file_path, json) {
                    tracing::warn!("Failed to save tenant file: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialize tenant data: {}", e);
            }
        }
    }

    /// Return the number of tenants in the store.
    pub fn count(&self) -> usize {
        self.tenants_by_key.read().map(|t| t.len()).unwrap_or(0)
    }

    /// Return the number of active tenants.
    pub fn active_count(&self) -> usize {
        self.tenants_by_key
            .read()
            .map(|t| t.values().filter(|v| v.active).count())
            .unwrap_or(0)
    }
}

impl Default for TenantStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Usage statistics for a single tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantUsage {
    pub tenant_id: String,
    pub name: String,
    pub total_requests: u64,
    pub last_request_at: Option<String>,
    pub rate_limit_rpm: u32,
    pub active: bool,
}

/// Generate a deterministic tenant ID from the name (lowercase, hyphenated).
fn generate_tenant_id(name: &str) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = base.trim_matches('-').to_string();
    // Append a short hash to avoid collisions
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let hash = simple_hash(ts);
    format!("{}-{:08x}", trimmed, hash as u32)
}

/// Generate a random API key using system entropy.
fn generate_api_key() -> String {
    // Use system time nanos + address of a stack variable for entropy.
    // This is NOT cryptographically secure -- production should use a CSPRNG.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos();
    // Mix in some additional entropy from stack address
    let stack_var: u64 = 0;
    let addr = &stack_var as *const u64 as u64;
    let seed = nanos ^ (addr as u128) ^ ((addr as u128) << 64);

    let h1 = simple_hash(seed);
    let h2 = simple_hash(seed.wrapping_add(0xdeadbeef));
    format!("zk_{:016x}{:016x}", h1, h2)
}

/// Simple mixing hash (splitmix64-like). Not cryptographic.
fn simple_hash(mut x: u128) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x as u64
}

/// Get current time as ISO 8601 string (UTC).
fn now_iso8601() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    // Simple UTC formatting without chrono dependency
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Compute year/month/day from days since 1970-01-01
    let (year, month, day) = days_to_ymd(days_since_epoch);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since epoch to (year, month, day). Civil calendar.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_by_key() {
        let store = TenantStore::new();
        let tenant = store.create("Acme Corp", 100).unwrap();
        assert!(tenant.api_key.starts_with("zk_"));
        assert_eq!(tenant.name, "Acme Corp");
        assert!(tenant.active);
        assert_eq!(tenant.rate_limit_rpm, 100);
        assert_eq!(tenant.total_requests, 0);

        // Should be retrievable by key
        let found = store.get_by_key(&tenant.api_key).unwrap();
        assert_eq!(found.id, tenant.id);
    }

    #[test]
    fn test_get_by_id() {
        let store = TenantStore::new();
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        let found = store.get_by_id("acme").unwrap();
        assert_eq!(found.api_key, tenant.api_key);
        assert_eq!(found.name, "Acme Corp");
    }

    #[test]
    fn test_duplicate_id_fails() {
        let store = TenantStore::new();
        store.create_with_id("acme", "Acme Corp", 100).unwrap();
        let err = store.create_with_id("acme", "Acme 2", 50);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_revoke_by_id() {
        let store = TenantStore::new();
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        // Should be found before revoke
        assert!(store.get_by_key(&tenant.api_key).is_some());

        // Revoke
        assert!(store.revoke("acme"));

        // Should not be found by get_by_key (which filters inactive)
        assert!(store.get_by_key(&tenant.api_key).is_none());

        // But should still be found by get_by_id (which includes inactive)
        let found = store.get_by_id("acme").unwrap();
        assert!(!found.active);
    }

    #[test]
    fn test_revoke_nonexistent_returns_false() {
        let store = TenantStore::new();
        assert!(!store.revoke("nonexistent"));
    }

    #[test]
    fn test_revoke_by_key() {
        let store = TenantStore::new();
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();
        assert!(store.revoke_by_key(&tenant.api_key));
        assert!(store.get_by_key(&tenant.api_key).is_none());
    }

    #[test]
    fn test_record_request() {
        let store = TenantStore::new();
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        store.record_request(&tenant.api_key);
        store.record_request(&tenant.api_key);

        let found = store.get_by_id("acme").unwrap();
        assert_eq!(found.total_requests, 2);
        assert!(found.last_request_at.is_some());
    }

    #[test]
    fn test_list_tenants() {
        let store = TenantStore::new();
        store.create_with_id("acme", "Acme Corp", 100).unwrap();
        store.create_with_id("beta", "Beta Inc", 200).unwrap();

        let tenants = store.list();
        assert_eq!(tenants.len(), 2);
    }

    #[test]
    fn test_usage() {
        let store = TenantStore::new();
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();
        store.record_request(&tenant.api_key);

        let usage = store.usage("acme").unwrap();
        assert_eq!(usage.tenant_id, "acme");
        assert_eq!(usage.total_requests, 1);
        assert_eq!(usage.rate_limit_rpm, 100);
        assert!(usage.active);
    }

    #[test]
    fn test_usage_nonexistent() {
        let store = TenantStore::new();
        assert!(store.usage("nonexistent").is_none());
    }

    #[test]
    fn test_count() {
        let store = TenantStore::new();
        assert_eq!(store.count(), 0);
        assert_eq!(store.active_count(), 0);

        store.create_with_id("a", "A", 100).unwrap();
        store.create_with_id("b", "B", 100).unwrap();
        assert_eq!(store.count(), 2);
        assert_eq!(store.active_count(), 2);

        store.revoke("a");
        assert_eq!(store.count(), 2);
        assert_eq!(store.active_count(), 1);
    }

    #[test]
    fn test_unknown_key_returns_none() {
        let store = TenantStore::new();
        assert!(store.get_by_key("nonexistent").is_none());
    }

    #[test]
    fn test_api_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("zk_"));
        // Should be "zk_" + 32 hex chars
        assert_eq!(key.len(), 3 + 32);
    }

    #[test]
    fn test_now_iso8601_format() {
        let ts = now_iso8601();
        // Should match YYYY-MM-DDTHH:MM:SSZ
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.len(), 20);
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
    }

    #[test]
    fn test_file_persistence() {
        let dir = std::env::temp_dir().join("verifier-tenant-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("tenants.json");
        let path_str = file_path.to_str().unwrap().to_string();

        // Create store, add tenants, save
        {
            let store = TenantStore::load(&path_str);
            store.create_with_id("acme", "Acme Corp", 100).unwrap();
            store.create_with_id("beta", "Beta Inc", 50).unwrap();
            store.record_request(&store.get_by_id("acme").unwrap().api_key);
            // save_to_file is called by create_with_id, but let's also save after recording
            store.save_to_file();
        }

        // Load from file, verify
        {
            let store = TenantStore::load(&path_str);
            assert_eq!(store.count(), 2);

            let acme = store.get_by_id("acme").unwrap();
            assert_eq!(acme.name, "Acme Corp");
            assert_eq!(acme.rate_limit_rpm, 100);
            assert_eq!(acme.total_requests, 1);
            assert!(acme.active);

            let beta = store.get_by_id("beta").unwrap();
            assert_eq!(beta.name, "Beta Inc");
            assert_eq!(beta.rate_limit_rpm, 50);
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let store = TenantStore::load("/tmp/nonexistent-tenant-file-abc123.json");
        assert_eq!(store.count(), 0);
    }
}
