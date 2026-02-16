//! Proving Pipeline Metrics
//!
//! Detailed stage-level timing for the proving pipeline. Each proving job
//! goes through multiple stages, and this module tracks how long each takes
//! so we can identify bottlenecks:
//!
//! ```text
//! [fetch] → [preflight] → [strategy] → [prove] → [compress] → [submit]
//!   ↑           ↑             ↑           ↑           ↑           ↑
//!   └───────────┴─────────────┴───────────┴───────────┴───────────┘
//!                     ProveTimeline tracks all stages
//! ```
//!
//! ## Integration
//!
//! Use `ProveTimeline::start()` at the beginning of a job, call `.enter_stage()`
//! at each transition, and `.finish()` when done. The finished timeline can be
//! logged, sent to metrics, or exported as JSON.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::info;

// ═══════════════════════════════════════════════════════════════════════════════
// Pipeline Stages
// ═══════════════════════════════════════════════════════════════════════════════

/// Stages of the proving pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProveStage {
    /// Fetching program ELF and input data.
    Fetch,
    /// Preflight execution (execute without proving).
    Preflight,
    /// Strategy selection (Direct/Segmented/Continuation).
    Strategy,
    /// Segment tuning (choosing po2 and thread count).
    SegmentTuning,
    /// Actual proof generation.
    Proving,
    /// Proof compression (Composite → Succinct → Groth16).
    Compression,
    /// Submitting proof on-chain.
    Submission,
    /// Waiting in queue before processing.
    QueueWait,
}

impl ProveStage {
    /// All stages in pipeline order.
    pub fn all() -> &'static [ProveStage] {
        &[
            ProveStage::QueueWait,
            ProveStage::Fetch,
            ProveStage::Preflight,
            ProveStage::Strategy,
            ProveStage::SegmentTuning,
            ProveStage::Proving,
            ProveStage::Compression,
            ProveStage::Submission,
        ]
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            ProveStage::Fetch => "fetch",
            ProveStage::Preflight => "preflight",
            ProveStage::Strategy => "strategy",
            ProveStage::SegmentTuning => "segment_tuning",
            ProveStage::Proving => "proving",
            ProveStage::Compression => "compression",
            ProveStage::Submission => "submission",
            ProveStage::QueueWait => "queue_wait",
        }
    }
}

impl std::fmt::Display for ProveStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Stage Timing
// ═══════════════════════════════════════════════════════════════════════════════

/// Timing for a single stage.
#[derive(Debug, Clone)]
pub struct StageTiming {
    pub stage: ProveStage,
    pub duration: Duration,
    /// Optional metadata (e.g., "po2=20, threads=8").
    pub metadata: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Prove Timeline
// ═══════════════════════════════════════════════════════════════════════════════

/// Timeline tracking all stages of a single proving job.
///
/// Usage:
/// ```ignore
/// let mut timeline = ProveTimeline::start(request_id);
/// timeline.enter_stage(ProveStage::Fetch);
/// // ... fetch data ...
/// timeline.enter_stage(ProveStage::Preflight);
/// // ... run preflight ...
/// timeline.enter_stage_with_meta(ProveStage::Proving, "po2=20, threads=8");
/// // ... prove ...
/// let finished = timeline.finish(true);
/// info!("{}", finished);
/// ```
pub struct ProveTimeline {
    /// Request/job ID for correlation.
    pub request_id: u64,
    /// When this timeline was created.
    start_time: Instant,
    /// Current stage being tracked.
    current_stage: Option<ProveStage>,
    /// When the current stage started.
    stage_start: Instant,
    /// Completed stage timings.
    stages: Vec<StageTiming>,
    /// Metadata for the current stage.
    current_metadata: Option<String>,
}

impl ProveTimeline {
    /// Start a new timeline for a request.
    pub fn start(request_id: u64) -> Self {
        Self {
            request_id,
            start_time: Instant::now(),
            current_stage: None,
            stage_start: Instant::now(),
            stages: Vec::new(),
            current_metadata: None,
        }
    }

    /// Enter a new stage, completing the previous one.
    pub fn enter_stage(&mut self, stage: ProveStage) {
        self.close_current_stage();
        self.current_stage = Some(stage);
        self.stage_start = Instant::now();
        self.current_metadata = None;
    }

    /// Enter a new stage with metadata.
    pub fn enter_stage_with_meta(&mut self, stage: ProveStage, metadata: &str) {
        self.close_current_stage();
        self.current_stage = Some(stage);
        self.stage_start = Instant::now();
        self.current_metadata = Some(metadata.to_string());
    }

    /// Add metadata to the current stage.
    pub fn set_metadata(&mut self, metadata: &str) {
        self.current_metadata = Some(metadata.to_string());
    }

    /// Close the current stage and record its timing.
    fn close_current_stage(&mut self) {
        if let Some(stage) = self.current_stage.take() {
            self.stages.push(StageTiming {
                stage,
                duration: self.stage_start.elapsed(),
                metadata: self.current_metadata.take(),
            });
        }
    }

    /// Finish the timeline and produce a summary.
    pub fn finish(mut self, success: bool) -> FinishedTimeline {
        self.close_current_stage();

        let total_time = self.start_time.elapsed();

        FinishedTimeline {
            request_id: self.request_id,
            success,
            total_time,
            stages: self.stages,
        }
    }

    /// Get elapsed time since timeline started.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// Completed timeline with all stage timings.
#[derive(Debug, Clone)]
pub struct FinishedTimeline {
    pub request_id: u64,
    pub success: bool,
    pub total_time: Duration,
    pub stages: Vec<StageTiming>,
}

impl FinishedTimeline {
    /// Get the duration of a specific stage (or zero if not recorded).
    pub fn stage_duration(&self, stage: ProveStage) -> Duration {
        self.stages
            .iter()
            .find(|s| s.stage == stage)
            .map(|s| s.duration)
            .unwrap_or(Duration::ZERO)
    }

    /// Get the proving stage duration (the main bottleneck).
    pub fn prove_duration(&self) -> Duration {
        self.stage_duration(ProveStage::Proving)
    }

    /// Fraction of total time spent in each stage.
    pub fn stage_fractions(&self) -> Vec<(ProveStage, f64)> {
        let total = self.total_time.as_secs_f64();
        if total == 0.0 {
            return vec![];
        }
        self.stages
            .iter()
            .map(|s| (s.stage, s.duration.as_secs_f64() / total))
            .collect()
    }

    /// Overhead: total time minus proving time.
    pub fn overhead(&self) -> Duration {
        self.total_time
            .saturating_sub(self.prove_duration())
    }

    /// Overhead as percentage of total time.
    pub fn overhead_pct(&self) -> f64 {
        let total = self.total_time.as_secs_f64();
        if total == 0.0 {
            return 0.0;
        }
        self.overhead().as_secs_f64() / total * 100.0
    }
}

impl std::fmt::Display for FinishedTimeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Pipeline[req={}] {} in {:?} (overhead: {:.1}%): ",
            self.request_id,
            if self.success { "OK" } else { "FAIL" },
            self.total_time,
            self.overhead_pct(),
        )?;

        for (i, stage) in self.stages.iter().enumerate() {
            if i > 0 {
                write!(f, " → ")?;
            }
            write!(f, "{}={:?}", stage.stage.name(), stage.duration)?;
            if let Some(ref meta) = stage.metadata {
                write!(f, "({})", meta)?;
            }
        }

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Aggregate Pipeline Metrics
// ═══════════════════════════════════════════════════════════════════════════════

/// Aggregate metrics across all proving jobs, broken down by stage.
pub struct PipelineMetrics {
    /// Per-stage aggregate times (stored as microseconds).
    stage_times: [RwLock<Vec<u64>>; 8],
    /// Total timelines recorded.
    total_jobs: AtomicU64,
    /// Successful jobs.
    successful_jobs: AtomicU64,
}

impl PipelineMetrics {
    /// Create a new pipeline metrics collector.
    pub fn new() -> Self {
        Self {
            stage_times: [
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
                RwLock::new(Vec::new()),
            ],
            total_jobs: AtomicU64::new(0),
            successful_jobs: AtomicU64::new(0),
        }
    }

    /// Record a finished timeline.
    pub fn record(&self, timeline: &FinishedTimeline) {
        self.total_jobs.fetch_add(1, Ordering::Relaxed);
        if timeline.success {
            self.successful_jobs.fetch_add(1, Ordering::Relaxed);
        }

        for stage_timing in &timeline.stages {
            let idx = Self::stage_index(stage_timing.stage);
            if let Ok(mut times) = self.stage_times[idx].write() {
                times.push(stage_timing.duration.as_micros() as u64);
                // Keep last 1000 samples
                if times.len() > 1000 {
                    times.remove(0);
                }
            }
        }
    }

    /// Get a snapshot of pipeline metrics.
    pub fn snapshot(&self) -> PipelineSnapshot {
        let mut stage_stats = Vec::new();

        for stage in ProveStage::all() {
            let idx = Self::stage_index(*stage);
            let times = self.stage_times[idx]
                .read()
                .map(|t| t.clone())
                .unwrap_or_default();

            if !times.is_empty() {
                stage_stats.push(StageStats {
                    stage: *stage,
                    count: times.len() as u64,
                    avg: Self::avg_duration(&times),
                    p50: Self::percentile_duration(&times, 50),
                    p95: Self::percentile_duration(&times, 95),
                    p99: Self::percentile_duration(&times, 99),
                    max: Self::max_duration(&times),
                });
            }
        }

        PipelineSnapshot {
            total_jobs: self.total_jobs.load(Ordering::Relaxed),
            successful_jobs: self.successful_jobs.load(Ordering::Relaxed),
            stage_stats,
        }
    }

    /// Log the current pipeline metrics as a summary table.
    pub fn log_summary(&self) {
        let snap = self.snapshot();
        if snap.total_jobs == 0 {
            return;
        }

        info!(
            "Pipeline metrics: {}/{} jobs succeeded",
            snap.successful_jobs, snap.total_jobs
        );

        for s in &snap.stage_stats {
            info!(
                "  {:15} avg={:>8?}  p95={:>8?}  p99={:>8?}  max={:>8?}  (n={})",
                s.stage.name(),
                s.avg,
                s.p95,
                s.p99,
                s.max,
                s.count
            );
        }
    }

    fn stage_index(stage: ProveStage) -> usize {
        match stage {
            ProveStage::QueueWait => 0,
            ProveStage::Fetch => 1,
            ProveStage::Preflight => 2,
            ProveStage::Strategy => 3,
            ProveStage::SegmentTuning => 4,
            ProveStage::Proving => 5,
            ProveStage::Compression => 6,
            ProveStage::Submission => 7,
        }
    }

    fn avg_duration(times: &[u64]) -> Duration {
        if times.is_empty() {
            return Duration::ZERO;
        }
        let sum: u64 = times.iter().sum();
        Duration::from_micros(sum / times.len() as u64)
    }

    fn percentile_duration(times: &[u64], p: usize) -> Duration {
        if times.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = times.to_vec();
        sorted.sort();
        let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
        Duration::from_micros(sorted[idx])
    }

    fn max_duration(times: &[u64]) -> Duration {
        times
            .iter()
            .max()
            .map(|&m| Duration::from_micros(m))
            .unwrap_or(Duration::ZERO)
    }
}

impl Default for PipelineMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Global pipeline metrics instance.
static PIPELINE_METRICS: once_cell::sync::Lazy<PipelineMetrics> =
    once_cell::sync::Lazy::new(PipelineMetrics::new);

/// Get the global pipeline metrics collector.
pub fn pipeline_metrics() -> &'static PipelineMetrics {
    &PIPELINE_METRICS
}

/// Snapshot of pipeline metrics.
#[derive(Debug, Clone)]
pub struct PipelineSnapshot {
    pub total_jobs: u64,
    pub successful_jobs: u64,
    pub stage_stats: Vec<StageStats>,
}

impl PipelineSnapshot {
    /// Success rate as percentage.
    pub fn success_rate(&self) -> f64 {
        if self.total_jobs == 0 {
            return 100.0;
        }
        (self.successful_jobs as f64 / self.total_jobs as f64) * 100.0
    }

    /// Average total proving time (sum of all stage averages).
    pub fn avg_total_time(&self) -> Duration {
        self.stage_stats.iter().map(|s| s.avg).sum()
    }

    /// Which stage dominates (highest average time)?
    pub fn bottleneck(&self) -> Option<&StageStats> {
        self.stage_stats.iter().max_by_key(|s| s.avg)
    }

    /// Format as Prometheus metrics.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::new();

        out.push_str("# HELP prove_pipeline_jobs_total Total proving jobs\n");
        out.push_str("# TYPE prove_pipeline_jobs_total counter\n");
        out.push_str(&format!("prove_pipeline_jobs_total {}\n\n", self.total_jobs));

        out.push_str("# HELP prove_pipeline_success_total Successful proving jobs\n");
        out.push_str("# TYPE prove_pipeline_success_total counter\n");
        out.push_str(&format!(
            "prove_pipeline_success_total {}\n\n",
            self.successful_jobs
        ));

        out.push_str(
            "# HELP prove_pipeline_stage_seconds_avg Average stage duration in seconds\n",
        );
        out.push_str("# TYPE prove_pipeline_stage_seconds_avg gauge\n");
        for s in &self.stage_stats {
            out.push_str(&format!(
                "prove_pipeline_stage_seconds_avg{{stage=\"{}\"}} {:.6}\n",
                s.stage.name(),
                s.avg.as_secs_f64()
            ));
        }
        out.push('\n');

        out.push_str(
            "# HELP prove_pipeline_stage_seconds_p95 P95 stage duration in seconds\n",
        );
        out.push_str("# TYPE prove_pipeline_stage_seconds_p95 gauge\n");
        for s in &self.stage_stats {
            out.push_str(&format!(
                "prove_pipeline_stage_seconds_p95{{stage=\"{}\"}} {:.6}\n",
                s.stage.name(),
                s.p95.as_secs_f64()
            ));
        }

        out
    }
}

impl std::fmt::Display for PipelineSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "=== Pipeline Metrics ({}/{} jobs, {:.1}% success) ===",
            self.successful_jobs,
            self.total_jobs,
            self.success_rate()
        )?;

        for s in &self.stage_stats {
            writeln!(
                f,
                "  {:15} avg={:>8?}  p50={:>8?}  p95={:>8?}  p99={:>8?}  max={:>8?}",
                s.stage.name(),
                s.avg,
                s.p50,
                s.p95,
                s.p99,
                s.max,
            )?;
        }

        if let Some(bottleneck) = self.bottleneck() {
            writeln!(
                f,
                "  Bottleneck: {} (avg {:?})",
                bottleneck.stage.name(),
                bottleneck.avg
            )?;
        }

        Ok(())
    }
}

/// Statistics for a single pipeline stage.
#[derive(Debug, Clone)]
pub struct StageStats {
    pub stage: ProveStage,
    pub count: u64,
    pub avg: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
    pub max: Duration,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeline_basic() {
        let mut timeline = ProveTimeline::start(1);

        timeline.enter_stage(ProveStage::Fetch);
        std::thread::sleep(Duration::from_millis(10));

        timeline.enter_stage(ProveStage::Preflight);
        std::thread::sleep(Duration::from_millis(10));

        timeline.enter_stage(ProveStage::Proving);
        std::thread::sleep(Duration::from_millis(10));

        let finished = timeline.finish(true);

        assert!(finished.success);
        assert_eq!(finished.stages.len(), 3);
        assert_eq!(finished.stages[0].stage, ProveStage::Fetch);
        assert_eq!(finished.stages[1].stage, ProveStage::Preflight);
        assert_eq!(finished.stages[2].stage, ProveStage::Proving);
    }

    #[test]
    fn test_timeline_with_metadata() {
        let mut timeline = ProveTimeline::start(42);

        timeline.enter_stage_with_meta(ProveStage::Proving, "po2=20, threads=8");
        std::thread::sleep(Duration::from_millis(5));

        let finished = timeline.finish(true);

        assert_eq!(finished.stages.len(), 1);
        assert_eq!(
            finished.stages[0].metadata.as_deref(),
            Some("po2=20, threads=8")
        );
    }

    #[test]
    fn test_stage_duration() {
        let mut timeline = ProveTimeline::start(1);

        timeline.enter_stage(ProveStage::Fetch);
        std::thread::sleep(Duration::from_millis(20));

        timeline.enter_stage(ProveStage::Proving);
        std::thread::sleep(Duration::from_millis(50));

        let finished = timeline.finish(true);

        // Proving should be longer than Fetch
        assert!(finished.stage_duration(ProveStage::Proving) > finished.stage_duration(ProveStage::Fetch));
        // Non-existent stage returns zero
        assert_eq!(finished.stage_duration(ProveStage::Compression), Duration::ZERO);
    }

    #[test]
    fn test_overhead() {
        let mut timeline = ProveTimeline::start(1);

        timeline.enter_stage(ProveStage::Fetch);
        std::thread::sleep(Duration::from_millis(10));

        timeline.enter_stage(ProveStage::Proving);
        std::thread::sleep(Duration::from_millis(30));

        let finished = timeline.finish(true);

        // Overhead = total - proving
        let overhead = finished.overhead();
        assert!(overhead >= Duration::from_millis(10));
    }

    #[test]
    fn test_stage_fractions() {
        let finished = FinishedTimeline {
            request_id: 1,
            success: true,
            total_time: Duration::from_secs(10),
            stages: vec![
                StageTiming {
                    stage: ProveStage::Fetch,
                    duration: Duration::from_secs(1),
                    metadata: None,
                },
                StageTiming {
                    stage: ProveStage::Proving,
                    duration: Duration::from_secs(9),
                    metadata: None,
                },
            ],
        };

        let fractions = finished.stage_fractions();
        assert_eq!(fractions.len(), 2);
        assert!((fractions[0].1 - 0.1).abs() < 0.01); // Fetch ~10%
        assert!((fractions[1].1 - 0.9).abs() < 0.01); // Proving ~90%
    }

    #[test]
    fn test_pipeline_metrics() {
        let metrics = PipelineMetrics::new();

        // Record a few timelines
        for i in 0..5 {
            let finished = FinishedTimeline {
                request_id: i,
                success: i < 4,
                total_time: Duration::from_millis(100 + i * 10),
                stages: vec![
                    StageTiming {
                        stage: ProveStage::Fetch,
                        duration: Duration::from_millis(10),
                        metadata: None,
                    },
                    StageTiming {
                        stage: ProveStage::Proving,
                        duration: Duration::from_millis(80 + i * 10),
                        metadata: None,
                    },
                ],
            };
            metrics.record(&finished);
        }

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.total_jobs, 5);
        assert_eq!(snapshot.successful_jobs, 4);
        assert_eq!(snapshot.stage_stats.len(), 2); // Fetch + Proving

        // Bottleneck should be Proving
        let bottleneck = snapshot.bottleneck().unwrap();
        assert_eq!(bottleneck.stage, ProveStage::Proving);
    }

    #[test]
    fn test_display() {
        let finished = FinishedTimeline {
            request_id: 42,
            success: true,
            total_time: Duration::from_secs(10),
            stages: vec![
                StageTiming {
                    stage: ProveStage::Fetch,
                    duration: Duration::from_millis(500),
                    metadata: None,
                },
                StageTiming {
                    stage: ProveStage::Proving,
                    duration: Duration::from_secs(9),
                    metadata: Some("po2=20".to_string()),
                },
            ],
        };

        let display = format!("{}", finished);
        assert!(display.contains("req=42"));
        assert!(display.contains("OK"));
        assert!(display.contains("fetch="));
        assert!(display.contains("proving="));
        assert!(display.contains("po2=20"));
    }

    #[test]
    fn test_prometheus_output() {
        let metrics = PipelineMetrics::new();

        let finished = FinishedTimeline {
            request_id: 1,
            success: true,
            total_time: Duration::from_secs(5),
            stages: vec![StageTiming {
                stage: ProveStage::Proving,
                duration: Duration::from_secs(4),
                metadata: None,
            }],
        };
        metrics.record(&finished);

        let prom = metrics.snapshot().to_prometheus();
        assert!(prom.contains("prove_pipeline_jobs_total 1"));
        assert!(prom.contains("prove_pipeline_success_total 1"));
        assert!(prom.contains("stage=\"proving\""));
    }

    #[test]
    fn test_empty_metrics() {
        let metrics = PipelineMetrics::new();
        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.total_jobs, 0);
        assert_eq!(snapshot.success_rate(), 100.0);
        assert!(snapshot.bottleneck().is_none());
        assert_eq!(snapshot.avg_total_time(), Duration::ZERO);
    }

    #[test]
    fn test_stage_names() {
        for stage in ProveStage::all() {
            assert!(!stage.name().is_empty());
            assert_eq!(format!("{}", stage), stage.name());
        }
    }
}
