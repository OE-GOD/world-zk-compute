//! Background watchdog for self-health monitoring.
//!
//! Periodically inspects the enclave's own metrics (error rate, latency,
//! rate-limited count) and logs warnings or errors when thresholds are
//! exceeded. On Linux, it also reads `/proc/self/status` to monitor
//! resident memory usage (skipped on other platforms where procfs is
//! unavailable).
//!
//! Also provides replay attack protection via [`ReplayProtection`]:
//! nonce deduplication (bounded LRU cache), timestamp freshness checks,
//! and chain ID validation.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::metrics::Metrics;

// ---------------------------------------------------------------------------
// Thresholds
// ---------------------------------------------------------------------------

/// Error rate above which we log WARN.
const ERROR_RATE_WARN: f64 = 0.05;
/// Error rate above which we log ERROR.
const ERROR_RATE_CRITICAL: f64 = 0.20;

/// Average latency (milliseconds) above which we log WARN.
const LATENCY_WARN_MS: f64 = 10_000.0;
/// Average latency (milliseconds) above which we log ERROR.
const LATENCY_CRITICAL_MS: f64 = 30_000.0;

/// Default interval between health checks (seconds).
pub const DEFAULT_CHECK_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Health status
// ---------------------------------------------------------------------------

/// Overall health status reported by the watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Result of a single check dimension (error_rate, latency, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckResult {
    Ok,
    Warn,
    Critical,
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckResult::Ok => write!(f, "ok"),
            CheckResult::Warn => write!(f, "warn"),
            CheckResult::Critical => write!(f, "critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// Detailed health response
// ---------------------------------------------------------------------------

/// JSON payload returned by `GET /health/detailed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedHealth {
    pub status: HealthStatus,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
    pub uptime_secs: u64,
    pub total_rate_limited: u64,
    pub total_inferences: u64,
    pub total_errors: u64,
    /// Resident set size in bytes (0 on non-Linux).
    pub memory_rss_bytes: u64,
    pub checks: HealthChecks,
}

/// Per-dimension check results embedded in [`DetailedHealth`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthChecks {
    pub error_rate: CheckResult,
    pub latency: CheckResult,
}

// ---------------------------------------------------------------------------
// Watchdog
// ---------------------------------------------------------------------------

/// Background health monitor.
///
/// Holds a reference to the shared [`Metrics`] and a snapshot of the
/// latest health evaluation (protected by an `RwLock` so the HTTP handler
/// can read it without blocking the periodic check).
pub struct Watchdog {
    metrics: Arc<Metrics>,
    /// Latest computed health, updated every check interval.
    latest: std::sync::RwLock<DetailedHealth>,
    /// When the watchdog was started (used for uptime if Metrics uptime
    /// is unavailable).
    _started: Instant,
}

impl Watchdog {
    /// Create a new watchdog referencing the given metrics.
    pub fn new(metrics: Arc<Metrics>) -> Self {
        let initial = Self::evaluate_health(&metrics);
        Self {
            metrics,
            latest: std::sync::RwLock::new(initial),
            _started: Instant::now(),
        }
    }

    /// Return the latest detailed health snapshot.
    pub fn detailed_health(&self) -> DetailedHealth {
        self.latest
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Run one health-check cycle: evaluate metrics, log if needed, store result.
    pub fn check(&self) {
        let health = Self::evaluate_health(&self.metrics);

        // Log warnings / errors
        if health.checks.error_rate == CheckResult::Critical {
            tracing::error!(
                error_rate = health.error_rate,
                "Watchdog: error rate critical (>{:.0}%)",
                ERROR_RATE_CRITICAL * 100.0,
            );
        } else if health.checks.error_rate == CheckResult::Warn {
            tracing::warn!(
                error_rate = health.error_rate,
                "Watchdog: error rate elevated (>{:.0}%)",
                ERROR_RATE_WARN * 100.0,
            );
        }

        if health.checks.latency == CheckResult::Critical {
            tracing::error!(
                avg_latency_ms = health.avg_latency_ms,
                "Watchdog: average latency critical (>{:.0}s)",
                LATENCY_CRITICAL_MS / 1000.0,
            );
        } else if health.checks.latency == CheckResult::Warn {
            tracing::warn!(
                avg_latency_ms = health.avg_latency_ms,
                "Watchdog: average latency elevated (>{:.0}s)",
                LATENCY_WARN_MS / 1000.0,
            );
        }

        // Memory logging (RSS is already captured in evaluate_health)
        if health.memory_rss_bytes > 0 {
            let rss_mb = health.memory_rss_bytes / (1024 * 1024);
            if rss_mb >= 1024 {
                tracing::error!(rss_mb, "Watchdog: resident memory very high");
            } else if rss_mb >= 512 {
                tracing::warn!(rss_mb, "Watchdog: resident memory elevated");
            }
        }

        // Store the snapshot
        match self.latest.write() {
            Ok(mut guard) => *guard = health,
            Err(_) => tracing::warn!("Watchdog: RwLock poisoned, health snapshot not updated"),
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Pure evaluation: computes health from a metrics snapshot.
    pub fn evaluate_health(metrics: &Metrics) -> DetailedHealth {
        let total_inferences = metrics.total_inferences.load(Ordering::Relaxed);
        let total_errors = metrics.total_errors.load(Ordering::Relaxed);
        let total_requests = total_inferences + total_errors;
        let total_rate_limited = metrics.total_rate_limited.load(Ordering::Relaxed);

        let error_rate = if total_requests == 0 {
            0.0
        } else {
            total_errors as f64 / total_requests as f64
        };

        let avg_latency_ms = metrics.avg_inference_ms();
        let uptime_secs = metrics.uptime_secs();

        let error_rate_check = if error_rate >= ERROR_RATE_CRITICAL {
            CheckResult::Critical
        } else if error_rate >= ERROR_RATE_WARN {
            CheckResult::Warn
        } else {
            CheckResult::Ok
        };

        let latency_check = if avg_latency_ms >= LATENCY_CRITICAL_MS {
            CheckResult::Critical
        } else if avg_latency_ms >= LATENCY_WARN_MS {
            CheckResult::Warn
        } else {
            CheckResult::Ok
        };

        let status = determine_status(error_rate_check, latency_check);

        let memory_rss_bytes = read_rss_bytes();

        DetailedHealth {
            status,
            error_rate,
            avg_latency_ms,
            uptime_secs,
            total_rate_limited,
            total_inferences,
            total_errors,
            memory_rss_bytes,
            checks: HealthChecks {
                error_rate: error_rate_check,
                latency: latency_check,
            },
        }
    }
}

/// Determine the overall status from individual check results.
///
/// If any check is Critical, overall status is Unhealthy.
/// If any check is Warn (and none Critical), overall status is Degraded.
/// Otherwise, Healthy.
pub fn determine_status(error_rate: CheckResult, latency: CheckResult) -> HealthStatus {
    if error_rate == CheckResult::Critical || latency == CheckResult::Critical {
        HealthStatus::Unhealthy
    } else if error_rate == CheckResult::Warn || latency == CheckResult::Warn {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    }
}

/// Spawn the background watchdog task.
///
/// Returns the [`Arc<Watchdog>`] so that the HTTP handler for
/// `/health/detailed` can query it.
pub fn spawn_watchdog(metrics: Arc<Metrics>, interval_secs: u64) -> Arc<Watchdog> {
    let watchdog = Arc::new(Watchdog::new(metrics));
    let wd = watchdog.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        // The first tick fires immediately -- run an initial check.
        interval.tick().await;
        wd.check();
        loop {
            interval.tick().await;
            wd.check();
        }
    });
    watchdog
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

/// Read the process's resident set size (RSS) in bytes from `/proc/self/status`.
///
/// Returns 0 on non-Linux platforms or if the file cannot be read / parsed.
fn read_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(contents) = std::fs::read_to_string("/proc/self/status") {
            for line in contents.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    // Format: "VmRSS:    12345 kB"
                    let parts: Vec<&str> = rest.split_whitespace().collect();
                    if let Some(kb_str) = parts.first() {
                        if let Ok(kb) = kb_str.parse::<u64>() {
                            return kb * 1024;
                        }
                    }
                    break;
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// Replay attack protection
// ---------------------------------------------------------------------------

/// Maximum number of nonces stored in the LRU cache before the oldest are
/// evicted. 100K entries at ~64 bytes per nonce string keeps memory under
/// ~10 MB even with long nonce values.
pub const MAX_NONCE_ENTRIES: usize = 100_000;

/// Default maximum age (in seconds) for a request timestamp to be
/// considered fresh. Requests older than this are rejected.
pub const DEFAULT_MAX_REQUEST_AGE_SECS: u64 = 60;

/// Errors returned by replay protection validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    /// The nonce has already been seen.
    DuplicateNonce,
    /// The request timestamp is too old (or too far in the future).
    ExpiredTimestamp,
    /// The chain ID in the request does not match the expected chain ID.
    InvalidChainId,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::DuplicateNonce => write!(f, "duplicate nonce"),
            ReplayError::ExpiredTimestamp => write!(f, "expired timestamp"),
            ReplayError::InvalidChainId => write!(f, "invalid chain_id"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Bounded LRU nonce registry backed by a `HashMap` for O(1) lookups and a
/// `VecDeque` for FIFO eviction ordering. When the registry reaches
/// [`MAX_NONCE_ENTRIES`], the oldest nonce is evicted to make room.
pub struct NonceRegistry {
    set: HashMap<String, ()>,
    order: VecDeque<String>,
    max_entries: usize,
}

impl NonceRegistry {
    /// Create a new registry with the given capacity limit.
    pub fn new(max_entries: usize) -> Self {
        Self {
            set: HashMap::with_capacity(max_entries.min(1024)),
            order: VecDeque::with_capacity(max_entries.min(1024)),
            max_entries,
        }
    }

    /// Check whether `nonce` has been seen before.
    ///
    /// Returns `true` if the nonce is new (and records it).
    /// Returns `false` if it is a duplicate.
    pub fn check_nonce(&mut self, nonce: &str) -> bool {
        if self.set.contains_key(nonce) {
            return false;
        }

        // Evict the oldest entry if we are at capacity.
        if self.set.len() >= self.max_entries {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }

        self.set.insert(nonce.to_owned(), ());
        self.order.push_back(nonce.to_owned());
        true
    }

    /// Number of nonces currently stored.
    #[allow(dead_code)] // Used in tests
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Whether the registry is empty.
    #[allow(dead_code)] // Used in tests
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

/// Check whether `timestamp_secs` (unix epoch) is within `max_age` seconds
/// of the current time. Rejects timestamps that are older than `max_age`
/// **or** more than `max_age` seconds in the future (clock-skew guard).
pub fn check_timestamp(timestamp_secs: u64, max_age: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Reject if too old.
    if now > timestamp_secs && (now - timestamp_secs) > max_age {
        return false;
    }
    // Reject if too far in the future (clock skew protection).
    if timestamp_secs > now && (timestamp_secs - now) > max_age {
        return false;
    }
    true
}

/// Simple equality check: does the request's `chain_id` match `expected`?
pub fn check_chain_id(request_chain_id: u64, expected: u64) -> bool {
    request_chain_id == expected
}

/// Combined replay protection that validates nonce uniqueness, timestamp
/// freshness, and chain ID in a single call.
///
/// Thread-safety: wrap in `std::sync::Mutex` or `tokio::sync::Mutex` when
/// sharing across request handlers.
pub struct ReplayProtection {
    nonces: NonceRegistry,
    /// Maximum allowed age of a request timestamp (seconds).
    pub max_request_age_secs: u64,
    /// The chain ID this enclave expects.
    pub expected_chain_id: u64,
}

impl ReplayProtection {
    /// Create a new `ReplayProtection` with the given configuration.
    pub fn new(max_request_age_secs: u64, expected_chain_id: u64) -> Self {
        Self {
            nonces: NonceRegistry::new(MAX_NONCE_ENTRIES),
            max_request_age_secs,
            expected_chain_id,
        }
    }

    /// Create a `ReplayProtection` with default settings (60 s max age,
    /// chain ID 1 for mainnet).
    #[allow(dead_code)] // Used in tests
    pub fn default_mainnet() -> Self {
        Self::new(DEFAULT_MAX_REQUEST_AGE_SECS, 1)
    }

    /// Validate a request. Returns `Ok(())` if all checks pass, or the
    /// first `ReplayError` encountered (checked in order: chain_id,
    /// timestamp, nonce).
    pub fn validate_request(
        &mut self,
        nonce: &str,
        timestamp_secs: u64,
        chain_id: u64,
    ) -> Result<(), ReplayError> {
        // 1. Chain ID (cheapest check first).
        if !check_chain_id(chain_id, self.expected_chain_id) {
            return Err(ReplayError::InvalidChainId);
        }

        // 2. Timestamp freshness.
        if !check_timestamp(timestamp_secs, self.max_request_age_secs) {
            return Err(ReplayError::ExpiredTimestamp);
        }

        // 3. Nonce uniqueness (mutates state, so checked last).
        if !self.nonces.check_nonce(nonce) {
            return Err(ReplayError::DuplicateNonce);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metrics() -> Arc<Metrics> {
        Arc::new(Metrics::new())
    }

    // -- Health status determination --

    #[test]
    fn test_determine_status_all_ok() {
        assert_eq!(
            determine_status(CheckResult::Ok, CheckResult::Ok),
            HealthStatus::Healthy,
        );
    }

    #[test]
    fn test_determine_status_warn_error_rate() {
        assert_eq!(
            determine_status(CheckResult::Warn, CheckResult::Ok),
            HealthStatus::Degraded,
        );
    }

    #[test]
    fn test_determine_status_warn_latency() {
        assert_eq!(
            determine_status(CheckResult::Ok, CheckResult::Warn),
            HealthStatus::Degraded,
        );
    }

    #[test]
    fn test_determine_status_critical_error_rate() {
        assert_eq!(
            determine_status(CheckResult::Critical, CheckResult::Ok),
            HealthStatus::Unhealthy,
        );
    }

    #[test]
    fn test_determine_status_critical_latency() {
        assert_eq!(
            determine_status(CheckResult::Ok, CheckResult::Critical),
            HealthStatus::Unhealthy,
        );
    }

    #[test]
    fn test_determine_status_mixed_critical_wins() {
        assert_eq!(
            determine_status(CheckResult::Warn, CheckResult::Critical),
            HealthStatus::Unhealthy,
        );
        assert_eq!(
            determine_status(CheckResult::Critical, CheckResult::Warn),
            HealthStatus::Unhealthy,
        );
    }

    // -- Threshold checks --

    #[test]
    fn test_healthy_when_no_requests() {
        let metrics = make_metrics();
        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.error_rate, 0.0);
        assert_eq!(health.avg_latency_ms, 0.0);
        assert_eq!(health.checks.error_rate, CheckResult::Ok);
        assert_eq!(health.checks.latency, CheckResult::Ok);
    }

    #[test]
    fn test_healthy_with_good_metrics() {
        let metrics = make_metrics();
        // 98 successes, 2 errors = 2% error rate (below 5% threshold)
        for _ in 0..98 {
            metrics.record_inference(std::time::Duration::from_millis(50));
        }
        for _ in 0..2 {
            metrics.record_error(std::time::Duration::from_millis(50));
        }

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.checks.error_rate, CheckResult::Ok);
        assert_eq!(health.checks.latency, CheckResult::Ok);
        assert!((health.error_rate - 0.02).abs() < 0.001);
    }

    #[test]
    fn test_degraded_error_rate_above_5_percent() {
        let metrics = make_metrics();
        // 90 successes, 10 errors = 10% error rate (above 5%, below 20%)
        for _ in 0..90 {
            metrics.record_inference(std::time::Duration::from_millis(50));
        }
        for _ in 0..10 {
            metrics.record_error(std::time::Duration::from_millis(50));
        }

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.status, HealthStatus::Degraded);
        assert_eq!(health.checks.error_rate, CheckResult::Warn);
        assert!((health.error_rate - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_unhealthy_error_rate_above_20_percent() {
        let metrics = make_metrics();
        // 3 successes, 1 error = 25% error rate (above 20%)
        for _ in 0..3 {
            metrics.record_inference(std::time::Duration::from_millis(50));
        }
        metrics.record_error(std::time::Duration::from_millis(50));

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.status, HealthStatus::Unhealthy);
        assert_eq!(health.checks.error_rate, CheckResult::Critical);
        assert!((health.error_rate - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_degraded_high_latency() {
        let metrics = make_metrics();
        // avg latency = 15000ms (above 10s, below 30s)
        metrics.record_inference(std::time::Duration::from_millis(15_000));

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.status, HealthStatus::Degraded);
        assert_eq!(health.checks.latency, CheckResult::Warn);
        assert!((health.avg_latency_ms - 15_000.0).abs() < 1.0);
    }

    #[test]
    fn test_unhealthy_very_high_latency() {
        let metrics = make_metrics();
        // avg latency = 35000ms (above 30s)
        metrics.record_inference(std::time::Duration::from_millis(35_000));

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.status, HealthStatus::Unhealthy);
        assert_eq!(health.checks.latency, CheckResult::Critical);
        assert!((health.avg_latency_ms - 35_000.0).abs() < 1.0);
    }

    #[test]
    fn test_rate_limited_count_reported() {
        let metrics = make_metrics();
        metrics.record_rate_limited();
        metrics.record_rate_limited();
        metrics.record_rate_limited();

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.total_rate_limited, 3);
    }

    // -- Watchdog struct --

    #[test]
    fn test_watchdog_new_defaults_healthy() {
        let metrics = make_metrics();
        let wd = Watchdog::new(metrics);
        let health = wd.detailed_health();
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_watchdog_check_updates_snapshot() {
        let metrics = make_metrics();
        let wd = Watchdog::new(metrics.clone());

        // Initially healthy
        assert_eq!(wd.detailed_health().status, HealthStatus::Healthy);

        // Inject bad metrics: 3 successes, 2 errors = 40% error rate
        for _ in 0..3 {
            metrics.record_inference(std::time::Duration::from_millis(50));
        }
        for _ in 0..2 {
            metrics.record_error(std::time::Duration::from_millis(50));
        }

        // Run a check
        wd.check();

        let health = wd.detailed_health();
        assert_eq!(health.status, HealthStatus::Unhealthy);
        assert_eq!(health.checks.error_rate, CheckResult::Critical);
        assert!((health.error_rate - 0.4).abs() < 0.001);
    }

    #[test]
    fn test_watchdog_uptime_is_reported() {
        let metrics = make_metrics();
        let health = Watchdog::evaluate_health(&metrics);
        // Uptime should be very small (just created)
        assert!(health.uptime_secs <= 1);
    }

    // -- Serialization --

    #[test]
    fn test_detailed_health_json_format() {
        let metrics = make_metrics();
        metrics.record_inference(std::time::Duration::from_millis(100));

        let health = Watchdog::evaluate_health(&metrics);
        let json = serde_json::to_string(&health).expect("serialize DetailedHealth");

        // Verify expected fields are present
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"error_rate\":"));
        assert!(json.contains("\"avg_latency_ms\":"));
        assert!(json.contains("\"uptime_secs\":"));
        assert!(json.contains("\"total_rate_limited\":"));
        assert!(json.contains("\"checks\":"));
        assert!(json.contains("\"latency\":\"ok\""));
    }

    #[test]
    fn test_detailed_health_deserialize_roundtrip() {
        let metrics = make_metrics();
        metrics.record_inference(std::time::Duration::from_millis(200));
        metrics.record_error(std::time::Duration::from_millis(100));

        let health = Watchdog::evaluate_health(&metrics);
        let json = serde_json::to_string(&health).unwrap();
        let deserialized: DetailedHealth = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.status, health.status);
        assert!((deserialized.error_rate - health.error_rate).abs() < 0.0001);
        assert!((deserialized.avg_latency_ms - health.avg_latency_ms).abs() < 0.01);
        assert_eq!(deserialized.checks.error_rate, health.checks.error_rate);
        assert_eq!(deserialized.checks.latency, health.checks.latency);
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "healthy");
        assert_eq!(format!("{}", HealthStatus::Degraded), "degraded");
        assert_eq!(format!("{}", HealthStatus::Unhealthy), "unhealthy");
    }

    #[test]
    fn test_check_result_display() {
        assert_eq!(format!("{}", CheckResult::Ok), "ok");
        assert_eq!(format!("{}", CheckResult::Warn), "warn");
        assert_eq!(format!("{}", CheckResult::Critical), "critical");
    }

    // -- Edge cases --

    #[test]
    fn test_error_rate_exactly_at_warn_threshold() {
        let metrics = make_metrics();
        // 95 successes, 5 errors = exactly 5% error rate (== threshold => warn)
        for _ in 0..95 {
            metrics.record_inference(std::time::Duration::from_millis(50));
        }
        for _ in 0..5 {
            metrics.record_error(std::time::Duration::from_millis(50));
        }

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.checks.error_rate, CheckResult::Warn);
    }

    #[test]
    fn test_error_rate_exactly_at_critical_threshold() {
        let metrics = make_metrics();
        // 80 successes, 20 errors = exactly 20% error rate (== threshold => critical)
        for _ in 0..80 {
            metrics.record_inference(std::time::Duration::from_millis(50));
        }
        for _ in 0..20 {
            metrics.record_error(std::time::Duration::from_millis(50));
        }

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.checks.error_rate, CheckResult::Critical);
    }

    #[test]
    fn test_latency_exactly_at_warn_threshold() {
        let metrics = make_metrics();
        // avg latency = exactly 10000ms
        metrics.record_inference(std::time::Duration::from_millis(10_000));

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.checks.latency, CheckResult::Warn);
    }

    #[test]
    fn test_latency_exactly_at_critical_threshold() {
        let metrics = make_metrics();
        // avg latency = exactly 30000ms
        metrics.record_inference(std::time::Duration::from_millis(30_000));

        let health = Watchdog::evaluate_health(&metrics);
        assert_eq!(health.checks.latency, CheckResult::Critical);
    }

    // -----------------------------------------------------------------------
    // Replay protection tests
    // -----------------------------------------------------------------------

    /// Helper: current unix timestamp.
    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    // -- NonceRegistry --

    #[test]
    fn test_nonce_dedup() {
        let mut reg = NonceRegistry::new(10);
        assert!(reg.check_nonce("abc123"));
        // Second use of the same nonce must be rejected.
        assert!(!reg.check_nonce("abc123"));
        // A different nonce should still be accepted.
        assert!(reg.check_nonce("def456"));
    }

    #[test]
    fn test_nonce_eviction() {
        let cap = MAX_NONCE_ENTRIES; // 100_000
        let mut reg = NonceRegistry::new(cap);

        // Fill to capacity.
        for i in 0..cap {
            assert!(reg.check_nonce(&format!("n-{}", i)));
        }
        assert_eq!(reg.len(), cap);

        // Adding one more should evict the oldest ("n-0").
        assert!(reg.check_nonce("overflow"));
        assert_eq!(reg.len(), cap); // Still at cap, not cap+1.

        // "n-1" is the next oldest but was NOT evicted yet, so it is still a dup.
        assert!(!reg.check_nonce("n-1"));

        // "n-0" was evicted, so it should be accepted again.
        assert!(reg.check_nonce("n-0"));
        // Adding "n-0" evicted "n-1" (next oldest), so now "n-1" is fresh again.
        assert!(reg.check_nonce("n-1"));
    }

    // -- Timestamp checks --

    #[test]
    fn test_expired_timestamp() {
        let max_age = 60;
        // Timestamp 120 seconds in the past is too old.
        let old_ts = now_secs().saturating_sub(120);
        assert!(!check_timestamp(old_ts, max_age));
    }

    #[test]
    fn test_valid_timestamp() {
        let max_age = 60;
        // Timestamp 5 seconds in the past is fine.
        let recent_ts = now_secs().saturating_sub(5);
        assert!(check_timestamp(recent_ts, max_age));

        // Current timestamp is fine.
        assert!(check_timestamp(now_secs(), max_age));
    }

    #[test]
    fn test_future_timestamp_within_skew() {
        let max_age = 60;
        // 10 seconds in the future is within allowed skew.
        let future_ts = now_secs() + 10;
        assert!(check_timestamp(future_ts, max_age));
    }

    #[test]
    fn test_future_timestamp_exceeds_skew() {
        let max_age = 60;
        // 120 seconds in the future exceeds allowed skew.
        let far_future_ts = now_secs() + 120;
        assert!(!check_timestamp(far_future_ts, max_age));
    }

    // -- Chain ID checks --

    #[test]
    fn test_wrong_chain_id() {
        assert!(!check_chain_id(42, 1));
        assert!(!check_chain_id(11155111, 1)); // Sepolia vs mainnet
    }

    #[test]
    fn test_correct_chain_id() {
        assert!(check_chain_id(1, 1));
        assert!(check_chain_id(11155111, 11155111));
    }

    // -- ReplayProtection combined --

    #[test]
    fn test_validate_request_all_pass() {
        let mut rp = ReplayProtection::new(60, 1);
        let ts = now_secs();
        let result = rp.validate_request("unique-nonce-1", ts, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_request_duplicate() {
        let mut rp = ReplayProtection::new(60, 1);
        let ts = now_secs();

        // First request succeeds.
        assert!(rp.validate_request("nonce-dup", ts, 1).is_ok());

        // Replayed request with the same nonce fails.
        let err = rp.validate_request("nonce-dup", ts, 1).unwrap_err();
        assert_eq!(err, ReplayError::DuplicateNonce);
    }

    #[test]
    fn test_validate_request_bad_chain_id() {
        let mut rp = ReplayProtection::new(60, 1);
        let ts = now_secs();

        let err = rp.validate_request("nonce-chain", ts, 42).unwrap_err();
        assert_eq!(err, ReplayError::InvalidChainId);
    }

    #[test]
    fn test_validate_request_expired() {
        let mut rp = ReplayProtection::new(60, 1);
        let old_ts = now_secs().saturating_sub(120);

        let err = rp.validate_request("nonce-old", old_ts, 1).unwrap_err();
        assert_eq!(err, ReplayError::ExpiredTimestamp);
    }

    #[test]
    fn test_replay_error_display() {
        assert_eq!(
            format!("{}", ReplayError::DuplicateNonce),
            "duplicate nonce"
        );
        assert_eq!(
            format!("{}", ReplayError::ExpiredTimestamp),
            "expired timestamp"
        );
        assert_eq!(
            format!("{}", ReplayError::InvalidChainId),
            "invalid chain_id"
        );
    }

    #[test]
    fn test_nonce_registry_is_empty() {
        let reg = NonceRegistry::new(10);
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn test_replay_protection_default_mainnet() {
        let rp = ReplayProtection::default_mainnet();
        assert_eq!(rp.max_request_age_secs, DEFAULT_MAX_REQUEST_AGE_SECS);
        assert_eq!(rp.expected_chain_id, 1);
    }
}
