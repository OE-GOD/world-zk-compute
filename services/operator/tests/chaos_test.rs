//! Chaos tests for operator resilience under failure conditions.
//!
//! Tests verify graceful degradation — no panics, proper error handling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tee_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, State};
use tee_operator::middleware::{RateLimitConfig, RateLimiter};
use tee_operator::{prune_and_evict, prune_old_disputes, PruneConfig};

// ---------------------------------------------------------------------------
// Circuit breaker chaos tests
// ---------------------------------------------------------------------------

#[test]
fn chaos_circuit_breaker_rapid_failures() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        recovery_timeout: Duration::from_millis(100),
        success_threshold_for_close: 1,
    };
    let cb = CircuitBreaker::new(config);

    // Rapid consecutive failures should trip breaker
    for _ in 0..10 {
        cb.record_failure();
    }
    assert_eq!(cb.state(), State::Open);
    assert!(cb.allow_request().is_err());
}

#[test]
fn chaos_circuit_breaker_rapid_state_transitions() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        recovery_timeout: Duration::from_millis(10),
        success_threshold_for_close: 1,
    };
    let cb = CircuitBreaker::new(config);

    // Cycle through states rapidly
    for _ in 0..20 {
        cb.record_failure(); // -> Open
        std::thread::sleep(Duration::from_millis(15)); // Wait for recovery
        assert!(cb.allow_request().is_ok()); // -> HalfOpen
        cb.record_success(); // -> Closed
        assert_eq!(cb.state(), State::Closed);
    }
    // Should not panic or enter inconsistent state
    assert_eq!(cb.state(), State::Closed);
}

#[test]
fn chaos_circuit_breaker_concurrent_access() {
    let config = CircuitBreakerConfig {
        failure_threshold: 5,
        recovery_timeout: Duration::from_secs(1),
        success_threshold_for_close: 2,
    };
    let cb = Arc::new(CircuitBreaker::new(config));

    let handles: Vec<_> = (0..20)
        .map(|i| {
            let cb = cb.clone();
            std::thread::spawn(move || {
                for _ in 0..100 {
                    let _ = cb.allow_request();
                    if i % 2 == 0 {
                        cb.record_failure();
                    } else {
                        cb.record_success();
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Should be in a valid state (not crashed)
    let state = cb.state();
    assert!(
        state == State::Closed || state == State::Open || state == State::HalfOpen,
        "Invalid state after concurrent access"
    );
}

// ---------------------------------------------------------------------------
// Rate limiter chaos tests
// ---------------------------------------------------------------------------

#[test]
fn chaos_rate_limiter_burst_many_ips() {
    let limiter = RateLimiter::new(RateLimitConfig {
        max_requests_per_minute: 60,
        max_burst: 5,
        max_ips: 100,
    });

    // Simulate 200 different IPs (exceeds max_ips)
    for i in 0..200 {
        let ip = format!("10.0.{}.{}", i / 256, i % 256);
        let _ = limiter.check(&ip);
    }

    // Should not crash, and tracked IPs should be bounded
    assert!(limiter.tracked_ips() <= 100);
}

#[test]
fn chaos_rate_limiter_same_ip_flood() {
    let limiter = RateLimiter::new(RateLimitConfig {
        max_requests_per_minute: 60,
        max_burst: 5,
        max_ips: 100,
    });

    // Flood from single IP
    let mut limited = 0u64;
    for _ in 0..1000 {
        if limiter.check("10.0.0.1").is_err() {
            limited += 1;
        }
    }

    // Most requests should be rate-limited (only first 5 allowed as burst)
    assert!(
        limited >= 990,
        "Expected most requests to be limited, got {limited}"
    );
    assert_eq!(limiter.total_rate_limited(), limited);
}

#[test]
fn chaos_rate_limiter_concurrent_flood() {
    let limiter = Arc::new(RateLimiter::new(RateLimitConfig {
        max_requests_per_minute: 60,
        max_burst: 10,
        max_ips: 1000,
    }));

    let handles: Vec<_> = (0..10)
        .map(|thread_id| {
            let limiter = limiter.clone();
            std::thread::spawn(move || {
                for i in 0..100 {
                    let ip = format!("10.{}.{}.{}", thread_id, i / 256, i % 256);
                    let _ = limiter.check(&ip);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Should be bounded and not panic
    assert!(limiter.tracked_ips() <= 1000);
}

// ---------------------------------------------------------------------------
// Dispute pruning chaos tests
// ---------------------------------------------------------------------------

#[test]
fn chaos_pruning_large_dataset() {
    let mut disputes = HashMap::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Insert 100K disputes with varying ages
    for i in 0..100_000u64 {
        let age = i * 60; // Each dispute 1 minute apart
        disputes.insert(format!("dispute-{i}"), now - age);
    }

    let config = PruneConfig {
        max_dispute_age_secs: 86400, // 1 day
        max_disputes: 1000,
    };

    let start = Instant::now();
    let (pruned, evicted) = prune_and_evict(&mut disputes, &config);
    let elapsed = start.elapsed();

    // Should handle 100K entries without excessive time
    assert!(
        elapsed < Duration::from_secs(5),
        "Pruning took too long: {:?}",
        elapsed
    );
    assert!(pruned > 0, "Should have pruned old disputes");
    assert_eq!(
        disputes.len(),
        1000,
        "Should be at max_disputes after eviction"
    );
    assert!(pruned + evicted > 0, "Combined should remove entries");
}

#[test]
fn chaos_pruning_all_expired() {
    let mut disputes = HashMap::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // All disputes are 90 days old
    for i in 0..1000u64 {
        disputes.insert(format!("old-{i}"), now - 90 * 86400);
    }

    let config = PruneConfig::default(); // 30 day cutoff
    let pruned = prune_old_disputes(&mut disputes, &config);

    assert_eq!(pruned, 1000);
    assert!(disputes.is_empty());
}

#[test]
fn chaos_pruning_empty_map() {
    let mut disputes = HashMap::new();
    let config = PruneConfig::default();
    let (pruned, evicted) = prune_and_evict(&mut disputes, &config);
    assert_eq!(pruned, 0);
    assert_eq!(evicted, 0);
}

// ---------------------------------------------------------------------------
// Edge case tests
// ---------------------------------------------------------------------------

#[test]
fn chaos_zero_burst_rate_limiter() {
    let limiter = RateLimiter::new(RateLimitConfig {
        max_requests_per_minute: 0,
        max_burst: 0,
        max_ips: 100,
    });

    // All requests should be rate-limited with zero burst
    for _ in 0..10 {
        assert!(limiter.check("any-ip").is_err());
    }
}

#[test]
fn chaos_circuit_breaker_zero_threshold() {
    let config = CircuitBreakerConfig {
        failure_threshold: 0,
        recovery_timeout: Duration::from_millis(10),
        success_threshold_for_close: 0,
    };
    let cb = CircuitBreaker::new(config);

    // With 0 threshold, any failure should trip
    cb.record_failure();
    assert_eq!(cb.state(), State::Open);
}
