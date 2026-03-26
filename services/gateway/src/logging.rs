//! Request/response audit logging middleware.
//!
//! Emits structured JSON log entries for every HTTP request passing through the
//! gateway. Each entry includes: timestamp, unique request ID, method, path,
//! response status, latency, and a client identifier extracted from the
//! `Authorization` header (or `"anonymous"` when absent).
//!
//! The generated request ID is also returned to the caller in the
//! `X-Request-Id` response header so that operators and clients can correlate
//! log entries with specific requests.
//!
//! # Configuration
//!
//! | Variable             | Description                                     | Default  |
//! |----------------------|-------------------------------------------------|----------|
//! | `GATEWAY_LOG_LEVEL`  | Minimum log level (`trace`, `debug`, `info`, ..)| `info`   |
//! | `GATEWAY_LOG_FORMAT` | `json` for machine-readable, `pretty` for human | `json`   |

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use std::time::Instant;

/// Header name for the unique per-request identifier.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Axum middleware that logs every request in structured JSON format and
/// attaches an `X-Request-Id` header to the response.
pub async fn request_logger(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // Extract a client identifier from the Authorization header.  We take the
    // whole header value (not just the bearer token) so callers using other
    // auth schemes still get a recognisable identifier.  If no header is
    // present the entry is tagged "anonymous".
    let client_id = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string();

    let request_id = uuid::Uuid::new_v4().to_string();

    let start = Instant::now();
    let mut response = next.run(req).await;
    let latency = start.elapsed();

    let log_entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "request_id": request_id,
        "method": method.as_str(),
        "path": path,
        "status": response.status().as_u16(),
        "latency_ms": latency.as_millis(),
        "client_id": client_id,
    });

    tracing::info!(target: "audit", "{}", log_entry);

    // Attach the request ID as a response header so callers can reference it.
    if let Ok(val) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, val);
    }

    response
}

/// Initialise the `tracing_subscriber` with settings derived from environment
/// variables.
///
/// - `GATEWAY_LOG_LEVEL` (default `"info"`) controls the minimum severity.
/// - `GATEWAY_LOG_FORMAT` (default `"json"`) selects between compact JSON
///   output (`"json"`) and the default human-readable formatter (`"pretty"`).
pub fn init_tracing() {
    let level = std::env::var("GATEWAY_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let format = std::env::var("GATEWAY_LOG_FORMAT").unwrap_or_else(|_| "json".to_string());

    // Build a filter that respects the explicit level but also allows
    // `RUST_LOG` to override at a finer granularity.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("api_gateway={level},tower_http={level}"))
    });

    match format.as_str() {
        "pretty" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .pretty()
                .init();
        }
        // Default: compact JSON-ish output (tracing-subscriber's default
        // formatter is compact and machine-parseable enough; for true
        // structured JSON use the `tracing-subscriber` json feature).
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{any, get};
    use axum::{body::Body, middleware, Router};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use tracing_subscriber::layer::SubscriberExt;

    /// Trivial handler used by logging middleware tests.
    async fn ok_handler() -> &'static str {
        "ok"
    }

    /// Build a minimal router with only the logging middleware applied.
    /// Uses `any` so all HTTP methods are accepted (needed for testing POST).
    fn test_app() -> Router {
        Router::new()
            .route("/test", any(ok_handler))
            .layer(middleware::from_fn(request_logger))
    }

    /// A thin `io::Write` wrapper around a shared `Vec<u8>` so we can capture
    /// tracing output in tests.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Create a tracing subscriber that writes JSON output into the provided
    /// buffer, and return a `DefaultGuard` that sets it as the thread-local
    /// default for the lifetime of the guard.
    fn install_test_subscriber(buf: &Arc<Mutex<Vec<u8>>>) -> tracing::subscriber::DefaultGuard {
        let buf_clone = Arc::clone(buf);
        let writer = move || -> Box<dyn std::io::Write + Send> {
            Box::new(BufWriter(Arc::clone(&buf_clone)))
        };

        let fmt_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(writer)
            .with_target(true);

        let subscriber = tracing_subscriber::registry().with(fmt_layer);
        tracing::subscriber::set_default(subscriber)
    }

    #[tokio::test]
    async fn test_request_id_returned_in_header() {
        let app = test_app();

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let request_id = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("X-Request-Id header must be present");

        // Must be a valid UUID v4 string.
        let id_str = request_id.to_str().unwrap();
        let parsed = uuid::Uuid::parse_str(id_str);
        assert!(
            parsed.is_ok(),
            "X-Request-Id must be a valid UUID: {id_str}"
        );
        assert_eq!(
            parsed.unwrap().get_version(),
            Some(uuid::Version::Random),
            "UUID must be v4"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_log_entry_contains_all_fields() {
        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        // The guard keeps the subscriber active for the duration of this scope,
        // including across `.await` points on the current-thread runtime.
        let _guard = install_test_subscriber(&buf);

        let app = test_app();
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/test")
                    .header("Authorization", "Bearer test-key-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let captured = buf.lock().unwrap();
        let log_output = String::from_utf8_lossy(&captured);

        // The audit log entry is JSON-encoded inside the outer tracing JSON's
        // "message" field.  Field names appear with escaped quotes (e.g.
        // `\"request_id\"`).  We search for the unquoted field names which are
        // present regardless of JSON nesting.
        assert!(
            log_output.contains("request_id"),
            "log must contain request_id"
        );
        assert!(log_output.contains("method"), "log must contain method");
        assert!(log_output.contains("path"), "log must contain path");
        assert!(log_output.contains("status"), "log must contain status");
        assert!(
            log_output.contains("latency_ms"),
            "log must contain latency_ms"
        );
        assert!(
            log_output.contains("client_id"),
            "log must contain client_id"
        );
        assert!(
            log_output.contains("timestamp"),
            "log must contain timestamp"
        );
        assert!(
            log_output.contains("test-key-123"),
            "log must contain the client credential"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_latency_is_measured() {
        async fn slow_handler() -> &'static str {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            "slow"
        }

        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let _guard = install_test_subscriber(&buf);

        let app = Router::new()
            .route("/slow", get(slow_handler))
            .layer(middleware::from_fn(request_logger));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/slow")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let captured = buf.lock().unwrap();
        let log_output = String::from_utf8_lossy(&captured);

        assert!(
            log_output.contains("latency_ms"),
            "log must contain latency_ms"
        );

        // Parse out the latency value to confirm it is > 0.
        // The audit message is JSON-encoded inside the outer tracing JSON, so
        // field names are escaped.  We look for `latency_ms\": ` or
        // `latency_ms\":` followed by a number.
        let needle = "latency_ms";
        if let Some(pos) = log_output.find(needle) {
            // Skip past the field name, colon separator, and any non-digit chars.
            let after = &log_output[pos + needle.len()..];
            let num_str: String = after
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let latency: u64 = num_str.parse().unwrap_or(0);
            assert!(
                latency >= 10,
                "latency_ms should be >= 10 (handler sleeps 10ms), got {latency}"
            );
        } else {
            panic!("Could not find latency_ms in log output");
        }
    }
}
