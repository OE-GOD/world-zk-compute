//! Metrics collection for the indexer service.
//!
//! Uses atomic counters for lock-free metric tracking.
//! Exposes metrics in Prometheus text exposition format via GET /metrics.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::http::header;
use axum::response::IntoResponse;

/// Atomic metrics counters for the indexer.
pub struct IndexerMetrics {
    /// Total events indexed across all types.
    pub events_indexed: AtomicU64,
    /// Total blocks processed by the indexer.
    pub blocks_processed: AtomicU64,
    /// Latest block number that has been indexed.
    pub latest_block: AtomicU64,
    /// Total number of poll cycles completed.
    pub poll_cycles: AtomicU64,
    /// Total number of poll errors.
    pub poll_errors: AtomicU64,
    /// Total results stored in the database.
    pub total_results: AtomicU64,
    /// Total API requests served.
    pub api_requests: AtomicU64,
    /// Instant when the indexer started.
    pub start_time: Instant,
}

impl IndexerMetrics {
    /// Create a new, zeroed metrics instance.
    pub fn new() -> Self {
        Self {
            events_indexed: AtomicU64::new(0),
            blocks_processed: AtomicU64::new(0),
            latest_block: AtomicU64::new(0),
            poll_cycles: AtomicU64::new(0),
            poll_errors: AtomicU64::new(0),
            total_results: AtomicU64::new(0),
            api_requests: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Record that events were indexed in a block range.
    pub fn record_events(&self, count: u64) {
        self.events_indexed.fetch_add(count, Ordering::Relaxed);
    }

    /// Record blocks processed in a poll cycle.
    pub fn record_blocks(&self, count: u64) {
        self.blocks_processed.fetch_add(count, Ordering::Relaxed);
    }

    /// Update the latest indexed block number.
    pub fn set_latest_block(&self, block: u64) {
        self.latest_block.store(block, Ordering::Relaxed);
    }

    /// Record a completed poll cycle.
    pub fn record_poll_cycle(&self) {
        self.poll_cycles.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a poll error.
    pub fn record_poll_error(&self) {
        self.poll_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Update the total results count.
    pub fn set_total_results(&self, count: u64) {
        self.total_results.store(count, Ordering::Relaxed);
    }

    /// Record an API request.
    pub fn record_api_request(&self) {
        self.api_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Seconds since the indexer started.
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render_prometheus(&self) -> String {
        let events = self.events_indexed.load(Ordering::Relaxed);
        let blocks = self.blocks_processed.load(Ordering::Relaxed);
        let latest = self.latest_block.load(Ordering::Relaxed);
        let cycles = self.poll_cycles.load(Ordering::Relaxed);
        let errors = self.poll_errors.load(Ordering::Relaxed);
        let results = self.total_results.load(Ordering::Relaxed);
        let requests = self.api_requests.load(Ordering::Relaxed);
        let uptime = self.uptime_secs();

        format!(
            "\
# HELP indexer_events_indexed_total Total events indexed\n\
# TYPE indexer_events_indexed_total counter\n\
indexer_events_indexed_total {events}\n\
# HELP indexer_blocks_processed_total Total blocks processed\n\
# TYPE indexer_blocks_processed_total counter\n\
indexer_blocks_processed_total {blocks}\n\
# HELP indexer_latest_block Latest indexed block number\n\
# TYPE indexer_latest_block gauge\n\
indexer_latest_block {latest}\n\
# HELP indexer_poll_cycles_total Total poll cycles completed\n\
# TYPE indexer_poll_cycles_total counter\n\
indexer_poll_cycles_total {cycles}\n\
# HELP indexer_poll_errors_total Total poll errors\n\
# TYPE indexer_poll_errors_total counter\n\
indexer_poll_errors_total {errors}\n\
# HELP indexer_total_results Total results in database\n\
# TYPE indexer_total_results gauge\n\
indexer_total_results {results}\n\
# HELP indexer_api_requests_total Total API requests served\n\
# TYPE indexer_api_requests_total counter\n\
indexer_api_requests_total {requests}\n\
# HELP indexer_uptime_seconds Indexer uptime in seconds\n\
# TYPE indexer_uptime_seconds gauge\n\
indexer_uptime_seconds {uptime}\n"
        )
    }
}

impl Default for IndexerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum handler for the /metrics endpoint.
pub async fn metrics_handler(
    axum::extract::State(metrics): axum::extract::State<std::sync::Arc<IndexerMetrics>>,
) -> impl IntoResponse {
    let body = metrics.render_prometheus();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_zeroed() {
        let m = IndexerMetrics::new();
        assert_eq!(m.events_indexed.load(Ordering::Relaxed), 0);
        assert_eq!(m.blocks_processed.load(Ordering::Relaxed), 0);
        assert_eq!(m.latest_block.load(Ordering::Relaxed), 0);
        assert_eq!(m.poll_cycles.load(Ordering::Relaxed), 0);
        assert_eq!(m.poll_errors.load(Ordering::Relaxed), 0);
        assert_eq!(m.total_results.load(Ordering::Relaxed), 0);
        assert_eq!(m.api_requests.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_events() {
        let m = IndexerMetrics::new();
        m.record_events(5);
        m.record_events(3);
        assert_eq!(m.events_indexed.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn test_record_blocks() {
        let m = IndexerMetrics::new();
        m.record_blocks(100);
        m.record_blocks(50);
        assert_eq!(m.blocks_processed.load(Ordering::Relaxed), 150);
    }

    #[test]
    fn test_set_latest_block() {
        let m = IndexerMetrics::new();
        m.set_latest_block(42);
        assert_eq!(m.latest_block.load(Ordering::Relaxed), 42);
        m.set_latest_block(100);
        assert_eq!(m.latest_block.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn test_poll_cycle_and_errors() {
        let m = IndexerMetrics::new();
        m.record_poll_cycle();
        m.record_poll_cycle();
        m.record_poll_error();
        assert_eq!(m.poll_cycles.load(Ordering::Relaxed), 2);
        assert_eq!(m.poll_errors.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_api_requests() {
        let m = IndexerMetrics::new();
        for _ in 0..10 {
            m.record_api_request();
        }
        assert_eq!(m.api_requests.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn test_uptime() {
        let m = IndexerMetrics::new();
        assert!(m.uptime_secs() <= 1);
    }

    #[test]
    fn test_render_prometheus_format() {
        let m = IndexerMetrics::new();
        m.record_events(10);
        m.record_blocks(200);
        m.set_latest_block(500);
        m.record_poll_cycle();
        m.record_poll_error();
        m.set_total_results(42);
        m.record_api_request();

        let output = m.render_prometheus();

        assert!(output.contains("# HELP indexer_events_indexed_total Total events indexed"));
        assert!(output.contains("# TYPE indexer_events_indexed_total counter"));
        assert!(output.contains("indexer_events_indexed_total 10"));

        assert!(output.contains("# TYPE indexer_blocks_processed_total counter"));
        assert!(output.contains("indexer_blocks_processed_total 200"));

        assert!(output.contains("# TYPE indexer_latest_block gauge"));
        assert!(output.contains("indexer_latest_block 500"));

        assert!(output.contains("# TYPE indexer_poll_cycles_total counter"));
        assert!(output.contains("indexer_poll_cycles_total 1"));

        assert!(output.contains("# TYPE indexer_poll_errors_total counter"));
        assert!(output.contains("indexer_poll_errors_total 1"));

        assert!(output.contains("# TYPE indexer_total_results gauge"));
        assert!(output.contains("indexer_total_results 42"));

        assert!(output.contains("# TYPE indexer_api_requests_total counter"));
        assert!(output.contains("indexer_api_requests_total 1"));

        assert!(output.contains("# TYPE indexer_uptime_seconds gauge"));
        assert!(output.contains("indexer_uptime_seconds "));
    }

    #[test]
    fn test_render_prometheus_zero_values() {
        let m = IndexerMetrics::new();
        let output = m.render_prometheus();
        assert!(output.contains("indexer_events_indexed_total 0"));
        assert!(output.contains("indexer_blocks_processed_total 0"));
        assert!(output.contains("indexer_latest_block 0"));
        assert!(output.contains("indexer_poll_errors_total 0"));
        assert!(output.contains("indexer_total_results 0"));
    }

    #[test]
    fn test_render_ends_with_newline() {
        let m = IndexerMetrics::new();
        let output = m.render_prometheus();
        assert!(output.ends_with('\n'));
        assert!(!output.contains("\n\n"));
    }

    #[test]
    fn test_concurrent_updates() {
        let m = std::sync::Arc::new(IndexerMetrics::new());
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let m = m.clone();
                std::thread::spawn(move || {
                    m.record_events(1);
                    m.record_blocks(1);
                    m.record_poll_cycle();
                    m.record_api_request();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.events_indexed.load(Ordering::Relaxed), 10);
        assert_eq!(m.blocks_processed.load(Ordering::Relaxed), 10);
        assert_eq!(m.poll_cycles.load(Ordering::Relaxed), 10);
        assert_eq!(m.api_requests.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn test_default() {
        let m = IndexerMetrics::default();
        assert_eq!(m.events_indexed.load(Ordering::Relaxed), 0);
    }
}
