//! Distributed Tracing
//!
//! Traces requests across the prover system:
//! - Unique trace ID per job
//! - Spans for each operation (fetch, prove, submit)
//! - Context propagation across async tasks
//! - Export to Jaeger, Zipkin, or OTLP
//!
//! ## Usage
//!
//! ```ignore
//! let span = tracer.start_job(task_id);
//! let _guard = span.enter();
//!
//! tracer.span("fetch_input", || async {
//!     // fetch logic
//! }).await;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Trace ID (128-bit, compatible with W3C Trace Context)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId(pub [u8; 16]);

impl TraceId {
    /// Generate a new random trace ID
    pub fn generate() -> Self {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes);
        Self(bytes)
    }

    /// Create from hex string
    pub fn from_hex(s: &str) -> Option<Self> {
        let bytes = hex::decode(s).ok()?;
        if bytes.len() != 16 {
            return None;
        }
        let mut arr = [0u8; 16];
        arr.copy_from_slice(&bytes);
        Some(Self(arr))
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Span ID (64-bit)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId(pub [u8; 8]);

impl SpanId {
    /// Generate a new random span ID
    pub fn generate() -> Self {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 8];
        rng.fill(&mut bytes);
        Self(bytes)
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl std::fmt::Display for SpanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Span status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanStatus {
    /// Operation in progress
    InProgress,
    /// Operation completed successfully
    Ok,
    /// Operation failed
    Error,
}

/// A trace span representing a unit of work
#[derive(Debug, Clone)]
pub struct Span {
    /// Trace this span belongs to
    pub trace_id: TraceId,
    /// Unique span ID
    pub span_id: SpanId,
    /// Parent span ID (if any)
    pub parent_id: Option<SpanId>,
    /// Operation name
    pub name: String,
    /// Service name
    pub service: String,
    /// Start time
    pub start_time: SystemTime,
    /// End time (None if in progress)
    pub end_time: Option<SystemTime>,
    /// Duration
    pub duration: Option<Duration>,
    /// Status
    pub status: SpanStatus,
    /// Tags/attributes
    pub tags: HashMap<String, String>,
    /// Events/logs within the span
    pub events: Vec<SpanEvent>,
}

/// An event within a span
#[derive(Debug, Clone)]
pub struct SpanEvent {
    pub name: String,
    pub timestamp: SystemTime,
    pub attributes: HashMap<String, String>,
}

impl Span {
    /// Create a new span
    pub fn new(trace_id: TraceId, name: &str, service: &str) -> Self {
        Self {
            trace_id,
            span_id: SpanId::generate(),
            parent_id: None,
            name: name.to_string(),
            service: service.to_string(),
            start_time: SystemTime::now(),
            end_time: None,
            duration: None,
            status: SpanStatus::InProgress,
            tags: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Create a child span
    pub fn child(&self, name: &str) -> Self {
        let mut span = Self::new(self.trace_id, name, &self.service);
        span.parent_id = Some(self.span_id);
        span
    }

    /// Add a tag
    pub fn tag(mut self, key: &str, value: &str) -> Self {
        self.tags.insert(key.to_string(), value.to_string());
        self
    }

    /// Add an event
    pub fn event(&mut self, name: &str) {
        self.events.push(SpanEvent {
            name: name.to_string(),
            timestamp: SystemTime::now(),
            attributes: HashMap::new(),
        });
    }

    /// Add an event with attributes
    pub fn event_with_attrs(&mut self, name: &str, attrs: HashMap<String, String>) {
        self.events.push(SpanEvent {
            name: name.to_string(),
            timestamp: SystemTime::now(),
            attributes: attrs,
        });
    }

    /// Mark span as complete
    pub fn finish(&mut self) {
        self.end_time = Some(SystemTime::now());
        self.duration = self.end_time.unwrap().duration_since(self.start_time).ok();
        if self.status == SpanStatus::InProgress {
            self.status = SpanStatus::Ok;
        }
    }

    /// Mark span as failed
    pub fn fail(&mut self, error: &str) {
        self.status = SpanStatus::Error;
        self.tags.insert("error".to_string(), error.to_string());
        self.finish();
    }

    /// Get duration in milliseconds
    pub fn duration_ms(&self) -> Option<u64> {
        self.duration.map(|d| d.as_millis() as u64)
    }
}

/// Trace context for propagation
#[derive(Debug, Clone)]
pub struct TraceContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub sampled: bool,
}

impl TraceContext {
    /// Create new context
    pub fn new(trace_id: TraceId, span_id: SpanId) -> Self {
        Self {
            trace_id,
            span_id,
            sampled: true,
        }
    }

    /// Parse from W3C traceparent header
    /// Format: version-trace_id-span_id-flags
    pub fn from_traceparent(header: &str) -> Option<Self> {
        let parts: Vec<&str> = header.split('-').collect();
        if parts.len() != 4 {
            return None;
        }

        let trace_id = TraceId::from_hex(parts[1])?;
        let span_bytes = hex::decode(parts[2]).ok()?;
        if span_bytes.len() != 8 {
            return None;
        }
        let mut span_arr = [0u8; 8];
        span_arr.copy_from_slice(&span_bytes);
        let span_id = SpanId(span_arr);

        let flags = u8::from_str_radix(parts[3], 16).ok()?;
        let sampled = flags & 0x01 != 0;

        Some(Self {
            trace_id,
            span_id,
            sampled,
        })
    }

    /// Convert to W3C traceparent header
    pub fn to_traceparent(&self) -> String {
        let flags = if self.sampled { "01" } else { "00" };
        format!(
            "00-{}-{}-{}",
            self.trace_id.to_hex(),
            self.span_id.to_hex(),
            flags
        )
    }
}

/// Tracer configuration
#[derive(Debug, Clone)]
pub struct TracerConfig {
    /// Service name
    pub service_name: String,
    /// Sample rate (0.0 - 1.0)
    pub sample_rate: f64,
    /// Export endpoint (Jaeger, Zipkin, OTLP)
    pub export_endpoint: Option<String>,
    /// Export format
    pub export_format: ExportFormat,
    /// Max spans to buffer before export
    pub buffer_size: usize,
    /// Export interval
    pub export_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Jaeger Thrift
    Jaeger,
    /// Zipkin JSON
    Zipkin,
    /// OpenTelemetry Protocol
    Otlp,
    /// Log to stdout (for debugging)
    Log,
}

impl Default for TracerConfig {
    fn default() -> Self {
        Self {
            service_name: "world-zk-prover".to_string(),
            sample_rate: 1.0, // Sample all traces
            export_endpoint: None,
            export_format: ExportFormat::Log,
            buffer_size: 1000,
            export_interval: Duration::from_secs(10),
        }
    }
}

impl TracerConfig {
    /// Create from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(name) = std::env::var("SERVICE_NAME") {
            config.service_name = name;
        }

        if let Ok(rate) = std::env::var("TRACE_SAMPLE_RATE") {
            if let Ok(r) = rate.parse() {
                config.sample_rate = r;
            }
        }

        if let Ok(endpoint) = std::env::var("JAEGER_ENDPOINT") {
            config.export_endpoint = Some(endpoint);
            config.export_format = ExportFormat::Jaeger;
        } else if let Ok(endpoint) = std::env::var("ZIPKIN_ENDPOINT") {
            config.export_endpoint = Some(endpoint);
            config.export_format = ExportFormat::Zipkin;
        } else if let Ok(endpoint) = std::env::var("OTLP_ENDPOINT") {
            config.export_endpoint = Some(endpoint);
            config.export_format = ExportFormat::Otlp;
        }

        config
    }
}

/// Distributed tracer
pub struct Tracer {
    config: TracerConfig,
    /// Active spans
    active_spans: RwLock<HashMap<SpanId, Span>>,
    /// Completed spans buffer
    completed_spans: RwLock<Vec<Span>>,
    /// Stats
    spans_created: AtomicU64,
    spans_exported: AtomicU64,
}

impl Tracer {
    /// Create a new tracer
    pub fn new(config: TracerConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            active_spans: RwLock::new(HashMap::new()),
            completed_spans: RwLock::new(Vec::new()),
            spans_created: AtomicU64::new(0),
            spans_exported: AtomicU64::new(0),
        })
    }

    /// Should this trace be sampled?
    fn should_sample(&self) -> bool {
        if self.config.sample_rate >= 1.0 {
            return true;
        }
        if self.config.sample_rate <= 0.0 {
            return false;
        }
        rand::random::<f64>() < self.config.sample_rate
    }

    /// Start a new trace for a job
    pub async fn start_job(&self, task_id: &[u8; 32]) -> Span {
        let trace_id = TraceId::generate();
        let mut span = Span::new(trace_id, "process_job", &self.config.service_name);
        span.tags
            .insert("task_id".to_string(), hex::encode(task_id));

        self.spans_created.fetch_add(1, Ordering::Relaxed);

        let mut active = self.active_spans.write().await;
        active.insert(span.span_id, span.clone());

        debug!("Started trace {} for job", trace_id);
        span
    }

    /// Start a child span
    pub async fn start_span(&self, parent: &Span, name: &str) -> Span {
        let span = parent.child(name);
        self.spans_created.fetch_add(1, Ordering::Relaxed);

        let mut active = self.active_spans.write().await;
        active.insert(span.span_id, span.clone());

        span
    }

    /// End a span
    pub async fn end_span(&self, mut span: Span) {
        span.finish();

        let mut active = self.active_spans.write().await;
        active.remove(&span.span_id);

        let mut completed = self.completed_spans.write().await;
        completed.push(span);

        // Check if we should export
        if completed.len() >= self.config.buffer_size {
            drop(completed);
            self.export().await;
        }
    }

    /// End a span with error
    pub async fn end_span_error(&self, mut span: Span, error: &str) {
        span.fail(error);

        let mut active = self.active_spans.write().await;
        active.remove(&span.span_id);

        let mut completed = self.completed_spans.write().await;
        completed.push(span);
    }

    /// Export completed spans
    pub async fn export(&self) {
        let mut completed = self.completed_spans.write().await;
        if completed.is_empty() {
            return;
        }

        let spans: Vec<Span> = completed.drain(..).collect();
        drop(completed);

        let count = spans.len();

        match self.config.export_format {
            ExportFormat::Log => {
                for span in &spans {
                    info!(
                        "TRACE: {} {} {} {:?} {:?}",
                        span.trace_id,
                        span.name,
                        span.duration_ms().unwrap_or(0),
                        span.status,
                        span.tags
                    );
                }
            }
            ExportFormat::Jaeger => {
                if let Some(endpoint) = &self.config.export_endpoint {
                    if let Err(e) = self.export_jaeger(&spans, endpoint).await {
                        tracing::error!("Failed to export to Jaeger: {}", e);
                    }
                }
            }
            ExportFormat::Zipkin => {
                if let Some(endpoint) = &self.config.export_endpoint {
                    if let Err(e) = self.export_zipkin(&spans, endpoint).await {
                        tracing::error!("Failed to export to Zipkin: {}", e);
                    }
                }
            }
            ExportFormat::Otlp => {
                if let Some(endpoint) = &self.config.export_endpoint {
                    if let Err(e) = self.export_otlp(&spans, endpoint).await {
                        tracing::error!("Failed to export to OTLP: {}", e);
                    }
                }
            }
        }

        self.spans_exported.fetch_add(count as u64, Ordering::Relaxed);
    }

    /// Export to Jaeger
    async fn export_jaeger(&self, spans: &[Span], endpoint: &str) -> Result<(), String> {
        // Build Jaeger batch format
        let batch: Vec<serde_json::Value> = spans
            .iter()
            .map(|s| {
                serde_json::json!({
                    "traceIdHigh": i64::from_be_bytes(s.trace_id.0[0..8].try_into().unwrap()),
                    "traceIdLow": i64::from_be_bytes(s.trace_id.0[8..16].try_into().unwrap()),
                    "spanId": i64::from_be_bytes(s.span_id.0.try_into().unwrap()),
                    "operationName": s.name,
                    "startTime": s.start_time.duration_since(UNIX_EPOCH).unwrap().as_micros() as i64,
                    "duration": s.duration_ms().unwrap_or(0) * 1000,
                    "tags": s.tags.iter().map(|(k, v)| {
                        serde_json::json!({"key": k, "vStr": v})
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();

        let client = reqwest::Client::new();
        client
            .post(endpoint)
            .json(&serde_json::json!({ "spans": batch }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Export to Zipkin
    async fn export_zipkin(&self, spans: &[Span], endpoint: &str) -> Result<(), String> {
        let batch: Vec<serde_json::Value> = spans
            .iter()
            .map(|s| {
                serde_json::json!({
                    "traceId": s.trace_id.to_hex(),
                    "id": s.span_id.to_hex(),
                    "parentId": s.parent_id.map(|p| p.to_hex()),
                    "name": s.name,
                    "timestamp": s.start_time.duration_since(UNIX_EPOCH).unwrap().as_micros() as i64,
                    "duration": s.duration_ms().unwrap_or(0) * 1000,
                    "localEndpoint": { "serviceName": s.service },
                    "tags": s.tags,
                })
            })
            .collect();

        let client = reqwest::Client::new();
        client
            .post(endpoint)
            .json(&batch)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Export to OTLP
    async fn export_otlp(&self, spans: &[Span], endpoint: &str) -> Result<(), String> {
        // Simplified OTLP format
        let resource_spans = serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [{
                        "key": "service.name",
                        "value": { "stringValue": self.config.service_name }
                    }]
                },
                "scopeSpans": [{
                    "spans": spans.iter().map(|s| {
                        serde_json::json!({
                            "traceId": base64::encode(s.trace_id.0),
                            "spanId": base64::encode(s.span_id.0),
                            "parentSpanId": s.parent_id.map(|p| base64::encode(p.0)),
                            "name": s.name,
                            "startTimeUnixNano": s.start_time.duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64,
                            "endTimeUnixNano": s.end_time.map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64),
                            "attributes": s.tags.iter().map(|(k, v)| {
                                serde_json::json!({"key": k, "value": {"stringValue": v}})
                            }).collect::<Vec<_>>(),
                        })
                    }).collect::<Vec<_>>()
                }]
            }]
        });

        let client = reqwest::Client::new();
        client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&resource_spans)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    /// Get statistics
    pub fn stats(&self) -> TracerStats {
        TracerStats {
            spans_created: self.spans_created.load(Ordering::Relaxed),
            spans_exported: self.spans_exported.load(Ordering::Relaxed),
        }
    }

    /// Run periodic export
    pub async fn run_exporter(self: Arc<Self>) {
        loop {
            tokio::time::sleep(self.config.export_interval).await;
            self.export().await;
        }
    }
}

/// Tracer statistics
#[derive(Debug, Clone)]
pub struct TracerStats {
    pub spans_created: u64,
    pub spans_exported: u64,
}

/// Global tracer instance
static TRACER: once_cell::sync::OnceCell<Arc<Tracer>> = once_cell::sync::OnceCell::new();

/// Initialize global tracer
pub fn init_tracer(config: TracerConfig) -> Arc<Tracer> {
    TRACER.get_or_init(|| Tracer::new(config)).clone()
}

/// Get global tracer
pub fn tracer() -> Arc<Tracer> {
    TRACER
        .get()
        .cloned()
        .unwrap_or_else(|| init_tracer(TracerConfig::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_id() {
        let id = TraceId::generate();
        let hex = id.to_hex();
        assert_eq!(hex.len(), 32);

        let parsed = TraceId::from_hex(&hex).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_span_id() {
        let id = SpanId::generate();
        let hex = id.to_hex();
        assert_eq!(hex.len(), 16);
    }

    #[test]
    fn test_span_creation() {
        let trace_id = TraceId::generate();
        let span = Span::new(trace_id, "test_op", "test_service");

        assert_eq!(span.name, "test_op");
        assert_eq!(span.service, "test_service");
        assert_eq!(span.status, SpanStatus::InProgress);
    }

    #[test]
    fn test_child_span() {
        let trace_id = TraceId::generate();
        let parent = Span::new(trace_id, "parent", "service");
        let child = parent.child("child");

        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.parent_id, Some(parent.span_id));
    }

    #[test]
    fn test_span_finish() {
        let trace_id = TraceId::generate();
        let mut span = Span::new(trace_id, "test", "service");

        std::thread::sleep(std::time::Duration::from_millis(10));
        span.finish();

        assert_eq!(span.status, SpanStatus::Ok);
        assert!(span.duration.is_some());
        assert!(span.duration_ms().unwrap() >= 10);
    }

    #[test]
    fn test_span_fail() {
        let trace_id = TraceId::generate();
        let mut span = Span::new(trace_id, "test", "service");
        span.fail("something went wrong");

        assert_eq!(span.status, SpanStatus::Error);
        assert_eq!(span.tags.get("error"), Some(&"something went wrong".to_string()));
    }

    #[test]
    fn test_trace_context_roundtrip() {
        let ctx = TraceContext::new(TraceId::generate(), SpanId::generate());
        let header = ctx.to_traceparent();

        let parsed = TraceContext::from_traceparent(&header).unwrap();
        assert_eq!(ctx.trace_id, parsed.trace_id);
        assert_eq!(ctx.span_id, parsed.span_id);
    }

    #[test]
    fn test_traceparent_parse() {
        let header = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let ctx = TraceContext::from_traceparent(header).unwrap();

        assert!(ctx.sampled);
    }

    #[tokio::test]
    async fn test_tracer() {
        let tracer = Tracer::new(TracerConfig::default());

        let task_id = [1u8; 32];
        let span = tracer.start_job(&task_id).await;

        assert_eq!(span.name, "process_job");

        tracer.end_span(span).await;

        let stats = tracer.stats();
        assert_eq!(stats.spans_created, 1);
    }

    #[tokio::test]
    async fn test_nested_spans() {
        let tracer = Tracer::new(TracerConfig::default());

        let task_id = [1u8; 32];
        let parent = tracer.start_job(&task_id).await;
        let child = tracer.start_span(&parent, "child_op").await;

        assert_eq!(child.parent_id, Some(parent.span_id));

        tracer.end_span(child).await;
        tracer.end_span(parent).await;

        let stats = tracer.stats();
        assert_eq!(stats.spans_created, 2);
    }
}
