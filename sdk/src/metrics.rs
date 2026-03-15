//! Prometheus metrics for the World ZK Compute SDK.
//!
//! This module is only available when the `metrics` feature flag is enabled.
//! It provides pre-defined metrics for tracking SDK request performance:
//!
//! - `sdk_request_duration_seconds` — histogram of request durations
//! - `sdk_request_total` — counter of total requests (labeled by method and status)
//! - `sdk_retry_total` — counter of retry attempts (labeled by method)
//!
//! # Example
//!
//! ```rust,no_run
//! use world_zk_sdk::metrics::SDKMetrics;
//!
//! let metrics = SDKMetrics::new().expect("failed to create metrics");
//!
//! // Record a successful request
//! metrics.request_total.with_label_values(&["verify_single_tx", "ok"]).inc();
//! metrics.request_duration.with_label_values(&["verify_single_tx"]).observe(0.42);
//!
//! // Record a retry
//! metrics.retry_total.with_label_values(&["verify_single_tx"]).inc();
//! ```

use prometheus::{self, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};

/// Pre-registered Prometheus metrics for the SDK.
///
/// Create with [`SDKMetrics::new()`] (uses the global default registry) or
/// [`SDKMetrics::with_registry()`] (uses a custom registry).
pub struct SDKMetrics {
    /// Histogram tracking request duration in seconds.
    ///
    /// Labels: `["method"]`
    ///
    /// Default buckets: 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0
    pub request_duration: HistogramVec,

    /// Counter of total SDK requests.
    ///
    /// Labels: `["method", "status"]`
    ///
    /// `status` is typically `"ok"` or `"error"`.
    pub request_total: IntCounterVec,

    /// Counter of retry attempts.
    ///
    /// Labels: `["method"]`
    ///
    /// Incremented each time a retryable error triggers a retry attempt.
    pub retry_total: IntCounterVec,
}

impl SDKMetrics {
    /// Create and register metrics with the global default Prometheus registry.
    ///
    /// Returns an error if metrics with the same names are already registered.
    pub fn new() -> Result<Self, prometheus::Error> {
        Self::with_registry(&prometheus::default_registry())
    }

    /// Create and register metrics with a custom Prometheus registry.
    ///
    /// Returns an error if metrics with the same names are already registered
    /// in the provided registry.
    pub fn with_registry(registry: &Registry) -> Result<Self, prometheus::Error> {
        let duration_buckets = vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

        let request_duration = HistogramVec::new(
            HistogramOpts::new(
                "sdk_request_duration_seconds",
                "Duration of SDK requests in seconds",
            )
            .buckets(duration_buckets),
            &["method"],
        )?;

        let request_total = IntCounterVec::new(
            Opts::new("sdk_request_total", "Total number of SDK requests"),
            &["method", "status"],
        )?;

        let retry_total = IntCounterVec::new(
            Opts::new("sdk_retry_total", "Total number of SDK retry attempts"),
            &["method"],
        )?;

        registry.register(Box::new(request_duration.clone()))?;
        registry.register(Box::new(request_total.clone()))?;
        registry.register(Box::new(retry_total.clone()))?;

        Ok(Self {
            request_duration,
            request_total,
            retry_total,
        })
    }

    /// Record a successful request.
    ///
    /// Increments the `sdk_request_total` counter with status `"ok"` and
    /// observes the duration in the `sdk_request_duration_seconds` histogram.
    pub fn record_success(&self, method: &str, duration_secs: f64) {
        self.request_total
            .with_label_values(&[method, "ok"])
            .inc();
        self.request_duration
            .with_label_values(&[method])
            .observe(duration_secs);
    }

    /// Record a failed request.
    ///
    /// Increments the `sdk_request_total` counter with status `"error"` and
    /// observes the duration in the `sdk_request_duration_seconds` histogram.
    pub fn record_error(&self, method: &str, duration_secs: f64) {
        self.request_total
            .with_label_values(&[method, "error"])
            .inc();
        self.request_duration
            .with_label_values(&[method])
            .observe(duration_secs);
    }

    /// Record a retry attempt for the given method.
    pub fn record_retry(&self, method: &str) {
        self.retry_total.with_label_values(&[method]).inc();
    }

    /// Record a request with the given method name, status, and duration.
    ///
    /// This is a general-purpose recording method. `status` can be any string
    /// (e.g. `"ok"`, `"error"`, `"timeout"`). The duration is observed in the
    /// histogram and the counter is incremented for the `(method, status)` pair.
    pub fn record_request(&self, method: &str, status: &str, duration_secs: f64) {
        self.request_total
            .with_label_values(&[method, status])
            .inc();
        self.request_duration
            .with_label_values(&[method])
            .observe(duration_secs);
    }
}

/// Create and register SDK metrics with the given Prometheus registry.
///
/// This is a convenience function equivalent to `SDKMetrics::with_registry(registry)`.
///
/// # Errors
///
/// Returns an error if metrics with the same names are already registered
/// in the provided registry.
pub fn register_metrics(registry: &Registry) -> Result<SDKMetrics, prometheus::Error> {
    SDKMetrics::with_registry(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sdk_metrics_creation() {
        let registry = Registry::new();
        let metrics = SDKMetrics::with_registry(&registry).expect("should create metrics");

        // Verify metrics can be used without panicking
        metrics.record_success("test_method", 0.1);
        metrics.record_error("test_method", 0.2);
        metrics.record_retry("test_method");

        // Verify counter values
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["test_method", "ok"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["test_method", "error"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .retry_total
                .with_label_values(&["test_method"])
                .get(),
            1
        );
    }

    #[test]
    fn test_sdk_metrics_duplicate_registration_fails() {
        let registry = Registry::new();
        let _metrics = SDKMetrics::with_registry(&registry).expect("first should succeed");
        let result = SDKMetrics::with_registry(&registry);
        assert!(result.is_err(), "duplicate registration should fail");
    }

    #[test]
    fn test_sdk_metrics_histogram_observe() {
        let registry = Registry::new();
        let metrics = SDKMetrics::with_registry(&registry).expect("should create metrics");

        metrics.record_success("verify_single_tx", 0.42);
        metrics.record_success("verify_single_tx", 1.5);
        metrics.record_success("verify_batch", 5.0);

        let duration_metric = metrics
            .request_duration
            .with_label_values(&["verify_single_tx"]);
        let sample_count = duration_metric.get_sample_count();
        assert_eq!(sample_count, 2);

        let batch_metric = metrics
            .request_duration
            .with_label_values(&["verify_batch"]);
        assert_eq!(batch_metric.get_sample_count(), 1);
    }

    #[test]
    fn test_sdk_metrics_multiple_methods() {
        let registry = Registry::new();
        let metrics = SDKMetrics::with_registry(&registry).expect("should create metrics");

        metrics.record_success("register_circuit", 0.5);
        metrics.record_error("verify_single_tx", 2.0);
        metrics.record_retry("verify_single_tx");
        metrics.record_retry("verify_single_tx");
        metrics.record_retry("verify_batch");

        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["register_circuit", "ok"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["verify_single_tx", "error"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .retry_total
                .with_label_values(&["verify_single_tx"])
                .get(),
            2
        );
        assert_eq!(
            metrics
                .retry_total
                .with_label_values(&["verify_batch"])
                .get(),
            1
        );
    }

    #[test]
    fn test_record_request_general() {
        let registry = Registry::new();
        let metrics = SDKMetrics::with_registry(&registry).expect("should create metrics");

        metrics.record_request("submit_result", "ok", 0.5);
        metrics.record_request("submit_result", "error", 1.2);
        metrics.record_request("submit_result", "timeout", 30.0);
        metrics.record_request("claim_job", "ok", 0.1);

        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["submit_result", "ok"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["submit_result", "error"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["submit_result", "timeout"])
                .get(),
            1
        );
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["claim_job", "ok"])
                .get(),
            1
        );

        // Duration histogram should have 3 observations for submit_result
        let duration = metrics
            .request_duration
            .with_label_values(&["submit_result"]);
        assert_eq!(duration.get_sample_count(), 3);
    }

    #[test]
    fn test_register_metrics_convenience() {
        let registry = Registry::new();
        let metrics = register_metrics(&registry).expect("should create metrics");

        metrics.record_request("test_method", "ok", 0.42);
        assert_eq!(
            metrics
                .request_total
                .with_label_values(&["test_method", "ok"])
                .get(),
            1
        );
    }

    #[test]
    fn test_register_metrics_duplicate_fails() {
        let registry = Registry::new();
        let _metrics = register_metrics(&registry).expect("first should succeed");
        let result = register_metrics(&registry);
        assert!(result.is_err(), "duplicate registration should fail");
    }
}
