//! Multi-tenant API key management with file-based persistence.
//!
//! Supports multiple API keys with per-tenant rate limits and usage tracking.
//! Tenant data is stored in a JSON file for persistence across restarts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Longest tenant name the HTTP handlers accept, in characters.
pub const MAX_TENANT_NAME_CHARS: usize = 128;

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
    /// Held for the whole of a save, so that saves reach the file one at a time.
    save_lock: Mutex<()>,
    /// File path for persistence (None = in-memory only).
    file_path: Option<PathBuf>,
}

impl TenantStore {
    /// Create an empty in-memory tenant store.
    pub fn new() -> Self {
        Self {
            tenants_by_key: RwLock::new(HashMap::new()),
            id_to_key: RwLock::new(HashMap::new()),
            save_lock: Mutex::new(()),
            file_path: None,
        }
    }

    /// Load tenants from a JSON file. If the file doesn't exist, starts empty.
    /// A file that exists but cannot be read or parsed is an error.
    pub fn load(path: &str) -> Result<Self, String> {
        let file_path = PathBuf::from(path);
        let mut by_key = HashMap::new();
        let mut id_map = HashMap::new();

        match std::fs::read_to_string(&file_path) {
            Ok(contents) => {
                let data: TenantFile = serde_json::from_str(&contents)
                    .map_err(|e| format!("failed to parse tenant file {path}: {e}"))?;
                for tenant in data.tenants {
                    id_map.insert(tenant.id.clone(), tenant.api_key.clone());
                    by_key.insert(tenant.api_key.clone(), tenant);
                }
                tracing::info!("Loaded {} tenants from {}", by_key.len(), path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("Tenant file {} not found, starting with empty store", path);
            }
            Err(e) => return Err(format!("failed to read tenant file {path}: {e}")),
        }

        Ok(Self {
            tenants_by_key: RwLock::new(by_key),
            id_to_key: RwLock::new(id_map),
            save_lock: Mutex::new(()),
            file_path: Some(file_path),
        })
    }

    /// Look up a tenant by API key. Returns None if not found or inactive.
    pub fn get_by_key(&self, api_key: &str) -> Option<Tenant> {
        let tenants = self.tenants_by_key.read().ok()?;
        tenants.get(api_key).filter(|t| t.active).cloned()
    }

    /// Look up a tenant by ID (regardless of active status).
    pub fn get_by_id(&self, tenant_id: &str) -> Option<Tenant> {
        // The `id_to_key` guard is released before `tenants_by_key` is locked:
        // `insert` takes the two locks in the opposite order.
        let api_key = {
            let id_map = self.id_to_key.read().ok()?;
            id_map.get(tenant_id)?.clone()
        };
        let tenants = self.tenants_by_key.read().ok()?;
        tenants.get(&api_key).cloned()
    }

    /// Create a new tenant, returning the generated API key.
    pub fn create(&self, name: &str, rate_limit_rpm: u32) -> Result<Tenant, String> {
        let id = generate_tenant_id(name)?;
        let api_key = generate_api_key()?;
        let now = now_iso8601();

        let tenant = Tenant {
            id,
            name: name.to_string(),
            api_key,
            rate_limit_rpm,
            active: true,
            total_requests: 0,
            created_at: now,
            last_request_at: None,
        };

        self.insert(&tenant)?;
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
        let api_key = generate_api_key()?;
        let now = now_iso8601();

        let tenant = Tenant {
            id: id.to_string(),
            name: name.to_string(),
            api_key,
            rate_limit_rpm,
            active: true,
            total_requests: 0,
            created_at: now,
            last_request_at: None,
        };

        self.insert(&tenant)?;
        self.save_to_file();
        Ok(tenant)
    }

    /// Add a tenant to both maps. Fails if its ID or its API key is already taken.
    fn insert(&self, tenant: &Tenant) -> Result<(), String> {
        let mut tenants = self.tenants_by_key.write().map_err(|e| e.to_string())?;
        let mut id_map = self.id_to_key.write().map_err(|e| e.to_string())?;

        if id_map.contains_key(&tenant.id) {
            return Err(format!("tenant '{}' already exists", tenant.id));
        }
        if tenants.contains_key(&tenant.api_key) {
            return Err("generated API key is already in use".to_string());
        }

        id_map.insert(tenant.id.clone(), tenant.api_key.clone());
        tenants.insert(tenant.api_key.clone(), tenant.clone());
        Ok(())
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
    ///
    /// The file holds every tenant's API key: it is replaced atomically and,
    /// on Unix, is readable and writable by the owner only.
    pub fn save_to_file(&self) {
        let file_path = match &self.file_path {
            Some(p) => p.clone(),
            None => return,
        };

        // Taken before the snapshot and held until the file is in place, so
        // that an older snapshot cannot be written after a newer one.
        let _saving = self.save_lock.lock().unwrap_or_else(|e| e.into_inner());

        let tenants: Vec<Tenant> = match self.tenants_by_key.read() {
            Ok(t) => t.values().cloned().collect(),
            Err(_) => return,
        };

        let data = TenantFile { tenants };
        match serde_json::to_string_pretty(&data) {
            Ok(json) => {
                if let Err(e) = write_private_file_atomic(&file_path, json.as_bytes()) {
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

/// Generate a tenant ID from the name (lowercase, hyphenated) followed by
/// eight random hex digits that keep equal names apart.
fn generate_tenant_id(name: &str) -> Result<String, String> {
    generate_tenant_id_with(name, fill_random)
}

/// [`generate_tenant_id`] with the four suffix bytes taken from `fill`.
fn generate_tenant_id_with(
    name: &str,
    fill: impl FnOnce(&mut [u8]) -> Result<(), String>,
) -> Result<String, String> {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = base.trim_matches('-').to_string();
    let mut suffix = [0u8; 4];
    fill(&mut suffix)?;
    Ok(format!("{}-{}", trimmed, hex::encode(suffix)))
}

/// Generate an API key: `zk_` followed by 32 hex digits, which encode 16 bytes
/// from the operating system's random source.
fn generate_api_key() -> Result<String, String> {
    generate_api_key_with(fill_random)
}

/// [`generate_api_key`] with the 16 bytes taken from `fill`.
fn generate_api_key_with(
    fill: impl FnOnce(&mut [u8]) -> Result<(), String>,
) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    fill(&mut bytes)?;
    Ok(format!("zk_{}", hex::encode(bytes)))
}

/// Fill `buf` from the operating system's random source.
fn fill_random(buf: &mut [u8]) -> Result<(), String> {
    getrandom::getrandom(buf).map_err(|e| format!("operating system random source failed: {e}"))
}

/// Path of the temporary file that a save of `path` is written to first.
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Replace `path` with `contents`: write a temporary file in the same
/// directory, sync it and rename it over `path`, so that a failed write leaves
/// the previous file in place. On Unix the file is created readable and
/// writable by the owner only.
fn write_private_file_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let tmp = temp_path(path);
    // A temporary file left behind by an interrupted save is not reused.
    let _ = std::fs::remove_file(&tmp);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let result = options
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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

    /// A fresh, empty directory for one test's files.
    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("verifier-tenant-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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
        let key = generate_api_key().unwrap();
        assert!(key.starts_with("zk_"));
        // Should be "zk_" + 32 hex chars
        assert_eq!(key.len(), 3 + 32);
        assert!(key[3..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_api_keys_are_distinct() {
        // Two keys made back to back on one thread, i.e. in the same instant.
        assert_ne!(generate_api_key().unwrap(), generate_api_key().unwrap());

        let keys: std::collections::HashSet<String> =
            (0..10_000).map(|_| generate_api_key().unwrap()).collect();
        assert_eq!(keys.len(), 10_000);
    }

    /// A key and a tenant-id suffix are the hex digits of the bytes they are
    /// made from, and of nothing else. (That the service makes them from the
    /// operating system's random source is one line each in
    /// `generate_api_key` and `generate_tenant_id`, and `fill_random`; this
    /// test does not reach those.)
    #[test]
    fn test_key_and_id_encode_the_random_bytes() {
        let counting = |buf: &mut [u8]| {
            for (i, byte) in buf.iter_mut().enumerate() {
                *byte = i as u8 + 0xa0;
            }
            Ok(())
        };
        assert_eq!(
            generate_api_key_with(counting).unwrap(),
            "zk_a0a1a2a3a4a5a6a7a8a9aaabacadaeaf"
        );
        assert_eq!(
            generate_tenant_id_with("Acme Corp", counting).unwrap(),
            "acme-corp-a0a1a2a3"
        );

        // A random source that fails gives no key and no ID.
        let failing = |_: &mut [u8]| Err("no entropy".to_string());
        assert_eq!(generate_api_key_with(failing).unwrap_err(), "no entropy");
        assert_eq!(
            generate_tenant_id_with("acme", failing).unwrap_err(),
            "no entropy"
        );
    }

    /// Wait until the clock ticks, so that what follows starts early in a tick.
    fn wait_for_clock_tick() {
        let started = std::time::Instant::now();
        let tick = std::time::SystemTime::now();
        while std::time::SystemTime::now() == tick {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(1),
                "the system clock did not move for a second"
            );
        }
    }

    /// A key or a tenant-id suffix derived from the clock repeats when two are
    /// made within one clock tick. Wait for a tick to start, make two at once,
    /// many times: no pair may be equal. (Where the clock steps in nanoseconds
    /// two calls never share a tick, so there this cannot tell a clock-derived
    /// value from a random one.)
    #[test]
    fn test_keys_made_within_one_clock_tick_differ() {
        for _ in 0..200 {
            wait_for_clock_tick();
            let keys = (generate_api_key().unwrap(), generate_api_key().unwrap());
            assert_ne!(keys.0, keys.1);

            wait_for_clock_tick();
            let ids = (
                generate_tenant_id("acme").unwrap(),
                generate_tenant_id("acme").unwrap(),
            );
            assert_ne!(ids.0, ids.1);
        }
    }

    #[test]
    fn test_every_tenant_gets_its_own_key() {
        let store = TenantStore::new();
        let tenants: Vec<Tenant> = (0..2000)
            .map(|i| store.create_with_id(&format!("t{i}"), "T", 60).unwrap())
            .collect();

        assert_eq!(store.count(), 2000);
        for tenant in &tenants {
            assert_eq!(store.get_by_key(&tenant.api_key).unwrap().id, tenant.id);
        }
    }

    #[test]
    fn test_tenant_id_suffix() {
        let id = generate_tenant_id("Acme Corp").unwrap();
        let suffix = id.strip_prefix("acme-corp-").unwrap();
        assert_eq!(suffix.len(), 8);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));

        // Equal names made back to back, i.e. in the same instant, still get
        // different IDs.
        let ids: Vec<String> = (0..1000)
            .map(|_| generate_tenant_id("a").unwrap())
            .collect();
        assert!(ids.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn test_duplicate_api_key_is_refused() {
        let store = TenantStore::new();
        let first = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        let mut second = first.clone();
        second.id = "beta".to_string();
        let err = store.insert(&second).unwrap_err();
        assert!(err.contains("API key"), "unexpected error: {err}");

        // The first tenant keeps its key and no second ID points at it.
        assert_eq!(store.get_by_key(&first.api_key).unwrap().id, "acme");
        assert!(store.get_by_id("beta").is_none());
        assert_eq!(store.count(), 1);
    }

    /// `create_with_id` and `usage` take the two locks in different orders.
    /// Run against each other from two threads, both must finish.
    #[test]
    fn test_concurrent_create_and_usage_do_not_deadlock() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        const ROUNDS: usize = 50_000;

        let store = Arc::new(TenantStore::new());
        store.create_with_id("acme", "Acme", 60).unwrap();
        let (done_tx, done_rx) = mpsc::channel();

        let (s, tx) = (store.clone(), done_tx.clone());
        std::thread::spawn(move || {
            for i in 0..ROUNDS {
                // 64 IDs: most calls take both locks and end in "already exists".
                let _ = s.create_with_id(&format!("t{}", i % 64), "T", 60);
            }
            let _ = tx.send(());
        });
        let (s, tx) = (store.clone(), done_tx);
        std::thread::spawn(move || {
            for _ in 0..ROUNDS {
                let _ = s.usage("acme");
            }
            let _ = tx.send(());
        });

        for _ in 0..2 {
            done_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("create_with_id and usage did not both finish within 30 s");
        }
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
            let store = TenantStore::load(&path_str).unwrap();
            store.create_with_id("acme", "Acme Corp", 100).unwrap();
            store.create_with_id("beta", "Beta Inc", 50).unwrap();
            store.record_request(&store.get_by_id("acme").unwrap().api_key);
            // save_to_file is called by create_with_id, but let's also save after recording
            store.save_to_file();
        }

        // Load from file, verify
        {
            let store = TenantStore::load(&path_str).unwrap();
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
        let store = TenantStore::load("/tmp/nonexistent-tenant-file-abc123.json").unwrap();
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_load_corrupt_file_is_an_error() {
        let dir = test_dir("corrupt");
        let file_path = dir.join("tenants.json");
        // What a save cut short leaves behind: valid JSON up to some point.
        let truncated = r#"{"tenants":[{"id":"acme","name":"Acme Corp","api_key":"zk_00"#;
        std::fs::write(&file_path, truncated).unwrap();

        let err = TenantStore::load(file_path.to_str().unwrap())
            .err()
            .expect("a corrupt tenant file must not load as an empty store");
        assert!(err.contains("failed to parse"), "unexpected error: {err}");
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), truncated);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pre-created tenant file must hold an empty tenant list; a 0-byte
    /// file is a parse error like any other truncated file.
    #[test]
    fn test_load_empty_file_is_an_error_and_empty_list_loads() {
        let dir = test_dir("empty");
        let file_path = dir.join("tenants.json");

        std::fs::write(&file_path, "").unwrap();
        let err = TenantStore::load(file_path.to_str().unwrap())
            .err()
            .expect("a 0-byte tenant file must not load as an empty store");
        assert!(err.contains("failed to parse"), "unexpected error: {err}");

        std::fs::write(&file_path, r#"{"tenants":[]}"#).unwrap();
        let store = TenantStore::load(file_path.to_str().unwrap()).unwrap();
        assert_eq!(store.count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_unreadable_file_is_an_error() {
        // A directory exists at the path but cannot be read as a file.
        let dir = test_dir("unreadable");

        let err = TenantStore::load(dir.to_str().unwrap())
            .err()
            .expect("an unreadable tenant file must not load as an empty store");
        assert!(err.contains("failed to read"), "unexpected error: {err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_tenant_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("mode");
        let file_path = dir.join("tenants.json");
        let mode = || std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;

        // A new file is created private.
        let store = TenantStore::load(file_path.to_str().unwrap()).unwrap();
        store.create_with_id("acme", "Acme Corp", 100).unwrap();
        assert_eq!(mode(), 0o600);

        // A file that was readable by others is private after the next save.
        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        store.create_with_id("beta", "Beta Inc", 50).unwrap();
        assert_eq!(mode(), 0o600);

        assert!(!temp_path(&file_path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_failed_save_keeps_previous_file() {
        let dir = test_dir("failed-save");
        let file_path = dir.join("tenants.json");
        let path_str = file_path.to_str().unwrap();

        let store = TenantStore::load(path_str).unwrap();
        store.create_with_id("acme", "Acme Corp", 100).unwrap();
        let before = std::fs::read_to_string(&file_path).unwrap();

        // The next save fails: a directory sits where its temporary file goes.
        std::fs::create_dir(temp_path(&file_path)).unwrap();
        store.create_with_id("beta", "Beta Inc", 50).unwrap();

        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), before);
        assert_eq!(TenantStore::load(path_str).unwrap().count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A save takes its snapshot only once it holds `save_lock`, so whichever
    /// save writes last has also seen the latest state.
    #[test]
    fn test_save_takes_snapshot_under_save_lock() {
        use std::sync::Arc;

        let dir = test_dir("save-order");
        let file_path = dir.join("tenants.json");
        let path_str = file_path.to_str().unwrap();

        let store = Arc::new(TenantStore::load(path_str).unwrap());
        let tenant = store.create_with_id("acme", "Acme Corp", 100).unwrap();

        let saving = store.save_lock.lock().unwrap();
        let saver = {
            let store = store.clone();
            std::thread::spawn(move || store.save_to_file())
        };
        // Let the save get as far as it can without the lock, then change the
        // state it is about to write.
        std::thread::sleep(std::time::Duration::from_millis(200));
        store.record_request(&tenant.api_key);
        drop(saving);
        saver.join().unwrap();

        let saved = TenantStore::load(path_str).unwrap();
        assert_eq!(saved.get_by_id("acme").unwrap().total_requests, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
