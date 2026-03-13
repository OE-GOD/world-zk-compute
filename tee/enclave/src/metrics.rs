//! Metrics collection for the TEE enclave.
//!
//! Uses atomic counters so the hot path (`/infer`) never contends on a mutex.
//! The `/metrics` endpoint reads the counters with `Relaxed` ordering (sufficient
//! for monotonic counters that are only ever incremented).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Atomic metrics counters. Stored inside `AppState` (behind `Arc`).
pub struct Metrics {
    /// Total number of successful inference requests.
    pub total_inferences: AtomicU64,
    /// Total number of failed inference requests.
    pub total_errors: AtomicU64,
    /// Cumulative inference latency in microseconds (for computing average).
    pub total_latency_us: AtomicU64,
    /// Number of latency samples (same as total_inferences + total_errors, but
    /// tracked separately so the average is self-consistent even under races).
    pub latency_count: AtomicU64,
    /// Number of attestation document refreshes (background + on-demand).
    pub attestation_refreshes: AtomicU64,
    /// Number of requests rejected by rate limiting.
    pub total_rate_limited: AtomicU64,
    /// Instant when the server started.
    pub start_time: Instant,
}

impl Metrics {
    /// Create a new, zeroed metrics instance.
    pub fn new() -> Self {
        Self {
            total_inferences: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            attestation_refreshes: AtomicU64::new(0),
            total_rate_limited: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Record a successful inference with its latency.
    pub fn record_inference(&self, latency: std::time::Duration) {
        self.total_inferences.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed inference with its latency.
    pub fn record_error(&self, latency: std::time::Duration) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency.as_micros() as u64, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a rate-limited request.
    pub fn record_rate_limited(&self) {
        self.total_rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an attestation refresh.
    pub fn record_attestation_refresh(&self) {
        self.attestation_refreshes.fetch_add(1, Ordering::Relaxed);
    }

    /// Compute average inference latency in milliseconds.
    pub fn avg_inference_ms(&self) -> f64 {
        let count = self.latency_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let total_us = self.total_latency_us.load(Ordering::Relaxed);
        (total_us as f64) / (count as f64) / 1000.0
    }

    /// Seconds since the server started.
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Build a snapshot suitable for JSON serialization.
    pub fn snapshot(
        &self,
        model_name: &str,
        model_hash: &str,
        num_trees: usize,
        num_features: usize,
        num_classes: usize,
    ) -> MetricsSnapshot {
        MetricsSnapshot {
            total_inferences: self.total_inferences.load(Ordering::Relaxed),
            total_errors: self.total_errors.load(Ordering::Relaxed),
            avg_inference_ms: self.avg_inference_ms(),
            uptime_secs: self.uptime_secs(),
            model_name: model_name.to_string(),
            model_hash: model_hash.to_string(),
            attestation_refreshes: self.attestation_refreshes.load(Ordering::Relaxed),
            total_rate_limited: self.total_rate_limited.load(Ordering::Relaxed),
            num_trees,
            num_features,
            num_classes,
        }
    }
}

/// A point-in-time snapshot of all metrics, ready for JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub total_inferences: u64,
    pub total_errors: u64,
    pub avg_inference_ms: f64,
    pub uptime_secs: u64,
    pub model_name: String,
    pub model_hash: String,
    pub attestation_refreshes: u64,
    pub total_rate_limited: u64,
    pub num_trees: usize,
    pub num_features: usize,
    pub num_classes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new_is_zeroed() {
        let m = Metrics::new();
        assert_eq!(m.total_inferences.load(Ordering::Relaxed), 0);
        assert_eq!(m.total_errors.load(Ordering::Relaxed), 0);
        assert_eq!(m.total_latency_us.load(Ordering::Relaxed), 0);
        assert_eq!(m.latency_count.load(Ordering::Relaxed), 0);
        assert_eq!(m.attestation_refreshes.load(Ordering::Relaxed), 0);
        assert_eq!(m.avg_inference_ms(), 0.0);
    }

    #[test]
    fn test_record_inference() {
        let m = Metrics::new();
        m.record_inference(std::time::Duration::from_millis(10));
        m.record_inference(std::time::Duration::from_millis(20));

        assert_eq!(m.total_inferences.load(Ordering::Relaxed), 2);
        assert_eq!(m.total_errors.load(Ordering::Relaxed), 0);
        assert_eq!(m.latency_count.load(Ordering::Relaxed), 2);
        // avg = (10000 + 20000) us / 2 / 1000 = 15.0 ms
        assert!((m.avg_inference_ms() - 15.0).abs() < 0.01);
    }

    #[test]
    fn test_record_error() {
        let m = Metrics::new();
        m.record_error(std::time::Duration::from_millis(5));

        assert_eq!(m.total_inferences.load(Ordering::Relaxed), 0);
        assert_eq!(m.total_errors.load(Ordering::Relaxed), 1);
        assert_eq!(m.latency_count.load(Ordering::Relaxed), 1);
        assert!((m.avg_inference_ms() - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_record_attestation_refresh() {
        let m = Metrics::new();
        m.record_attestation_refresh();
        m.record_attestation_refresh();
        m.record_attestation_refresh();

        assert_eq!(m.attestation_refreshes.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_uptime_is_positive() {
        let m = Metrics::new();
        // Uptime should be 0 or very small (we just created it)
        assert!(m.uptime_secs() <= 1);
    }

    #[test]
    fn test_snapshot() {
        let m = Metrics::new();
        m.record_inference(std::time::Duration::from_millis(100));
        m.record_error(std::time::Duration::from_millis(50));
        m.record_attestation_refresh();

        let snap = m.snapshot("test-model", "0xabc123", 10, 4, 3);

        assert_eq!(snap.total_inferences, 1);
        assert_eq!(snap.total_errors, 1);
        assert_eq!(snap.attestation_refreshes, 1);
        assert_eq!(snap.model_name, "test-model");
        assert_eq!(snap.model_hash, "0xabc123");
        assert_eq!(snap.num_trees, 10);
        assert_eq!(snap.num_features, 4);
        assert_eq!(snap.num_classes, 3);
        // avg = (100000 + 50000) us / 2 / 1000 = 75.0 ms
        assert!((snap.avg_inference_ms - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_avg_inference_ms_no_samples() {
        let m = Metrics::new();
        // No samples should return 0.0, not NaN or panic
        assert_eq!(m.avg_inference_ms(), 0.0);
    }

    #[test]
    fn test_snapshot_serializes_to_json() {
        let m = Metrics::new();
        m.record_inference(std::time::Duration::from_millis(42));

        let snap = m.snapshot("iris-model", "0xdeadbeef", 5, 4, 2);
        let json = serde_json::to_string(&snap).expect("serialize snapshot");

        // Verify all fields are present in JSON
        assert!(json.contains("\"total_inferences\":1"));
        assert!(json.contains("\"total_errors\":0"));
        assert!(json.contains("\"avg_inference_ms\":"));
        assert!(json.contains("\"uptime_secs\":"));
        assert!(json.contains("\"model_name\":\"iris-model\""));
        assert!(json.contains("\"model_hash\":\"0xdeadbeef\""));
        assert!(json.contains("\"attestation_refreshes\":0"));
        assert!(json.contains("\"num_trees\":5"));
        assert!(json.contains("\"num_features\":4"));
        assert!(json.contains("\"num_classes\":2"));
    }
}
