//! Structured Logging with JSON Output
//!
//! Provides flexible logging configuration:
//! - Text format for development
//! - JSON format for production/log aggregation
//! - File output option
//! - Log levels via RUST_LOG
//!
//! ## Usage
//!
//! ```rust,ignore
//! use logging::{init_logging, LogConfig};
//!
//! // Text output (default)
//! init_logging(LogConfig::default())?;
//!
//! // JSON output for production
//! init_logging(LogConfig::json())?;
//! ```

use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Logging configuration
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Output format (text, json)
    pub format: LogFormat,
    /// Log file path (optional)
    pub file: Option<String>,
    /// Include timestamps
    pub timestamps: bool,
    /// Include source location (file, line)
    pub source_location: bool,
    /// Include span events (enter, exit)
    pub span_events: bool,
    /// Include thread IDs
    pub thread_ids: bool,
    /// Include thread names
    pub thread_names: bool,
    /// ANSI colors (only for text format)
    pub ansi_colors: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::Text,
            file: None,
            timestamps: true,
            source_location: false,
            span_events: false,
            thread_ids: false,
            thread_names: false,
            ansi_colors: true,
        }
    }
}

impl LogConfig {
    /// Create config for JSON output
    pub fn json() -> Self {
        Self {
            format: LogFormat::Json,
            source_location: true,
            ansi_colors: false,
            ..Default::default()
        }
    }

    /// Create config for production
    pub fn production() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::Json,
            source_location: true,
            span_events: false,
            thread_ids: true,
            ansi_colors: false,
            ..Default::default()
        }
    }

    /// Create config for development
    pub fn development() -> Self {
        Self {
            level: "debug".to_string(),
            format: LogFormat::Text,
            source_location: true,
            span_events: true,
            ansi_colors: true,
            ..Default::default()
        }
    }

    /// Set log level
    pub fn with_level(mut self, level: impl Into<String>) -> Self {
        self.level = level.into();
        self
    }

    /// Set JSON format
    pub fn with_json(mut self) -> Self {
        self.format = LogFormat::Json;
        self.ansi_colors = false;
        self
    }

    /// Set log file
    pub fn with_file(mut self, path: impl Into<String>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Enable source location
    pub fn with_source_location(mut self) -> Self {
        self.source_location = true;
        self
    }

    /// Get span events configuration
    fn get_span_events(&self) -> FmtSpan {
        if self.span_events {
            FmtSpan::ENTER | FmtSpan::EXIT
        } else {
            FmtSpan::NONE
        }
    }
}

/// Log output format
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogFormat {
    /// Human-readable text format
    Text,
    /// Structured JSON format
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" | "plain" => Ok(LogFormat::Text),
            "json" => Ok(LogFormat::Json),
            _ => Err(format!("Unknown log format: {}. Use 'text' or 'json'", s)),
        }
    }
}

/// Initialize logging with the given configuration
pub fn init_logging(config: &LogConfig) -> anyhow::Result<()> {
    // Build filter from level and RUST_LOG
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    match config.format {
        LogFormat::Text => init_text_logging(config, filter),
        LogFormat::Json => init_json_logging(config, filter),
    }
}

fn init_text_logging(config: &LogConfig, filter: EnvFilter) -> anyhow::Result<()> {
    let span_events = config.get_span_events();

    if config.timestamps {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .with_ansi(config.ansi_colors)
                    .with_span_events(span_events)
                    .with_thread_ids(config.thread_ids)
                    .with_thread_names(config.thread_names)
                    .with_file(config.source_location)
                    .with_line_number(config.source_location),
            )
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                fmt::layer()
                    .without_time()
                    .with_ansi(config.ansi_colors)
                    .with_span_events(span_events)
                    .with_thread_ids(config.thread_ids)
                    .with_thread_names(config.thread_names)
                    .with_file(config.source_location)
                    .with_line_number(config.source_location),
            )
            .init();
    }

    Ok(())
}

fn init_json_logging(config: &LogConfig, filter: EnvFilter) -> anyhow::Result<()> {
    let span_events = config.get_span_events();

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_span_events(span_events)
                .with_thread_ids(config.thread_ids)
                .with_thread_names(config.thread_names)
                .with_file(config.source_location)
                .with_line_number(config.source_location)
                .with_current_span(true),
        )
        .init();

    Ok(())
}

/// Log context for structured fields
#[derive(Debug, Clone)]
pub struct LogContext {
    fields: Vec<(String, String)>,
}

impl LogContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    /// Add a field to the context
    pub fn with<K: Into<String>, V: ToString>(mut self, key: K, value: V) -> Self {
        self.fields.push((key.into(), value.to_string()));
        self
    }

    /// Add job ID to context
    pub fn with_job_id(self, job_id: u64) -> Self {
        self.with("job_id", job_id)
    }

    /// Add image ID to context
    pub fn with_image_id(self, image_id: &str) -> Self {
        self.with("image_id", image_id)
    }

    /// Add duration to context
    pub fn with_duration(self, duration: std::time::Duration) -> Self {
        self.with("duration_ms", duration.as_millis())
    }

    /// Add error to context
    pub fn with_error(self, error: &dyn std::fmt::Display) -> Self {
        self.with("error", error.to_string())
    }
}

impl Default for LogContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper functions for common log patterns
pub mod helpers {
    use tracing::{info, warn, error, debug};
    use std::time::Duration;

    /// Log job start
    pub fn log_job_start(job_id: u64, image_id: &str) {
        info!(
            job_id = job_id,
            image_id = %image_id,
            "Job started"
        );
    }

    /// Log job completion
    pub fn log_job_complete(job_id: u64, duration: Duration, cycles: u64) {
        info!(
            job_id = job_id,
            duration_ms = duration.as_millis() as u64,
            cycles = cycles,
            "Job completed"
        );
    }

    /// Log job failure
    pub fn log_job_failed(job_id: u64, error: &str) {
        error!(
            job_id = job_id,
            error = %error,
            "Job failed"
        );
    }

    /// Log RPC call
    pub fn log_rpc_call(method: &str, duration: Duration, success: bool) {
        if success {
            debug!(
                method = %method,
                duration_ms = duration.as_millis() as u64,
                "RPC call succeeded"
            );
        } else {
            warn!(
                method = %method,
                duration_ms = duration.as_millis() as u64,
                "RPC call failed"
            );
        }
    }

    /// Log cache event
    pub fn log_cache_event(image_id: &str, hit: bool) {
        debug!(
            image_id = %image_id,
            cache_hit = hit,
            "Cache lookup"
        );
    }

    /// Log proof generation
    pub fn log_proof_generated(job_id: u64, duration: Duration, size_bytes: usize) {
        info!(
            job_id = job_id,
            duration_ms = duration.as_millis() as u64,
            size_bytes = size_bytes,
            "Proof generated"
        );
    }

    /// Log transaction submission
    pub fn log_tx_submitted(job_id: u64, tx_hash: &str) {
        info!(
            job_id = job_id,
            tx_hash = %tx_hash,
            "Transaction submitted"
        );
    }

    /// Log transaction confirmed
    pub fn log_tx_confirmed(job_id: u64, tx_hash: &str, block: u64, gas_used: u64) {
        info!(
            job_id = job_id,
            tx_hash = %tx_hash,
            block = block,
            gas_used = gas_used,
            "Transaction confirmed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_config_default() {
        let config = LogConfig::default();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, LogFormat::Text);
        assert!(config.timestamps);
    }

    #[test]
    fn test_log_config_json() {
        let config = LogConfig::json();
        assert_eq!(config.format, LogFormat::Json);
        assert!(config.source_location);
        assert!(!config.ansi_colors);
    }

    #[test]
    fn test_log_config_production() {
        let config = LogConfig::production();
        assert_eq!(config.level, "info");
        assert_eq!(config.format, LogFormat::Json);
        assert!(config.thread_ids);
    }

    #[test]
    fn test_log_config_development() {
        let config = LogConfig::development();
        assert_eq!(config.level, "debug");
        assert_eq!(config.format, LogFormat::Text);
        assert!(config.span_events);
    }

    #[test]
    fn test_log_format_parse() {
        assert_eq!("text".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("json".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert_eq!("TEXT".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("JSON".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert!("invalid".parse::<LogFormat>().is_err());
    }

    #[test]
    fn test_log_config_builder() {
        let config = LogConfig::default()
            .with_level("debug")
            .with_json()
            .with_source_location()
            .with_file("/var/log/prover.log");

        assert_eq!(config.level, "debug");
        assert_eq!(config.format, LogFormat::Json);
        assert!(config.source_location);
        assert_eq!(config.file, Some("/var/log/prover.log".to_string()));
    }

    #[test]
    fn test_log_context() {
        let ctx = LogContext::new()
            .with_job_id(123)
            .with_image_id("abc123")
            .with_duration(std::time::Duration::from_secs(5));

        assert_eq!(ctx.fields.len(), 3);
    }

    #[test]
    fn test_span_events() {
        let config = LogConfig::default();
        assert_eq!(config.get_span_events(), FmtSpan::NONE);

        let config = LogConfig::development();
        assert_ne!(config.get_span_events(), FmtSpan::NONE);
    }
}
