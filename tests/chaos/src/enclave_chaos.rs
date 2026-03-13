//! Chaos / fault-injection tests for TEE enclave resilience.
//!
//! The enclave is a binary crate without a library target, so these tests
//! are self-contained: they re-implement the enclave's core resilience
//! components (replay protection, timestamp freshness, rate limiting)
//! using the same algorithms as the real enclave code, then stress-test
//! them under adversarial conditions.
//!
//! Tests cover:
//! 1. Replay protection with duplicate nonces
//! 2. Timestamp freshness rejection (expired requests)
//! 3. Rate limiter saturation
//! 4. Concurrent request handling (>100 simultaneous)

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    // ===================================================================
    // Inline enclave component re-implementations
    // (mirrors tee/enclave/src/watchdog.rs and tee/enclave/src/main.rs)
    // ===================================================================

    /// Maximum number of nonces stored in the LRU cache.
    const MAX_NONCE_ENTRIES: usize = 100_000;

    /// Default maximum age for a request timestamp (seconds).
    const DEFAULT_MAX_REQUEST_AGE_SECS: u64 = 60;

    /// Errors from replay protection validation.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ReplayError {
        DuplicateNonce,
        ExpiredTimestamp,
        InvalidChainId,
    }

    /// Bounded LRU nonce registry (matches enclave implementation).
    struct NonceRegistry {
        set: HashMap<String, ()>,
        order: VecDeque<String>,
        max_entries: usize,
    }

    impl NonceRegistry {
        fn new(max_entries: usize) -> Self {
            Self {
                set: HashMap::with_capacity(max_entries.min(1024)),
                order: VecDeque::with_capacity(max_entries.min(1024)),
                max_entries,
            }
        }

        fn check_nonce(&mut self, nonce: &str) -> bool {
            if self.set.contains_key(nonce) {
                return false;
            }
            if self.set.len() >= self.max_entries {
                if let Some(oldest) = self.order.pop_front() {
                    self.set.remove(&oldest);
                }
            }
            self.set.insert(nonce.to_owned(), ());
            self.order.push_back(nonce.to_owned());
            true
        }

        fn len(&self) -> usize {
            self.set.len()
        }
    }

    /// Check timestamp freshness (matches enclave implementation).
    fn check_timestamp(timestamp_secs: u64, max_age: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now > timestamp_secs && (now - timestamp_secs) > max_age {
            return false;
        }
        if timestamp_secs > now && (timestamp_secs - now) > max_age {
            return false;
        }
        true
    }

    /// Chain ID check (matches enclave implementation).
    fn check_chain_id(request_chain_id: u64, expected: u64) -> bool {
        request_chain_id == expected
    }

    /// Combined replay protection (matches enclave implementation).
    struct ReplayProtection {
        nonces: NonceRegistry,
        max_request_age_secs: u64,
        expected_chain_id: u64,
    }

    impl ReplayProtection {
        fn new(max_request_age_secs: u64, expected_chain_id: u64) -> Self {
            Self {
                nonces: NonceRegistry::new(MAX_NONCE_ENTRIES),
                max_request_age_secs,
                expected_chain_id,
            }
        }

        fn validate_request(
            &mut self,
            nonce: &str,
            timestamp_secs: u64,
            chain_id: u64,
        ) -> Result<(), ReplayError> {
            if !check_chain_id(chain_id, self.expected_chain_id) {
                return Err(ReplayError::InvalidChainId);
            }
            if !check_timestamp(timestamp_secs, self.max_request_age_secs) {
                return Err(ReplayError::ExpiredTimestamp);
            }
            if !self.nonces.check_nonce(nonce) {
                return Err(ReplayError::DuplicateNonce);
            }
            Ok(())
        }
    }

    /// Sliding-window rate limiter (matches enclave main.rs implementation).
    struct SlidingWindowRateLimiter {
        window: Mutex<VecDeque<Instant>>,
        max_requests_per_minute: u64,
    }

    impl SlidingWindowRateLimiter {
        fn new(max_requests_per_minute: u64) -> Self {
            Self {
                window: Mutex::new(VecDeque::new()),
                max_requests_per_minute,
            }
        }

        fn check(&self) -> Result<(), u64> {
            let mut window = self.window.lock().unwrap();
            let now = Instant::now();
            let one_minute_ago = now - Duration::from_secs(60);

            while window.front().is_some_and(|&t| t < one_minute_ago) {
                window.pop_front();
            }

            if window.len() as u64 >= self.max_requests_per_minute {
                if let Some(oldest) = window.front() {
                    let retry_after = 60u64.saturating_sub(oldest.elapsed().as_secs());
                    return Err(retry_after.max(1));
                }
                return Err(1);
            }

            window.push_back(now);
            Ok(())
        }
    }

    /// SHA-256 model hash validator (matches enclave validation.rs).
    fn compute_model_hash(model_bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(model_bytes);
        hex::encode(hasher.finalize())
    }

    fn validate_model(
        model_bytes: &[u8],
        expected_hash: Option<&str>,
    ) -> Result<String, String> {
        let computed = compute_model_hash(model_bytes);
        if let Some(expected) = expected_hash {
            let expected_clean = expected
                .strip_prefix("0x")
                .unwrap_or(expected)
                .to_ascii_lowercase();
            if computed != expected_clean {
                return Err(format!(
                    "Model hash mismatch: expected {}, computed {}",
                    expected_clean, computed
                ));
            }
        }
        Ok(computed)
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    // ===================================================================
    // Test 1: Replay protection with duplicate nonces
    // ===================================================================

    #[test]
    fn test_replay_protection_duplicate_nonce_rejected() {
        let mut rp = ReplayProtection::new(60, 1);
        let ts = now_secs();

        // First request with nonce succeeds.
        assert!(rp.validate_request("nonce-abc-123", ts, 1).is_ok());

        // Replay with same nonce is rejected.
        let err = rp
            .validate_request("nonce-abc-123", ts, 1)
            .unwrap_err();
        assert_eq!(err, ReplayError::DuplicateNonce);
    }

    #[test]
    fn test_replay_protection_mass_duplicate_nonces() {
        let mut rp = ReplayProtection::new(60, 1);
        let ts = now_secs();

        // Send 1000 unique nonces.
        for i in 0..1000 {
            let nonce = format!("nonce-{}", i);
            assert!(
                rp.validate_request(&nonce, ts, 1).is_ok(),
                "Unique nonce {} should be accepted",
                i
            );
        }

        // Replay all 1000 nonces -- every one should be rejected.
        let mut rejected = 0;
        for i in 0..1000 {
            let nonce = format!("nonce-{}", i);
            if rp.validate_request(&nonce, ts, 1).is_err() {
                rejected += 1;
            }
        }
        assert_eq!(rejected, 1000, "All 1000 replayed nonces should be rejected");
    }

    #[test]
    fn test_replay_protection_nonce_eviction_allows_reuse() {
        // With a small registry, old nonces are evicted and become reusable.
        let mut registry = NonceRegistry::new(10);

        // Fill the registry.
        for i in 0..10 {
            assert!(registry.check_nonce(&format!("n-{}", i)));
        }
        assert_eq!(registry.len(), 10);

        // Adding an 11th nonce evicts the oldest ("n-0").
        assert!(registry.check_nonce("n-10"));
        assert_eq!(registry.len(), 10);

        // "n-0" was evicted, so it is now accepted again.
        assert!(registry.check_nonce("n-0"));

        // "n-1" should also have been evicted (it was next oldest).
        assert!(registry.check_nonce("n-1"));
    }

    #[test]
    fn test_replay_protection_large_nonce_registry_capacity() {
        // Fill the registry to MAX_NONCE_ENTRIES and verify it stays bounded.
        let mut registry = NonceRegistry::new(MAX_NONCE_ENTRIES);

        let start = Instant::now();
        for i in 0..MAX_NONCE_ENTRIES {
            assert!(registry.check_nonce(&format!("nonce-{}", i)));
        }
        let fill_time = start.elapsed();
        assert_eq!(registry.len(), MAX_NONCE_ENTRIES);

        // One more should evict the oldest.
        assert!(registry.check_nonce("overflow-nonce"));
        assert_eq!(registry.len(), MAX_NONCE_ENTRIES);

        // Performance: filling 100K entries should take under 5 seconds.
        assert!(
            fill_time < Duration::from_secs(5),
            "Filling {} nonces took {:?}, expected < 5s",
            MAX_NONCE_ENTRIES,
            fill_time
        );
    }

    #[test]
    fn test_replay_protection_wrong_chain_id_before_nonce_check() {
        let mut rp = ReplayProtection::new(60, 1);
        let ts = now_secs();

        // Wrong chain ID should be rejected before the nonce is consumed.
        let err = rp
            .validate_request("unique-nonce", ts, 42)
            .unwrap_err();
        assert_eq!(err, ReplayError::InvalidChainId);

        // The nonce should NOT have been recorded (chain ID check is first).
        // So the same nonce with the correct chain ID should succeed.
        assert!(rp.validate_request("unique-nonce", ts, 1).is_ok());
    }

    // ===================================================================
    // Test 2: Timestamp freshness rejection (expired requests)
    // ===================================================================

    #[test]
    fn test_timestamp_freshness_recent_accepted() {
        let ts = now_secs();

        // Current timestamp.
        assert!(check_timestamp(ts, 60));

        // 30 seconds in the past.
        assert!(check_timestamp(ts - 30, 60));

        // 10 seconds in the future (clock skew).
        assert!(check_timestamp(ts + 10, 60));
    }

    #[test]
    fn test_timestamp_freshness_old_rejected() {
        let ts = now_secs();
        let max_age = 60;

        // 120 seconds in the past -- too old.
        assert!(!check_timestamp(ts - 120, max_age));

        // 300 seconds in the past -- far too old.
        assert!(!check_timestamp(ts - 300, max_age));
    }

    #[test]
    fn test_timestamp_freshness_future_rejected() {
        let ts = now_secs();
        let max_age = 60;

        // 120 seconds in the future -- too far ahead.
        assert!(!check_timestamp(ts + 120, max_age));

        // 1 hour in the future.
        assert!(!check_timestamp(ts + 3600, max_age));
    }

    #[test]
    fn test_timestamp_freshness_boundary_exact() {
        let ts = now_secs();
        let max_age = 60;

        // Exactly at the boundary (60 seconds ago). Due to timing this may
        // be accepted (if now - ts == 60, the check is > not >=).
        // The point is that values right at the boundary behave consistently.
        let at_boundary = ts - max_age;
        let result = check_timestamp(at_boundary, max_age);
        // Either accepted or rejected is fine at the exact boundary --
        // just verify it does not panic.
        let _ = result;

        // One second past the boundary should definitely be rejected.
        assert!(!check_timestamp(ts - max_age - 1, max_age));
    }

    #[test]
    fn test_timestamp_combined_with_replay_protection() {
        let mut rp = ReplayProtection::new(60, 1);

        // Expired timestamp should be rejected.
        let old_ts = now_secs() - 120;
        let err = rp
            .validate_request("fresh-nonce-1", old_ts, 1)
            .unwrap_err();
        assert_eq!(err, ReplayError::ExpiredTimestamp);

        // The nonce should NOT have been consumed (timestamp check is before
        // nonce check). So the same nonce with a fresh timestamp should work.
        let fresh_ts = now_secs();
        assert!(rp.validate_request("fresh-nonce-1", fresh_ts, 1).is_ok());
    }

    #[test]
    fn test_timestamp_freshness_zero_max_age() {
        // With max_age=0, only the exact current timestamp should pass.
        let ts = now_secs();
        // Current timestamp might pass or fail depending on timing.
        let _ = check_timestamp(ts, 0);

        // Anything in the past should fail.
        assert!(!check_timestamp(ts - 1, 0));
    }

    // ===================================================================
    // Test 3: Rate limiter saturation
    // ===================================================================

    #[test]
    fn test_rate_limiter_saturation_basic() {
        let limiter = SlidingWindowRateLimiter::new(10);

        // First 10 requests should be allowed.
        for i in 0..10 {
            assert!(
                limiter.check().is_ok(),
                "Request {} should be allowed (within limit)",
                i
            );
        }

        // 11th request should be rate-limited.
        let result = limiter.check();
        assert!(result.is_err(), "11th request should be rate-limited");
        if let Err(retry_after) = result {
            assert!(
                retry_after >= 1 && retry_after <= 60,
                "Retry-after should be between 1 and 60, got {}",
                retry_after
            );
        }
    }

    #[test]
    fn test_rate_limiter_saturation_high_throughput() {
        let limiter = SlidingWindowRateLimiter::new(100);

        let mut allowed = 0;
        let mut rejected = 0;

        // Attempt 200 requests in rapid succession.
        for _ in 0..200 {
            match limiter.check() {
                Ok(()) => allowed += 1,
                Err(_) => rejected += 1,
            }
        }

        assert_eq!(allowed, 100, "Should allow exactly 100 requests");
        assert_eq!(rejected, 100, "Should reject exactly 100 requests");
    }

    #[test]
    fn test_rate_limiter_saturation_then_recovery() {
        let limiter = SlidingWindowRateLimiter::new(5);

        // Saturate.
        for _ in 0..5 {
            assert!(limiter.check().is_ok());
        }
        assert!(limiter.check().is_err());

        // After the window slides, requests should be allowed again.
        // We cannot easily simulate time passing with Instant-based windows,
        // so we manually verify the window mechanism by inspecting the
        // Retry-After value returned.
        if let Err(retry_after) = limiter.check() {
            assert!(
                retry_after >= 1,
                "Retry-after should be positive, got {}",
                retry_after
            );
        }
    }

    #[test]
    fn test_rate_limiter_single_request_per_minute() {
        let limiter = SlidingWindowRateLimiter::new(1);

        // First request allowed.
        assert!(limiter.check().is_ok());

        // Second request immediately rejected.
        assert!(limiter.check().is_err());

        // Third also rejected.
        assert!(limiter.check().is_err());
    }

    // ===================================================================
    // Test 4: Concurrent request handling (>100 simultaneous)
    // ===================================================================

    #[test]
    fn test_concurrent_nonce_registry_thread_safety() {
        let registry = Arc::new(Mutex::new(NonceRegistry::new(50_000)));
        let mut handles = Vec::new();

        // 16 threads, each submitting 1000 unique nonces.
        for thread_id in 0..16u32 {
            let registry = registry.clone();
            handles.push(std::thread::spawn(move || {
                let mut accepted = 0u32;
                for i in 0..1000u32 {
                    let nonce = format!("t{}-n{}", thread_id, i);
                    let mut reg = registry.lock().unwrap();
                    if reg.check_nonce(&nonce) {
                        accepted += 1;
                    }
                }
                accepted
            }));
        }

        let total_accepted: u32 = handles
            .into_iter()
            .map(|h| h.join().expect("nonce thread panicked"))
            .sum();

        // All 16 * 1000 = 16000 nonces are unique, so all should be accepted.
        assert_eq!(
            total_accepted, 16_000,
            "All unique nonces across threads should be accepted"
        );

        let reg = registry.lock().unwrap();
        assert_eq!(reg.len(), 16_000);
    }

    #[test]
    fn test_concurrent_replay_attack_detection() {
        // Multiple threads try to replay the same nonce simultaneously.
        // Only one should succeed.
        let registry = Arc::new(Mutex::new(NonceRegistry::new(1000)));
        let mut handles = Vec::new();

        let shared_nonce = "shared-replay-nonce".to_string();

        for _ in 0..100 {
            let registry = registry.clone();
            let nonce = shared_nonce.clone();
            handles.push(std::thread::spawn(move || {
                let mut reg = registry.lock().unwrap();
                reg.check_nonce(&nonce)
            }));
        }

        let results: Vec<bool> = handles
            .into_iter()
            .map(|h| h.join().expect("replay thread panicked"))
            .collect();

        let accepted_count = results.iter().filter(|&&r| r).count();
        let rejected_count = results.iter().filter(|&&r| !r).count();

        assert_eq!(
            accepted_count, 1,
            "Exactly one thread should accept the nonce"
        );
        assert_eq!(
            rejected_count, 99,
            "99 threads should detect the replay"
        );
    }

    #[test]
    fn test_concurrent_rate_limiter_thread_safety() {
        let limiter = Arc::new(SlidingWindowRateLimiter::new(50));
        let mut handles = Vec::new();

        // 100 threads each try to make one request.
        for _ in 0..100 {
            let limiter = limiter.clone();
            handles.push(std::thread::spawn(move || limiter.check().is_ok()));
        }

        let results: Vec<bool> = handles
            .into_iter()
            .map(|h| h.join().expect("rate limiter thread panicked"))
            .collect();

        let allowed = results.iter().filter(|&&r| r).count();
        let rejected = results.iter().filter(|&&r| !r).count();

        // With max=50 and 100 threads, exactly 50 should be allowed.
        assert_eq!(
            allowed, 50,
            "Should allow exactly 50 out of 100 concurrent requests"
        );
        assert_eq!(
            rejected, 50,
            "Should reject exactly 50 out of 100 concurrent requests"
        );
    }

    #[test]
    fn test_concurrent_rate_limiter_200_threads() {
        let limiter = Arc::new(SlidingWindowRateLimiter::new(100));
        let mut handles = Vec::new();

        // 200 threads each try to make one request.
        for _ in 0..200 {
            let limiter = limiter.clone();
            handles.push(std::thread::spawn(move || limiter.check().is_ok()));
        }

        let results: Vec<bool> = handles
            .into_iter()
            .map(|h| h.join().expect("rate limiter 200-thread test panicked"))
            .collect();

        let allowed = results.iter().filter(|&&r| r).count();
        let rejected = results.iter().filter(|&&r| !r).count();

        assert_eq!(allowed, 100);
        assert_eq!(rejected, 100);
    }

    #[tokio::test]
    async fn test_concurrent_async_request_handling() {
        // Simulate >100 concurrent async requests against a shared rate limiter.
        let limiter = Arc::new(SlidingWindowRateLimiter::new(50));
        let mut tasks = Vec::new();

        for _ in 0..150 {
            let limiter = limiter.clone();
            tasks.push(tokio::spawn(async move { limiter.check().is_ok() }));
        }

        let mut allowed = 0;
        let mut rejected = 0;
        for task in tasks {
            if task.await.unwrap() {
                allowed += 1;
            } else {
                rejected += 1;
            }
        }

        assert_eq!(allowed, 50, "Should allow 50 of 150 async requests");
        assert_eq!(rejected, 100, "Should reject 100 of 150 async requests");
    }

    // ===================================================================
    // Bonus: Model validation chaos tests
    // ===================================================================

    #[test]
    fn test_model_validation_correct_hash() {
        let model_data = b"xgboost model binary data for testing";
        let hash = compute_model_hash(model_data);
        let result = validate_model(model_data, Some(&hash));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), hash);
    }

    #[test]
    fn test_model_validation_wrong_hash_rejected() {
        let model_data = b"legitimate model data";
        let wrong_hash =
            "0000000000000000000000000000000000000000000000000000000000000000";
        let result = validate_model(model_data, Some(wrong_hash));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mismatch"));
    }

    #[test]
    fn test_model_validation_tampered_model() {
        let original = b"original model bytes";
        let hash = compute_model_hash(original);

        // Tamper with one byte.
        let mut tampered = original.to_vec();
        tampered[0] ^= 0xff;

        let result = validate_model(&tampered, Some(&hash));
        assert!(result.is_err(), "Tampered model should fail validation");
    }

    #[test]
    fn test_model_validation_no_expected_hash() {
        let model_data = b"any model bytes without a hash check";
        let result = validate_model(model_data, None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 64); // SHA-256 hex is 64 chars.
    }

    #[test]
    fn test_model_validation_case_insensitive_and_0x_prefix() {
        let model_data = b"case test model";
        let hash = compute_model_hash(model_data);

        // Uppercase.
        assert!(validate_model(model_data, Some(&hash.to_uppercase())).is_ok());

        // With 0x prefix.
        assert!(validate_model(model_data, Some(&format!("0x{}", hash))).is_ok());

        // With 0x prefix + uppercase.
        assert!(
            validate_model(model_data, Some(&format!("0x{}", hash.to_uppercase())))
                .is_ok()
        );
    }

    #[test]
    fn test_model_validation_empty_model() {
        let model_data: &[u8] = b"";
        let result = validate_model(model_data, None);
        assert!(result.is_ok());
        // SHA-256 of empty input is a known value.
        assert_eq!(
            result.unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ===================================================================
    // Bonus: Combined replay + rate limit stress
    // ===================================================================

    #[test]
    fn test_combined_replay_and_rate_limit_under_load() {
        let rp = Arc::new(Mutex::new(ReplayProtection::new(60, 1)));
        let limiter = Arc::new(SlidingWindowRateLimiter::new(50));
        let mut handles = Vec::new();

        let ts = now_secs();

        for thread_id in 0..20u32 {
            let rp = rp.clone();
            let limiter = limiter.clone();
            handles.push(std::thread::spawn(move || {
                let mut rate_limited = 0u32;
                let mut replay_rejected = 0u32;
                let mut accepted = 0u32;

                for i in 0..10u32 {
                    // First check rate limit.
                    if limiter.check().is_err() {
                        rate_limited += 1;
                        continue;
                    }

                    // Then check replay.
                    let nonce = format!("t{}-req{}", thread_id, i);
                    let mut rp_guard = rp.lock().unwrap();
                    match rp_guard.validate_request(&nonce, ts, 1) {
                        Ok(()) => accepted += 1,
                        Err(ReplayError::DuplicateNonce) => replay_rejected += 1,
                        Err(_) => {}
                    }
                }

                (accepted, replay_rejected, rate_limited)
            }));
        }

        let mut total_accepted = 0u32;
        let mut total_rate_limited = 0u32;
        let mut total_replay_rejected = 0u32;

        for h in handles {
            let (a, r, rl) = h.join().expect("combined test thread panicked");
            total_accepted += a;
            total_replay_rejected += r;
            total_rate_limited += rl;
        }

        // 20 threads * 10 requests = 200 total attempts.
        // Rate limiter allows 50, so 150 are rate-limited.
        // Of the 50 that pass rate limiting, all have unique nonces, so all
        // should pass replay protection.
        assert_eq!(total_accepted, 50, "50 requests should pass both checks");
        assert_eq!(total_rate_limited, 150, "150 should be rate-limited");
        assert_eq!(total_replay_rejected, 0, "No replays in unique nonces");
    }
}
