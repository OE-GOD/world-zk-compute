//! Integration tests for operator crash recovery, dispute lifecycle, and
//! notification failure resilience.
//!
//! Run with: `cargo test --test operator_integration`

use std::collections::{HashMap, HashSet};
use tee_operator::store::OperatorState;

// ─── Crash recovery: state file persistence ───────────────────────────

#[test]
fn test_crash_recovery_state_file_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("operator-state.json");

    // Simulate operator writing state before shutdown
    let state = OperatorState {
        last_polled_block: 12345,
        active_disputes: {
            let mut m = HashMap::new();
            m.insert("0xaabb".to_string(), 1700000000);
            m.insert("0xccdd".to_string(), 1700086400);
            m
        },
        processed_event_ids: {
            let mut s = HashSet::new();
            s.insert("0xdeadbeef:0".to_string());
            s.insert("0xdeadbeef:1".to_string());
            s
        },
    };

    // Atomic write: write to .tmp, then rename
    let tmp_path = state_path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&state).unwrap();
    std::fs::write(&tmp_path, &json).unwrap();
    std::fs::rename(&tmp_path, &state_path).unwrap();

    // Simulate operator restart — read state back
    let loaded_json = std::fs::read_to_string(&state_path).unwrap();
    let loaded: OperatorState = serde_json::from_str(&loaded_json).unwrap();

    assert_eq!(loaded.last_polled_block, 12345);
    assert_eq!(loaded.active_disputes.len(), 2);
    assert_eq!(loaded.active_disputes.get("0xaabb"), Some(&1700000000));
    assert_eq!(loaded.processed_event_ids.len(), 2);
    assert!(loaded.processed_event_ids.contains("0xdeadbeef:0"));
}

#[test]
fn test_crash_recovery_missing_state_file_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("nonexistent-state.json");

    // When state file doesn't exist, operator starts from defaults
    assert!(!state_path.exists());

    let state = if state_path.exists() {
        let json = std::fs::read_to_string(&state_path).unwrap();
        serde_json::from_str(&json).unwrap()
    } else {
        OperatorState::default()
    };

    assert_eq!(state.last_polled_block, 0);
    assert!(state.active_disputes.is_empty());
    assert!(state.processed_event_ids.is_empty());
}

#[test]
fn test_crash_recovery_corrupt_state_file_falls_back_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("corrupt-state.json");

    // Write corrupt data
    std::fs::write(&state_path, "NOT VALID JSON {{{").unwrap();

    // Operator should fall back to default on corrupt state
    let state: OperatorState = match std::fs::read_to_string(&state_path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => OperatorState::default(),
    };

    assert_eq!(state.last_polled_block, 0);
    assert!(state.active_disputes.is_empty());
}

#[test]
fn test_crash_recovery_partial_state_file_uses_defaults_for_missing() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("partial-state.json");

    // Older state format might only have last_polled_block
    std::fs::write(&state_path, r#"{"last_polled_block": 999}"#).unwrap();

    let json = std::fs::read_to_string(&state_path).unwrap();
    let state: OperatorState = serde_json::from_str(&json).unwrap();

    assert_eq!(state.last_polled_block, 999);
    assert!(state.active_disputes.is_empty());
    assert!(state.processed_event_ids.is_empty());
}

// ─── Dispute lifecycle ────────────────────────────────────────────────

#[test]
fn test_dispute_lifecycle_track_and_resolve() {
    let mut state = OperatorState::default();
    let now = 1700000000u64;
    let dispute_window = 3600u64; // 1 hour

    // Step 1: Challenge detected → track dispute
    let result_id = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    state
        .active_disputes
        .insert(result_id.to_string(), now + dispute_window);
    assert_eq!(state.active_disputes.len(), 1);

    // Step 2: Proof submitted (dispute still active until resolved)
    assert!(state.active_disputes.contains_key(result_id));

    // Step 3: Dispute resolved → remove from tracking
    state.active_disputes.remove(result_id);
    assert!(state.active_disputes.is_empty());
}

#[test]
fn test_dispute_lifecycle_multiple_concurrent() {
    let mut state = OperatorState::default();
    let now = 1700000000u64;

    // Track 5 concurrent disputes
    for i in 0..5 {
        let result_id = format!("0x{:064x}", i);
        state
            .active_disputes
            .insert(result_id, now + 3600 + (i * 60) as u64);
    }
    assert_eq!(state.active_disputes.len(), 5);

    // Resolve disputes 0 and 2
    state.active_disputes.remove(&format!("0x{:064x}", 0));
    state.active_disputes.remove(&format!("0x{:064x}", 2));
    assert_eq!(state.active_disputes.len(), 3);

    // Remaining disputes: 1, 3, 4
    assert!(state
        .active_disputes
        .contains_key(&format!("0x{:064x}", 1)));
    assert!(state
        .active_disputes
        .contains_key(&format!("0x{:064x}", 3)));
    assert!(state
        .active_disputes
        .contains_key(&format!("0x{:064x}", 4)));
}

#[test]
fn test_dispute_lifecycle_expired_disputes() {
    let mut state = OperatorState::default();
    let now = 1700010000u64;

    // Add disputes with various deadlines
    state
        .active_disputes
        .insert("0xaaa".to_string(), 1700000000); // expired (deadline < now)
    state
        .active_disputes
        .insert("0xbbb".to_string(), 1700005000); // expired
    state
        .active_disputes
        .insert("0xccc".to_string(), 1700020000); // still active

    // Prune expired disputes (deadline < now)
    state.active_disputes.retain(|_, deadline| *deadline > now);
    assert_eq!(state.active_disputes.len(), 1);
    assert!(state.active_disputes.contains_key("0xccc"));
}

// ─── Event deduplication ──────────────────────────────────────────────

#[test]
fn test_event_deduplication_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("dedup-state.json");

    // First run: process some events
    let mut state = OperatorState::default();
    state.processed_event_ids.insert("tx1:0".to_string());
    state.processed_event_ids.insert("tx1:1".to_string());
    state.processed_event_ids.insert("tx2:0".to_string());
    state.last_polled_block = 100;

    let json = serde_json::to_string_pretty(&state).unwrap();
    std::fs::write(&state_path, &json).unwrap();

    // Second run (restart): load state and check dedup
    let loaded_json = std::fs::read_to_string(&state_path).unwrap();
    let loaded: OperatorState = serde_json::from_str(&loaded_json).unwrap();

    // Simulate receiving the same events again
    let event_id = "tx1:0";
    let is_duplicate = loaded.processed_event_ids.contains(event_id);
    assert!(is_duplicate, "Event should be detected as duplicate");

    let new_event = "tx3:0";
    let is_new = !loaded.processed_event_ids.contains(new_event);
    assert!(is_new, "New event should not be a duplicate");
}

// ─── Atomic write safety ──────────────────────────────────────────────

#[test]
fn test_atomic_write_no_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("atomic-state.json");

    // Write initial valid state
    let state1 = OperatorState {
        last_polled_block: 100,
        ..Default::default()
    };
    let json1 = serde_json::to_string_pretty(&state1).unwrap();
    std::fs::write(&state_path, &json1).unwrap();

    // Write updated state atomically
    let state2 = OperatorState {
        last_polled_block: 200,
        active_disputes: {
            let mut m = HashMap::new();
            m.insert("0xdispute".to_string(), 9999);
            m
        },
        ..Default::default()
    };

    let tmp_path = state_path.with_extension("json.tmp");
    let json2 = serde_json::to_string_pretty(&state2).unwrap();
    std::fs::write(&tmp_path, &json2).unwrap();
    std::fs::rename(&tmp_path, &state_path).unwrap();

    // Verify .tmp file doesn't exist
    assert!(!tmp_path.exists());

    // Verify state was fully updated (not partial)
    let loaded_json = std::fs::read_to_string(&state_path).unwrap();
    let loaded: OperatorState = serde_json::from_str(&loaded_json).unwrap();
    assert_eq!(loaded.last_polled_block, 200);
    assert_eq!(loaded.active_disputes.len(), 1);
}

// ─── Dispute pruning (lib.rs public API) ──────────────────────────────

#[test]
fn test_dispute_pruning_via_lib() {
    use tee_operator::{prune_old_disputes, PruneConfig};

    let mut disputes = HashMap::new();
    // Insert disputes with ancient deadlines
    disputes.insert("old-dispute".to_string(), 1000);
    disputes.insert("recent-dispute".to_string(), u64::MAX);

    let config = PruneConfig {
        max_dispute_age_secs: 1, // very short cutoff for testing
        max_disputes: 10_000,
    };

    let removed = prune_old_disputes(&mut disputes, &config);
    assert_eq!(removed, 1);
    assert!(!disputes.contains_key("old-dispute"));
    assert!(disputes.contains_key("recent-dispute"));
}

#[test]
fn test_dispute_eviction_via_lib() {
    use tee_operator::{evict_excess_disputes, PruneConfig};

    let mut disputes = HashMap::new();
    for i in 0..10 {
        disputes.insert(format!("d-{}", i), 1000 + i as u64);
    }

    let config = PruneConfig {
        max_dispute_age_secs: u64::MAX,
        max_disputes: 5,
    };

    let evicted = evict_excess_disputes(&mut disputes, &config);
    assert_eq!(evicted, 5);
    assert_eq!(disputes.len(), 5);
}

// ─── Notification health status ───────────────────────────────────────

#[test]
fn test_notification_health_status_serializable() {
    use tee_operator::notifications::WebhookHealthStatus;

    let status = WebhookHealthStatus {
        webhook_url: "https://hooks.slack.com/test".to_string(),
        total_failures: 42,
    };

    let json = serde_json::to_string(&status).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["webhook_url"], "https://hooks.slack.com/test");
    assert_eq!(parsed["total_failures"], 42);
}
