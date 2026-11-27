use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::tenant::TenantStore;

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

/// How often buckets that have refilled completely are dropped.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// The token buckets of all clients seen recently.
struct Buckets {
    by_client: HashMap<String, TokenBucket>,
    /// Last time full buckets were dropped.
    last_prune: Instant,
}

impl Buckets {
    /// Drop every bucket that has refilled to its maximum. Such a bucket is
    /// indistinguishable from the new one its client would be given.
    fn prune(&mut self) {
        self.by_client.retain(|_, bucket| {
            bucket.refill();
            bucket.tokens < bucket.max_tokens
        });
        self.last_prune = Instant::now();
    }
}

/// Shared rate limiter state.
///
/// Supports two modes:
/// 1. **Default mode**: All clients share the same `default_rpm` limit.
/// 2. **Per-tenant mode**: When a `TenantStore` is attached, tenant API keys
///    get their individually configured `rate_limit_rpm`, and non-tenant
///    clients fall back to `default_rpm`.
#[derive(Clone)]
pub struct RateLimitState {
    buckets: Arc<Mutex<Buckets>>,
    prune_interval: Duration,
    default_rpm: u32,
    tenant_store: Option<Arc<TenantStore>>,
}

#[derive(Serialize)]
struct RateLimitError {
    error: String,
}

impl RateLimitState {
    /// Create a new rate limiter with the given default requests-per-minute limit.
    pub fn new(rpm: u32) -> Self {
        Self {
            buckets: Arc::new(Mutex::new(Buckets {
                by_client: HashMap::new(),
                last_prune: Instant::now(),
            })),
            prune_interval: PRUNE_INTERVAL,
            default_rpm: rpm,
            tenant_store: None,
        }
    }

    /// Attach a tenant store for per-tenant rate limits.
    pub fn with_tenant_store(mut self, store: Arc<TenantStore>) -> Self {
        self.tenant_store = Some(store);
        self
    }

    /// Resolve the RPM limit for a client ID.
    ///
    /// If the client ID is `key:<api_key>` and a tenant store is available,
    /// looks up the tenant's configured rate limit. Otherwise returns `default_rpm`.
    fn resolve_rpm(&self, client_id: &str) -> u32 {
        if let Some(api_key) = client_id.strip_prefix("key:") {
            if let Some(store) = &self.tenant_store {
                if let Some(tenant) = store.get_by_key(api_key) {
                    return tenant.rate_limit_rpm;
                }
            }
        }
        self.default_rpm
    }

    /// Try to consume a token for the given client ID.
    /// Returns `(allowed, remaining, reset_seconds)`.
    ///
    /// If the client has a per-tenant rate limit that differs from the bucket's
    /// current configuration, the bucket is recreated with the new limit.
    /// Buckets that have refilled completely are dropped once per prune interval.
    async fn check(&self, client_id: &str) -> (bool, u32, u64) {
        let rpm = self.resolve_rpm(client_id);
        let mut buckets = self.buckets.lock().await;

        if buckets.last_prune.elapsed() >= self.prune_interval {
            buckets.prune();
        }

        let bucket = buckets
            .by_client
            .entry(client_id.to_string())
            .or_insert_with(|| TokenBucket::new(rpm));

        // If the tenant's rate limit changed, recreate the bucket.
        if (bucket.max_tokens - rpm as f64).abs() > 0.5 {
            *bucket = TokenBucket::new(rpm);
        }

        bucket.try_consume()
    }
}

/// Axum middleware function for rate limiting.
///
/// Uses the `X-API-Key` header (via `extract_client_id`) to identify callers.
/// If no API key is present, falls back to IP-based identification.
///
/// - Returns 429 with `X-RateLimit-Remaining` and `X-RateLimit-Reset` headers
///   when the limit is exceeded.
/// - Adds rate limit headers to successful responses too.
pub async fn rate_limit_middleware(
    axum::extract::State(state): axum::extract::State<RateLimitState>,
    request: Request,
    next: Next,
) -> Response {
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
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", reset_secs.to_string().parse().unwrap());
        return response;
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-ratelimit-remaining",
        remaining.to_string().parse().unwrap(),
    );
    headers.insert("x-ratelimit-reset", reset_secs.to_string().parse().unwrap());
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

    #[tokio::test]
    async fn test_refilled_buckets_are_dropped() {
        let mut state = RateLimitState::new(60); // 60/min = 1/sec
        state.prune_interval = Duration::ZERO; // prune on every check

        state.check("idle").await;
        state.check("busy").await;
        assert_eq!(state.buckets.lock().await.by_client.len(), 2);

        // "idle" used its one token 2 seconds ago, so it has it back.
        state
            .buckets
            .lock()
            .await
            .by_client
            .get_mut("idle")
            .unwrap()
            .last_refill = Instant::now() - Duration::from_secs(2);

        state.check("new").await;
        {
            let buckets = state.buckets.lock().await;
            let mut clients: Vec<&str> = buckets.by_client.keys().map(String::as_str).collect();
            clients.sort_unstable();
            assert_eq!(clients, ["busy", "new"]);
        }

        // A client whose bucket was dropped is treated as before: a full bucket.
        let (allowed, remaining, _) = state.check("idle").await;
        assert!(allowed);
        assert_eq!(remaining, 59);
    }

    #[tokio::test]
    async fn test_buckets_in_use_survive_pruning() {
        let mut state = RateLimitState::new(2);
        state.prune_interval = Duration::ZERO;

        // Pruning on every check must not hand an exhausted client new tokens.
        let (allowed, _, _) = state.check("client").await;
        assert!(allowed);
        let (allowed, _, _) = state.check("client").await;
        assert!(allowed);
        let (allowed, _, _) = state.check("client").await;
        assert!(!allowed);
    }

    #[tokio::test]
    async fn test_per_tenant_rate_limit() {
        let store = Arc::new(TenantStore::new());

        // Create tenant with 2 RPM limit
        let tenant = store.create_with_id("slow", "Slow Tenant", 2).unwrap();
        let state = RateLimitState::new(100).with_tenant_store(store);

        let client_id = format!("key:{}", tenant.api_key);

        // Tenant should get 2 requests
        let (allowed, _, _) = state.check(&client_id).await;
        assert!(allowed, "first request should be allowed");
        let (allowed, _, _) = state.check(&client_id).await;
        assert!(allowed, "second request should be allowed");

        // Third should be denied (tenant limit is 2)
        let (allowed, _, _) = state.check(&client_id).await;
        assert!(
            !allowed,
            "third request should be denied (per-tenant limit)"
        );

        // Non-tenant client should still get 100 requests
        let (allowed, _, _) = state.check("ip:1.2.3.4").await;
        assert!(allowed, "non-tenant should use default limit");
    }

    #[tokio::test]
    async fn test_unknown_key_uses_default_limit() {
        let store = Arc::new(TenantStore::new());
        let state = RateLimitState::new(3).with_tenant_store(store);

        // Unknown key should get default limit (3)
        let client_id = "key:unknown-key";
        for i in 0..3 {
            let (allowed, _, _) = state.check(client_id).await;
            assert!(allowed, "request {i} should use default limit");
        }
        let (allowed, _, _) = state.check(client_id).await;
        assert!(!allowed, "4th request should be denied at default limit");
    }

    #[tokio::test]
    async fn test_resolve_rpm_without_store() {
        let state = RateLimitState::new(50);
        assert_eq!(state.resolve_rpm("key:anything"), 50);
        assert_eq!(state.resolve_rpm("ip:1.2.3.4"), 50);
    }

    #[tokio::test]
    async fn test_resolve_rpm_with_store() {
        let store = Arc::new(TenantStore::new());
        let tenant = store.create_with_id("fast", "Fast Tenant", 500).unwrap();
        let state = RateLimitState::new(100).with_tenant_store(store);

        // Tenant key should resolve to tenant's RPM
        assert_eq!(state.resolve_rpm(&format!("key:{}", tenant.api_key)), 500);

        // Unknown key should resolve to default
        assert_eq!(state.resolve_rpm("key:unknown"), 100);

        // IP-based should resolve to default
        assert_eq!(state.resolve_rpm("ip:1.2.3.4"), 100);
    }
}
