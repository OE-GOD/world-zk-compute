//! Retry logic with exponential backoff for resilient RPC communication.

use std::future::Future;
use std::time::Duration;

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
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: 0.2,
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
}

/// Simple pseudo-random fraction [0, 1) based on a seed. Not cryptographic.
fn pseudo_random_fraction(seed: u32) -> f64 {
    let x = seed.wrapping_mul(2654435761);
    (x as f64) / (u32::MAX as f64)
}

/// Returns true if the error message suggests a transient/retryable failure.
pub fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    // Connection / transport errors
    msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("broken pipe")
        || msg.contains("eof")
        // HTTP status codes typically retryable
        || msg.contains("429")
        || msg.contains("too many requests")
        || msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("rate limit")
}

/// Execute an async closure with retries according to the given policy.
///
/// Only retries if `should_retry` returns true for the error. Non-retryable errors
/// are returned immediately.
pub async fn retry_with_backoff<F, Fut, T>(
    policy: &RetryPolicy,
    mut f: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut last_err = None;

    for attempt in 0..=policy.max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt == policy.max_retries || !is_retryable(&e) {
                    return Err(e);
                }
                last_err = Some(e);
                let delay = policy.delay_for(attempt);
                tokio::time::sleep(delay).await;
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
    }

    #[test]
    fn test_default_policy() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 3);
        assert_eq!(p.base_delay, Duration::from_secs(1));
        assert_eq!(p.max_delay, Duration::from_secs(30));
        assert!((p.jitter - 0.2).abs() < f64::EPSILON);
    }
}
