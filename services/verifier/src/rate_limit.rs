use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::auth::extract_client_id;

/// Token bucket for a single client.
#[derive(Debug)]
struct TokenBucket {
    /// Current number of tokens available.
    tokens: f64,
    /// Maximum tokens (bucket capacity).
    max_tokens: f64,
    /// Tokens added per second.
    refill_rate: f64,
    /// Last time tokens were refilled.
    last_refill: Instant,
}

impl TokenBucket {
    fn new(rpm: u32) -> Self {
        let max_tokens = rpm as f64;
        let refill_rate = rpm as f64 / 60.0; // tokens per second
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume one token. Returns (allowed, remaining, reset_seconds).
    fn try_consume(&mut self) -> (bool, u32, u64) {
        self.refill();

        let remaining = self.tokens.floor() as u32;
        let reset_secs = if self.tokens < 1.0 {
            // How long until we have 1 token
            ((1.0 - self.tokens) / self.refill_rate).ceil() as u64
        } else {
            0
        };

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            let remaining_after = self.tokens.floor() as u32;
            (true, remaining_after, reset_secs)
        } else {
            (false, 0, reset_secs)
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }
}

/// Shared rate limiter state.
#[derive(Clone)]
pub struct RateLimitState {
    buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
    rpm: u32,
}

#[derive(Serialize)]
struct RateLimitError {
    error: String,
}

impl RateLimitState {
    /// Create a new rate limiter with the given requests-per-minute limit.
    pub fn new(rpm: u32) -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
            rpm,
        }
    }

    /// Try to consume a token for the given client ID.
    /// Returns `(allowed, remaining, reset_seconds)`.
    async fn check(&self, client_id: &str) -> (bool, u32, u64) {
        let mut buckets = self.buckets.lock().await;
        let bucket = buckets
            .entry(client_id.to_string())
            .or_insert_with(|| TokenBucket::new(self.rpm));
        bucket.try_consume()
    }
}

/// Axum middleware function for rate limiting.
///
/// Uses the `X-API-Key` header (via `extract_client_id`) to identify callers.
/// If no API key is present, falls back to IP-based identification.
///
/// - `/health` is never rate-limited.
/// - Returns 429 with `X-RateLimit-Remaining` and `X-RateLimit-Reset` headers
///   when the limit is exceeded.
/// - Adds rate limit headers to successful responses too.
pub async fn rate_limit_middleware(
    axum::extract::State(state): axum::extract::State<RateLimitState>,
    request: Request,
    next: Next,
) -> Response {
    // /health is never rate-limited
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    let client_id = extract_client_id(request.headers());
    let (allowed, remaining, reset_secs) = state.check(&client_id).await;

    if !allowed {
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(RateLimitError {
                error: "Rate limit exceeded".to_string(),
            }),
        )
            .into_response();

        let headers = response.headers_mut();
        headers.insert(
            "x-ratelimit-remaining",
            "0".parse().unwrap(),
        );
        headers.insert(
            "x-ratelimit-reset",
            reset_secs.to_string().parse().unwrap(),
        );
        return response;
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-ratelimit-remaining",
        remaining.to_string().parse().unwrap(),
    );
    headers.insert(
        "x-ratelimit-reset",
        reset_secs.to_string().parse().unwrap(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_token_bucket_basic() {
        let state = RateLimitState::new(5); // 5 per minute

        // First 5 requests should succeed
        for i in 0..5 {
            let (allowed, remaining, _) = state.check("test-client").await;
            assert!(allowed, "request {i} should be allowed");
            assert_eq!(remaining, (4 - i) as u32);
        }

        // 6th request should fail
        let (allowed, remaining, reset) = state.check("test-client").await;
        assert!(!allowed, "6th request should be denied");
        assert_eq!(remaining, 0);
        assert!(reset > 0, "reset should be positive");
    }

    #[tokio::test]
    async fn test_separate_clients() {
        let state = RateLimitState::new(2);

        // Client A uses up their tokens
        let (allowed, _, _) = state.check("client-a").await;
        assert!(allowed);
        let (allowed, _, _) = state.check("client-a").await;
        assert!(allowed);
        let (allowed, _, _) = state.check("client-a").await;
        assert!(!allowed);

        // Client B should still have tokens
        let (allowed, _, _) = state.check("client-b").await;
        assert!(allowed);
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(60); // 60/min = 1/sec

        // Consume all tokens
        for _ in 0..60 {
            let (allowed, _, _) = bucket.try_consume();
            assert!(allowed);
        }

        // Should be exhausted
        let (allowed, _, _) = bucket.try_consume();
        assert!(!allowed);

        // Simulate time passing by manually adjusting last_refill
        bucket.last_refill = Instant::now() - std::time::Duration::from_secs(2);
        // After 2 seconds at 1/sec, should have ~2 tokens
        let (allowed, _, _) = bucket.try_consume();
        assert!(allowed);
    }
}
