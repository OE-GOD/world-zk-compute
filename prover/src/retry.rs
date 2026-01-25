//! Retry Logic with Exponential Backoff
//!
//! Provides robust retry mechanisms for:
//! - RPC calls that may fail transiently
//! - Network requests with timeouts
//! - Operations that need jitter to avoid thundering herd
//!
//! ## Usage
//!
//! ```rust,ignore
//! use retry::{retry, RetryConfig};
//!
//! let result = retry(RetryConfig::default(), || async {
//!     make_rpc_call().await
//! }).await?;
//! ```

use rand::Rng;
use std::future::Future;
use std::time::Duration;
use tracing::{debug, warn};

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries)
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff (e.g., 2.0 = double each time)
    pub backoff_multiplier: f64,
    /// Add random jitter to delays (0.0 - 1.0)
    pub jitter: f64,
    /// Retry on these error types
    pub retry_on: RetryOn,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: 0.1,
            retry_on: RetryOn::All,
        }
    }
}

impl RetryConfig {
    /// Create a config for aggressive retries (many quick retries)
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 1.5,
            jitter: 0.2,
            retry_on: RetryOn::All,
        }
    }

    /// Create a config for patient retries (fewer, longer delays)
    pub fn patient() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 3.0,
            jitter: 0.3,
            retry_on: RetryOn::All,
        }
    }

    /// Create a config for RPC calls
    pub fn rpc() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            jitter: 0.25,
            retry_on: RetryOn::Transient,
        }
    }

    /// Create a config for network operations
    pub fn network() -> Self {
        Self {
            max_retries: 4,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: 0.15,
            retry_on: RetryOn::Transient,
        }
    }

    /// Set maximum retries
    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// Set initial delay
    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set maximum delay
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Calculate delay for a given attempt
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let base_delay = self.initial_delay.as_secs_f64()
            * self.backoff_multiplier.powi(attempt as i32 - 1);

        let max_delay = self.max_delay.as_secs_f64();
        let delay = base_delay.min(max_delay);

        // Add jitter (skip if jitter is zero to avoid empty range)
        let final_delay = if self.jitter > 0.0 {
            let jitter_range = delay * self.jitter;
            let jitter = rand::thread_rng().gen_range(-jitter_range..jitter_range);
            (delay + jitter).max(0.0)
        } else {
            delay
        };

        Duration::from_secs_f64(final_delay)
    }
}

/// When to retry
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetryOn {
    /// Retry on all errors
    All,
    /// Only retry on transient errors (network, timeout, rate limit)
    Transient,
    /// Never retry (useful for disabling retries)
    Never,
}

/// Error classification for retry decisions
pub trait Retryable {
    /// Check if this error is retryable
    fn is_retryable(&self) -> bool;

    /// Check if this is a transient error
    fn is_transient(&self) -> bool;
}

/// Implement Retryable for anyhow::Error
impl Retryable for anyhow::Error {
    fn is_retryable(&self) -> bool {
        // Check for known retryable error patterns
        let msg = self.to_string().to_lowercase();

        // Network errors
        if msg.contains("connection")
            || msg.contains("timeout")
            || msg.contains("reset")
            || msg.contains("refused")
        {
            return true;
        }

        // Rate limiting
        if msg.contains("rate limit") || msg.contains("too many requests") || msg.contains("429") {
            return true;
        }

        // Temporary server errors
        if msg.contains("503") || msg.contains("502") || msg.contains("500") {
            return true;
        }

        false
    }

    fn is_transient(&self) -> bool {
        self.is_retryable()
    }
}

/// Retry result
#[derive(Debug)]
pub struct RetryResult<T, E> {
    /// The final result
    pub result: Result<T, E>,
    /// Number of attempts made
    pub attempts: u32,
    /// Total time spent retrying
    pub total_time: Duration,
}

impl<T, E> RetryResult<T, E> {
    /// Unwrap the result, returning the value or panicking
    pub fn unwrap(self) -> T
    where
        E: std::fmt::Debug,
    {
        self.result.unwrap()
    }

    /// Get the result
    pub fn into_result(self) -> Result<T, E> {
        self.result
    }
}

/// Execute an async operation with retries
pub async fn retry<T, E, F, Fut>(config: RetryConfig, mut operation: F) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Retryable + std::fmt::Display,
{
    let start = std::time::Instant::now();
    let mut attempts = 0;

    loop {
        attempts += 1;

        match operation().await {
            Ok(value) => {
                return RetryResult {
                    result: Ok(value),
                    attempts,
                    total_time: start.elapsed(),
                };
            }
            Err(error) => {
                let should_retry = match config.retry_on {
                    RetryOn::All => true,
                    RetryOn::Transient => error.is_transient(),
                    RetryOn::Never => false,
                };

                if !should_retry || attempts > config.max_retries {
                    return RetryResult {
                        result: Err(error),
                        attempts,
                        total_time: start.elapsed(),
                    };
                }

                let delay = config.delay_for_attempt(attempts);
                warn!(
                    "Attempt {}/{} failed: {}. Retrying in {:?}",
                    attempts,
                    config.max_retries + 1,
                    error,
                    delay
                );

                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Execute an async operation with retries (convenience function)
pub async fn retry_with<T, E, F, Fut>(
    max_retries: u32,
    operation: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Retryable + std::fmt::Display,
{
    let config = RetryConfig::default().with_max_retries(max_retries);
    retry(config, operation).await.into_result()
}

/// Retry with custom delay function
pub async fn retry_with_delay<T, E, F, Fut, D>(
    max_retries: u32,
    mut delay_fn: D,
    mut operation: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    D: FnMut(u32) -> Duration,
    E: std::fmt::Display,
{
    let mut attempts = 0;

    loop {
        attempts += 1;

        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempts > max_retries {
                    return Err(error);
                }

                let delay = delay_fn(attempts);
                debug!(
                    "Attempt {}/{} failed: {}. Retrying in {:?}",
                    attempts,
                    max_retries + 1,
                    error,
                    delay
                );

                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Constant delay function
pub fn constant_delay(delay: Duration) -> impl FnMut(u32) -> Duration {
    move |_| delay
}

/// Linear delay function
pub fn linear_delay(initial: Duration, increment: Duration) -> impl FnMut(u32) -> Duration {
    move |attempt| initial + increment * (attempt - 1)
}

/// Exponential delay function with cap
pub fn exponential_delay(
    initial: Duration,
    multiplier: f64,
    max: Duration,
) -> impl FnMut(u32) -> Duration {
    move |attempt| {
        let delay = initial.as_secs_f64() * multiplier.powi(attempt as i32 - 1);
        Duration::from_secs_f64(delay.min(max.as_secs_f64()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Test error type
    #[derive(Debug)]
    struct TestError {
        message: String,
        transient: bool,
    }

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl Retryable for TestError {
        fn is_retryable(&self) -> bool {
            self.transient
        }

        fn is_transient(&self) -> bool {
            self.transient
        }
    }

    #[test]
    fn test_delay_calculation() {
        let config = RetryConfig {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            jitter: 0.0, // No jitter for predictable test
            ..Default::default()
        };

        assert_eq!(config.delay_for_attempt(0), Duration::ZERO);
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(200));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(400));
    }

    #[test]
    fn test_delay_max_cap() {
        let config = RetryConfig {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 10.0,
            jitter: 0.0,
            ..Default::default()
        };

        // Should be capped at max_delay
        assert_eq!(config.delay_for_attempt(5), Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        let config = RetryConfig::default();
        let attempts = AtomicU32::new(0);

        let result = retry(config, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, TestError>(42) }
        })
        .await;

        assert_eq!(result.result.unwrap(), 42);
        assert_eq!(result.attempts, 1);
    }

    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let config = RetryConfig {
            initial_delay: Duration::from_millis(1),
            ..Default::default()
        };
        let attempts = AtomicU32::new(0);

        let result = retry(config, || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if attempt < 3 {
                    Err(TestError {
                        message: "transient error".to_string(),
                        transient: true,
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.result.unwrap(), 42);
        assert_eq!(result.attempts, 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            ..Default::default()
        };
        let attempts = AtomicU32::new(0);

        let result = retry(config, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<i32, _>(TestError {
                    message: "persistent error".to_string(),
                    transient: true,
                })
            }
        })
        .await;

        assert!(result.result.is_err());
        assert_eq!(result.attempts, 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_no_retry_on_non_transient() {
        let config = RetryConfig {
            max_retries: 5,
            retry_on: RetryOn::Transient,
            initial_delay: Duration::from_millis(1),
            ..Default::default()
        };
        let attempts = AtomicU32::new(0);

        let result = retry(config, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<i32, _>(TestError {
                    message: "non-transient error".to_string(),
                    transient: false,
                })
            }
        })
        .await;

        assert!(result.result.is_err());
        assert_eq!(result.attempts, 1); // No retries for non-transient
    }

    #[tokio::test]
    async fn test_retry_never() {
        let config = RetryConfig {
            max_retries: 5,
            retry_on: RetryOn::Never,
            ..Default::default()
        };
        let attempts = AtomicU32::new(0);

        let result = retry(config, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<i32, _>(TestError {
                    message: "error".to_string(),
                    transient: true,
                })
            }
        })
        .await;

        assert!(result.result.is_err());
        assert_eq!(result.attempts, 1);
    }

    #[test]
    fn test_constant_delay() {
        let mut delay = constant_delay(Duration::from_secs(1));
        assert_eq!(delay(1), Duration::from_secs(1));
        assert_eq!(delay(5), Duration::from_secs(1));
    }

    #[test]
    fn test_linear_delay() {
        let mut delay = linear_delay(Duration::from_secs(1), Duration::from_secs(2));
        assert_eq!(delay(1), Duration::from_secs(1));
        assert_eq!(delay(2), Duration::from_secs(3));
        assert_eq!(delay(3), Duration::from_secs(5));
    }

    #[test]
    fn test_exponential_delay_fn() {
        let mut delay = exponential_delay(
            Duration::from_secs(1),
            2.0,
            Duration::from_secs(10),
        );
        assert_eq!(delay(1), Duration::from_secs(1));
        assert_eq!(delay(2), Duration::from_secs(2));
        assert_eq!(delay(3), Duration::from_secs(4));
        assert_eq!(delay(5), Duration::from_secs(10)); // Capped
    }

    #[test]
    fn test_anyhow_retryable() {
        let timeout_err = anyhow::anyhow!("connection timeout");
        assert!(timeout_err.is_retryable());

        let rate_limit_err = anyhow::anyhow!("rate limit exceeded");
        assert!(rate_limit_err.is_retryable());

        let logic_err = anyhow::anyhow!("invalid argument");
        assert!(!logic_err.is_retryable());
    }
}
