//! OpenTelemetry distributed tracing setup for the worldzk-enclave service.
//!
//! Provides a [`TracingConfig`] struct that reads configuration from environment
//! variables and an [`init_tracing`] function that initializes the `tracing`
//! subscriber with layered fmt output. When the `tracing-opentelemetry` and
//! `opentelemetry-otlp` crates are added to `Cargo.toml`, the OTLP exporter
//! layer can be wired in alongside the existing fmt layer.
//!
//! # Environment Variables
//!
//! | Variable                       | Default                    | Description                        |
//! |-------------------------------|----------------------------|------------------------------------|
//! | `OTEL_ENABLED`                | `false`                    | Enable OpenTelemetry export        |
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317`    | OTLP collector gRPC endpoint       |
//! | `OTEL_SERVICE_NAME`           | `worldzk-enclave`          | Service name for traces            |
//! | `LOG_LEVEL`                   | `info`                     | Log level filter directive         |
//!
//! Note: The enclave's `tracing-subscriber` dependency does not include the
//! `env-filter` or `json` features. Log filtering is done via a simple
//! `LevelFilter` derived from the `LOG_LEVEL` env var. If you need `RUST_LOG`
//! directive-style filtering, add the `env-filter` feature to `Cargo.toml`.

use tracing::Span;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Default OTLP gRPC endpoint.
pub const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";

/// Default service name for the enclave.
pub const DEFAULT_SERVICE_NAME: &str = "worldzk-enclave";

/// Default log level.
pub const DEFAULT_LOG_LEVEL: &str = "info";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the tracing / OpenTelemetry subsystem.
#[derive(Debug, Clone, PartialEq)]
pub struct TracingConfig {
    /// Logical service name reported to the collector.
    pub service_name: String,
    /// OTLP gRPC endpoint (e.g. `http://localhost:4317`).
    pub otlp_endpoint: String,
    /// Tracing filter level (e.g. `info`, `debug`, `trace`, `warn`, `error`).
    pub log_level: String,
    /// Whether the OpenTelemetry export pipeline is enabled.
    pub enabled: bool,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            service_name: DEFAULT_SERVICE_NAME.to_string(),
            otlp_endpoint: DEFAULT_OTLP_ENDPOINT.to_string(),
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            enabled: false,
        }
    }
}

impl TracingConfig {
    /// Build a [`TracingConfig`] from environment variables.
    ///
    /// Any variable that is unset or empty falls back to the default value.
    pub fn from_env() -> Self {
        let service_name =
            non_empty_env("OTEL_SERVICE_NAME").unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());
        let otlp_endpoint = non_empty_env("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string());
        let log_level = non_empty_env("LOG_LEVEL").unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string());
        let enabled = non_empty_env("OTEL_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        Self {
            service_name,
            otlp_endpoint,
            log_level,
            enabled,
        }
    }

    /// Build a [`TracingConfig`] with explicit values, using defaults for any
    /// `None` parameter.
    #[allow(dead_code)] // Used in tests and by lib.rs consumers
    pub fn new(service_name: Option<&str>, otlp_endpoint: Option<&str>) -> Self {
        Self {
            service_name: service_name.unwrap_or(DEFAULT_SERVICE_NAME).to_string(),
            otlp_endpoint: otlp_endpoint.unwrap_or(DEFAULT_OTLP_ENDPOINT).to_string(),
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Parse a log-level string into a [`tracing::level_filters::LevelFilter`].
fn parse_level_filter(level: &str) -> tracing::level_filters::LevelFilter {
    match level.to_lowercase().as_str() {
        "trace" => tracing::level_filters::LevelFilter::TRACE,
        "debug" => tracing::level_filters::LevelFilter::DEBUG,
        "info" => tracing::level_filters::LevelFilter::INFO,
        "warn" | "warning" => tracing::level_filters::LevelFilter::WARN,
        "error" => tracing::level_filters::LevelFilter::ERROR,
        "off" => tracing::level_filters::LevelFilter::OFF,
        _ => tracing::level_filters::LevelFilter::INFO,
    }
}

/// Initialise the global tracing subscriber.
///
/// Sets up a `tracing_subscriber` with a level filter parsed from `LOG_LEVEL`
/// and a human-readable fmt layer. When `OTEL_ENABLED=true`, additional
/// OpenTelemetry layers can be registered once the OTLP crates are added to
/// the dependency list.
///
/// This function should be called exactly once, early in `main()`.
///
/// # Panics
///
/// Panics if a global subscriber has already been set.
pub fn init_tracing(service_name: &str, otlp_endpoint: Option<&str>) {
    let config = TracingConfig {
        service_name: service_name.to_string(),
        otlp_endpoint: otlp_endpoint.unwrap_or(DEFAULT_OTLP_ENDPOINT).to_string(),
        ..TracingConfig::from_env()
    };
    init_tracing_with_config(&config);
}

/// Initialise the global tracing subscriber from a [`TracingConfig`].
///
/// Currently installs:
/// - A [`LevelFilter`](tracing::level_filters::LevelFilter) layer from `config.log_level`
/// - `fmt` layer (human-readable, with target and level)
///
/// When OpenTelemetry crates are available, an OTLP span-exporter layer will
/// be added here conditionally on `config.enabled`.
pub fn init_tracing_with_config(config: &TracingConfig) {
    let level_filter = parse_level_filter(&config.log_level);

    if config.enabled {
        // Enhanced output with span events and thread IDs for observability
        tracing_subscriber::registry()
            .with(level_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_thread_ids(true)
                    .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE),
            )
            .init();

        tracing::info!(
            service_name = %config.service_name,
            otlp_endpoint = %config.otlp_endpoint,
            "OpenTelemetry configured (OTLP export requires tracing-opentelemetry crate)"
        );
    } else {
        tracing_subscriber::registry()
            .with(level_filter)
            .with(tracing_subscriber::fmt::layer().with_target(true))
            .init();
    }

    tracing::debug!(
        service_name = %config.service_name,
        log_level = %config.log_level,
        otel_enabled = config.enabled,
        "Tracing subsystem initialised"
    );
}

/// Placeholder for graceful OpenTelemetry shutdown.
///
/// Once the `opentelemetry` crate is wired in, this function should call
/// `opentelemetry::global::shutdown_tracer_provider()` to flush any pending
/// spans before the process exits.
#[allow(dead_code)] // Will be wired in when opentelemetry crate is added
pub fn shutdown_tracer_provider() {
    tracing::debug!(
        "Tracer provider shutdown requested (no-op until opentelemetry crate is added)"
    );
    // When opentelemetry is available:
    // opentelemetry::global::shutdown_tracer_provider();
}

// ---------------------------------------------------------------------------
// Span helpers -- enclave-specific
// ---------------------------------------------------------------------------

/// Create a tracing span for an inference request.
///
/// Records the model name and number of input features.
#[allow(dead_code)] // Available for structured tracing in handler hot path
pub fn span_inference(model_name: &str, num_features: usize) -> Span {
    tracing::info_span!(
        "inference",
        otel.kind = "SERVER",
        service.name = DEFAULT_SERVICE_NAME,
        model_name = %model_name,
        num_features = num_features,
    )
}

/// Create a tracing span for an attestation generation or verification flow.
///
/// Records the attestation type (e.g. "nitro", "mock") and the chain ID.
#[allow(dead_code)] // Available for structured tracing in attestation flow
pub fn span_attestation(attestation_type: &str, chain_id: u64) -> Span {
    tracing::info_span!(
        "attestation",
        otel.kind = "INTERNAL",
        service.name = DEFAULT_SERVICE_NAME,
        attestation_type = %attestation_type,
        chain_id = chain_id,
    )
}

/// Create a tracing span for model loading / hot-reload.
///
/// Records the model file path and format.
#[allow(dead_code)] // Available for structured tracing in model reload flow
pub fn span_model_load(model_path: &str, model_format: &str) -> Span {
    tracing::info_span!(
        "model_load",
        otel.kind = "INTERNAL",
        service.name = DEFAULT_SERVICE_NAME,
        model_path = %model_path,
        model_format = %model_format,
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the value of an environment variable if it is set and non-empty.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| {
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Helper: clear all OTEL-related env vars to get predictable defaults.
    fn clear_otel_env() {
        std::env::remove_var("OTEL_ENABLED");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        std::env::remove_var("OTEL_SERVICE_NAME");
        std::env::remove_var("LOG_LEVEL");
    }

    #[test]
    #[serial]
    fn test_tracing_config_defaults() {
        clear_otel_env();
        let config = TracingConfig::from_env();
        assert_eq!(config.service_name, "worldzk-enclave");
        assert_eq!(config.otlp_endpoint, "http://localhost:4317");
        assert_eq!(config.log_level, "info");
        assert!(!config.enabled);
    }

    #[test]
    #[serial]
    fn test_tracing_config_from_env_enabled() {
        clear_otel_env();
        std::env::set_var("OTEL_ENABLED", "true");
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317");
        std::env::set_var("OTEL_SERVICE_NAME", "my-enclave");
        std::env::set_var("LOG_LEVEL", "trace");

        let config = TracingConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.otlp_endpoint, "http://collector:4317");
        assert_eq!(config.service_name, "my-enclave");
        assert_eq!(config.log_level, "trace");

        clear_otel_env();
    }

    #[test]
    #[serial]
    fn test_tracing_config_otel_enabled_values() {
        clear_otel_env();

        // "1" should enable
        std::env::set_var("OTEL_ENABLED", "1");
        assert!(TracingConfig::from_env().enabled);

        // "TRUE" (case-insensitive) should enable
        std::env::set_var("OTEL_ENABLED", "TRUE");
        assert!(TracingConfig::from_env().enabled);

        // "false" should disable
        std::env::set_var("OTEL_ENABLED", "false");
        assert!(!TracingConfig::from_env().enabled);

        // "0" should disable
        std::env::set_var("OTEL_ENABLED", "0");
        assert!(!TracingConfig::from_env().enabled);

        // empty string should disable
        std::env::set_var("OTEL_ENABLED", "");
        assert!(!TracingConfig::from_env().enabled);

        clear_otel_env();
    }

    #[test]
    fn test_tracing_config_new_with_overrides() {
        let config = TracingConfig::new(Some("custom-enclave"), Some("http://remote:4317"));
        assert_eq!(config.service_name, "custom-enclave");
        assert_eq!(config.otlp_endpoint, "http://remote:4317");
    }

    #[test]
    #[serial]
    fn test_tracing_config_new_with_defaults() {
        clear_otel_env();
        let config = TracingConfig::new(None, None);
        assert_eq!(config.service_name, DEFAULT_SERVICE_NAME);
        assert_eq!(config.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
    }

    #[test]
    fn test_parse_level_filter() {
        assert_eq!(
            parse_level_filter("trace"),
            tracing::level_filters::LevelFilter::TRACE
        );
        assert_eq!(
            parse_level_filter("debug"),
            tracing::level_filters::LevelFilter::DEBUG
        );
        assert_eq!(
            parse_level_filter("info"),
            tracing::level_filters::LevelFilter::INFO
        );
        assert_eq!(
            parse_level_filter("warn"),
            tracing::level_filters::LevelFilter::WARN
        );
        assert_eq!(
            parse_level_filter("warning"),
            tracing::level_filters::LevelFilter::WARN
        );
        assert_eq!(
            parse_level_filter("error"),
            tracing::level_filters::LevelFilter::ERROR
        );
        assert_eq!(
            parse_level_filter("off"),
            tracing::level_filters::LevelFilter::OFF
        );
        assert_eq!(
            parse_level_filter("INFO"),
            tracing::level_filters::LevelFilter::INFO
        );
        assert_eq!(
            parse_level_filter("unknown"),
            tracing::level_filters::LevelFilter::INFO
        );
    }

    #[test]
    fn test_span_inference_fields() {
        // Verify span creation does not panic (span may be disabled without a
        // global subscriber, which is expected in unit tests).
        let _span = span_inference("iris-xgboost", 4);
    }

    #[test]
    fn test_span_attestation_fields() {
        let _span = span_attestation("nitro", 11155111);
    }

    #[test]
    fn test_span_model_load_fields() {
        let _span = span_model_load("/app/model/model.json", "xgboost");
    }

    #[test]
    #[serial]
    fn test_non_empty_env_helper() {
        clear_otel_env();

        // Not set
        assert!(non_empty_env("__TEST_ENCLAVE_TRACING_NONEXISTENT__").is_none());

        // Set but empty
        std::env::set_var("__TEST_ENCLAVE_TRACING_EMPTY__", "");
        assert!(non_empty_env("__TEST_ENCLAVE_TRACING_EMPTY__").is_none());
        std::env::remove_var("__TEST_ENCLAVE_TRACING_EMPTY__");

        // Set with whitespace only
        std::env::set_var("__TEST_ENCLAVE_TRACING_WS__", "   ");
        assert!(non_empty_env("__TEST_ENCLAVE_TRACING_WS__").is_none());
        std::env::remove_var("__TEST_ENCLAVE_TRACING_WS__");

        // Set with value
        std::env::set_var("__TEST_ENCLAVE_TRACING_VAL__", "world");
        assert_eq!(
            non_empty_env("__TEST_ENCLAVE_TRACING_VAL__"),
            Some("world".to_string())
        );
        std::env::remove_var("__TEST_ENCLAVE_TRACING_VAL__");
    }

    #[test]
    fn test_shutdown_tracer_provider_does_not_panic() {
        shutdown_tracer_provider();
    }

    #[test]
    #[serial]
    fn test_tracing_config_default_trait() {
        let config = TracingConfig::default();
        assert_eq!(config.service_name, DEFAULT_SERVICE_NAME);
        assert_eq!(config.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
        assert_eq!(config.log_level, DEFAULT_LOG_LEVEL);
        assert!(!config.enabled);
    }
}
