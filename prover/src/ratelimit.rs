//! Rate Limiting for RPC Calls
//!
//! Prevents hitting RPC rate limits by throttling requests:
//! - Token bucket algorithm for smooth rate limiting
//! - Per-endpoint rate limits
//! - Automatic backoff on 429 responses
//!
//! ## Usage
//!
//! ```rust,ignore
//! let limiter = RateLimiter::new(RateLimitConfig::default());
//!
//! // Wait for permit before making RPC call
//! limiter.acquire().await;
//! let result = make_rpc_call().await;
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, warn};

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum requests per second
    pub requests_per_second: f64,
    /// Burst size (max tokens in bucket)
    pub burst_size: u32,
    /// Maximum concurrent requests
    pub max_concurrent: u32,
    /// Backoff multiplier when rate limited
    pub backoff_multiplier: f64,
    /// Maximum backoff duration
    pub max_backoff: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 10.0,
            burst_size: 20,
            max_concurrent: 10,
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
        }
    }
}

impl RateLimitConfig {
    /// Create config for Alchemy RPC (free tier)
    pub fn alchemy_free() -> Self {
        Self {
            requests_per_second: 5.0,
            burst_size: 10,
            max_concurrent: 5,
            ..Default::default()
        }
    }

    /// Create config for Alchemy RPC (growth tier)
    pub fn alchemy_growth() -> Self {
        Self {
            requests_per_second: 25.0,
            burst_size: 50,
            max_concurrent: 20,
            ..Default::default()
        }
    }

    /// Create config for local node (unlimited)
    pub fn unlimited() -> Self {
        Self {
            requests_per_second: 1000.0,
            burst_size: 1000,
            max_concurrent: 100,
            ..Default::default()
        }
    }
}

/// Token bucket rate limiter
pub struct RateLimiter {
    config: RateLimitConfig,
    /// Current tokens in bucket
    tokens: Mutex<f64>,
    /// Last token refill time
    last_refill: Mutex<Instant>,
    /// Semaphore for concurrent request limiting
    concurrent: Arc<Semaphore>,
    /// Current backoff duration
    backoff: AtomicU64,
    /// Statistics
    stats: RateLimitStats,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(config: RateLimitConfig) -> Self {
        let concurrent = Arc::new(Semaphore::new(config.max_concurrent as usize));

        Self {
            tokens: Mutex::new(config.burst_size as f64),
            last_refill: Mutex::new(Instant::now()),
            concurrent,
            backoff: AtomicU64::new(0),
            stats: RateLimitStats::default(),
            config,
        }
    }

    /// Acquire a permit to make a request
    pub async fn acquire(&self) -> RateLimitPermit {
        // Wait for concurrent slot
        let permit = self.concurrent.clone().acquire_owned().await.unwrap();

        // Check for active backoff
        let backoff_ms = self.backoff.load(Ordering::Relaxed);
        if backoff_ms > 0 {
            let backoff = Duration::from_millis(backoff_ms);
            debug!("Rate limit backoff: {:?}", backoff);
            tokio::time::sleep(backoff).await;
            self.stats.backoffs.fetch_add(1, Ordering::Relaxed);
        }

        // Wait for token
        self.wait_for_token().await;

        self.stats.requests.fetch_add(1, Ordering::Relaxed);

        RateLimitPermit {
            _permit: permit,
            limiter: self,
        }
    }

    /// Try to acquire a permit without waiting
    pub fn try_acquire(&self) -> Option<RateLimitPermit> {
        // Try to get concurrent slot
        let permit = self.concurrent.clone().try_acquire_owned().ok()?;

        // Try to get token
        if !self.try_take_token() {
            return None;
        }

        self.stats.requests.fetch_add(1, Ordering::Relaxed);

        Some(RateLimitPermit {
            _permit: permit,
            limiter: self,
        })
    }

    /// Wait for a token to become available
    async fn wait_for_token(&self) {
        loop {
            self.refill_tokens().await;

            let mut tokens = self.tokens.lock().await;
            if *tokens >= 1.0 {
                *tokens -= 1.0;
                return;
            }
            drop(tokens);

            // Calculate wait time for next token
            let wait_time = Duration::from_secs_f64(1.0 / self.config.requests_per_second);
            tokio::time::sleep(wait_time).await;
        }
    }

    /// Try to take a token without waiting
    fn try_take_token(&self) -> bool {
        // Use try_lock to avoid blocking
        if let Ok(mut tokens) = self.tokens.try_lock() {
            if *tokens >= 1.0 {
                *tokens -= 1.0;
                return true;
            }
        }
        false
    }

    /// Refill tokens based on elapsed time
    async fn refill_tokens(&self) {
        let mut last_refill = self.last_refill.lock().await;
        let mut tokens = self.tokens.lock().await;

        let now = Instant::now();
        let elapsed = now.duration_since(*last_refill);
        let new_tokens = elapsed.as_secs_f64() * self.config.requests_per_second;

        *tokens = (*tokens + new_tokens).min(self.config.burst_size as f64);
        *last_refill = now;
    }

    /// Record a rate limit response (429)
    pub fn record_rate_limit(&self) {
        self.stats.rate_limited.fetch_add(1, Ordering::Relaxed);

        // Increase backoff
        let current = self.backoff.load(Ordering::Relaxed);
        let new_backoff = if current == 0 {
            1000 // Start with 1 second
        } else {
            ((current as f64 * self.config.backoff_multiplier) as u64)
                .min(self.config.max_backoff.as_millis() as u64)
        };

        self.backoff.store(new_backoff, Ordering::Relaxed);
        warn!("Rate limited, backing off for {}ms", new_backoff);
    }

    /// Record a successful response (reset backoff)
    pub fn record_success(&self) {
        self.stats.successes.fetch_add(1, Ordering::Relaxed);

        // Gradually reduce backoff
        let current = self.backoff.load(Ordering::Relaxed);
        if current > 0 {
            let new_backoff = current / 2;
            self.backoff.store(new_backoff, Ordering::Relaxed);
        }
    }

    /// Get current statistics
    pub fn stats(&self) -> RateLimitSnapshot {
        RateLimitSnapshot {
            requests: self.stats.requests.load(Ordering::Relaxed),
            successes: self.stats.successes.load(Ordering::Relaxed),
            rate_limited: self.stats.rate_limited.load(Ordering::Relaxed),
            backoffs: self.stats.backoffs.load(Ordering::Relaxed),
            current_backoff_ms: self.backoff.load(Ordering::Relaxed),
        }
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        self.stats.requests.store(0, Ordering::Relaxed);
        self.stats.successes.store(0, Ordering::Relaxed);
        self.stats.rate_limited.store(0, Ordering::Relaxed);
        self.stats.backoffs.store(0, Ordering::Relaxed);
    }
}

/// Permit that must be held while making a request
pub struct RateLimitPermit<'a> {
    _permit: tokio::sync::OwnedSemaphorePermit,
    limiter: &'a RateLimiter,
}

impl<'a> RateLimitPermit<'a> {
    /// Mark the request as successful
    pub fn success(self) {
        self.limiter.record_success();
    }

    /// Mark the request as rate limited
    pub fn rate_limited(self) {
        self.limiter.record_rate_limit();
    }
}

/// Internal statistics
#[derive(Default)]
struct RateLimitStats {
    requests: AtomicU64,
    successes: AtomicU64,
    rate_limited: AtomicU64,
    backoffs: AtomicU64,
}

/// Snapshot of rate limit statistics
#[derive(Debug, Clone)]
pub struct RateLimitSnapshot {
    pub requests: u64,
    pub successes: u64,
    pub rate_limited: u64,
    pub backoffs: u64,
    pub current_backoff_ms: u64,
}

/// Per-endpoint rate limiter
pub struct EndpointRateLimiter {
    /// Default limiter for unknown endpoints
    default: RateLimiter,
    /// Per-endpoint limiters
    endpoints: Mutex<HashMap<String, Arc<RateLimiter>>>,
    /// Config for new endpoints
    config: RateLimitConfig,
}

impl EndpointRateLimiter {
    /// Create a new endpoint rate limiter
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            default: RateLimiter::new(config.clone()),
            endpoints: Mutex::new(HashMap::new()),
            config,
        }
    }

    /// Get or create limiter for an endpoint
    pub async fn get(&self, endpoint: &str) -> Arc<RateLimiter> {
        let mut endpoints = self.endpoints.lock().await;

        if let Some(limiter) = endpoints.get(endpoint) {
            return limiter.clone();
        }

        let limiter = Arc::new(RateLimiter::new(self.config.clone()));
        endpoints.insert(endpoint.to_string(), limiter.clone());
        limiter
    }

    /// Get the default limiter
    pub fn default_limiter(&self) -> &RateLimiter {
        &self.default
    }
}

/// Sliding window rate limiter (alternative implementation)
pub struct SlidingWindowLimiter {
    /// Window size
    window: Duration,
    /// Maximum requests per window
    max_requests: u64,
    /// Request timestamps
    requests: Mutex<Vec<Instant>>,
}

impl SlidingWindowLimiter {
    /// Create a new sliding window limiter
    pub fn new(window: Duration, max_requests: u64) -> Self {
        Self {
            window,
            max_requests,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Check if a request is allowed
    pub async fn is_allowed(&self) -> bool {
        let mut requests = self.requests.lock().await;
        let now = Instant::now();

        // Remove old requests outside window
        requests.retain(|&t| now.duration_since(t) < self.window);

        if requests.len() < self.max_requests as usize {
            requests.push(now);
            true
        } else {
            false
        }
    }

    /// Wait until a request is allowed
    pub async fn wait(&self) {
        loop {
            if self.is_allowed().await {
                return;
            }

            // Wait a bit before checking again
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Get current request count in window
    pub async fn current_count(&self) -> usize {
        let mut requests = self.requests.lock().await;
        let now = Instant::now();
        requests.retain(|&t| now.duration_since(t) < self.window);
        requests.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let config = RateLimitConfig {
            requests_per_second: 100.0,
            burst_size: 10,
            max_concurrent: 5,
            ..Default::default()
        };
        let limiter = RateLimiter::new(config);

        // Should be able to acquire permits
        let permit = limiter.acquire().await;
        permit.success();

        let stats = limiter.stats();
        assert_eq!(stats.requests, 1);
        assert_eq!(stats.successes, 1);
    }

    #[tokio::test]
    async fn test_rate_limiter_concurrent() {
        let config = RateLimitConfig {
            requests_per_second: 100.0,
            burst_size: 10,
            max_concurrent: 2,
            ..Default::default()
        };
        let limiter = Arc::new(RateLimiter::new(config));

        // Acquire max concurrent permits
        let permit1 = limiter.acquire().await;
        let permit2 = limiter.acquire().await;

        // Try to acquire another (should fail immediately)
        assert!(limiter.try_acquire().is_none());

        // Release one
        drop(permit1);

        // Now should succeed
        let _permit3 = limiter.acquire().await;

        drop(permit2);
    }

    #[tokio::test]
    async fn test_rate_limit_backoff() {
        let config = RateLimitConfig {
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(10),
            ..Default::default()
        };
        let limiter = RateLimiter::new(config);

        // Record rate limits
        limiter.record_rate_limit();
        assert_eq!(limiter.backoff.load(Ordering::Relaxed), 1000);

        limiter.record_rate_limit();
        assert_eq!(limiter.backoff.load(Ordering::Relaxed), 2000);

        limiter.record_rate_limit();
        assert_eq!(limiter.backoff.load(Ordering::Relaxed), 4000);
    }

    #[tokio::test]
    async fn test_backoff_reduction() {
        let config = RateLimitConfig::default();
        let limiter = RateLimiter::new(config);

        // Set backoff
        limiter.backoff.store(4000, Ordering::Relaxed);

        // Record successes
        limiter.record_success();
        assert_eq!(limiter.backoff.load(Ordering::Relaxed), 2000);

        limiter.record_success();
        assert_eq!(limiter.backoff.load(Ordering::Relaxed), 1000);

        limiter.record_success();
        assert_eq!(limiter.backoff.load(Ordering::Relaxed), 500);
    }

    #[tokio::test]
    async fn test_sliding_window() {
        let limiter = SlidingWindowLimiter::new(Duration::from_millis(100), 3);

        // First 3 requests should pass
        assert!(limiter.is_allowed().await);
        assert!(limiter.is_allowed().await);
        assert!(limiter.is_allowed().await);

        // 4th should fail
        assert!(!limiter.is_allowed().await);

        // Wait for window to pass
        tokio::time::sleep(Duration::from_millis(110)).await;

        // Should be allowed again
        assert!(limiter.is_allowed().await);
    }

    #[tokio::test]
    async fn test_endpoint_limiter() {
        let config = RateLimitConfig::default();
        let limiter = EndpointRateLimiter::new(config);

        let l1 = limiter.get("eth_getBalance").await;
        let l2 = limiter.get("eth_getBalance").await;
        let l3 = limiter.get("eth_call").await;

        // Same endpoint should return same limiter
        assert!(Arc::ptr_eq(&l1, &l2));

        // Different endpoint should return different limiter
        assert!(!Arc::ptr_eq(&l1, &l3));
    }

    #[test]
    fn test_configs() {
        let free = RateLimitConfig::alchemy_free();
        assert_eq!(free.requests_per_second, 5.0);

        let growth = RateLimitConfig::alchemy_growth();
        assert_eq!(growth.requests_per_second, 25.0);

        let unlimited = RateLimitConfig::unlimited();
        assert_eq!(unlimited.requests_per_second, 1000.0);
    }

    #[tokio::test]
    async fn test_reset_stats() {
        let config = RateLimitConfig::default();
        let limiter = RateLimiter::new(config);

        let permit = limiter.acquire().await;
        permit.success();

        let stats = limiter.stats();
        assert_eq!(stats.requests, 1);

        limiter.reset_stats();

        let stats = limiter.stats();
        assert_eq!(stats.requests, 0);
    }
}
