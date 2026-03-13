#![allow(dead_code)]
//! OpenTelemetry distributed tracing setup for the worldzk-operator service.
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
//! | `OTEL_SERVICE_NAME`           | `worldzk-operator`         | Service name for traces            |
//! | `LOG_LEVEL`                   | `info`                     | Log level filter directive         |

use tracing::Span;
use tracing_subscriber::EnvFilter;

/// Default OTLP gRPC endpoint.
pub const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:4317";

/// Default service name for the operator.
pub const DEFAULT_SERVICE_NAME: &str = "worldzk-operator";

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
    /// Tracing filter directive (e.g. `info`, `debug`, `tee_operator=trace`).
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

/// Initialise the global tracing subscriber.
///
/// Sets up a `tracing_subscriber` with an `EnvFilter` (respecting `LOG_LEVEL`
/// and `RUST_LOG`) and a human-readable fmt layer. When `OTEL_ENABLED=true`,
/// additional OpenTelemetry layers can be registered once the OTLP crates are
/// added to the dependency list.
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
/// - `EnvFilter` layer (reads `RUST_LOG`, falls back to `config.log_level`)
/// - `fmt` layer (human-readable, with target and level)
///
/// When OpenTelemetry crates are available, an OTLP span-exporter layer will
/// be added here conditionally on `config.enabled`.
pub fn init_tracing_with_config(config: &TracingConfig) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    let json_format = std::env::var("RUST_LOG_FORMAT")
        .map(|v| v == "json")
        .unwrap_or(false);

    if json_format {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .init();
    }

    if config.enabled {
        tracing::info!(
            service_name = %config.service_name,
            otlp_endpoint = %config.otlp_endpoint,
            "OpenTelemetry configured (OTLP export requires tracing-opentelemetry crate)"
        );
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
pub fn shutdown_tracer_provider() {
    tracing::debug!(
        "Tracer provider shutdown requested (no-op until opentelemetry crate is added)"
    );
    // When opentelemetry is available:
    // opentelemetry::global::shutdown_tracer_provider();
}

// ---------------------------------------------------------------------------
// Span helpers -- operator-specific
// ---------------------------------------------------------------------------

/// Create a tracing span for a watch-cycle iteration.
///
/// Records the block range being polled and the number of events found.
pub fn span_watch_cycle(from_block: u64, to_block: u64, chain_id: u64) -> Span {
    tracing::info_span!(
        "watch_cycle",
        otel.kind = "INTERNAL",
        service.name = DEFAULT_SERVICE_NAME,
        chain_id = chain_id,
        from_block = from_block,
        to_block = to_block,
    )
}

/// Create a tracing span for submitting a proof on-chain.
///
/// Records the result identifier and target contract address.
pub fn span_submit_proof(result_id: &str, contract_address: &str) -> Span {
    tracing::info_span!(
        "submit_proof",
        otel.kind = "CLIENT",
        service.name = DEFAULT_SERVICE_NAME,
        result_id = %result_id,
        contract_address = %contract_address,
    )
}

/// Create a tracing span for a dispute resolution flow.
///
/// Records the result identifier and the challenger address.
pub fn span_dispute(result_id: &str, challenger: &str) -> Span {
    tracing::info_span!(
        "dispute",
        otel.kind = "INTERNAL",
        service.name = DEFAULT_SERVICE_NAME,
        result_id = %result_id,
        challenger = %challenger,
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

    /// Helper: clear all OTEL-related env vars to get predictable defaults.
    fn clear_otel_env() {
        std::env::remove_var("OTEL_ENABLED");
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        std::env::remove_var("OTEL_SERVICE_NAME");
        std::env::remove_var("LOG_LEVEL");
        std::env::remove_var("RUST_LOG");
        std::env::remove_var("RUST_LOG_FORMAT");
    }

    #[test]
    fn test_tracing_config_defaults() {
        clear_otel_env();
        let config = TracingConfig::from_env();
        assert_eq!(config.service_name, "worldzk-operator");
        assert_eq!(config.otlp_endpoint, "http://localhost:4317");
        assert_eq!(config.log_level, "info");
        assert!(!config.enabled);
    }

    #[test]
    fn test_tracing_config_from_env_enabled() {
        clear_otel_env();
        std::env::set_var("OTEL_ENABLED", "true");
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317");
        std::env::set_var("OTEL_SERVICE_NAME", "my-operator");
        std::env::set_var("LOG_LEVEL", "debug");

        let config = TracingConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.otlp_endpoint, "http://collector:4317");
        assert_eq!(config.service_name, "my-operator");
        assert_eq!(config.log_level, "debug");

        clear_otel_env();
    }

    #[test]
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
        let config = TracingConfig::new(Some("custom-service"), Some("http://remote:4317"));
        assert_eq!(config.service_name, "custom-service");
        assert_eq!(config.otlp_endpoint, "http://remote:4317");
    }

    #[test]
    fn test_tracing_config_new_with_defaults() {
        clear_otel_env();
        let config = TracingConfig::new(None, None);
        assert_eq!(config.service_name, DEFAULT_SERVICE_NAME);
        assert_eq!(config.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
    }

    #[test]
    fn test_span_watch_cycle_fields() {
        // Verify span creation does not panic (span may be disabled without a
        // global subscriber, which is expected in unit tests).
        let _span = span_watch_cycle(100, 200, 1);
    }

    #[test]
    fn test_span_submit_proof_fields() {
        let _span = span_submit_proof("0xabc123", "0xContractAddr");
    }

    #[test]
    fn test_span_dispute_fields() {
        let _span = span_dispute("0xresult1", "0xchallenger1");
    }

    #[test]
    fn test_non_empty_env_helper() {
        clear_otel_env();

        // Not set
        assert!(non_empty_env("__TEST_TRACING_NONEXISTENT__").is_none());

        // Set but empty
        std::env::set_var("__TEST_TRACING_EMPTY__", "");
        assert!(non_empty_env("__TEST_TRACING_EMPTY__").is_none());
        std::env::remove_var("__TEST_TRACING_EMPTY__");

        // Set with whitespace only
        std::env::set_var("__TEST_TRACING_WS__", "   ");
        assert!(non_empty_env("__TEST_TRACING_WS__").is_none());
        std::env::remove_var("__TEST_TRACING_WS__");

        // Set with value
        std::env::set_var("__TEST_TRACING_VAL__", "hello");
        assert_eq!(
            non_empty_env("__TEST_TRACING_VAL__"),
            Some("hello".to_string())
        );
        std::env::remove_var("__TEST_TRACING_VAL__");
    }

    #[test]
    fn test_shutdown_tracer_provider_does_not_panic() {
        // The current implementation is a no-op; verify it does not panic.
        shutdown_tracer_provider();
    }

    #[test]
    fn test_tracing_config_default_trait() {
        let config = TracingConfig::default();
        assert_eq!(config.service_name, DEFAULT_SERVICE_NAME);
        assert_eq!(config.otlp_endpoint, DEFAULT_OTLP_ENDPOINT);
        assert_eq!(config.log_level, DEFAULT_LOG_LEVEL);
        assert!(!config.enabled);
    }
}
