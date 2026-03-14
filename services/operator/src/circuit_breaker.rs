//! Circuit breaker pattern for resilient RPC communication.
//!
//! Prevents cascading failures by short-circuiting calls to an unhealthy
//! backend after a configurable number of consecutive failures.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Normal operation — calls pass through.
    Closed,
    /// Backend is unhealthy — calls are rejected immediately.
    Open,
    /// Tentatively allowing trial calls to test recovery.
    HalfOpen,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::Closed => write!(f, "Closed"),
            State::Open => write!(f, "Open"),
            State::HalfOpen => write!(f, "HalfOpen"),
        }
    }
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the breaker.
    pub failure_threshold: u32,
    /// How long to wait in Open state before transitioning to HalfOpen.
    pub recovery_timeout: Duration,
    /// Number of consecutive successes in HalfOpen required to close the
    /// breaker again.
    pub success_threshold_for_close: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
            success_threshold_for_close: 2,
        }
    }
}

/// Error returned when the circuit breaker is open.
#[derive(Debug, thiserror::Error)]
pub enum CircuitBreakerError {
    #[error("circuit breaker is open — backend unavailable (reopens in {remaining_secs:.1}s)")]
    Open { remaining_secs: f64 },
    #[error("call failed: {0}")]
    Inner(#[from] anyhow::Error),
}

/// Snapshot of circuit breaker metrics for monitoring and health checks.
#[derive(Debug, Clone)]
pub struct CircuitBreakerMetrics {
    /// Current state of the breaker.
    pub state: State,
    /// Total number of times the breaker has tripped (Closed/HalfOpen -> Open).
    pub total_trips: u64,
    /// Total number of state transitions (any direction).
    pub state_changes: u64,
    /// Current consecutive failure count.
    pub consecutive_failures: u32,
    /// Current consecutive success count (relevant in HalfOpen).
    pub consecutive_successes: u32,
}

/// A circuit breaker that wraps an unreliable call target.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    inner: Mutex<InnerState>,
    /// Metrics: total number of times the breaker has tripped (Closed -> Open).
    pub trips_total: AtomicU64,
    /// Metrics: total number of state transitions.
    state_changes_total: AtomicU64,
}

struct InnerState {
    state: State,
    consecutive_failures: u32,
    consecutive_successes: u32,
    opened_at: Option<Instant>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            inner: Mutex::new(InnerState {
                state: State::Closed,
                consecutive_failures: 0,
                consecutive_successes: 0,
                opened_at: None,
            }),
            trips_total: AtomicU64::new(0),
            state_changes_total: AtomicU64::new(0),
        }
    }

    /// Get the current state of the circuit breaker.
    pub fn state(&self) -> State {
        let mut inner = self.inner.lock().expect("circuit breaker mutex poisoned");
        self.maybe_transition_to_half_open(&mut inner);
        inner.state
    }

    /// Check if a request is allowed through the circuit breaker.
    ///
    /// Returns `Ok(())` if the breaker is Closed or HalfOpen.
    /// Returns `Err(CircuitBreakerError::Open)` if the breaker is Open and the
    /// recovery timeout has not yet elapsed.
    /// If the breaker is Open and the recovery timeout has elapsed, transitions
    /// to HalfOpen and allows the request.
    pub fn allow_request(&self) -> Result<(), CircuitBreakerError> {
        let mut inner = self.inner.lock().expect("circuit breaker mutex poisoned");
        self.maybe_transition_to_half_open(&mut inner);

        match inner.state {
            State::Closed | State::HalfOpen => Ok(()),
            State::Open => {
                let remaining = self
                    .config
                    .recovery_timeout
                    .checked_sub(inner.opened_at.unwrap_or_else(Instant::now).elapsed())
                    .unwrap_or(Duration::ZERO);
                Err(CircuitBreakerError::Open {
                    remaining_secs: remaining.as_secs_f64(),
                })
            }
        }
    }

    /// Transition Open → HalfOpen if recovery timeout has elapsed.
    fn maybe_transition_to_half_open(&self, inner: &mut InnerState) {
        if inner.state == State::Open {
            if let Some(opened_at) = inner.opened_at {
                if opened_at.elapsed() >= self.config.recovery_timeout {
                    inner.state = State::HalfOpen;
                    inner.consecutive_successes = 0;
                    self.state_changes_total.fetch_add(1, Ordering::Relaxed);
                    tracing::info!("Circuit breaker: Open -> HalfOpen");
                }
            }
        }
    }

    /// Record a successful call.
    pub fn record_success(&self) {
        let mut inner = self.inner.lock().expect("circuit breaker mutex poisoned");
        inner.consecutive_failures = 0;

        match inner.state {
            State::HalfOpen => {
                inner.consecutive_successes += 1;
                if inner.consecutive_successes >= self.config.success_threshold_for_close {
                    inner.state = State::Closed;
                    inner.opened_at = None;
                    inner.consecutive_successes = 0;
                    self.state_changes_total.fetch_add(1, Ordering::Relaxed);
                    tracing::info!("Circuit breaker: HalfOpen -> Closed");
                }
            }
            State::Closed => {
                // Already healthy, nothing to do.
            }
            State::Open => {
                // Shouldn't happen (calls blocked), but handle gracefully.
            }
        }
    }

    /// Record a failed call.
    pub fn record_failure(&self) {
        let mut inner = self.inner.lock().expect("circuit breaker mutex poisoned");
        inner.consecutive_successes = 0;
        inner.consecutive_failures += 1;

        match inner.state {
            State::Closed => {
                if inner.consecutive_failures >= self.config.failure_threshold {
                    inner.state = State::Open;
                    inner.opened_at = Some(Instant::now());
                    self.trips_total.fetch_add(1, Ordering::Relaxed);
                    self.state_changes_total.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        failures = inner.consecutive_failures,
                        "Circuit breaker tripped: Closed -> Open"
                    );
                }
            }
            State::HalfOpen => {
                // Single failure in HalfOpen -> back to Open.
                inner.state = State::Open;
                inner.opened_at = Some(Instant::now());
                self.trips_total.fetch_add(1, Ordering::Relaxed);
                self.state_changes_total.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("Circuit breaker re-opened: HalfOpen -> Open");
            }
            State::Open => {
                // Already open, nothing to do.
            }
        }
    }

    /// Return the current number of consecutive failures.
    pub fn consecutive_failures(&self) -> u32 {
        let inner = self.inner.lock().expect("circuit breaker mutex poisoned");
        inner.consecutive_failures
    }

    /// Returns the number of times the circuit breaker has tripped from
    /// Closed to Open (or HalfOpen to Open).
    pub fn trip_count(&self) -> u64 {
        self.trips_total.load(Ordering::Relaxed)
    }

    /// Returns a snapshot of current circuit breaker metrics.
    pub fn metrics(&self) -> CircuitBreakerMetrics {
        let mut inner = self.inner.lock().expect("circuit breaker mutex poisoned");
        self.maybe_transition_to_half_open(&mut inner);
        CircuitBreakerMetrics {
            state: inner.state,
            total_trips: self.trips_total.load(Ordering::Relaxed),
            state_changes: self.state_changes_total.load(Ordering::Relaxed),
            consecutive_failures: inner.consecutive_failures,
            consecutive_successes: inner.consecutive_successes,
        }
    }

    /// Execute a synchronous operation through the circuit breaker.
    ///
    /// Returns `CircuitBreakerError::Open` immediately if the breaker is open.
    /// Otherwise, executes the closure and records success/failure.
    pub fn call_sync<F, T>(&self, f: F) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> anyhow::Result<T>,
    {
        self.allow_request()?;

        match f() {
            Ok(val) => {
                self.record_success();
                Ok(val)
            }
            Err(e) => {
                self.record_failure();
                Err(CircuitBreakerError::Inner(e))
            }
        }
    }

    /// Execute an async operation through the circuit breaker.
    ///
    /// Returns `CircuitBreakerError::Open` immediately if the breaker is open.
    /// Otherwise, executes the operation and records success/failure.
    pub async fn call<F, Fut, T>(&self, f: F) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        self.allow_request()?;

        match f().await {
            Ok(val) => {
                self.record_success();
                Ok(val)
            }
            Err(e) => {
                self.record_failure();
                Err(CircuitBreakerError::Inner(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_millis(50),
            success_threshold_for_close: 2,
        }
    }

    #[test]
    fn test_initial_state_closed() {
        let cb = CircuitBreaker::new(fast_config());
        assert_eq!(cb.state(), State::Closed);
        assert_eq!(cb.trip_count(), 0);
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn test_open_after_threshold_failures() {
        let cb = CircuitBreaker::new(fast_config());
        cb.record_failure();
        assert_eq!(cb.state(), State::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), State::Closed);
        cb.record_failure(); // threshold = 3
        assert_eq!(cb.state(), State::Open);
        assert_eq!(cb.trip_count(), 1);
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new(fast_config());
        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // resets to 0
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), State::Closed); // only 2 consecutive, not 3
    }

    #[test]
    fn test_half_open_after_recovery_timeout() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(10),
            success_threshold_for_close: 1,
        });
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), State::Open);

        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(cb.state(), State::HalfOpen);
        // allow_request should succeed in HalfOpen (trial call allowed)
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn test_close_after_success_threshold() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(10),
            success_threshold_for_close: 2,
        });
        cb.record_failure();
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(cb.state(), State::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), State::HalfOpen); // need 2 successes
        cb.record_success();
        assert_eq!(cb.state(), State::Closed);
    }

    #[test]
    fn test_reopen_on_half_open_failure() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(10),
            success_threshold_for_close: 2,
        });
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.trip_count(), 1);

        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(cb.state(), State::HalfOpen);

        cb.record_failure(); // back to Open
        assert_eq!(cb.state(), State::Open);
        assert_eq!(cb.trip_count(), 2);
    }

    #[test]
    fn test_trip_count() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(10),
            success_threshold_for_close: 1,
        });
        assert_eq!(cb.trip_count(), 0);

        // First trip: Closed -> Open
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.trip_count(), 1);

        // Recover: Open -> HalfOpen -> Closed
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(cb.state(), State::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), State::Closed);
        assert_eq!(cb.trip_count(), 1); // no new trip from recovery

        // Second trip: Closed -> Open
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.trip_count(), 2);

        // Third trip: Open -> HalfOpen -> Open (failure in HalfOpen)
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(cb.state(), State::HalfOpen);
        cb.record_failure();
        assert_eq!(cb.trip_count(), 3);
    }

    #[tokio::test]
    async fn test_call_success_through_breaker() {
        let cb = CircuitBreaker::new(fast_config());
        let result = cb.call(|| async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(cb.state(), State::Closed);
    }

    #[tokio::test]
    async fn test_call_failure_through_breaker() {
        let cb = CircuitBreaker::new(fast_config());
        for _ in 0..3 {
            let _ = cb
                .call(|| async { Err::<i32, _>(anyhow::anyhow!("rpc error")) })
                .await;
        }
        assert_eq!(cb.state(), State::Open);

        // Next call should be rejected immediately
        let err = cb
            .call(|| async { Ok::<_, anyhow::Error>(1) })
            .await
            .unwrap_err();
        assert!(matches!(err, CircuitBreakerError::Open { .. }));
    }

    #[test]
    fn test_default_config() {
        let cfg = CircuitBreakerConfig::default();
        assert_eq!(cfg.failure_threshold, 5);
        assert_eq!(cfg.recovery_timeout, Duration::from_secs(30));
        assert_eq!(cfg.success_threshold_for_close, 2);
    }

    #[test]
    fn test_consecutive_failures_accessor() {
        let cb = CircuitBreaker::new(fast_config());
        assert_eq!(cb.consecutive_failures(), 0);

        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 1);

        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_call_sync_success() {
        let cb = CircuitBreaker::new(fast_config());
        let result = cb.call_sync(|| Ok(99));
        assert_eq!(result.unwrap(), 99);
        assert_eq!(cb.state(), State::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_call_sync_failure_trips_breaker() {
        let cb = CircuitBreaker::new(fast_config()); // threshold = 3
        for _ in 0..3 {
            let _ = cb.call_sync(|| Err::<i32, _>(anyhow::anyhow!("sync fail")));
        }
        assert_eq!(cb.state(), State::Open);
        assert_eq!(cb.consecutive_failures(), 3);

        // Subsequent sync call should be rejected
        let err = cb.call_sync(|| Ok::<_, anyhow::Error>(1)).unwrap_err();
        assert!(matches!(err, CircuitBreakerError::Open { .. }));
    }

    #[test]
    fn test_metrics_tracks_state_changes() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            success_threshold_for_close: 1,
        });

        // Initial: no changes
        let m = cb.metrics();
        assert_eq!(m.state, State::Closed);
        assert_eq!(m.total_trips, 0);
        assert_eq!(m.state_changes, 0);
        assert_eq!(m.consecutive_failures, 0);
        assert_eq!(m.consecutive_successes, 0);

        // Trip: Closed -> Open (1 state change, 1 trip)
        cb.record_failure();
        let m = cb.metrics();
        assert_eq!(m.state, State::Open);
        assert_eq!(m.total_trips, 1);
        assert_eq!(m.state_changes, 1);

        // Wait for timeout: Open -> HalfOpen (2 state changes)
        std::thread::sleep(Duration::from_millis(15));
        let m = cb.metrics();
        assert_eq!(m.state, State::HalfOpen);
        assert_eq!(m.state_changes, 2);

        // Succeed: HalfOpen -> Closed (3 state changes)
        cb.record_success();
        let m = cb.metrics();
        assert_eq!(m.state, State::Closed);
        assert_eq!(m.state_changes, 3);
        assert_eq!(m.total_trips, 1); // trips unchanged by recovery
    }

    #[test]
    fn test_state_display() {
        assert_eq!(State::Closed.to_string(), "Closed");
        assert_eq!(State::Open.to_string(), "Open");
        assert_eq!(State::HalfOpen.to_string(), "HalfOpen");
    }

    #[test]
    fn test_error_display_messages() {
        let open_err = CircuitBreakerError::Open {
            remaining_secs: 15.3,
        };
        let msg = open_err.to_string();
        assert!(msg.contains("circuit breaker is open"));
        assert!(msg.contains("15.3s"));

        let inner_err = CircuitBreakerError::Inner(anyhow::anyhow!("connection refused"));
        let msg = inner_err.to_string();
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn test_thread_safety() {
        use std::sync::Arc;

        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1000,
            recovery_timeout: Duration::from_secs(60),
            success_threshold_for_close: 2,
        }));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let cb_clone = cb.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    let _ = cb_clone.call_sync(|| Ok::<_, anyhow::Error>(1));
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        // 400 successful calls across 8 threads, breaker should remain closed
        assert_eq!(cb.state(), State::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }
}
