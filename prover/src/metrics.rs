//! Metrics and Monitoring
//!
//! Track prover performance for optimization and debugging.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Global metrics instance
static METRICS: once_cell::sync::Lazy<Metrics> = once_cell::sync::Lazy::new(Metrics::new);

/// Get global metrics
pub fn metrics() -> &'static Metrics {
    &METRICS
}

/// Prover metrics
pub struct Metrics {
    // Counters
    proofs_generated: AtomicU64,
    proofs_failed: AtomicU64,
    total_cycles: AtomicU64,
    total_bytes_processed: AtomicU64,

    // Timings (stored as microseconds)
    proof_times: RwLock<Vec<u64>>,
    fetch_times: RwLock<Vec<u64>>,
    submit_times: RwLock<Vec<u64>>,

    // Current state
    active_proofs: AtomicU64,
    start_time: Instant,
}

impl Metrics {
    /// Create new metrics
    pub fn new() -> Self {
        Self {
            proofs_generated: AtomicU64::new(0),
            proofs_failed: AtomicU64::new(0),
            total_cycles: AtomicU64::new(0),
            total_bytes_processed: AtomicU64::new(0),
            proof_times: RwLock::new(Vec::new()),
            fetch_times: RwLock::new(Vec::new()),
            submit_times: RwLock::new(Vec::new()),
            active_proofs: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Record a successful proof
    pub fn record_proof_success(&self, duration: Duration, cycles: u64, bytes: u64) {
        self.proofs_generated.fetch_add(1, Ordering::Relaxed);
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);
        self.total_bytes_processed.fetch_add(bytes, Ordering::Relaxed);

        if let Ok(mut times) = self.proof_times.write() {
            times.push(duration.as_micros() as u64);
            // Keep last 1000 samples
            if times.len() > 1000 {
                times.remove(0);
            }
        }
    }

    /// Record a failed proof
    pub fn record_proof_failure(&self) {
        self.proofs_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record fetch time
    pub fn record_fetch_time(&self, duration: Duration) {
        if let Ok(mut times) = self.fetch_times.write() {
            times.push(duration.as_micros() as u64);
            if times.len() > 1000 {
                times.remove(0);
            }
        }
    }

    /// Record submit time
    pub fn record_submit_time(&self, duration: Duration) {
        if let Ok(mut times) = self.submit_times.write() {
            times.push(duration.as_micros() as u64);
            if times.len() > 1000 {
                times.remove(0);
            }
        }
    }

    /// Increment active proofs
    pub fn start_proof(&self) {
        self.active_proofs.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active proofs
    pub fn end_proof(&self) {
        self.active_proofs.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get current snapshot
    pub fn snapshot(&self) -> MetricsSnapshot {
        let proof_times = self.proof_times.read()
            .map(|t| t.clone())
            .unwrap_or_default();

        let fetch_times = self.fetch_times.read()
            .map(|t| t.clone())
            .unwrap_or_default();

        let submit_times = self.submit_times.read()
            .map(|t| t.clone())
            .unwrap_or_default();

        MetricsSnapshot {
            uptime: self.start_time.elapsed(),
            proofs_generated: self.proofs_generated.load(Ordering::Relaxed),
            proofs_failed: self.proofs_failed.load(Ordering::Relaxed),
            total_cycles: self.total_cycles.load(Ordering::Relaxed),
            total_bytes_processed: self.total_bytes_processed.load(Ordering::Relaxed),
            active_proofs: self.active_proofs.load(Ordering::Relaxed),
            avg_proof_time: Self::avg(&proof_times),
            p99_proof_time: Self::percentile(&proof_times, 99),
            avg_fetch_time: Self::avg(&fetch_times),
            avg_submit_time: Self::avg(&submit_times),
        }
    }

    fn avg(times: &[u64]) -> Duration {
        if times.is_empty() {
            return Duration::ZERO;
        }
        let sum: u64 = times.iter().sum();
        Duration::from_micros(sum / times.len() as u64)
    }

    fn percentile(times: &[u64], p: usize) -> Duration {
        if times.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = times.to_vec();
        sorted.sort();
        let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
        Duration::from_micros(sorted[idx])
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of metrics at a point in time
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub uptime: Duration,
    pub proofs_generated: u64,
    pub proofs_failed: u64,
    pub total_cycles: u64,
    pub total_bytes_processed: u64,
    pub active_proofs: u64,
    pub avg_proof_time: Duration,
    pub p99_proof_time: Duration,
    pub avg_fetch_time: Duration,
    pub avg_submit_time: Duration,
}

impl MetricsSnapshot {
    /// Success rate as percentage
    pub fn success_rate(&self) -> f64 {
        let total = self.proofs_generated + self.proofs_failed;
        if total == 0 {
            return 100.0;
        }
        (self.proofs_generated as f64 / total as f64) * 100.0
    }

    /// Proofs per hour
    pub fn proofs_per_hour(&self) -> f64 {
        let hours = self.uptime.as_secs_f64() / 3600.0;
        if hours < 0.001 {
            return 0.0;
        }
        self.proofs_generated as f64 / hours
    }

    /// Average cycles per proof
    pub fn avg_cycles_per_proof(&self) -> u64 {
        if self.proofs_generated == 0 {
            return 0;
        }
        self.total_cycles / self.proofs_generated
    }
}

impl std::fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Prover Metrics ===")?;
        writeln!(f, "Uptime: {:?}", self.uptime)?;
        writeln!(f, "Proofs: {} generated, {} failed ({:.1}% success)",
                 self.proofs_generated, self.proofs_failed, self.success_rate())?;
        writeln!(f, "Throughput: {:.1} proofs/hour", self.proofs_per_hour())?;
        writeln!(f, "Active: {} proofs in progress", self.active_proofs)?;
        writeln!(f, "Avg proof time: {:?}", self.avg_proof_time)?;
        writeln!(f, "P99 proof time: {:?}", self.p99_proof_time)?;
        writeln!(f, "Total cycles: {} ({} avg/proof)",
                 self.total_cycles, self.avg_cycles_per_proof())?;
        writeln!(f, "Data processed: {:.2} MB",
                 self.total_bytes_processed as f64 / 1024.0 / 1024.0)?;
        Ok(())
    }
}

/// Timer for measuring durations
pub struct Timer {
    start: Instant,
}

impl Timer {
    pub fn start() -> Self {
        Self { start: Instant::now() }
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Extended prover metrics for API
pub struct ProverMetrics {
    inner: Metrics,
    jobs_processed: AtomicU64,
    jobs_failed: AtomicU64,
    total_earnings: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

impl ProverMetrics {
    pub fn new() -> Self {
        Self {
            inner: Metrics::new(),
            jobs_processed: AtomicU64::new(0),
            jobs_failed: AtomicU64::new(0),
            total_earnings: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }

    pub fn record_job_success(&self, earnings: u64, duration: Duration, cycles: u64, bytes: u64) {
        self.jobs_processed.fetch_add(1, Ordering::Relaxed);
        self.total_earnings.fetch_add(earnings, Ordering::Relaxed);
        self.inner.record_proof_success(duration, cycles, bytes);
    }

    pub fn record_job_failure(&self) {
        self.jobs_failed.fetch_add(1, Ordering::Relaxed);
        self.inner.record_proof_failure();
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_stats(&self) -> ProverStats {
        let snapshot = self.inner.snapshot();
        ProverStats {
            jobs_processed: self.jobs_processed.load(Ordering::Relaxed),
            jobs_failed: self.jobs_failed.load(Ordering::Relaxed),
            total_proofs: snapshot.proofs_generated,
            total_earnings: self.total_earnings.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            avg_proof_time_ms: snapshot.avg_proof_time.as_millis() as u64,
            uptime_seconds: snapshot.uptime.as_secs(),
        }
    }

    pub fn to_prometheus(&self) -> String {
        let stats = self.get_stats();
        let snapshot = self.inner.snapshot();

        format!(
r#"# HELP prover_jobs_processed_total Total number of jobs processed
# TYPE prover_jobs_processed_total counter
prover_jobs_processed_total {}

# HELP prover_jobs_failed_total Total number of failed jobs
# TYPE prover_jobs_failed_total counter
prover_jobs_failed_total {}

# HELP prover_proofs_generated_total Total proofs generated
# TYPE prover_proofs_generated_total counter
prover_proofs_generated_total {}

# HELP prover_earnings_wei_total Total earnings in wei
# TYPE prover_earnings_wei_total counter
prover_earnings_wei_total {}

# HELP prover_cache_hits_total Total cache hits
# TYPE prover_cache_hits_total counter
prover_cache_hits_total {}

# HELP prover_cache_misses_total Total cache misses
# TYPE prover_cache_misses_total counter
prover_cache_misses_total {}

# HELP prover_proof_time_seconds Average proof generation time
# TYPE prover_proof_time_seconds gauge
prover_proof_time_seconds {}

# HELP prover_active_proofs Current number of active proofs
# TYPE prover_active_proofs gauge
prover_active_proofs {}

# HELP prover_uptime_seconds Prover uptime in seconds
# TYPE prover_uptime_seconds gauge
prover_uptime_seconds {}

# HELP prover_success_rate Success rate percentage
# TYPE prover_success_rate gauge
prover_success_rate {}
"#,
            stats.jobs_processed,
            stats.jobs_failed,
            stats.total_proofs,
            stats.total_earnings,
            stats.cache_hits,
            stats.cache_misses,
            snapshot.avg_proof_time.as_secs_f64(),
            snapshot.active_proofs,
            stats.uptime_seconds,
            stats.success_rate(),
        )
    }
}

impl Default for ProverMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Stats for JSON export
#[derive(Debug, Clone)]
pub struct ProverStats {
    pub jobs_processed: u64,
    pub jobs_failed: u64,
    pub total_proofs: u64,
    pub total_earnings: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub avg_proof_time_ms: u64,
    pub uptime_seconds: u64,
}

impl ProverStats {
    pub fn success_rate(&self) -> f64 {
        let total = self.jobs_processed + self.jobs_failed;
        if total == 0 {
            return 100.0;
        }
        (self.jobs_processed as f64 / total as f64) * 100.0
    }

    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            return 0.0;
        }
        (self.cache_hits as f64 / total as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recording() {
        let metrics = Metrics::new();

        metrics.record_proof_success(
            Duration::from_secs(10),
            1_000_000,
            1024,
        );

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.proofs_generated, 1);
        assert_eq!(snapshot.total_cycles, 1_000_000);
    }

    #[test]
    fn test_success_rate() {
        let snapshot = MetricsSnapshot {
            uptime: Duration::from_secs(3600),
            proofs_generated: 90,
            proofs_failed: 10,
            total_cycles: 0,
            total_bytes_processed: 0,
            active_proofs: 0,
            avg_proof_time: Duration::ZERO,
            p99_proof_time: Duration::ZERO,
            avg_fetch_time: Duration::ZERO,
            avg_submit_time: Duration::ZERO,
        };

        assert_eq!(snapshot.success_rate(), 90.0);
    }
}
