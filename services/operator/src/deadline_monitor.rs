use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use alloy::primitives::{Address, B256};

/// Tracks active disputes and their deadlines, emitting warnings as deadlines approach.
///
/// The monitor maintains a set of active disputes (result_id → deadline) and
/// checks them periodically. When a deadline is within the warning threshold,
/// it logs at WARN level. When within the critical threshold, it logs at ERROR.
///
/// Usage:
/// ```ignore
/// let monitor = DeadlineMonitor::new();
///
/// // When a challenge event is received:
/// monitor.track_dispute(result_id, challenger, dispute_deadline_timestamp);
///
/// // Periodically (e.g., in the watcher loop):
/// monitor.check_deadlines(current_timestamp);
///
/// // When a dispute is resolved or finalized:
/// monitor.remove_dispute(&result_id);
/// ```
pub struct DeadlineMonitor {
    /// Active disputes: result_id → DisputeInfo
    disputes: HashMap<B256, DisputeInfo>,

    /// Warning threshold — warn when deadline is within this duration (default: 5 min)
    warn_threshold_secs: u64,

    /// Critical threshold — error when deadline is within this duration (default: 1 min)
    critical_threshold_secs: u64,

    /// Metrics counters
    pub total_warnings_issued: AtomicU64,
    pub total_critical_issued: AtomicU64,
    pub total_disputes_tracked: AtomicU64,
    pub total_disputes_expired: AtomicU64,

    /// Track which disputes have already been warned/critical to avoid log spam
    warned: HashMap<B256, AlertLevel>,
}

#[derive(Debug, Clone)]
pub struct DisputeInfo {
    /// The result ID being disputed
    pub result_id: B256,
    /// Who challenged the result
    pub challenger: Address,
    /// Unix timestamp when the dispute deadline expires
    pub deadline_timestamp: u64,
    /// When we started tracking this dispute
    pub tracked_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertLevel {
    /// Warning has been issued (5 min threshold)
    Warned,
    /// Critical alert has been issued (1 min threshold)
    Critical,
    /// Deadline has passed — expired
    Expired,
}

/// Summary of a deadline check cycle.
#[derive(Debug, Default)]
pub struct DeadlineCheckResult {
    pub total_active: usize,
    pub warnings: Vec<B256>,
    pub criticals: Vec<B256>,
    pub expired: Vec<B256>,
}

impl DeadlineMonitor {
    /// Create a new monitor with default thresholds (warn: 5 min, critical: 1 min).
    pub fn new() -> Self {
        Self {
            disputes: HashMap::new(),
            warn_threshold_secs: 300,    // 5 minutes
            critical_threshold_secs: 60, // 1 minute
            total_warnings_issued: AtomicU64::new(0),
            total_critical_issued: AtomicU64::new(0),
            total_disputes_tracked: AtomicU64::new(0),
            total_disputes_expired: AtomicU64::new(0),
            warned: HashMap::new(),
        }
    }

    /// Create a monitor with custom thresholds.
    pub fn with_thresholds(warn_secs: u64, critical_secs: u64) -> Self {
        Self {
            warn_threshold_secs: warn_secs,
            critical_threshold_secs: critical_secs,
            ..Self::new()
        }
    }

    /// Start tracking a new dispute.
    pub fn track_dispute(&mut self, result_id: B256, challenger: Address, deadline_timestamp: u64) {
        tracing::info!(
            result_id = %result_id,
            challenger = %challenger,
            deadline = deadline_timestamp,
            "Tracking new dispute deadline"
        );

        self.disputes.insert(
            result_id,
            DisputeInfo {
                result_id,
                challenger,
                deadline_timestamp,
                tracked_at: Instant::now(),
            },
        );
        self.total_disputes_tracked.fetch_add(1, Ordering::Relaxed);
    }

    /// Stop tracking a dispute (resolved or finalized).
    pub fn remove_dispute(&mut self, result_id: &B256) -> Option<DisputeInfo> {
        self.warned.remove(result_id);
        self.disputes.remove(result_id)
    }

    /// Check all active disputes against the current timestamp.
    /// Emits warnings/errors via tracing and returns a summary.
    pub fn check_deadlines(&mut self, current_timestamp: u64) -> DeadlineCheckResult {
        let mut result = DeadlineCheckResult {
            total_active: self.disputes.len(),
            ..Default::default()
        };

        for (result_id, info) in &self.disputes {
            if current_timestamp >= info.deadline_timestamp {
                // Deadline has passed
                let prev = self.warned.get(result_id);
                if prev != Some(&AlertLevel::Expired) {
                    tracing::error!(
                        result_id = %result_id,
                        challenger = %info.challenger,
                        deadline = info.deadline_timestamp,
                        "DISPUTE DEADLINE EXPIRED — challenger wins by timeout if not resolved"
                    );
                    self.warned.insert(*result_id, AlertLevel::Expired);
                    self.total_disputes_expired.fetch_add(1, Ordering::Relaxed);
                }
                result.expired.push(*result_id);
            } else {
                let remaining = info.deadline_timestamp - current_timestamp;

                if remaining <= self.critical_threshold_secs {
                    let prev = self.warned.get(result_id);
                    if prev != Some(&AlertLevel::Critical) && prev != Some(&AlertLevel::Expired) {
                        tracing::error!(
                            result_id = %result_id,
                            remaining_secs = remaining,
                            deadline = info.deadline_timestamp,
                            "CRITICAL: Dispute deadline in <{} seconds — submit proof NOW",
                            self.critical_threshold_secs
                        );
                        self.warned.insert(*result_id, AlertLevel::Critical);
                        self.total_critical_issued.fetch_add(1, Ordering::Relaxed);
                    }
                    result.criticals.push(*result_id);
                } else if remaining <= self.warn_threshold_secs {
                    let prev = self.warned.get(result_id);
                    if prev.is_none() {
                        tracing::warn!(
                            result_id = %result_id,
                            remaining_secs = remaining,
                            deadline = info.deadline_timestamp,
                            "WARNING: Dispute deadline approaching in <{} seconds",
                            self.warn_threshold_secs
                        );
                        self.warned.insert(*result_id, AlertLevel::Warned);
                        self.total_warnings_issued.fetch_add(1, Ordering::Relaxed);
                    }
                    result.warnings.push(*result_id);
                }
            }
        }

        result
    }

    /// Get the number of active disputes being tracked.
    pub fn active_count(&self) -> usize {
        self.disputes.len()
    }

    /// Get all active dispute infos.
    pub fn active_disputes(&self) -> Vec<&DisputeInfo> {
        self.disputes.values().collect()
    }

    /// Get the time remaining for a specific dispute (in seconds), or None if not tracked.
    pub fn time_remaining(&self, result_id: &B256, current_timestamp: u64) -> Option<i64> {
        self.disputes
            .get(result_id)
            .map(|info| info.deadline_timestamp as i64 - current_timestamp as i64)
    }

    /// Get metrics snapshot.
    pub fn metrics(&self) -> DeadlineMetrics {
        DeadlineMetrics {
            active_disputes: self.disputes.len() as u64,
            total_tracked: self.total_disputes_tracked.load(Ordering::Relaxed),
            total_warnings: self.total_warnings_issued.load(Ordering::Relaxed),
            total_criticals: self.total_critical_issued.load(Ordering::Relaxed),
            total_expired: self.total_disputes_expired.load(Ordering::Relaxed),
        }
    }
}

impl Default for DeadlineMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics snapshot from the deadline monitor.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeadlineMetrics {
    pub active_disputes: u64,
    pub total_tracked: u64,
    pub total_warnings: u64,
    pub total_criticals: u64,
    pub total_expired: u64,
}

/// Compute the dispute deadline timestamp from block timestamp + DISPUTE_WINDOW.
/// The TEEMLVerifier uses `DISPUTE_WINDOW = 24 hours = 86400 seconds`.
pub const DISPUTE_WINDOW_SECS: u64 = 86400;

/// Compute dispute deadline from challenge timestamp.
pub fn compute_dispute_deadline(challenge_timestamp: u64) -> u64 {
    challenge_timestamp + DISPUTE_WINDOW_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(n: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[31] = n;
        B256::from(bytes)
    }

    fn addr(n: u8) -> Address {
        let mut bytes = [0u8; 20];
        bytes[19] = n;
        Address::from(bytes)
    }

    #[test]
    fn test_new_monitor() {
        let m = DeadlineMonitor::new();
        assert_eq!(m.active_count(), 0);
        assert_eq!(m.warn_threshold_secs, 300);
        assert_eq!(m.critical_threshold_secs, 60);
    }

    #[test]
    fn test_custom_thresholds() {
        let m = DeadlineMonitor::with_thresholds(600, 120);
        assert_eq!(m.warn_threshold_secs, 600);
        assert_eq!(m.critical_threshold_secs, 120);
    }

    #[test]
    fn test_track_and_remove() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        let challenger = addr(2);

        m.track_dispute(id, challenger, 1000);
        assert_eq!(m.active_count(), 1);
        assert_eq!(m.total_disputes_tracked.load(Ordering::Relaxed), 1);

        let info = m.remove_dispute(&id);
        assert!(info.is_some());
        assert_eq!(info.unwrap().deadline_timestamp, 1000);
        assert_eq!(m.active_count(), 0);
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut m = DeadlineMonitor::new();
        assert!(m.remove_dispute(&rid(99)).is_none());
    }

    #[test]
    fn test_no_alerts_when_far_from_deadline() {
        let mut m = DeadlineMonitor::new();
        m.track_dispute(rid(1), addr(2), 10000);

        // Current time is well before deadline
        let result = m.check_deadlines(5000);
        assert_eq!(result.total_active, 1);
        assert!(result.warnings.is_empty());
        assert!(result.criticals.is_empty());
        assert!(result.expired.is_empty());
    }

    #[test]
    fn test_warning_at_5min() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        // 200 seconds remaining — within 300s warn threshold
        let result = m.check_deadlines(9800);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0], id);
        assert!(result.criticals.is_empty());
        assert_eq!(m.total_warnings_issued.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_critical_at_1min() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        // 30 seconds remaining — within 60s critical threshold
        let result = m.check_deadlines(9970);
        assert_eq!(result.criticals.len(), 1);
        assert_eq!(result.criticals[0], id);
        assert_eq!(m.total_critical_issued.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_expired() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        let result = m.check_deadlines(10001);
        assert_eq!(result.expired.len(), 1);
        assert_eq!(result.expired[0], id);
        assert_eq!(m.total_disputes_expired.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_no_duplicate_warnings() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        // First check — warning issued
        let r1 = m.check_deadlines(9800);
        assert_eq!(r1.warnings.len(), 1);
        assert_eq!(m.total_warnings_issued.load(Ordering::Relaxed), 1);

        // Second check at same time — no new warning
        let r2 = m.check_deadlines(9800);
        assert_eq!(r2.warnings.len(), 1); // Still in warnings list
        assert_eq!(m.total_warnings_issued.load(Ordering::Relaxed), 1); // But counter unchanged
    }

    #[test]
    fn test_warning_escalates_to_critical() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        // Warning phase
        m.check_deadlines(9800);
        assert_eq!(m.total_warnings_issued.load(Ordering::Relaxed), 1);
        assert_eq!(m.total_critical_issued.load(Ordering::Relaxed), 0);

        // Critical phase
        m.check_deadlines(9950);
        assert_eq!(m.total_critical_issued.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_critical_escalates_to_expired() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        m.check_deadlines(9950); // Critical
        m.check_deadlines(10001); // Expired

        assert_eq!(m.total_critical_issued.load(Ordering::Relaxed), 1);
        assert_eq!(m.total_disputes_expired.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_multiple_disputes() {
        let mut m = DeadlineMonitor::new();
        m.track_dispute(rid(1), addr(10), 10000);
        m.track_dispute(rid(2), addr(20), 11000);
        m.track_dispute(rid(3), addr(30), 12000);

        assert_eq!(m.active_count(), 3);

        // Check at 9800: rid(1) in warning range (200s left), others safe (>300s)
        let r = m.check_deadlines(9800);
        assert_eq!(r.total_active, 3);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.criticals.is_empty());
    }

    #[test]
    fn test_time_remaining() {
        let mut m = DeadlineMonitor::new();
        m.track_dispute(rid(1), addr(2), 10000);

        assert_eq!(m.time_remaining(&rid(1), 9000), Some(1000));
        assert_eq!(m.time_remaining(&rid(1), 10000), Some(0));
        assert_eq!(m.time_remaining(&rid(1), 10500), Some(-500));
        assert_eq!(m.time_remaining(&rid(99), 9000), None);
    }

    #[test]
    fn test_active_disputes_list() {
        let mut m = DeadlineMonitor::new();
        m.track_dispute(rid(1), addr(10), 10000);
        m.track_dispute(rid(2), addr(20), 20000);

        let disputes = m.active_disputes();
        assert_eq!(disputes.len(), 2);
    }

    #[test]
    fn test_metrics() {
        let mut m = DeadlineMonitor::new();
        m.track_dispute(rid(1), addr(2), 10000);
        m.track_dispute(rid(2), addr(3), 10100);
        m.check_deadlines(9800); // Should warn rid(1)
        m.check_deadlines(9950); // Should critical rid(1), warn rid(2)

        let metrics = m.metrics();
        assert_eq!(metrics.active_disputes, 2);
        assert_eq!(metrics.total_tracked, 2);
        assert!(metrics.total_warnings >= 1);
    }

    #[test]
    fn test_compute_dispute_deadline() {
        assert_eq!(compute_dispute_deadline(1000), 1000 + 86400);
    }

    #[test]
    fn test_default_impl() {
        let m = DeadlineMonitor::default();
        assert_eq!(m.active_count(), 0);
    }

    #[test]
    fn test_exact_threshold_boundary() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        // Exactly 300 seconds remaining — should trigger warning
        let r = m.check_deadlines(9700);
        assert_eq!(r.warnings.len(), 1);

        // Exactly 60 seconds remaining — should trigger critical
        let mut m2 = DeadlineMonitor::new();
        m2.track_dispute(id, addr(2), 10000);
        let r2 = m2.check_deadlines(9940);
        assert_eq!(r2.criticals.len(), 1);
    }

    #[test]
    fn test_remove_clears_alert_state() {
        let mut m = DeadlineMonitor::new();
        let id = rid(1);
        m.track_dispute(id, addr(2), 10000);

        // Trigger warning
        m.check_deadlines(9800);
        assert_eq!(m.total_warnings_issued.load(Ordering::Relaxed), 1);

        // Remove and re-add
        m.remove_dispute(&id);
        m.track_dispute(id, addr(2), 20000);

        // Should be able to warn again
        m.check_deadlines(19800);
        assert_eq!(m.total_warnings_issued.load(Ordering::Relaxed), 2);
    }
}
