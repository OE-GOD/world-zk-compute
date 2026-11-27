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
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use ipnet::IpNet;
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

/// Configuration for trusted reverse proxies.
///
/// When the peer (socket) IP matches a trusted proxy CIDR, the
/// `X-Forwarded-For` / `X-Real-IP` headers are honoured. Otherwise
/// only the socket address is used, preventing header-spoofing attacks.
#[derive(Debug, Clone)]
pub struct TrustedProxyConfig {
    /// CIDR ranges that are trusted reverse proxies.
    trusted_cidrs: Vec<IpNet>,
}

impl TrustedProxyConfig {
    /// Parse a comma-separated list of CIDR ranges (e.g. "10.0.0.0/8,172.16.0.0/12").
    /// Skips invalid entries with a warning.
    pub fn from_env_str(s: &str) -> Self {
        let trusted_cidrs: Vec<IpNet> = s
            .split(',')
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .filter_map(|c| match c.parse::<IpNet>() {
                Ok(net) => Some(net),
                Err(e) => {
                    tracing::warn!("Ignoring invalid trusted proxy CIDR '{}': {}", c, e);
                    None
                }
            })
            .collect();
        Self { trusted_cidrs }
    }

    /// Create an empty config (no proxies trusted -- fail-secure default).
    pub fn none() -> Self {
        Self {
            trusted_cidrs: Vec::new(),
        }
    }

    /// Returns `true` if the given IP falls within any trusted CIDR.
    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trusted_cidrs.iter().any(|net| net.contains(&ip))
    }
}

impl Default for TrustedProxyConfig {
    fn default() -> Self {
        Self::none()
    }
}

/// Shared state for the rate limiter, tracking per-IP request timestamps.
#[derive(Clone)]
pub struct RateLimitState {
    config: RateLimitConfig,
    trusted_proxies: TrustedProxyConfig,
    /// Map from IP address to list of request timestamps within the window.
    requests: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
}

impl RateLimitState {
    pub fn new(config: RateLimitConfig, trusted_proxies: TrustedProxyConfig) -> Self {
        Self {
            config,
            trusted_proxies,
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
            let reset_secs = (oldest + self.config.window).duration_since(now).as_secs() + 1;
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
                (oldest + self.config.window).duration_since(now).as_secs() + 1
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

/// Shortest period of the background cleanup in [`RateLimitLayer::new`].
const MIN_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

/// Tower [`Layer`] that applies per-IP rate limiting.
#[derive(Clone)]
pub struct RateLimitLayer {
    state: RateLimitState,
}

impl RateLimitLayer {
    /// Create the layer and spawn the background task that drops the buckets
    /// of clients with no request inside the window. Without it every client
    /// address seen keeps a map entry for as long as the process runs. Needs a
    /// tokio runtime.
    pub fn new(config: RateLimitConfig, trusted_proxies: TrustedProxyConfig) -> Self {
        // Once a window has passed a bucket can be empty, so cleaning up more
        // often than that frees nothing. A zero window would make the
        // interval panic.
        let period = config.window.max(MIN_CLEANUP_INTERVAL);
        let state = RateLimitState::new(config, trusted_proxies);

        let cleanup_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(period);
            loop {
                interval.tick().await;
                cleanup_state.cleanup();
            }
        });

        Self { state }
    }

    /// Create a layer without spawning the background cleanup task, for tests
    /// that build a router outside a tokio runtime. Production code cannot
    /// call it, so it cannot end up with a map that never shrinks.
    #[cfg(test)]
    pub fn new_without_cleanup(
        config: RateLimitConfig,
        trusted_proxies: TrustedProxyConfig,
    ) -> Self {
        Self {
            state: RateLimitState::new(config, trusted_proxies),
        }
    }

    /// The proxies whose forwarding headers this layer trusts, so that other
    /// per-client limits can name clients the same way.
    pub fn trusted_proxies(&self) -> &TrustedProxyConfig {
        &self.state.trusted_proxies
    }

    /// Number of client addresses that currently have a bucket.
    #[cfg(test)]
    pub(crate) fn tracked_clients(&self) -> usize {
        self.state
            .requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
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

/// Extract the client IP of a request (see [`client_ip`]).
fn extract_client_ip<B>(req: &Request<B>, trusted_proxies: &TrustedProxyConfig) -> IpAddr {
    let peer_ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip());
    client_ip(peer_ip, req.headers(), trusted_proxies)
}

/// The address a client is counted under.
///
/// Without a trusted proxy in front this is the socket peer. When the peer is
/// a trusted proxy, `X-Forwarded-For` is read from the right: each proxy
/// appends the address it received the request from, so the right-hand
/// entries were written by our own proxies and everything to the left of
/// them by the client. The first entry from the right that is not a trusted
/// proxy is the client; what lies to its left is never used. An entry that
/// is not a plain IP address, or a list made only of trusted proxies, gives
/// the peer. `X-Real-IP` is read only when there is no `X-Forwarded-For`.
/// Without `ConnectInfo` the address is localhost.
pub(crate) fn client_ip(
    peer_ip: Option<IpAddr>,
    headers: &HeaderMap,
    trusted_proxies: &TrustedProxyConfig,
) -> IpAddr {
    let Some(peer) = peer_ip else {
        return IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    };
    if !trusted_proxies.is_trusted(peer) {
        return peer;
    }

    // A proxy may add its own X-Forwarded-For line instead of extending the
    // client's, so the lines are read as one list, last line last.
    let mut forwarded = headers.get_all("x-forwarded-for").iter().peekable();
    if forwarded.peek().is_some() {
        let values: Vec<&HeaderValue> = forwarded.collect();
        for value in values.into_iter().rev() {
            let Ok(value) = value.to_str() else {
                return peer;
            };
            for entry in value.rsplit(',') {
                match entry.trim().parse::<IpAddr>() {
                    Ok(ip) if trusted_proxies.is_trusted(ip) => continue,
                    Ok(ip) => return ip,
                    Err(_) => return peer,
                }
            }
        }
        return peer;
    }

    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            if let Ok(ip) = value.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    peer
}

/// Append standard rate-limit headers to a response.
fn set_rate_limit_headers(resp: &mut Response<Body>, info: &RateLimitInfo) {
    let headers = resp.headers_mut();
    headers.insert("x-ratelimit-limit", HeaderValue::from(info.limit as u64));
    headers.insert(
        "x-ratelimit-remaining",
        HeaderValue::from(info.remaining as u64),
    );
    headers.insert("x-ratelimit-reset", HeaderValue::from(info.reset_secs));
}

fn rate_limit_response(info: &RateLimitInfo) -> Response<Body> {
    let body = serde_json::json!({
        "error": "Too many requests. Please retry later.",
        "retry_after_seconds": info.reset_secs,
    });

    let body_str = serde_json::to_string(&body)
        .unwrap_or_else(|_| r#"{"error":"Too many requests. Please retry later."}"#.to_string());
    let mut resp = Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header("content-type", "application/json")
        .header("retry-after", info.reset_secs.to_string())
        .body(Body::from(body_str))
        .unwrap_or_else(|_| Response::new(Body::from("Too many requests")));
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
        let ip = extract_client_ip(&req, &self.state.trusted_proxies);

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
        let state = RateLimitState::new(
            RateLimitConfig {
                max_requests: 5,
                window: Duration::from_secs(60),
            },
            TrustedProxyConfig::default(),
        );

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        for i in 0..5u32 {
            let info = state.check(ip).unwrap();
            assert_eq!(info.limit, 5);
            assert_eq!(info.remaining, 5 - i - 1);
        }
    }

    #[test]
    fn test_blocks_requests_over_limit() {
        let state = RateLimitState::new(
            RateLimitConfig {
                max_requests: 3,
                window: Duration::from_secs(60),
            },
            TrustedProxyConfig::default(),
        );

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
        let state = RateLimitState::new(
            RateLimitConfig {
                max_requests: 2,
                window: Duration::from_secs(60),
            },
            TrustedProxyConfig::default(),
        );

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
        let state = RateLimitState::new(
            RateLimitConfig {
                max_requests: 100,
                window: Duration::from_secs(0), // instant expiry for testing
            },
            TrustedProxyConfig::default(),
        );

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        state.check(ip).ok();

        state.cleanup();

        let map = state.requests.lock().unwrap();
        assert!(map.is_empty() || map.get(&ip).is_none_or(|v| v.is_empty()));
    }

    /// A request from `peer` carrying one header line per entry of `xff`.
    fn request_via(peer: Ipv4Addr, xff: &[&str]) -> Request<Body> {
        let mut builder = Request::builder();
        for line in xff {
            builder = builder.header("x-forwarded-for", *line);
        }
        let mut req = builder.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                IpAddr::V4(peer),
                12345,
            )));
        req
    }

    #[test]
    fn test_xff_uses_right_most_entry_not_written_by_a_trusted_proxy() {
        // The trusted proxy 10.0.0.1 appended 70.41.3.18, the address it saw.
        // 203.0.113.50 was already in the header the client sent.
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");
        let req = request_via(Ipv4Addr::new(10, 0, 0, 1), &["203.0.113.50, 70.41.3.18"]);

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(70, 41, 3, 18)));
    }

    #[test]
    fn test_xff_entries_written_by_the_client_cannot_pick_the_bucket() {
        let trusted = TrustedProxyConfig::from_env_str("127.0.0.0/8");
        for spoofed in ["1.2.3.4", "5.6.7.8", "10.0.0.1, 192.0.2.9", "not-an-ip"] {
            let req = request_via(Ipv4Addr::LOCALHOST, &[&format!("{spoofed}, 198.51.100.7")]);
            assert_eq!(
                extract_client_ip(&req, &trusted),
                IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)),
                "client-written prefix {spoofed:?}"
            );
        }
    }

    #[test]
    fn test_xff_lines_added_by_a_proxy_are_read_last() {
        // The client sent its own line; the proxy added a second one.
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");
        let req = request_via(
            Ipv4Addr::new(10, 0, 0, 1),
            &["203.0.113.50", "198.51.100.7, 10.0.0.2"],
        );

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)));
    }

    #[test]
    fn test_xff_of_only_trusted_proxies_gives_the_peer() {
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");
        let req = request_via(Ipv4Addr::new(10, 0, 0, 1), &["10.0.0.3, 10.0.0.2"]);

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn test_xff_unparseable_entry_from_the_right_gives_the_peer() {
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");
        let req = request_via(
            Ipv4Addr::new(10, 0, 0, 1),
            &["203.0.113.50, garbage, 10.0.0.2"],
        );

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn test_x_real_ip_not_used_when_xff_is_present_but_unusable() {
        // X-Forwarded-For is there but names no client, so the request is
        // counted under the proxy, not under an X-Real-IP the client may
        // have written.
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");
        let mut req = request_via(Ipv4Addr::new(10, 0, 0, 1), &["not-an-ip"]);
        req.headers_mut()
            .insert("x-real-ip", HeaderValue::from_static("203.0.113.50"));

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[tokio::test]
    async fn test_layer_drops_buckets_of_idle_clients() {
        let layer = RateLimitLayer::new(
            RateLimitConfig {
                max_requests: 3,
                window: Duration::from_secs(1),
            },
            TrustedProxyConfig::none(),
        );
        for i in 0..50u8 {
            layer
                .state
                .check(IpAddr::V4(Ipv4Addr::new(192, 0, 2, i)))
                .unwrap();
        }
        assert_eq!(layer.tracked_clients(), 50);

        // The window is 1 s and the cleanup runs every second.
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(layer.tracked_clients(), 0);
    }

    #[test]
    fn test_extract_ip_from_x_real_ip() {
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");

        let mut req = Request::builder()
            .header("x-real-ip", "198.51.100.42")
            .body(Body::empty())
            .unwrap();
        // Insert ConnectInfo for a trusted proxy
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                12345,
            )));

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 42)));
    }

    #[test]
    fn test_extract_ip_fallback() {
        let req = Request::builder().body(Body::empty()).unwrap();

        let ip = extract_client_ip(&req, &TrustedProxyConfig::none());
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn test_extract_ip_xff_ignored_from_untrusted_peer() {
        // Request has XFF but peer is not trusted -- should use fallback
        let req = Request::builder()
            .header("x-forwarded-for", "203.0.113.50, 70.41.3.18")
            .body(Body::empty())
            .unwrap();

        let trusted = TrustedProxyConfig::none();
        let ip = extract_client_ip(&req, &trusted);
        // No ConnectInfo, so fallback to localhost
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn test_extract_ip_xff_skips_trusted_proxies_from_the_right() {
        // 10.0.0.1 is a second trusted proxy in the chain, so the client is
        // the entry in front of it. 6.6.6.6 was written by the client.
        let trusted = TrustedProxyConfig::from_env_str("10.0.0.0/8");
        let req = request_via(
            Ipv4Addr::new(10, 0, 0, 1),
            &["6.6.6.6, 203.0.113.50, 10.0.0.1"],
        );

        let ip = extract_client_ip(&req, &trusted);
        assert_eq!(ip, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));
    }

    #[test]
    fn test_trusted_proxy_config_parsing() {
        let config = TrustedProxyConfig::from_env_str("10.0.0.0/8, 172.16.0.0/12");
        assert!(config.is_trusted(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(config.is_trusted(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!config.is_trusted(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn test_trusted_proxy_config_empty() {
        let config = TrustedProxyConfig::none();
        assert!(!config.is_trusted(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn test_trusted_proxy_config_invalid_cidr_skipped() {
        let config = TrustedProxyConfig::from_env_str("10.0.0.0/8, not-a-cidr, 172.16.0.0/12");
        assert!(config.is_trusted(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(config.is_trusted(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
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
        let state = RateLimitState::new(
            RateLimitConfig {
                max_requests: 10,
                window: Duration::from_secs(60),
            },
            TrustedProxyConfig::default(),
        );

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
