//! Comprehensive Health Check System
//!
//! Provides deep health checks for all dependencies and subsystems.
//! Supports Kubernetes liveness and readiness probes.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Health check status
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum HealthStatus {
    /// Component is healthy
    Healthy,
    /// Component is degraded but functional
    Degraded,
    /// Component is unhealthy
    Unhealthy,
    /// Health check not yet run
    Unknown,
}

impl HealthStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }

    pub fn to_http_status(&self) -> u16 {
        match self {
            Self::Healthy => 200,
            Self::Degraded => 200,
            Self::Unhealthy => 503,
            Self::Unknown => 503,
        }
    }
}

/// Result of a single health check
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthCheckResult {
    /// Component name
    pub name: String,
    /// Health status
    pub status: HealthStatus,
    /// Latency of the health check
    pub latency_ms: u64,
    /// Optional message
    pub message: Option<String>,
    /// Last check timestamp
    pub last_check: u64,
}

impl HealthCheckResult {
    pub fn healthy(name: impl Into<String>, latency: Duration) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            latency_ms: latency.as_millis() as u64,
            message: None,
            last_check: current_timestamp(),
        }
    }

    pub fn degraded(name: impl Into<String>, latency: Duration, message: String) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded,
            latency_ms: latency.as_millis() as u64,
            message: Some(message),
            last_check: current_timestamp(),
        }
    }

    pub fn unhealthy(name: impl Into<String>, latency: Duration, message: String) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy,
            latency_ms: latency.as_millis() as u64,
            message: Some(message),
            last_check: current_timestamp(),
        }
    }
}

/// Overall system health
#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemHealth {
    /// Overall status
    pub status: HealthStatus,
    /// Individual component checks
    pub components: Vec<HealthCheckResult>,
    /// Total check duration
    pub total_latency_ms: u64,
    /// Timestamp
    pub timestamp: u64,
    /// Version
    pub version: String,
}

impl SystemHealth {
    pub fn from_results(results: Vec<HealthCheckResult>, latency: Duration) -> Self {
        let status = if results.iter().any(|r| r.status == HealthStatus::Unhealthy) {
            HealthStatus::Unhealthy
        } else if results.iter().any(|r| r.status == HealthStatus::Degraded) {
            HealthStatus::Degraded
        } else if results.iter().all(|r| r.status == HealthStatus::Healthy) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unknown
        };

        Self {
            status,
            components: results,
            total_latency_ms: latency.as_millis() as u64,
            timestamp: current_timestamp(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Health check configuration
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    /// Timeout for individual checks
    pub check_timeout: Duration,
    /// Cache duration for health results
    pub cache_duration: Duration,
    /// RPC endpoint to check
    pub rpc_url: Option<String>,
    /// Maximum memory usage percentage
    pub max_memory_percent: f64,
    /// Maximum queue size before degraded
    pub max_queue_size: usize,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            check_timeout: Duration::from_secs(5),
            cache_duration: Duration::from_secs(10),
            rpc_url: None,
            max_memory_percent: 90.0,
            max_queue_size: 1000,
        }
    }
}

/// Cached health check result
struct CachedHealth {
    result: SystemHealth,
    expires_at: Instant,
}

/// Callback for checking queue size
pub type QueueSizeCallback = Arc<dyn Fn() -> usize + Send + Sync>;

/// Health checker with caching
pub struct HealthChecker {
    config: HealthCheckConfig,
    cache: RwLock<Option<CachedHealth>>,
    queue_size_fn: Option<QueueSizeCallback>,
}

impl HealthChecker {
    /// Create a new health checker
    pub fn new(config: HealthCheckConfig) -> Self {
        Self {
            config,
            cache: RwLock::new(None),
            queue_size_fn: None,
        }
    }

    /// Set queue size callback
    pub fn with_queue_check(mut self, callback: QueueSizeCallback) -> Self {
        self.queue_size_fn = Some(callback);
        self
    }

    /// Run all health checks
    pub async fn check(&self) -> SystemHealth {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.as_ref() {
                if cached.expires_at > Instant::now() {
                    return cached.result.clone();
                }
            }
        }

        // Run all checks
        let start = Instant::now();
        let mut results = Vec::new();

        // Self check (always healthy if we can respond)
        results.push(HealthCheckResult::healthy("self", Duration::from_micros(1)));

        // RPC check
        if let Some(ref url) = self.config.rpc_url {
            results.push(self.check_rpc(url).await);
        }

        // Queue check
        if let Some(ref callback) = self.queue_size_fn {
            results.push(self.check_queue(callback));
        }

        let health = SystemHealth::from_results(results, start.elapsed());

        // Cache the result
        {
            let mut cache = self.cache.write().await;
            *cache = Some(CachedHealth {
                result: health.clone(),
                expires_at: Instant::now() + self.config.cache_duration,
            });
        }

        health
    }

    /// Simple liveness check (is the process running?)
    pub async fn liveness(&self) -> bool {
        true // If we can respond, we're alive
    }

    /// Readiness check (can we serve traffic?)
    pub async fn readiness(&self) -> bool {
        let health = self.check().await;
        health.status.is_healthy()
    }

    async fn check_rpc(&self, url: &str) -> HealthCheckResult {
        let start = Instant::now();

        // Simple TCP connect check
        match tokio::time::timeout(
            self.config.check_timeout,
            check_tcp_connect(url),
        ).await {
            Ok(Ok(())) => HealthCheckResult::healthy("rpc", start.elapsed()),
            Ok(Err(e)) => HealthCheckResult::unhealthy("rpc", start.elapsed(), e),
            Err(_) => HealthCheckResult::unhealthy(
                "rpc",
                start.elapsed(),
                "RPC connection timed out".to_string(),
            ),
        }
    }

    fn check_queue(&self, callback: &QueueSizeCallback) -> HealthCheckResult {
        let start = Instant::now();
        let size = callback();
        let latency = start.elapsed();

        if size > self.config.max_queue_size {
            HealthCheckResult::unhealthy(
                "queue",
                latency,
                format!("Queue size {} exceeds max {}", size, self.config.max_queue_size),
            )
        } else if size > self.config.max_queue_size * 80 / 100 {
            HealthCheckResult::degraded(
                "queue",
                latency,
                format!("Queue size {} approaching max {}", size, self.config.max_queue_size),
            )
        } else {
            HealthCheckResult::healthy("queue", latency)
        }
    }
}

// Helper functions

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn check_tcp_connect(url: &str) -> Result<(), String> {
    // Parse URL to get host:port
    let addr = if url.starts_with("http://") || url.starts_with("https://") {
        let parsed = url.trim_start_matches("http://").trim_start_matches("https://");
        let host_port: Vec<&str> = parsed.split('/').next().unwrap_or("").split(':').collect();
        let host = host_port.first().unwrap_or(&"localhost");
        let port = if url.starts_with("https://") {
            host_port.get(1).and_then(|p| p.parse().ok()).unwrap_or(443)
        } else {
            host_port.get(1).and_then(|p| p.parse().ok()).unwrap_or(80)
        };
        format!("{}:{}", host, port)
    } else {
        url.to_string()
    };

    tokio::net::TcpStream::connect(&addr)
        .await
        .map(|_| ())
        .map_err(|e| format!("Connection failed: {}", e))
}

/// Dependency check builder for easy setup
pub struct DependencyChecker {
    checks: Vec<DependencyCheck>,
}

struct DependencyCheck {
    name: String,
    check_fn: Box<dyn Fn() -> DependencyStatus + Send + Sync>,
}

/// Status of a dependency
#[derive(Debug, Clone)]
pub enum DependencyStatus {
    Available,
    Degraded(String),
    Unavailable(String),
}

impl DependencyChecker {
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    /// Add a dependency check
    pub fn add<F>(mut self, name: impl Into<String>, check: F) -> Self
    where
        F: Fn() -> DependencyStatus + Send + Sync + 'static,
    {
        self.checks.push(DependencyCheck {
            name: name.into(),
            check_fn: Box::new(check),
        });
        self
    }

    /// Run all dependency checks
    pub fn check_all(&self) -> Vec<HealthCheckResult> {
        self.checks
            .iter()
            .map(|dep| {
                let start = Instant::now();
                let status = (dep.check_fn)();
                let latency = start.elapsed();

                match status {
                    DependencyStatus::Available => {
                        HealthCheckResult::healthy(&dep.name, latency)
                    }
                    DependencyStatus::Degraded(msg) => {
                        HealthCheckResult::degraded(&dep.name, latency, msg)
                    }
                    DependencyStatus::Unavailable(msg) => {
                        HealthCheckResult::unhealthy(&dep.name, latency, msg)
                    }
                }
            })
            .collect()
    }
}

impl Default for DependencyChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_checker() {
        let config = HealthCheckConfig::default();
        let checker = HealthChecker::new(config);

        let health = checker.check().await;
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_liveness() {
        let checker = HealthChecker::new(HealthCheckConfig::default());
        assert!(checker.liveness().await);
    }

    #[test]
    fn test_health_status() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(HealthStatus::Degraded.is_healthy());
        assert!(!HealthStatus::Unhealthy.is_healthy());
        assert!(!HealthStatus::Unknown.is_healthy());
    }

    #[test]
    fn test_http_status_codes() {
        assert_eq!(HealthStatus::Healthy.to_http_status(), 200);
        assert_eq!(HealthStatus::Degraded.to_http_status(), 200);
        assert_eq!(HealthStatus::Unhealthy.to_http_status(), 503);
    }

    #[tokio::test]
    async fn test_cache() {
        let config = HealthCheckConfig {
            cache_duration: Duration::from_millis(100),
            ..Default::default()
        };
        let checker = HealthChecker::new(config);

        // First check
        let health1 = checker.check().await;
        let ts1 = health1.timestamp;

        // Second check (should be cached)
        let health2 = checker.check().await;
        assert_eq!(health2.timestamp, ts1);

        // Wait for cache to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Third check (should be fresh)
        let health3 = checker.check().await;
        assert!(health3.timestamp >= ts1);
    }

    #[tokio::test]
    async fn test_queue_check() {
        let config = HealthCheckConfig {
            max_queue_size: 100,
            ..Default::default()
        };

        // Healthy queue
        let checker = HealthChecker::new(config.clone())
            .with_queue_check(Arc::new(|| 50));
        let health = checker.check().await;
        assert!(health.components.iter().any(|c| c.name == "queue" && c.status == HealthStatus::Healthy));

        // Degraded queue
        let checker = HealthChecker::new(config.clone())
            .with_queue_check(Arc::new(|| 85));
        let health = checker.check().await;
        assert!(health.components.iter().any(|c| c.name == "queue" && c.status == HealthStatus::Degraded));

        // Unhealthy queue
        let checker = HealthChecker::new(config)
            .with_queue_check(Arc::new(|| 150));
        let health = checker.check().await;
        assert!(health.components.iter().any(|c| c.name == "queue" && c.status == HealthStatus::Unhealthy));
    }

    #[test]
    fn test_system_health_from_results() {
        let results = vec![
            HealthCheckResult::healthy("a", Duration::from_millis(1)),
            HealthCheckResult::healthy("b", Duration::from_millis(2)),
        ];

        let health = SystemHealth::from_results(results, Duration::from_millis(3));
        assert_eq!(health.status, HealthStatus::Healthy);

        let results = vec![
            HealthCheckResult::healthy("a", Duration::from_millis(1)),
            HealthCheckResult::degraded("b", Duration::from_millis(2), "warning".into()),
        ];

        let health = SystemHealth::from_results(results, Duration::from_millis(3));
        assert_eq!(health.status, HealthStatus::Degraded);

        let results = vec![
            HealthCheckResult::healthy("a", Duration::from_millis(1)),
            HealthCheckResult::unhealthy("b", Duration::from_millis(2), "error".into()),
        ];

        let health = SystemHealth::from_results(results, Duration::from_millis(3));
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_dependency_checker() {
        let checker = DependencyChecker::new()
            .add("database", || DependencyStatus::Available)
            .add("cache", || DependencyStatus::Degraded("slow".into()))
            .add("external", || DependencyStatus::Unavailable("down".into()));

        let results = checker.check_all();
        assert_eq!(results.len(), 3);
        assert!(results.iter().any(|r| r.name == "database" && r.status == HealthStatus::Healthy));
        assert!(results.iter().any(|r| r.name == "cache" && r.status == HealthStatus::Degraded));
        assert!(results.iter().any(|r| r.name == "external" && r.status == HealthStatus::Unhealthy));
    }
}
