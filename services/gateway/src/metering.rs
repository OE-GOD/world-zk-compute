//! Usage metering — track proofs per tenant per day.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::RwLock;

/// Daily usage record for a tenant.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DailyUsage {
    pub verifications: u64,
    pub submissions: u64,
    pub proofs_generated: u64,
}

/// Thread-safe usage meter.
pub struct UsageMeter {
    /// Key: "tenant_id:YYYY-MM-DD"
    records: RwLock<HashMap<String, DailyUsage>>,
}

impl UsageMeter {
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
        }
    }

    pub fn record_verification(&self, tenant_id: &str) {
        let key = Self::key(tenant_id);
        if let Ok(mut records) = self.records.write() {
            records.entry(key).or_default().verifications += 1;
        }
    }

    pub fn record_submission(&self, tenant_id: &str) {
        let key = Self::key(tenant_id);
        if let Ok(mut records) = self.records.write() {
            records.entry(key).or_default().submissions += 1;
        }
    }

    pub fn record_proof(&self, tenant_id: &str) {
        let key = Self::key(tenant_id);
        if let Ok(mut records) = self.records.write() {
            records.entry(key).or_default().proofs_generated += 1;
        }
    }

    pub fn get_usage(&self, tenant_id: &str, date: &str) -> DailyUsage {
        let key = format!("{tenant_id}:{date}");
        self.records
            .read()
            .ok()
            .and_then(|r| r.get(&key).cloned())
            .unwrap_or_default()
    }

    pub fn get_all_usage(&self) -> HashMap<String, DailyUsage> {
        self.records.read().ok().map(|r| r.clone()).unwrap_or_default()
    }

    fn key(tenant_id: &str) -> String {
        let date = today();
        format!("{tenant_id}:{date}")
    }
}

impl Default for UsageMeter {
    fn default() -> Self {
        Self::new()
    }
}

fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    // Hinnant's algorithm
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_get() {
        let meter = UsageMeter::new();
        meter.record_verification("acme");
        meter.record_verification("acme");
        meter.record_submission("acme");

        let usage = meter.get_usage("acme", &today());
        assert_eq!(usage.verifications, 2);
        assert_eq!(usage.submissions, 1);
        assert_eq!(usage.proofs_generated, 0);
    }

    #[test]
    fn test_different_tenants() {
        let meter = UsageMeter::new();
        meter.record_verification("tenant-a");
        meter.record_verification("tenant-b");
        meter.record_verification("tenant-b");

        let a = meter.get_usage("tenant-a", &today());
        let b = meter.get_usage("tenant-b", &today());
        assert_eq!(a.verifications, 1);
        assert_eq!(b.verifications, 2);
    }

    #[test]
    fn test_unknown_tenant() {
        let meter = UsageMeter::new();
        let usage = meter.get_usage("unknown", "2026-01-01");
        assert_eq!(usage.verifications, 0);
    }
}
