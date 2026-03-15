//! Simple in-memory per-IP rate limiting middleware for axum.
//!
//! Uses a sliding window approach: each IP address is tracked with a list of
//! request timestamps. Requests older than the window are pruned on each check.
//! When the limit is exceeded, a 429 Too Many Requests response is returned
//! with a `Retry-After` header.
//!
//! Every response includes standard rate-limit headers:
//! - `X-RateLimit-Limit` — maximum requests per window
//! - `X-RateLimit-Remaining` — requests left in the current window
//! - `X-RateLimit-Reset` — seconds until the window resets

use axum::body::Body;
use axum::http::{HeaderValue, Request, Response, StatusCode};
use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tower::{Layer, Service};

/// Rate limit status returned by [`RateLimitState::check`].
#[derive(Debug, Clone, Copy)]
pub struct RateLimitInfo {
    /// Maximum requests allowed per window.
    pub limit: u32,
    /// Requests remaining in the current window.
    pub remaining: u32,
    /// Seconds until the window resets.
    pub reset_secs: u64,
}

/// Configuration for the rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of requests per window per IP address.
    pub max_requests: u32,
    /// Sliding window duration.
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window: Duration::from_secs(60),
        }
    }
}

/// Shared state for the rate limiter, tracking per-IP request timestamps.
#[derive(Clone)]
pub struct RateLimitState {
    config: RateLimitConfig,
    /// Map from IP address to list of request timestamps within the window.
    requests: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
}

impl RateLimitState {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check whether the given IP is allowed to make a request.
    /// Returns `Ok(RateLimitInfo)` if allowed, or `Err(RateLimitInfo)` if rate limited.
    pub fn check(&self, ip: IpAddr) -> Result<RateLimitInfo, RateLimitInfo> {
        let now = Instant::now();
        let cutoff = now - self.config.window;

        let mut map = self.requests.lock().unwrap_or_else(|e| e.into_inner());

        let timestamps = map.entry(ip).or_default();

        // Prune expired entries
        timestamps.retain(|t| *t > cutoff);

        let limit = self.config.max_requests;

        if timestamps.len() >= limit as usize {
            // Calculate retry-after: time until the oldest entry in the window expires
            let oldest = timestamps[0];
            let reset_secs = (oldest + self.config.window)
                .duration_since(now)
                .as_secs()
                + 1;
            Err(RateLimitInfo {
                limit,
                remaining: 0,
                reset_secs,
            })
        } else {
            let reset_secs = if timestamps.is_empty() {
                self.config.window.as_secs()
            } else {
                let oldest = timestamps[0];
                (oldest + self.config.window)
                    .duration_since(now)
                    .as_secs()
                    + 1
            };
            timestamps.push(now);
            // remaining is calculated AFTER pushing (this request counts)
            let remaining = limit.saturating_sub(timestamps.len() as u32);
            Ok(RateLimitInfo {
                limit,
                remaining,
                reset_secs,
            })
        }
    }

    /// Remove entries for IPs that have no recent requests.
    /// Call this periodically to prevent unbounded memory growth.
    pub fn cleanup(&self) {
        let now = Instant::now();
        let cutoff = now - self.config.window;

        let mut map = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_ip, timestamps| {
            timestamps.retain(|t| *t > cutoff);
            !timestamps.is_empty()
        });
    }
}

// ---------------------------------------------------------------------------
// Tower Layer + Service
// ---------------------------------------------------------------------------

/// Tower [`Layer`] that applies per-IP rate limiting.
#[derive(Clone)]
pub struct RateLimitLayer {
    state: RateLimitState,
}

impl RateLimitLayer {
    pub fn new(config: RateLimitConfig) -> Self {
        let state = RateLimitState::new(config);

        // Spawn a background cleanup task every 60 seconds to prevent memory leaks
        let cleanup_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                cleanup_state.cleanup();
            }
        });

        Self { state }
    }

    /// Create a layer without spawning the background cleanup task.
    /// Useful for tests where a tokio runtime may not be available at
    /// construction time.
    pub fn new_without_cleanup(config: RateLimitConfig) -> Self {
        Self {
            state: RateLimitState::new(config),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            state: self.state.clone(),
        }
    }
}

/// Tower [`Service`] that enforces per-IP rate limits.
#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    state: RateLimitState,
}

/// Extract client IP from the request.
///
/// Checks (in order):
/// 1. `X-Forwarded-For` header (first IP)
/// 2. `X-Real-IP` header
/// 3. Connected peer address from axum's `ConnectInfo`
/// 4. Fallback to 127.0.0.1
fn extract_client_ip<B>(req: &Request<B>) -> IpAddr {
    // Try X-Forwarded-For first
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            if let Some(first) = value.split(',').next() {
                if let Ok(ip) = first.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
    }

    // Try X-Real-IP
    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            if let Ok(ip) = value.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }

    // Try ConnectInfo (axum extension)
    if let Some(connect_info) = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return connect_info.0.ip();
    }

    // Fallback
    IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

/// Append standard rate-limit headers to a response.
fn set_rate_limit_headers(resp: &mut Response<Body>, info: &RateLimitInfo) {
    let headers = resp.headers_mut();
    headers.insert(
        "x-ratelimit-limit",
        HeaderValue::from(info.limit as u64),
    );
    headers.insert(
        "x-ratelimit-remaining",
        HeaderValue::from(info.remaining as u64),
    );
    headers.insert(
        "x-ratelimit-reset",
        HeaderValue::from(info.reset_secs),
    );
}

fn rate_limit_response(info: &RateLimitInfo) -> Response<Body> {
    let body = serde_json::json!({
        "error": "Too many requests. Please retry later.",
        "retry_after_seconds": info.reset_secs,
    });

    let mut resp = Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header("content-type", "application/json")
        .header("retry-after", info.reset_secs.to_string())
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    set_rate_limit_headers(&mut resp, info);
    resp
}

impl<S> Service<Request<Body>> for RateLimitService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let ip = extract_client_ip(&req);

        match self.state.check(ip) {
            Ok(info) => {
                let future = self.inner.call(req);
                Box::pin(async move {
                    let mut resp = future.await?;
                    set_rate_limit_headers(&mut resp, &info);
                    Ok(resp)
                })
            }
            Err(info) => {
                // Audit log: rate limit violation
                tracing::warn!(
                    target: "audit",
                    event = "rate_limit_exceeded",
                    client_ip = %ip,
                    limit = info.limit,
                    retry_after_secs = info.reset_secs,
                    "Rate limit exceeded for client"
                );

                let response = rate_limit_response(&info);
                Box::pin(async move { Ok(response) })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_allows_requests_under_limit() {
        let state = RateLimitState::new(RateLimitConfig {
            max_requests: 5,
            window: Duration::from_secs(60),
        });

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        for i in 0..5u32 {
            let info = state.check(ip).unwrap();
            assert_eq!(info.limit, 5);
            assert_eq!(info.remaining, 5 - i - 1);
        }
    }

    #[test]
    fn test_blocks_requests_over_limit() {
        let state = RateLimitState::new(RateLimitConfig {
            max_requests: 3,
            window: Duration::from_secs(60),
        });

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        for _ in 0..3 {
            assert!(state.check(ip).is_ok());
        }
        // 4th request should be blocked
        let result = state.check(ip);
        assert!(result.is_err());
        let info = result.unwrap_err();
        assert_eq!(info.limit, 3);
        assert_eq!(info.remaining, 0);
        assert!(info.reset_secs > 0);
        assert!(info.reset_secs <= 61);
    }

    #[test]
    fn test_different_ips_tracked_separately() {
        let state = RateLimitState::new(RateLimitConfig {
            max_requests: 2,
            window: Duration::from_secs(60),
        });

        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        // Exhaust ip1's quota
        assert!(state.check(ip1).is_ok());
        assert!(state.check(ip1).is_ok());
        assert!(state.check(ip1).is_err());

        // ip2 should still be allowed
        assert!(state.check(ip2).is_ok());
        assert!(state.check(ip2).is_ok());
        assert!(state.check(ip2).is_err());
    }

    #[test]
    fn test_cleanup_removes_expired() {
        let state = RateLimitState::new(RateLimitConfig {
            max_requests: 100,
            window: Duration::from_secs(0), // instant expiry for testing
        });

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        state.check(ip).ok();

        state.cleanup();

        let map = state.requests.lock().unwrap();
        assert!(map.is_empty() || map.get(&ip).map_or(true, |v| v.is_empty()));
    }

    #[test]
    fn test_extract_ip_from_x_forwarded_for() {
        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.50, 70.41.3.18")
            .body(Body::empty())
            .unwrap();

        let ip = extract_client_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));
    }

    #[test]
    fn test_extract_ip_from_x_real_ip() {
        let req = Request::builder()
            .header("x-real-ip", "198.51.100.42")
            .body(Body::empty())
            .unwrap();

        let ip = extract_client_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42)));
    }

    #[test]
    fn test_extract_ip_fallback() {
        let req = Request::builder().body(Body::empty()).unwrap();

        let ip = extract_client_ip(&req);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn test_rate_limit_response_format() {
        let info = RateLimitInfo {
            limit: 100,
            remaining: 0,
            reset_secs: 42,
        };
        let resp = rate_limit_response(&info);
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers().get("retry-after").unwrap().to_str().unwrap(),
            "42"
        );
        assert_eq!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/json"
        );
        assert_eq!(
            resp.headers()
                .get("x-ratelimit-limit")
                .unwrap()
                .to_str()
                .unwrap(),
            "100"
        );
        assert_eq!(
            resp.headers()
                .get("x-ratelimit-remaining")
                .unwrap()
                .to_str()
                .unwrap(),
            "0"
        );
        assert_eq!(
            resp.headers()
                .get("x-ratelimit-reset")
                .unwrap()
                .to_str()
                .unwrap(),
            "42"
        );
    }

    #[test]
    fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.max_requests, 100);
        assert_eq!(config.window, Duration::from_secs(60));
    }

    #[test]
    fn test_rate_limit_info_on_success() {
        let state = RateLimitState::new(RateLimitConfig {
            max_requests: 10,
            window: Duration::from_secs(60),
        });

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        // First request
        let info = state.check(ip).unwrap();
        assert_eq!(info.limit, 10);
        assert_eq!(info.remaining, 9);
        assert!(info.reset_secs > 0);

        // Use up 8 more
        for _ in 0..8 {
            state.check(ip).unwrap();
        }

        // 10th request: last allowed
        let info = state.check(ip).unwrap();
        assert_eq!(info.remaining, 0);

        // 11th: blocked
        let info = state.check(ip).unwrap_err();
        assert_eq!(info.remaining, 0);
        assert_eq!(info.limit, 10);
    }
}
