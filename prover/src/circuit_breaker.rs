//! Circuit Breaker Pattern Implementation
//!
//! Prevents cascading failures by stopping requests to failing services.
//! States: Closed (normal) -> Open (failing) -> Half-Open (testing)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests flow through
    Closed,
    /// Service is failing - requests are rejected
    Open,
    /// Testing if service recovered - limited requests allowed
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Circuit breaker configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening circuit
    pub failure_threshold: u32,
    /// Number of successes in half-open to close circuit
    pub success_threshold: u32,
    /// Time to wait before transitioning from open to half-open
    pub reset_timeout: Duration,
    /// Time window for counting failures
    pub failure_window: Duration,
    /// Maximum requests allowed in half-open state
    pub half_open_max_requests: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            reset_timeout: Duration::from_secs(30),
            failure_window: Duration::from_secs(60),
            half_open_max_requests: 3,
        }
    }
}

impl CircuitBreakerConfig {
    /// Strict config for critical services
    pub fn strict() -> Self {
        Self {
            failure_threshold: 3,
            success_threshold: 5,
            reset_timeout: Duration::from_secs(60),
            failure_window: Duration::from_secs(30),
            half_open_max_requests: 1,
        }
    }

    /// Lenient config for less critical services
    pub fn lenient() -> Self {
        Self {
            failure_threshold: 10,
            success_threshold: 2,
            reset_timeout: Duration::from_secs(15),
            failure_window: Duration::from_secs(120),
            half_open_max_requests: 5,
        }
    }
}

/// Internal circuit state tracking
struct CircuitInternals {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure_time: Option<Instant>,
    last_state_change: Instant,
    half_open_requests: u32,
    failure_timestamps: Vec<Instant>,
}

impl CircuitInternals {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure_time: None,
            last_state_change: Instant::now(),
            half_open_requests: 0,
            failure_timestamps: Vec::new(),
        }
    }
}

/// Circuit breaker for a single service
pub struct CircuitBreaker {
    name: String,
    config: CircuitBreakerConfig,
    internals: RwLock<CircuitInternals>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(name: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            name: name.into(),
            config,
            internals: RwLock::new(CircuitInternals::new()),
        }
    }

    /// Get the current state
    pub async fn state(&self) -> CircuitState {
        let mut internals = self.internals.write().await;
        self.maybe_transition(&mut internals);
        internals.state
    }

    /// Check if request is allowed
    pub async fn allow_request(&self) -> bool {
        let mut internals = self.internals.write().await;
        self.maybe_transition(&mut internals);

        match internals.state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => {
                if internals.half_open_requests < self.config.half_open_max_requests {
                    internals.half_open_requests += 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful request
    pub async fn record_success(&self) {
        let mut internals = self.internals.write().await;

        match internals.state {
            CircuitState::Closed => {
                // Reset failure count on success
                internals.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                internals.success_count += 1;
                if internals.success_count >= self.config.success_threshold {
                    self.transition_to(&mut internals, CircuitState::Closed);
                    tracing::info!(
                        circuit = %self.name,
                        "Circuit breaker closed after {} successes",
                        internals.success_count
                    );
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but ignore
            }
        }
    }

    /// Record a failed request
    pub async fn record_failure(&self) {
        let mut internals = self.internals.write().await;
        let now = Instant::now();

        // Clean old failures outside the window
        internals.failure_timestamps.retain(|t| {
            now.duration_since(*t) < self.config.failure_window
        });
        internals.failure_timestamps.push(now);

        match internals.state {
            CircuitState::Closed => {
                internals.failure_count = internals.failure_timestamps.len() as u32;
                internals.last_failure_time = Some(now);

                if internals.failure_count >= self.config.failure_threshold {
                    self.transition_to(&mut internals, CircuitState::Open);
                    tracing::warn!(
                        circuit = %self.name,
                        failures = internals.failure_count,
                        "Circuit breaker opened"
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open goes back to open
                self.transition_to(&mut internals, CircuitState::Open);
                tracing::warn!(
                    circuit = %self.name,
                    "Circuit breaker reopened after half-open failure"
                );
            }
            CircuitState::Open => {
                internals.last_failure_time = Some(now);
            }
        }
    }

    /// Check for automatic state transitions
    fn maybe_transition(&self, internals: &mut CircuitInternals) {
        if internals.state == CircuitState::Open {
            let elapsed = internals.last_state_change.elapsed();
            if elapsed >= self.config.reset_timeout {
                self.transition_to(internals, CircuitState::HalfOpen);
                tracing::info!(
                    circuit = %self.name,
                    "Circuit breaker entering half-open state"
                );
            }
        }
    }

    fn transition_to(&self, internals: &mut CircuitInternals, new_state: CircuitState) {
        internals.state = new_state;
        internals.last_state_change = Instant::now();
        internals.success_count = 0;
        internals.half_open_requests = 0;

        if new_state == CircuitState::Closed {
            internals.failure_count = 0;
            internals.failure_timestamps.clear();
        }
    }

    /// Get circuit breaker stats
    pub async fn stats(&self) -> CircuitStats {
        let internals = self.internals.read().await;
        CircuitStats {
            name: self.name.clone(),
            state: internals.state,
            failure_count: internals.failure_count,
            success_count: internals.success_count,
            last_failure: internals.last_failure_time.map(|t| t.elapsed()),
            time_in_state: internals.last_state_change.elapsed(),
        }
    }

    /// Execute a function with circuit breaker protection
    pub async fn call<F, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: std::future::Future<Output = Result<T, E>>,
    {
        if !self.allow_request().await {
            return Err(CircuitBreakerError::Open(self.name.clone()));
        }

        match f.await {
            Ok(result) => {
                self.record_success().await;
                Ok(result)
            }
            Err(e) => {
                self.record_failure().await;
                Err(CircuitBreakerError::ServiceError(e))
            }
        }
    }
}

/// Circuit breaker statistics
#[derive(Debug, Clone)]
pub struct CircuitStats {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_failure: Option<Duration>,
    pub time_in_state: Duration,
}

/// Circuit breaker error
#[derive(Debug)]
pub enum CircuitBreakerError<E> {
    /// Circuit is open, request rejected
    Open(String),
    /// Underlying service error
    ServiceError(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CircuitBreakerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open(name) => write!(f, "Circuit breaker '{}' is open", name),
            Self::ServiceError(e) => write!(f, "Service error: {}", e),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for CircuitBreakerError<E> {}

/// Circuit breaker registry for managing multiple circuits
pub struct CircuitBreakerRegistry {
    breakers: RwLock<HashMap<String, Arc<CircuitBreaker>>>,
    default_config: CircuitBreakerConfig,
}

impl CircuitBreakerRegistry {
    /// Create a new registry
    pub fn new(default_config: CircuitBreakerConfig) -> Self {
        Self {
            breakers: RwLock::new(HashMap::new()),
            default_config,
        }
    }

    /// Get or create a circuit breaker
    pub async fn get(&self, name: &str) -> Arc<CircuitBreaker> {
        // Try read first
        {
            let breakers = self.breakers.read().await;
            if let Some(cb) = breakers.get(name) {
                return cb.clone();
            }
        }

        // Create new breaker
        let mut breakers = self.breakers.write().await;
        breakers
            .entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::new(name, self.default_config.clone()))
            })
            .clone()
    }

    /// Get or create with custom config
    pub async fn get_with_config(
        &self,
        name: &str,
        config: CircuitBreakerConfig,
    ) -> Arc<CircuitBreaker> {
        let mut breakers = self.breakers.write().await;
        breakers
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(CircuitBreaker::new(name, config)))
            .clone()
    }

    /// Get all circuit stats
    pub async fn all_stats(&self) -> Vec<CircuitStats> {
        let breakers = self.breakers.read().await;
        let mut stats = Vec::with_capacity(breakers.len());

        for cb in breakers.values() {
            stats.push(cb.stats().await);
        }

        stats
    }

    /// Check if any circuits are open
    pub async fn any_open(&self) -> bool {
        let breakers = self.breakers.read().await;
        for cb in breakers.values() {
            if cb.state().await == CircuitState::Open {
                return true;
            }
        }
        false
    }
}

impl Default for CircuitBreakerRegistry {
    fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_closed() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());

        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.allow_request().await);

        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_opens_on_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        // Record failures
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);

        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);
        assert!(!cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            reset_timeout: Duration::from_millis(50),
            half_open_max_requests: 2,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        // Open the circuit
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Open);

        // Wait for reset timeout
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Should transition to half-open
        assert_eq!(cb.state().await, CircuitState::HalfOpen);
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_circuit_breaker_closes_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            reset_timeout: Duration::from_millis(10),
            half_open_max_requests: 5,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        // Open the circuit
        cb.record_failure().await;
        cb.record_failure().await;

        // Wait for half-open
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        // Record successes
        cb.record_success().await;
        cb.record_success().await;

        assert_eq!(cb.state().await, CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_call() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());

        // Successful call
        let result: Result<i32, CircuitBreakerError<&str>> =
            cb.call(async { Ok(42) }).await;
        assert!(matches!(result, Ok(42)));

        // Failed call
        let result: Result<i32, CircuitBreakerError<&str>> =
            cb.call(async { Err("error") }).await;
        assert!(matches!(result, Err(CircuitBreakerError::ServiceError("error"))));
    }

    #[tokio::test]
    async fn test_registry() {
        let registry = CircuitBreakerRegistry::default();

        let cb1 = registry.get("service1").await;
        let cb2 = registry.get("service2").await;
        let cb1_again = registry.get("service1").await;

        // Same name returns same breaker
        assert!(Arc::ptr_eq(&cb1, &cb1_again));

        // Different names return different breakers
        assert!(!Arc::ptr_eq(&cb1, &cb2));
    }

    #[tokio::test]
    async fn test_failure_window() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            failure_window: Duration::from_millis(100),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        // Record 2 failures
        cb.record_failure().await;
        cb.record_failure().await;

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Record 1 more failure - should not open (old failures expired)
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitState::Closed);
    }
}
