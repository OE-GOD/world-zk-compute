//! Retry Policies with Exponential Backoff
//!
//! Provides configurable retry strategies for handling transient failures.

use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;

/// Retry policy configuration
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Add random jitter to prevent thundering herd
    pub jitter: bool,
    /// Jitter factor (0.0 - 1.0)
    pub jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_factor: 0.2,
        }
    }
}

impl RetryPolicy {
    /// Create a policy for RPC calls
    pub fn rpc() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_factor: 0.25,
        }
    }

    /// Create a policy for proof submission
    pub fn proof_submission() -> Self {
        Self {
            max_retries: 10,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 1.5,
            jitter: true,
            jitter_factor: 0.3,
        }
    }

    /// Create a policy for API calls
    pub fn api() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 2.0,
            jitter: true,
            jitter_factor: 0.2,
        }
    }

    /// Create a policy with no retries
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Create aggressive retry policy for critical operations
    pub fn aggressive() -> Self {
        Self {
            max_retries: 15,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(120),
            backoff_multiplier: 1.8,
            jitter: true,
            jitter_factor: 0.4,
        }
    }

    /// Calculate delay for a given attempt (0-indexed)
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let base_delay = self.initial_delay.as_secs_f64()
            * self.backoff_multiplier.powi(attempt as i32 - 1);
        let capped_delay = base_delay.min(self.max_delay.as_secs_f64());

        let final_delay = if self.jitter {
            let jitter_range = capped_delay * self.jitter_factor;
            let jitter = (rand::random::<f64>() - 0.5) * 2.0 * jitter_range;
            (capped_delay + jitter).max(0.0)
        } else {
            capped_delay
        };

        Duration::from_secs_f64(final_delay)
    }

    /// Get total maximum time for all retries
    pub fn max_total_delay(&self) -> Duration {
        let mut total = Duration::ZERO;
        for i in 1..=self.max_retries {
            total += Duration::from_secs_f64(
                (self.initial_delay.as_secs_f64()
                    * self.backoff_multiplier.powi(i as i32 - 1))
                .min(self.max_delay.as_secs_f64()),
            );
        }
        total
    }
}

/// Result of a retryable operation
#[derive(Debug)]
pub struct RetryResult<T, E> {
    /// The final result
    pub result: Result<T, E>,
    /// Number of attempts made
    pub attempts: u32,
    /// Total time spent retrying
    pub total_delay: Duration,
    /// Errors from each failed attempt
    pub errors: Vec<E>,
}

impl<T, E> RetryResult<T, E> {
    /// Check if the operation succeeded
    pub fn is_success(&self) -> bool {
        self.result.is_ok()
    }

    /// Unwrap the result
    pub fn into_result(self) -> Result<T, E> {
        self.result
    }
}

/// Trait for determining if an error is retryable
pub trait Retryable {
    /// Check if this error should trigger a retry
    fn is_retryable(&self) -> bool;
}

/// Blanket implementation for string errors
impl Retryable for String {
    fn is_retryable(&self) -> bool {
        let lower = self.to_lowercase();
        lower.contains("timeout")
            || lower.contains("connection")
            || lower.contains("temporary")
            || lower.contains("unavailable")
            || lower.contains("rate limit")
            || lower.contains("503")
            || lower.contains("502")
            || lower.contains("504")
    }
}

impl Retryable for &str {
    fn is_retryable(&self) -> bool {
        self.to_string().is_retryable()
    }
}

/// Execute an operation with retry policy
pub async fn retry<F, Fut, T, E>(policy: &RetryPolicy, mut operation: F) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Clone,
{
    let mut attempts = 0;
    let mut total_delay = Duration::ZERO;
    let mut errors = Vec::new();

    loop {
        attempts += 1;

        match operation().await {
            Ok(result) => {
                return RetryResult {
                    result: Ok(result),
                    attempts,
                    total_delay,
                    errors,
                };
            }
            Err(e) => {
                errors.push(e.clone());

                if attempts > policy.max_retries {
                    return RetryResult {
                        result: Err(e),
                        attempts,
                        total_delay,
                        errors,
                    };
                }

                let delay = policy.delay_for_attempt(attempts);
                total_delay += delay;

                tracing::debug!(
                    attempt = attempts,
                    max_retries = policy.max_retries,
                    delay_ms = delay.as_millis(),
                    "Retrying after failure"
                );

                sleep(delay).await;
            }
        }
    }
}

/// Execute with retry, only retrying on retryable errors
pub async fn retry_if<F, Fut, T, E>(
    policy: &RetryPolicy,
    mut operation: F,
) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Clone + Retryable,
{
    let mut attempts = 0;
    let mut total_delay = Duration::ZERO;
    let mut errors = Vec::new();

    loop {
        attempts += 1;

        match operation().await {
            Ok(result) => {
                return RetryResult {
                    result: Ok(result),
                    attempts,
                    total_delay,
                    errors,
                };
            }
            Err(e) => {
                let should_retry = e.is_retryable() && attempts <= policy.max_retries;
                errors.push(e.clone());

                if !should_retry {
                    return RetryResult {
                        result: Err(e),
                        attempts,
                        total_delay,
                        errors,
                    };
                }

                let delay = policy.delay_for_attempt(attempts);
                total_delay += delay;

                tracing::debug!(
                    attempt = attempts,
                    max_retries = policy.max_retries,
                    delay_ms = delay.as_millis(),
                    "Retrying retryable error"
                );

                sleep(delay).await;
            }
        }
    }
}

/// Execute with retry and custom retry condition
pub async fn retry_with_condition<F, Fut, T, E, C>(
    policy: &RetryPolicy,
    mut operation: F,
    should_retry: C,
) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Clone,
    C: Fn(&E, u32) -> bool,
{
    let mut attempts = 0;
    let mut total_delay = Duration::ZERO;
    let mut errors = Vec::new();

    loop {
        attempts += 1;

        match operation().await {
            Ok(result) => {
                return RetryResult {
                    result: Ok(result),
                    attempts,
                    total_delay,
                    errors,
                };
            }
            Err(e) => {
                let can_retry = attempts <= policy.max_retries && should_retry(&e, attempts);
                errors.push(e.clone());

                if !can_retry {
                    return RetryResult {
                        result: Err(errors.pop().unwrap()),
                        attempts,
                        total_delay,
                        errors,
                    };
                }

                let delay = policy.delay_for_attempt(attempts);
                total_delay += delay;
                sleep(delay).await;
            }
        }
    }
}

/// Builder for constructing retry policies
pub struct RetryPolicyBuilder {
    policy: RetryPolicy,
}

impl RetryPolicyBuilder {
    pub fn new() -> Self {
        Self {
            policy: RetryPolicy::default(),
        }
    }

    pub fn max_retries(mut self, n: u32) -> Self {
        self.policy.max_retries = n;
        self
    }

    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.policy.initial_delay = delay;
        self
    }

    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.policy.max_delay = delay;
        self
    }

    pub fn backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.policy.backoff_multiplier = multiplier;
        self
    }

    pub fn with_jitter(mut self, jitter_factor: f64) -> Self {
        self.policy.jitter = true;
        self.policy.jitter_factor = jitter_factor;
        self
    }

    pub fn without_jitter(mut self) -> Self {
        self.policy.jitter = false;
        self
    }

    pub fn build(self) -> RetryPolicy {
        self.policy
    }
}

impl Default for RetryPolicyBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_delay_calculation() {
        let policy = RetryPolicy {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            jitter: false,
            ..Default::default()
        };

        assert_eq!(policy.delay_for_attempt(0), Duration::ZERO);
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(200));
        assert_eq!(policy.delay_for_attempt(3), Duration::from_millis(400));
    }

    #[test]
    fn test_delay_capping() {
        let policy = RetryPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 10.0,
            jitter: false,
            ..Default::default()
        };

        // 1 * 10^4 = 10000 seconds, but capped at 5
        assert_eq!(policy.delay_for_attempt(5), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_retry_success_first_try() {
        let policy = RetryPolicy::default();
        let attempts = AtomicU32::new(0);

        let result = retry(&policy, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, &str>(42) }
        })
        .await;

        assert!(result.is_success());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let policy = RetryPolicy {
            max_retries: 5,
            initial_delay: Duration::from_millis(10),
            jitter: false,
            ..Default::default()
        };
        let attempts = AtomicU32::new(0);

        let result = retry(&policy, || {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err("fail")
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert!(result.is_success());
        assert_eq!(result.attempts, 3);
        assert_eq!(result.errors.len(), 2);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let policy = RetryPolicy {
            max_retries: 2,
            initial_delay: Duration::from_millis(10),
            jitter: false,
            ..Default::default()
        };

        let result = retry(&policy, || async { Err::<i32, _>("always fails") }).await;

        assert!(!result.is_success());
        assert_eq!(result.attempts, 3); // 1 initial + 2 retries
        assert_eq!(result.errors.len(), 3);
    }

    #[test]
    fn test_retryable_trait() {
        assert!("connection timeout".is_retryable());
        assert!("503 service unavailable".is_retryable());
        assert!("rate limit exceeded".is_retryable());
        assert!(!"invalid input".is_retryable());
        assert!(!"authentication failed".is_retryable());
    }

    #[tokio::test]
    async fn test_retry_if_retryable() {
        let policy = RetryPolicy {
            max_retries: 5,
            initial_delay: Duration::from_millis(10),
            jitter: false,
            ..Default::default()
        };
        let attempts = AtomicU32::new(0);

        let result = retry_if(&policy, || {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err("connection timeout".to_string()) // Retryable
                } else if n == 1 {
                    Err("invalid input".to_string()) // Not retryable
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        // Should stop at attempt 2 because "invalid input" is not retryable
        assert!(!result.is_success());
        assert_eq!(result.attempts, 2);
    }

    #[test]
    fn test_policy_builder() {
        let policy = RetryPolicyBuilder::new()
            .max_retries(10)
            .initial_delay(Duration::from_secs(1))
            .max_delay(Duration::from_secs(60))
            .backoff_multiplier(1.5)
            .with_jitter(0.3)
            .build();

        assert_eq!(policy.max_retries, 10);
        assert_eq!(policy.initial_delay, Duration::from_secs(1));
        assert_eq!(policy.max_delay, Duration::from_secs(60));
        assert_eq!(policy.backoff_multiplier, 1.5);
        assert!(policy.jitter);
        assert_eq!(policy.jitter_factor, 0.3);
    }
}
