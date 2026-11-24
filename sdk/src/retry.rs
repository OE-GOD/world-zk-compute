//! Retry logic with exponential backoff for resilient RPC communication.

use std::future::Future;
use std::time::Duration;

use crate::client::TransactionReverted;

/// Internal helper macro: emit a debug-level tracing event when the `tracing`
/// feature is enabled.  When disabled the call compiles to nothing.
macro_rules! trace_debug {
    ($($arg:tt)*) => {
        #[cfg(feature = "tracing")]
        {
            tracing::debug!($($arg)*);
        }
    };
}

/// Policy controlling retry behavior.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (0 = no retries, just the initial call).
    pub max_retries: u32,
    /// Base delay between retries (doubles each attempt).
    pub base_delay: Duration,
    /// Maximum delay cap.
    pub max_delay: Duration,
    /// Jitter fraction (0.0–1.0). Adds up to this fraction of the delay as random noise.
    pub jitter: f64,
    /// Per-attempt timeout. If set, each individual attempt is wrapped with
    /// `tokio::time::timeout()`. A timed-out attempt counts as a retryable error.
    pub timeout: Option<Duration>,
    /// Total operation timeout across all retries. If the cumulative elapsed time
    /// exceeds this value, the operation aborts immediately regardless of remaining
    /// retry budget.
    pub total_timeout: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: 0.2,
            timeout: Some(Duration::from_secs(30)),
            total_timeout: Some(Duration::from_secs(300)),
        }
    }
}

impl RetryPolicy {
    /// Compute the delay for attempt `n` (0-indexed).
    fn delay_for(&self, attempt: u32) -> Duration {
        let base_ms = self.base_delay.as_millis() as u64;
        let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(16));
        let max_ms = self.max_delay.as_millis() as u64;
        let capped_ms = exp_ms.min(max_ms);

        if self.jitter > 0.0 {
            // Deterministic-ish jitter based on attempt number to avoid rand dependency.
            // For real production use, consider adding `rand` crate.
            let jitter_factor = 1.0 + self.jitter * pseudo_random_fraction(attempt);
            Duration::from_millis((capped_ms as f64 * jitter_factor) as u64)
        } else {
            Duration::from_millis(capped_ms)
        }
    }

    /// Set the per-attempt timeout. Each individual call to the closure will be
    /// cancelled if it exceeds this duration. A timed-out attempt is treated as
    /// a retryable error.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set the total operation timeout across all retries. If the cumulative
    /// elapsed time (including delays) exceeds this value, the operation aborts
    /// immediately with a timeout error.
    pub fn with_total_timeout(mut self, total_timeout: Duration) -> Self {
        self.total_timeout = Some(total_timeout);
        self
    }
}

/// Simple pseudo-random fraction [0, 1) based on a seed. Not cryptographic.
fn pseudo_random_fraction(seed: u32) -> f64 {
    let x = seed.wrapping_mul(2654435761);
    (x as f64) / (u32::MAX as f64)
}

/// HTTP status codes worth retrying (rate limiting and server-side failures).
const RETRYABLE_STATUS_CODES: [&str; 5] = ["429", "500", "502", "503", "504"];

/// True if `code` occurs in `msg` as a number of its own, not as part of a
/// longer number or hex string (a gas amount, an address, a transaction hash).
fn contains_status_code(msg: &str, code: &str) -> bool {
    msg.match_indices(code).any(|(start, _)| {
        let before = msg[..start].chars().next_back();
        let after = msg[start + code.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// Returns true if the error message suggests a transient/retryable failure.
///
/// Every cause in the error chain is inspected, not only the outermost context.
/// A contract revert is never retryable: the same call reverts again. That
/// covers a call the node refused ("execution reverted") and a transaction
/// that was mined and reverted ([`TransactionReverted`]).
pub fn is_retryable(err: &anyhow::Error) -> bool {
    if err
        .chain()
        .any(|cause| cause.downcast_ref::<TransactionReverted>().is_some())
    {
        return false;
    }
    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("reverted") {
        return false;
    }
    // Connection / transport errors
    msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("broken pipe")
        || msg.contains("eof")
        // HTTP statuses typically retryable
        || msg.contains("too many requests")
        || msg.contains("rate limit")
        || RETRYABLE_STATUS_CODES
            .iter()
            .any(|code| contains_status_code(&msg, code))
}

/// Execute an async closure with retries according to the given policy.
///
/// Only retries if `should_retry` returns true for the error. Non-retryable errors
/// are returned immediately.
///
/// If `policy.timeout` is set, each individual attempt is wrapped with
/// `tokio::time::timeout()`. A timed-out attempt is treated as a retryable error.
///
/// If `policy.total_timeout` is set, the entire operation (including inter-attempt
/// delays) will abort once the cumulative elapsed time exceeds the limit.
///
/// The closure runs again from the start on every retry, so only wrap operations
/// that are safe to repeat (reads, `eth_call`, gas estimates). A transaction send
/// is not: an attempt that timed out or lost its connection may already have
/// broadcast the transaction, and the retry would broadcast a second one.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub async fn retry_with_backoff<F, Fut, T>(policy: &RetryPolicy, mut f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut last_err = None;
    let start = tokio::time::Instant::now();

    for attempt in 0..=policy.max_retries {
        // Check total timeout before starting a new attempt.
        if let Some(total) = policy.total_timeout {
            if start.elapsed() >= total {
                trace_debug!(
                    attempt,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "total timeout exceeded, aborting retries"
                );
                return Err(last_err.unwrap_or_else(|| {
                    anyhow::anyhow!("total timeout ({total:?}) exceeded after {attempt} attempt(s)")
                }));
            }
        }

        trace_debug!(
            attempt,
            max_retries = policy.max_retries,
            "starting attempt"
        );

        // Execute the attempt, optionally wrapped in a per-attempt timeout.
        let attempt_result = if let Some(per_attempt) = policy.timeout {
            match tokio::time::timeout(per_attempt, f()).await {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    trace_debug!(
                        attempt,
                        timeout_ms = per_attempt.as_millis() as u64,
                        "attempt timed out"
                    );
                    Err(anyhow::anyhow!("attempt timed out after {per_attempt:?}"))
                }
            }
        } else {
            f().await
        };

        match attempt_result {
            Ok(val) => {
                trace_debug!(attempt, "attempt succeeded");
                return Ok(val);
            }
            Err(e) => {
                let retryable = is_retryable(&e);
                trace_debug!(
                    attempt,
                    retryable,
                    error = %e,
                    "attempt failed"
                );
                if attempt == policy.max_retries || !retryable {
                    return Err(e);
                }
                last_err = Some(e);
                let delay = policy.delay_for(attempt);

                // If a total timeout is set, clamp the sleep so we don't overshoot.
                if let Some(total) = policy.total_timeout {
                    let remaining = total.saturating_sub(start.elapsed());
                    if remaining.is_zero() {
                        trace_debug!(attempt, "total timeout exceeded during backoff, aborting");
                        return Err(last_err.unwrap_or_else(|| {
                            anyhow::anyhow!(
                                "total timeout ({total:?}) exceeded after {attempt} attempt(s)"
                            )
                        }));
                    }
                    let actual_delay = delay.min(remaining);
                    trace_debug!(
                        attempt,
                        delay_ms = actual_delay.as_millis() as u64,
                        "sleeping before next retry (clamped by total timeout)"
                    );
                    tokio::time::sleep(actual_delay).await;
                } else {
                    trace_debug!(
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        "sleeping before next retry"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("retry exhausted with no error")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_succeeds_immediately() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            ..Default::default()
        };
        let result = retry_with_backoff(&policy, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retries_on_transient_failure() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            ..Default::default()
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(anyhow::anyhow!("connection refused"))
                } else {
                    Ok(99)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(counter.load(Ordering::SeqCst), 3); // initial + 2 retries
    }

    #[tokio::test]
    async fn test_gives_up_after_max_retries() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            ..Default::default()
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: anyhow::Result<i32> = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("503 service unavailable"))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 3); // initial + 2 retries
    }

    #[tokio::test]
    async fn test_no_retry_on_client_error() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            ..Default::default()
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: anyhow::Result<i32> = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("invalid argument: bad address"))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1); // no retry
    }

    #[tokio::test]
    async fn test_backoff_timing() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(5),
            jitter: 0.0,
            ..Default::default()
        };

        let d0 = policy.delay_for(0);
        let d1 = policy.delay_for(1);
        let d2 = policy.delay_for(2);

        assert_eq!(d0.as_millis(), 50);
        assert_eq!(d1.as_millis(), 100);
        assert_eq!(d2.as_millis(), 200);
    }

    #[test]
    fn test_delay_capped_at_max() {
        let policy = RetryPolicy {
            max_retries: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            jitter: 0.0,
            ..Default::default()
        };

        // After enough doublings, should be capped at max_delay
        let d = policy.delay_for(10);
        assert_eq!(d.as_secs(), 5);
    }

    #[test]
    fn test_is_retryable() {
        assert!(is_retryable(&anyhow::anyhow!("connection refused")));
        assert!(is_retryable(&anyhow::anyhow!("request timed out")));
        assert!(is_retryable(&anyhow::anyhow!("HTTP 429 too many requests")));
        assert!(is_retryable(&anyhow::anyhow!("502 bad gateway")));
        assert!(is_retryable(&anyhow::anyhow!("503 service unavailable")));
        assert!(!is_retryable(&anyhow::anyhow!("invalid argument")));
        assert!(!is_retryable(&anyhow::anyhow!("404 not found")));
        assert!(is_retryable(&anyhow::anyhow!(
            "HTTP error 503 with body: upstream unavailable"
        )));
        assert!(is_retryable(&anyhow::anyhow!("status code: 502, body: ")));
    }

    #[test]
    fn test_is_retryable_ignores_digits_inside_other_values() {
        // "500", "429", "503" inside a gas amount, an address or a hash are not HTTP statuses.
        assert!(!is_retryable(&anyhow::anyhow!(
            "gas required exceeds allowance (25000000)"
        )));
        assert!(!is_retryable(&anyhow::anyhow!(
            "unknown account 0x5030000000000000000000000000000000004290"
        )));
        assert!(!is_retryable(&anyhow::anyhow!(
            "ResultSubmitted event not found in receipt of 0x7a97c6efb019e23d135f790b6065dfc264f8930731d3500bf10c28ec5cf742be"
        )));
    }

    #[test]
    fn test_is_retryable_never_retries_reverts() {
        // Revert data is arbitrary hex and may look like a status code or "timeout".
        assert!(!is_retryable(&anyhow::anyhow!(
            "server returned an error response: error code 3: execution reverted: custom error 0x118cdaa7: 503 504"
        )));
        assert!(!is_retryable(&anyhow::anyhow!(
            "execution reverted: claim timeout not reached"
        )));
    }

    #[test]
    fn test_is_retryable_never_retries_mined_reverts() {
        // The block number of the reverted transaction is not an HTTP status.
        for block_number in [429, 500, 502, 503, 504] {
            let reverted = TransactionReverted {
                transaction_hash: alloy::primitives::B256::repeat_byte(0xab),
                block_number: Some(block_number),
                gas_used: 187_112_306,
            };
            let msg = reverted.to_string();
            assert!(msg.contains(&format!("block {block_number} ")), "{msg}");

            assert!(!is_retryable(&anyhow::Error::from(reverted.clone())));
            assert!(!is_retryable(
                &anyhow::Error::from(reverted).context("finalizing the result failed")
            ));
            // The same error after it was turned into a string on the way.
            assert!(!is_retryable(&anyhow::anyhow!("finalize failed: {msg}")));
        }
    }

    #[test]
    fn test_is_retryable_inspects_error_chain() {
        let err = anyhow::anyhow!("connection refused").context("submitting the result failed");
        assert_eq!(err.to_string(), "submitting the result failed");
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_default_policy() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 3);
        assert_eq!(p.base_delay, Duration::from_secs(1));
        assert_eq!(p.max_delay, Duration::from_secs(30));
        assert!((p.jitter - 0.2).abs() < f64::EPSILON);
        assert_eq!(p.timeout, Some(Duration::from_secs(30)));
        assert_eq!(p.total_timeout, Some(Duration::from_secs(300)));
    }

    #[test]
    fn test_builder_methods() {
        let p = RetryPolicy::default()
            .with_timeout(Duration::from_secs(5))
            .with_total_timeout(Duration::from_secs(60));
        assert_eq!(p.timeout, Some(Duration::from_secs(5)));
        assert_eq!(p.total_timeout, Some(Duration::from_secs(60)));
    }

    #[tokio::test]
    async fn test_per_attempt_timeout_triggers() {
        // Each attempt gets 50ms, but the closure sleeps for 200ms -- should time out.
        let policy = RetryPolicy {
            max_retries: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            timeout: Some(Duration::from_millis(50)),
            total_timeout: None,
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: anyhow::Result<i32> = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(42)
            }
        })
        .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("timed out"), "unexpected error: {err_msg}");
        // Should have attempted twice: initial + 1 retry (both timed out).
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_per_attempt_timeout_does_not_trigger_on_fast_op() {
        // Timeout is 500ms, but the closure returns immediately -- should succeed.
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            timeout: Some(Duration::from_millis(500)),
            total_timeout: None,
        };

        let result = retry_with_backoff(&policy, || async { Ok::<_, anyhow::Error>(7) }).await;
        assert_eq!(result.unwrap(), 7);
    }

    #[tokio::test]
    async fn test_total_timeout_aborts_across_retries() {
        // Total timeout is 80ms, each attempt takes ~1ms but delay is 50ms.
        // After attempt 0 fails, sleep 50ms, attempt 1 fails, sleep 50ms -> exceeds 80ms.
        let policy = RetryPolicy {
            max_retries: 10,
            base_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(50),
            jitter: 0.0,
            timeout: None,
            total_timeout: Some(Duration::from_millis(80)),
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result: anyhow::Result<i32> = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("connection refused"))
            }
        })
        .await;

        assert!(result.is_err());
        // Should have stopped well before 10 retries due to total timeout.
        let attempts = counter.load(Ordering::SeqCst);
        assert!(
            attempts <= 4,
            "expected at most 4 attempts with 80ms total timeout, got {attempts}"
        );
    }

    #[tokio::test]
    async fn test_total_timeout_succeeds_if_fast_enough() {
        // Total timeout is 500ms. Second attempt succeeds immediately.
        let policy = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
            timeout: None,
            total_timeout: Some(Duration::from_millis(500)),
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let result = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst);
                if n < 1 {
                    Err(anyhow::anyhow!("connection refused"))
                } else {
                    Ok(123)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 123);
    }

    #[tokio::test]
    async fn test_both_timeouts_combined() {
        // Per-attempt timeout: 30ms, total timeout: 150ms.
        // Closure sleeps 100ms (exceeds per-attempt), so each attempt times out.
        // Total timeout should cut off retries.
        let policy = RetryPolicy {
            max_retries: 20,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            jitter: 0.0,
            timeout: Some(Duration::from_millis(30)),
            total_timeout: Some(Duration::from_millis(150)),
        };
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let start = std::time::Instant::now();
        let result: anyhow::Result<i32> = retry_with_backoff(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(42)
            }
        })
        .await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        // Should complete within roughly the total timeout window.
        assert!(
            elapsed < Duration::from_millis(300),
            "took too long: {elapsed:?}"
        );
        let attempts = counter.load(Ordering::SeqCst);
        assert!(
            (2..=6).contains(&attempts),
            "unexpected attempt count: {attempts}"
        );
    }
}
