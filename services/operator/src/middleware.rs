//! Rate limiting middleware for the operator's HTTP endpoints.
//!
//! Uses a per-IP token bucket algorithm with bounded storage (LRU eviction
//! when the number of tracked IPs exceeds `max_ips`).

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, Response, StatusCode},
    middleware::Next,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{atomic::AtomicU64, Arc, Mutex};
use std::time::Instant;

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum sustained requests per minute per IP.
    pub max_requests_per_minute: u64,
    /// Maximum burst size (token bucket capacity).
    pub max_burst: u64,
    /// Maximum number of tracked IPs (LRU eviction beyond this).
    pub max_ips: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests_per_minute: 60,
            max_burst: 10,
            max_ips: 10_000,
        }
    }
}

/// Per-IP token bucket state.
#[derive(Debug, Clone)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(max_burst: u64) -> Self {
        Self {
            tokens: max_burst as f64,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time and try to consume one.
    /// Returns `true` if the request is allowed.
    fn try_consume(&mut self, rate_per_sec: f64, max_burst: u64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;

        // Refill tokens (capped at max_burst)
        self.tokens = (self.tokens + elapsed * rate_per_sec).min(max_burst as f64);

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Compute Retry-After seconds based on current token deficit.
    fn retry_after_secs(&self, rate_per_sec: f64) -> u64 {
        if rate_per_sec <= 0.0 {
            return 60;
        }
        let deficit = 1.0 - self.tokens;
        (deficit / rate_per_sec).ceil().max(1.0) as u64
    }
}

/// Shared rate limiter state.
#[derive(Clone)]
pub struct RateLimiter {
    config: RateLimitConfig,
    buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
    /// Counter for rate-limited requests (for metrics).
    pub rate_limited_total: Arc<AtomicU64>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(Mutex::new(HashMap::new())),
            rate_limited_total: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Check if a request from the given IP is allowed.
    /// Returns `Ok(())` if allowed, `Err(retry_after_secs)` if rate-limited.
    pub fn check(&self, ip: &str) -> Result<(), u64> {
        let rate_per_sec = self.config.max_requests_per_minute as f64 / 60.0;
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        // LRU eviction: remove oldest entries if over capacity
        if buckets.len() >= self.config.max_ips && !buckets.contains_key(ip) {
            // Find the entry with the oldest last_refill
            if let Some(oldest_key) = buckets
                .iter()
                .min_by_key(|(_, b)| b.last_refill)
                .map(|(k, _)| k.clone())
            {
                buckets.remove(&oldest_key);
            }
        }

        let bucket = buckets
            .entry(ip.to_string())
            .or_insert_with(|| TokenBucket::new(self.config.max_burst));

        if bucket.try_consume(rate_per_sec, self.config.max_burst) {
            Ok(())
        } else {
            let retry_after = bucket.retry_after_secs(rate_per_sec);
            self.rate_limited_total
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(retry_after)
        }
    }

    /// Returns the total number of rate-limited requests.
    #[allow(dead_code)] // used in tests; kept as public metrics API
    pub fn total_rate_limited(&self) -> u64 {
        self.rate_limited_total
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns the number of currently tracked IPs.
    #[allow(dead_code)] // used in tests; kept as public metrics API
    pub fn tracked_ips(&self) -> usize {
        self.buckets.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// Axum middleware layer that applies rate limiting based on client IP.
///
/// Usage in Axum router:
/// ```ignore
/// let limiter = RateLimiter::new(RateLimitConfig::default());
/// let app = Router::new()
///     .route("/api", get(handler))
///     .layer(axum::middleware::from_fn_with_state(
///         limiter,
///         rate_limit_middleware,
///     ));
/// ```
pub async fn rate_limit_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let ip = addr.ip().to_string();

    match limiter.check(&ip) {
        Ok(()) => next.run(request).await,
        Err(retry_after) => {
            tracing::debug!(
                ip = %ip,
                retry_after = retry_after,
                "Rate limited request"
            );

            Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("Retry-After", retry_after.to_string())
                .header("Content-Type", "text/plain")
                .body(Body::from(format!(
                    "Rate limit exceeded. Retry after {} seconds.",
                    retry_after
                )))
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(StatusCode::TOO_MANY_REQUESTS)
                        .body(Body::empty())
                        .expect("building a simple response with no headers should never fail")
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_burst() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 5,
            max_ips: 100,
        });

        // First 5 requests should be allowed (burst)
        for i in 0..5 {
            assert!(
                limiter.check("192.168.1.1").is_ok(),
                "Request {} should be allowed",
                i
            );
        }

        // 6th request should be rate-limited
        assert!(
            limiter.check("192.168.1.1").is_err(),
            "6th request should be rate-limited"
        );
    }

    #[test]
    fn test_rate_limiter_per_ip_isolation() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 2,
            max_ips: 100,
        });

        // IP 1: exhaust burst
        assert!(limiter.check("10.0.0.1").is_ok());
        assert!(limiter.check("10.0.0.1").is_ok());
        assert!(limiter.check("10.0.0.1").is_err());

        // IP 2: should still have full burst
        assert!(limiter.check("10.0.0.2").is_ok());
        assert!(limiter.check("10.0.0.2").is_ok());
    }

    #[test]
    fn test_rate_limiter_retry_after() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 1,
            max_ips: 100,
        });

        // Exhaust burst
        assert!(limiter.check("1.2.3.4").is_ok());

        // Should get Retry-After value
        match limiter.check("1.2.3.4") {
            Err(retry_after) => {
                assert!(retry_after >= 1, "Retry-After should be at least 1 second");
                assert!(retry_after <= 2, "Retry-After should be at most 2 seconds");
            }
            Ok(()) => panic!("Should have been rate-limited"),
        }
    }

    #[test]
    fn test_rate_limiter_lru_eviction() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 5,
            max_ips: 3,
        });

        // Fill up to max_ips
        limiter.check("ip-1").ok();
        limiter.check("ip-2").ok();
        limiter.check("ip-3").ok();
        assert_eq!(limiter.tracked_ips(), 3);

        // Adding a 4th IP should evict the oldest
        limiter.check("ip-4").ok();
        assert_eq!(limiter.tracked_ips(), 3);
    }

    #[test]
    fn test_rate_limited_counter() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 1,
            max_ips: 100,
        });

        assert_eq!(limiter.total_rate_limited(), 0);

        // First request: allowed
        limiter.check("test").ok();

        // Second request: rate-limited
        limiter.check("test").ok();
        assert_eq!(limiter.total_rate_limited(), 1);

        // Third request: also rate-limited
        limiter.check("test").ok();
        assert_eq!(limiter.total_rate_limited(), 2);
    }

    #[test]
    fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.max_requests_per_minute, 60);
        assert_eq!(config.max_burst, 10);
        assert_eq!(config.max_ips, 10_000);
    }

    #[test]
    fn test_zero_burst_always_limited() {
        let limiter = RateLimiter::new(RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 0,
            max_ips: 100,
        });

        // With 0 burst, all requests should be limited
        assert!(limiter.check("any-ip").is_err());
    }

    #[test]
    fn test_token_refill_over_time() {
        let mut bucket = TokenBucket::new(2);

        let rate = 60.0 / 60.0; // 1 per second

        // Consume both tokens
        assert!(bucket.try_consume(rate, 2));
        assert!(bucket.try_consume(rate, 2));
        assert!(!bucket.try_consume(rate, 2));

        // Simulate time passing by adjusting last_refill
        bucket.last_refill = Instant::now() - std::time::Duration::from_secs(2);

        // Should have refilled ~2 tokens
        assert!(bucket.try_consume(rate, 2));
    }
}
