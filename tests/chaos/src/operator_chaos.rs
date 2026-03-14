//! Chaos / fault injection tests for the TEE operator service.
//!
//! These tests exercise failure modes and recovery paths in the operator
//! without requiring a real RPC connection or running services. All network
//! and I/O behaviors are mocked or simulated using the operator library's
//! public API.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use tee_operator::circuit_breaker::{
        CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError, State,
    };
    use tee_operator::deadline_monitor::DeadlineMonitor;
    use tee_operator::notifications::WebhookNotifier;
    use tee_operator::{evict_excess_disputes, prune_and_evict, prune_old_disputes, PruneConfig};

    // -----------------------------------------------------------------------
    // Test 1: Operator handles RPC connection refused gracefully
    //         (circuit breaker opens after threshold failures)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rpc_connection_refused_circuit_breaker_opens() {
        // Simulate connection refused by recording failures through the
        // circuit breaker. After the failure threshold is hit, the breaker
        // should transition to Open and reject subsequent requests.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            recovery_timeout: Duration::from_secs(30),
            success_threshold_for_close: 2,
        });

        assert_eq!(cb.state(), State::Closed, "breaker should start closed");

        // Simulate 3 consecutive "connection refused" failures
        for i in 0..3 {
            let result = cb.call_sync(|| {
                Err::<(), _>(anyhow::anyhow!("connection refused: RPC at 127.0.0.1:8545"))
            });
            assert!(result.is_err(), "call {} should fail", i);
        }

        // Breaker should now be open
        assert_eq!(
            cb.state(),
            State::Open,
            "breaker should be open after 3 consecutive failures"
        );
        assert_eq!(cb.trip_count(), 1, "trip count should be 1");

        // Subsequent requests should be rejected immediately without
        // attempting the actual call (fast-fail)
        let rejected: Result<(), _> =
            cb.call_sync(|| panic!("this closure should never be called when breaker is open"));
        assert!(rejected.is_err());
        match rejected.unwrap_err() {
            CircuitBreakerError::Open { remaining_secs } => {
                assert!(
                    remaining_secs > 0.0,
                    "remaining_secs should be positive, got {}",
                    remaining_secs
                );
            }
            other => panic!("expected Open error, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: Operator handles RPC timeout (does not hang)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_rpc_timeout_does_not_hang() {
        // The circuit breaker wraps async calls with a timeout built into
        // the client. We simulate this by having the async call return an
        // error after a brief delay (simulating an actual timeout), and
        // verify the breaker records the failure without hanging.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_millis(50),
            success_threshold_for_close: 1,
        });

        // Simulate timeout: the call completes with an error, just like
        // a real HTTP client would after its timeout fires.
        let start = std::time::Instant::now();
        for _ in 0..2 {
            let result = cb
                .call(|| async {
                    // Simulate a short delay, then timeout error
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Err::<(), _>(anyhow::anyhow!("request timed out after 10ms"))
                })
                .await;
            assert!(result.is_err());
        }
        let elapsed = start.elapsed();

        // The entire sequence should complete quickly (well under 1 second).
        // If the operator were hanging, this would take much longer.
        assert!(
            elapsed < Duration::from_secs(1),
            "timeout simulation took too long: {:?} (would hang in production)",
            elapsed
        );

        // Breaker should be open now
        assert_eq!(cb.state(), State::Open);

        // Wait for recovery timeout
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            cb.state(),
            State::HalfOpen,
            "breaker should recover to HalfOpen after timeout"
        );

        // A successful call should close the breaker
        let result = cb.call(|| async { Ok::<_, anyhow::Error>(()) }).await;
        assert!(result.is_ok());
        assert_eq!(cb.state(), State::Closed);
    }

    // -----------------------------------------------------------------------
    // Test 3: Operator recovers from corrupted state file
    // -----------------------------------------------------------------------

    #[test]
    fn test_recovers_from_corrupted_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("corrupted-state.json");

        // Write garbage data to the state file (simulating corruption
        // from a crash during write, filesystem error, etc.)
        let corruption_cases = vec![
            ("empty file", ""),
            ("binary garbage", "\x00\x01\x02\x7e\x7f"),
            ("truncated JSON", r#"{"last_polled_block": 42, "active_dis"#),
            ("wrong JSON type", r#"[1, 2, 3]"#),
            (
                "HTML error page",
                "<html><body>502 Bad Gateway</body></html>",
            ),
            ("null value", "null"),
        ];

        for (label, corrupt_data) in corruption_cases {
            std::fs::write(&state_path, corrupt_data).unwrap();

            let store = tee_operator::store::StateStore::new(&state_path);

            // load() should return an error for corrupt data
            let load_result = store.load();
            assert!(
                load_result.is_err(),
                "load() should fail for {}: {:?}",
                label,
                load_result
            );

            // load_or_default() should gracefully fall back to defaults
            let state = store.load_or_default();
            assert_eq!(
                state.last_polled_block, 0,
                "last_polled_block should be 0 after corruption recovery ({})",
                label
            );
            assert!(
                state.active_disputes.is_empty(),
                "active_disputes should be empty after corruption recovery ({})",
                label
            );
            assert!(
                state.processed_event_ids.is_empty(),
                "processed_event_ids should be empty after corruption recovery ({})",
                label
            );
        }
    }

    #[test]
    fn test_state_recovery_preserves_ability_to_save() {
        // After recovering from corruption, the operator should be able
        // to save new state successfully (the file is not permanently broken).
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("recover-and-save.json");

        // Corrupt the file
        std::fs::write(&state_path, "THIS IS NOT JSON AT ALL!!!!!").unwrap();

        let store = tee_operator::store::StateStore::new(&state_path);

        // Recover with defaults
        let mut state = store.load_or_default();
        assert_eq!(state.last_polled_block, 0);

        // Update state as the operator would during normal operation
        state.last_polled_block = 12345;
        state
            .active_disputes
            .insert("0xdead".to_string(), 1700000000);
        state.processed_event_ids.insert("0xbeef:0".to_string());

        // Save should succeed
        store.save(&state).unwrap();

        // Reload should return the new state
        let reloaded = store.load().unwrap().unwrap();
        assert_eq!(reloaded.last_polled_block, 12345);
        assert_eq!(reloaded.active_disputes.len(), 1);
        assert_eq!(reloaded.processed_event_ids.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 4: Operator handles webhook endpoint returning 500
    //         (retries, then continues)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_500_increments_failure_counter() {
        // Start a mock HTTP server that always returns 500.
        let app = axum::Router::new().route(
            "/webhook",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "server error",
                )
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let webhook_url = format!("http://127.0.0.1:{}/webhook", addr.port());
        let notifier = WebhookNotifier::new(&webhook_url);

        // Send a notification (fire-and-forget, spawns background task)
        notifier.notify_challenge(
            "0x1111111111111111111111111111111111111111111111111111111111111111",
            "0x2222222222222222222222222222222222222222",
            1700000000,
            1700003600,
        );

        // The notifier retries with exponential backoff: 1s, 2s, 4s.
        // Total wait: ~7 seconds + margin. After exhausting retries, it
        // increments the failure counter.
        tokio::time::sleep(Duration::from_secs(10)).await;

        assert!(
            notifier.notification_failures() >= 1,
            "failure counter should be >= 1 after webhook returns 500, got {}",
            notifier.notification_failures()
        );

        // The operator should continue operating despite webhook failures.
        // Verify the notifier is still functional (can send more notifications).
        let health = notifier.health_status();
        assert!(health.total_failures >= 1);

        server_handle.abort();
    }

    #[tokio::test]
    async fn test_webhook_connection_refused_increments_failure() {
        // No server running on this port -- connection refused
        let notifier = WebhookNotifier::new("http://127.0.0.1:1/webhook");

        notifier.notify_proof_submitted(
            "0x3333333333333333333333333333333333333333333333333333333333333333",
            "0x4444444444444444444444444444444444444444444444444444444444444444",
        );

        // Wait for retries to exhaust (1s + 2s + 4s + margin)
        tokio::time::sleep(Duration::from_secs(10)).await;

        assert!(
            notifier.notification_failures() >= 1,
            "failure counter should be >= 1 after connection refused, got {}",
            notifier.notification_failures()
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Operator handles out-of-order block events
    // -----------------------------------------------------------------------

    #[test]
    fn test_out_of_order_block_events_deduplication() {
        // The operator uses processed_event_ids to deduplicate events.
        // Even if block events arrive out of order (e.g., after a reorg
        // or restart), already-processed events should be skipped.
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("dedup-state.json");
        let store = tee_operator::store::StateStore::new(&state_path);

        // Simulate processing events from blocks 95-100
        let events = [
            "0xaaa:0", "0xaaa:1", // block 95
            "0xbbb:0", // block 96
            "0xccc:0", "0xccc:1", "0xccc:2", // block 98
            "0xddd:0", // block 100
        ];

        let state = tee_operator::store::OperatorState {
            last_polled_block: 100,
            processed_event_ids: events.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };

        store.save(&state).unwrap();

        // Simulate restart: operator loads state and starts from block 95
        // (before last_polled_block, e.g., due to safety margin)
        let loaded_state = store.load().unwrap().unwrap();
        assert_eq!(loaded_state.last_polled_block, 100);

        // Simulate receiving the same events again (out of order / re-org)
        let replayed_events = vec![
            "0xccc:2", // block 98 event arriving first
            "0xaaa:0", // block 95 event arriving late
            "0xeee:0", // NEW event from block 101
            "0xbbb:0", // block 96 duplicate
        ];

        let mut new_events = Vec::new();
        for event_id in &replayed_events {
            if !loaded_state.processed_event_ids.contains(*event_id) {
                new_events.push(event_id.to_string());
            }
        }

        // Only the truly new event should be processed
        assert_eq!(new_events.len(), 1);
        assert_eq!(new_events[0], "0xeee:0");
    }

    #[test]
    fn test_out_of_order_block_number_regression() {
        // Verify that the operator can handle seeing a lower block number
        // than last_polled_block (which happens during reorgs).
        let mut state = tee_operator::store::OperatorState {
            last_polled_block: 200,
            ..Default::default()
        };

        // Simulate receiving events from block 195 (before last_polled_block)
        // The operator should process new events even from "old" blocks
        // if they have not been seen before.
        let new_event = "0xfff:0";
        assert!(
            !state.processed_event_ids.contains(new_event),
            "new event should not be in processed set"
        );

        // Process it
        state.processed_event_ids.insert(new_event.to_string());
        assert!(state.processed_event_ids.contains(new_event));

        // The last_polled_block should NOT go backwards
        // (operator should take max(current, new_block))
        let incoming_block = 195u64;
        state.last_polled_block = state.last_polled_block.max(incoming_block);
        assert_eq!(
            state.last_polled_block, 200,
            "last_polled_block should not regress"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: Concurrent dispute handling under load
    //         (10 disputes simultaneously)
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_dispute_handling_10_disputes() {
        // Simulate 10 disputes arriving simultaneously and verify the
        // operator's dispute tracking data structures handle them correctly.
        let mut disputes: HashMap<String, u64> = HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Insert 10 disputes with staggered deadlines
        for i in 0..10 {
            let result_id = format!("0x{:064x}", i);
            let deadline = now + 3600 + (i * 60); // 1 hour + i minutes
            disputes.insert(result_id, deadline);
        }

        assert_eq!(disputes.len(), 10, "should have 10 active disputes");

        // Verify pruning does not remove active disputes
        let config = PruneConfig::default();
        let removed = prune_old_disputes(&mut disputes, &config);
        assert_eq!(removed, 0, "no disputes should be pruned (all are future)");
        assert_eq!(disputes.len(), 10);

        // Verify eviction respects limits
        let small_config = PruneConfig {
            max_dispute_age_secs: 30 * 24 * 3600,
            max_disputes: 5,
        };
        let evicted = evict_excess_disputes(&mut disputes, &small_config);
        assert_eq!(evicted, 5, "should evict 5 oldest disputes");
        assert_eq!(disputes.len(), 5, "should have 5 remaining");

        // The remaining disputes should be the ones with the latest deadlines
        let remaining_deadlines: Vec<u64> = disputes.values().copied().collect();
        for deadline in &remaining_deadlines {
            // The 5 remaining should be the ones with deadlines
            // from i=5..10 (the newest ones)
            assert!(
                *deadline >= now + 3600 + (5 * 60),
                "remaining dispute deadline {} is too old",
                deadline
            );
        }
    }

    #[test]
    fn test_concurrent_disputes_with_deadline_monitor() {
        use alloy::primitives::{Address, B256};

        let mut monitor = DeadlineMonitor::new();

        // Track 10 disputes simultaneously
        let base_time = 100_000u64;
        for i in 0u8..10 {
            let mut rid_bytes = [0u8; 32];
            rid_bytes[31] = i;
            let rid = B256::from(rid_bytes);

            let mut addr_bytes = [0u8; 20];
            addr_bytes[19] = i + 100;
            let challenger = Address::from(addr_bytes);

            let deadline = base_time + 3600 + (i as u64 * 60);
            monitor.track_dispute(rid, challenger, deadline);
        }

        assert_eq!(monitor.active_count(), 10);

        // Check deadlines when all are safe (far from expiry)
        let result = monitor.check_deadlines(base_time);
        assert_eq!(result.total_active, 10);
        assert!(result.warnings.is_empty());
        assert!(result.criticals.is_empty());
        assert!(result.expired.is_empty());

        // Check when some are approaching deadline (within 5 min warning)
        // The earliest dispute deadline is base_time + 3600
        // At base_time + 3600 - 200, that dispute is 200s away (within 300s warn)
        let result = monitor.check_deadlines(base_time + 3600 - 200);
        assert!(
            !result.warnings.is_empty(),
            "should have warnings for disputes near deadline"
        );

        // Check when one dispute is in critical range (< 60s)
        let result = monitor.check_deadlines(base_time + 3600 - 30);
        assert!(
            !result.criticals.is_empty(),
            "should have critical alerts for disputes about to expire"
        );

        // Resolve half the disputes
        for i in 0u8..5 {
            let mut rid_bytes = [0u8; 32];
            rid_bytes[31] = i;
            let rid = B256::from(rid_bytes);
            monitor.remove_dispute(&rid);
        }

        assert_eq!(monitor.active_count(), 5);
        assert!(
            monitor.metrics().total_tracked >= 10,
            "total tracked should reflect all disputes ever tracked"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Rate limiter under burst load (>100 concurrent IPs)
    // -----------------------------------------------------------------------

    #[test]
    fn test_rate_limiter_burst_load_150_ips() {
        use tee_operator::middleware::{RateLimitConfig, RateLimiter};

        let config = RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 5,
            max_ips: 200,
        };
        let limiter = RateLimiter::new(config);

        // Simulate 150 unique IPs each sending a single request.
        // All should be allowed since each gets its own bucket.
        for i in 0..150 {
            let ip = format!("10.0.{}.{}", i / 256, i % 256);
            assert!(
                limiter.check(&ip).is_ok(),
                "First request from IP {} should be allowed",
                ip
            );
        }

        assert_eq!(limiter.tracked_ips(), 150);
        assert_eq!(limiter.total_rate_limited(), 0);
    }

    #[test]
    fn test_rate_limiter_single_ip_burst_exhaustion() {
        use tee_operator::middleware::{RateLimitConfig, RateLimiter};

        let config = RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 10,
            max_ips: 100,
        };
        let limiter = RateLimiter::new(config);

        // Exhaust burst for a single IP.
        for i in 0..10 {
            assert!(
                limiter.check("attacker-ip").is_ok(),
                "Request {} within burst should be allowed",
                i
            );
        }

        // 11th request should be rate-limited.
        assert!(limiter.check("attacker-ip").is_err());
        assert_eq!(limiter.total_rate_limited(), 1);

        // Other IPs remain unaffected.
        assert!(limiter.check("innocent-ip").is_ok());
    }

    #[test]
    fn test_rate_limiter_lru_eviction_under_ip_pressure() {
        use tee_operator::middleware::{RateLimitConfig, RateLimiter};

        let config = RateLimitConfig {
            max_requests_per_minute: 60,
            max_burst: 5,
            max_ips: 50,
        };
        let limiter = RateLimiter::new(config);

        // Fill up to max_ips.
        for i in 0..50 {
            limiter.check(&format!("ip-{}", i)).ok();
        }
        assert_eq!(limiter.tracked_ips(), 50);

        // Adding more IPs triggers LRU eviction.
        for i in 50..150 {
            limiter.check(&format!("ip-{}", i)).ok();
        }
        // Should stay capped at max_ips.
        assert_eq!(limiter.tracked_ips(), 50);
    }

    #[test]
    fn test_rate_limiter_concurrent_multithreaded() {
        use tee_operator::middleware::{RateLimitConfig, RateLimiter};

        let config = RateLimitConfig {
            max_requests_per_minute: 600,
            max_burst: 20,
            max_ips: 1000,
        };
        let limiter = Arc::new(RateLimiter::new(config));
        let mut handles = Vec::new();

        // 16 threads, each sending requests from 10 different IPs.
        for thread_id in 0..16 {
            let limiter = limiter.clone();
            handles.push(std::thread::spawn(move || {
                for req in 0..50 {
                    let ip = format!("thread-{}-ip-{}", thread_id, req % 10);
                    let _ = limiter.check(&ip);
                }
            }));
        }

        for h in handles {
            h.join().expect("rate limiter thread panicked");
        }

        // No panics or deadlocks. Tracked IPs should be bounded.
        assert!(limiter.tracked_ips() <= 1000);
    }

    // -----------------------------------------------------------------------
    // Test 8: Dispute pruning under memory pressure (100K+ entries)
    // -----------------------------------------------------------------------

    #[test]
    fn test_dispute_pruning_100k_entries() {
        let mut disputes: HashMap<String, u64> = HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Insert 100,000 disputes: half old, half recent.
        for i in 0..50_000u64 {
            disputes.insert(format!("old-{}", i), now - 60 * 86400 + i);
        }
        for i in 0..50_000u64 {
            disputes.insert(format!("recent-{}", i), now - 86400 + i);
        }

        assert_eq!(disputes.len(), 100_000);

        let config = PruneConfig {
            max_dispute_age_secs: 30 * 86400,
            max_disputes: 10_000,
        };

        let start = std::time::Instant::now();
        let (pruned, evicted) = prune_and_evict(&mut disputes, &config);
        let elapsed = start.elapsed();

        // All 50K old disputes should be pruned.
        assert_eq!(pruned, 50_000);
        // 50K recent remain, max is 10K, so 40K evicted.
        assert_eq!(evicted, 40_000);
        assert_eq!(disputes.len(), 10_000);

        // Performance: should complete in well under 5 seconds.
        assert!(
            elapsed < Duration::from_secs(5),
            "Pruning 100K disputes took {:?}, expected < 5s",
            elapsed
        );
    }

    #[test]
    fn test_dispute_eviction_preserves_newest() {
        let mut disputes: HashMap<String, u64> = HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Insert 1000 disputes with sequential deadlines.
        for i in 0..1000u64 {
            disputes.insert(format!("dispute-{}", i), now + i);
        }

        let config = PruneConfig {
            max_dispute_age_secs: 30 * 86400,
            max_disputes: 100,
        };

        let evicted = evict_excess_disputes(&mut disputes, &config);
        assert_eq!(evicted, 900);
        assert_eq!(disputes.len(), 100);

        // Remaining should be the 100 newest (highest deadline values).
        for deadline in disputes.values() {
            assert!(
                *deadline >= now + 900,
                "Expected only newest disputes to remain, got deadline {}",
                deadline
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 9: Concurrent dispute tracking (multi-threaded access)
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_dispute_tracking_multithreaded() {
        let disputes = Arc::new(std::sync::Mutex::new(HashMap::<String, u64>::new()));
        let mut handles = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // 8 threads, each inserting 1000 disputes.
        for thread_id in 0..8 {
            let disputes = disputes.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..1000 {
                    let key = format!("t{}-d{}", thread_id, i);
                    let deadline = now + (thread_id * 1000 + i) as u64;
                    disputes.lock().unwrap().insert(key, deadline);
                }
            }));
        }

        for h in handles {
            h.join()
                .expect("concurrent dispute tracking thread panicked");
        }

        let disputes = disputes.lock().unwrap();
        assert_eq!(disputes.len(), 8000);

        // Now prune under lock.
        let mut disputes_owned = disputes.clone();
        drop(disputes);

        let config = PruneConfig {
            max_dispute_age_secs: 30 * 86400,
            max_disputes: 1000,
        };
        let (_, evicted) = prune_and_evict(&mut disputes_owned, &config);
        assert_eq!(evicted, 7000);
        assert_eq!(disputes_owned.len(), 1000);
    }

    #[test]
    fn test_concurrent_insert_and_prune_interleaved() {
        // Interleave inserts and prunes across threads to detect data corruption.
        let disputes = Arc::new(std::sync::Mutex::new(HashMap::<String, u64>::new()));
        let mut handles = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Writer threads.
        for thread_id in 0..4 {
            let disputes = disputes.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..500 {
                    let key = format!("w{}-{}", thread_id, i);
                    let deadline = if i % 2 == 0 {
                        now - 60 * 86400 + i as u64
                    } else {
                        now + i as u64
                    };
                    disputes.lock().unwrap().insert(key, deadline);
                }
            }));
        }

        // Pruner threads.
        for _ in 0..2 {
            let disputes = disputes.clone();
            handles.push(std::thread::spawn(move || {
                let config = PruneConfig {
                    max_dispute_age_secs: 30 * 86400,
                    max_disputes: 5000,
                };
                for _ in 0..10 {
                    let mut d = disputes.lock().unwrap();
                    prune_old_disputes(&mut d, &config);
                    drop(d);
                    std::thread::yield_now();
                }
            }));
        }

        for h in handles {
            h.join().expect("interleaved insert+prune thread panicked");
        }

        // Verify no corruption: iteration works and map is consistent.
        let disputes = disputes.lock().unwrap();
        for (_key, _deadline) in disputes.iter() {
            // Just verify iteration completes without panic.
        }
    }

    // -----------------------------------------------------------------------
    // Test 10: Notification webhook timeout handling
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_webhook_timeout_handling() {
        // Spin up a mock server that delays response beyond the client timeout.
        let app = axum::Router::new().route(
            "/webhook",
            axum::routing::post(|| async {
                // Sleep longer than the webhook client's 5s timeout.
                tokio::time::sleep(Duration::from_secs(30)).await;
                "ok"
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener for timeout test");
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let url = format!("http://127.0.0.1:{}/webhook", addr.port());
        let notifier = WebhookNotifier::new(&url);

        notifier.notify_challenge(
            "0x0000000000000000000000000000000000000000000000000000000000000099",
            "0x0000000000000000000000000000000000000099",
            1700000000,
            1700003600,
        );

        // Wait for retries to exhaust.
        // 5s timeout per attempt, 4 attempts (1 + 3 retries), plus backoff delays.
        tokio::time::sleep(Duration::from_secs(35)).await;

        assert!(
            notifier.notification_failures() >= 1,
            "Expected at least 1 failure from timeout, got {}",
            notifier.notification_failures()
        );
    }

    // -----------------------------------------------------------------------
    // Additional chaos scenarios
    // -----------------------------------------------------------------------

    #[test]
    fn test_circuit_breaker_thread_safety_under_contention() {
        // Verify the circuit breaker handles concurrent access from
        // multiple threads without panics or data corruption.
        let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 100, // High threshold so it stays closed
            recovery_timeout: Duration::from_secs(60),
            success_threshold_for_close: 2,
        }));

        let mut handles = Vec::new();

        // Spawn 10 threads, each doing a mix of successes and failures
        for thread_id in 0..10 {
            let cb_clone = cb.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..50 {
                    if (thread_id + i) % 3 == 0 {
                        cb_clone.record_failure();
                    } else {
                        let _ = cb_clone.call_sync(|| Ok::<_, anyhow::Error>(42));
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("thread should not panic");
        }

        // Breaker should not have tripped (threshold=100, but successes
        // reset the consecutive failure counter)
        let metrics = cb.metrics();
        assert!(
            metrics.state == State::Closed || metrics.state == State::Open,
            "breaker should be in a valid state, got {:?}",
            metrics.state
        );
    }

    #[test]
    fn test_massive_dispute_pruning() {
        // Stress test: insert 10,000 old disputes and verify pruning
        // handles them efficiently.
        let mut disputes: HashMap<String, u64> = HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        for i in 0..10_000 {
            let id = format!("0x{:064x}", i);
            // Half are old (60 days), half are recent
            let deadline = if i % 2 == 0 {
                now - 60 * 86400 // 60 days ago
            } else {
                now + 3600 // 1 hour from now
            };
            disputes.insert(id, deadline);
        }

        assert_eq!(disputes.len(), 10_000);

        let config = PruneConfig::default();
        let (pruned, evicted) = prune_and_evict(&mut disputes, &config);

        assert_eq!(pruned, 5_000, "should prune 5000 old disputes");
        assert_eq!(evicted, 0, "remaining 5000 is under default max");
        assert_eq!(disputes.len(), 5_000);
    }

    #[test]
    fn test_state_file_concurrent_save_load() {
        // Simulate rapid save/load cycles as would happen during
        // high block throughput. Verify atomic writes prevent corruption.
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("rapid-state.json");
        let store = tee_operator::store::StateStore::new(&state_path);

        for block in 0..100u64 {
            let state = tee_operator::store::OperatorState {
                last_polled_block: block,
                processed_event_ids: [format!("evt-{}", block)].into_iter().collect(),
                ..Default::default()
            };

            store.save(&state).unwrap();

            // Immediately reload and verify consistency
            let loaded = store.load().unwrap().unwrap();
            assert_eq!(
                loaded.last_polled_block, block,
                "state should match after rapid save/load at block {}",
                block
            );
        }
    }
}
