/// Re-export modules needed by integration tests.
pub mod alerting;
pub mod audit;
pub mod circuit_breaker;
pub mod config;
pub mod deadline_monitor;
pub mod middleware;
pub mod notifications;
pub mod ssrf;
pub mod store;
pub mod watcher;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default maximum age for resolved disputes (30 days in seconds).
pub const DEFAULT_MAX_DISPUTE_AGE_SECS: u64 = 30 * 24 * 3600;

/// Default maximum number of tracked disputes before LRU eviction kicks in.
pub const DEFAULT_MAX_DISPUTES: usize = 10_000;

/// Configuration for dispute pruning.
#[derive(Debug, Clone)]
pub struct PruneConfig {
    /// Maximum age of resolved disputes in seconds (default: 30 days).
    pub max_dispute_age_secs: u64,
    /// Maximum number of disputes to track (default: 10,000).
    pub max_disputes: usize,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            max_dispute_age_secs: DEFAULT_MAX_DISPUTE_AGE_SECS,
            max_disputes: DEFAULT_MAX_DISPUTES,
        }
    }
}

/// Prune old resolved disputes from the active_disputes map.
///
/// Removes entries whose deadline has passed by more than `max_dispute_age_secs`.
/// Returns the number of disputes removed.
pub fn prune_old_disputes(
    active_disputes: &mut HashMap<String, u64>,
    config: &PruneConfig,
) -> usize {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let cutoff = now.saturating_sub(config.max_dispute_age_secs);
    let before = active_disputes.len();

    // Remove disputes whose deadline is older than the cutoff
    active_disputes.retain(|_id, deadline| *deadline > cutoff);

    let removed = before - active_disputes.len();
    if removed > 0 {
        tracing::info!(
            removed,
            remaining = active_disputes.len(),
            cutoff_timestamp = cutoff,
            "Pruned old disputes"
        );
    }
    removed
}

/// Evict oldest disputes if the map exceeds `max_disputes`.
///
/// Removes the entries with the smallest (oldest) deadlines first.
/// Returns the number of disputes evicted.
pub fn evict_excess_disputes(
    active_disputes: &mut HashMap<String, u64>,
    config: &PruneConfig,
) -> usize {
    if active_disputes.len() <= config.max_disputes {
        return 0;
    }

    let excess = active_disputes.len() - config.max_disputes;

    // Collect and sort by deadline (ascending = oldest first)
    let mut entries: Vec<(String, u64)> = active_disputes
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    entries.sort_by_key(|(_k, deadline)| *deadline);

    // Remove the oldest `excess` entries
    for (id, _deadline) in entries.into_iter().take(excess) {
        active_disputes.remove(&id);
    }

    tracing::info!(
        evicted = excess,
        remaining = active_disputes.len(),
        max = config.max_disputes,
        "Evicted excess disputes (LRU)"
    );
    excess
}

/// Run both pruning and LRU eviction. Returns (pruned, evicted).
pub fn prune_and_evict(
    active_disputes: &mut HashMap<String, u64>,
    config: &PruneConfig,
) -> (usize, usize) {
    let pruned = prune_old_disputes(active_disputes, config);
    let evicted = evict_excess_disputes(active_disputes, config);
    (pruned, evicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn test_prune_removes_old_disputes() {
        let mut disputes = HashMap::new();
        let now = now_secs();

        // Old dispute (60 days ago)
        disputes.insert("old-1".to_string(), now - 60 * 86400);
        // Recent dispute (1 day ago)
        disputes.insert("recent-1".to_string(), now - 86400);
        // Future dispute
        disputes.insert("future-1".to_string(), now + 86400);

        let config = PruneConfig::default(); // 30 day cutoff
        let removed = prune_old_disputes(&mut disputes, &config);

        assert_eq!(removed, 1);
        assert_eq!(disputes.len(), 2);
        assert!(!disputes.contains_key("old-1"));
        assert!(disputes.contains_key("recent-1"));
        assert!(disputes.contains_key("future-1"));
    }

    #[test]
    fn test_prune_nothing_to_remove() {
        let mut disputes = HashMap::new();
        let now = now_secs();
        disputes.insert("d1".to_string(), now + 1000);
        disputes.insert("d2".to_string(), now + 2000);

        let config = PruneConfig::default();
        let removed = prune_old_disputes(&mut disputes, &config);

        assert_eq!(removed, 0);
        assert_eq!(disputes.len(), 2);
    }

    #[test]
    fn test_prune_empty_map() {
        let mut disputes = HashMap::new();
        let config = PruneConfig::default();
        let removed = prune_old_disputes(&mut disputes, &config);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_prune_custom_age() {
        let mut disputes = HashMap::new();
        let now = now_secs();

        disputes.insert("d1".to_string(), now - 3600); // 1 hour ago
        disputes.insert("d2".to_string(), now - 7200); // 2 hours ago

        let config = PruneConfig {
            max_dispute_age_secs: 5400, // 1.5 hours
            max_disputes: 10_000,
        };
        let removed = prune_old_disputes(&mut disputes, &config);

        assert_eq!(removed, 1);
        assert!(disputes.contains_key("d1"));
        assert!(!disputes.contains_key("d2"));
    }

    #[test]
    fn test_evict_under_limit() {
        let mut disputes = HashMap::new();
        disputes.insert("d1".to_string(), 1000);
        disputes.insert("d2".to_string(), 2000);

        let config = PruneConfig {
            max_dispute_age_secs: DEFAULT_MAX_DISPUTE_AGE_SECS,
            max_disputes: 10,
        };
        let evicted = evict_excess_disputes(&mut disputes, &config);

        assert_eq!(evicted, 0);
        assert_eq!(disputes.len(), 2);
    }

    #[test]
    fn test_evict_over_limit_removes_oldest() {
        let mut disputes = HashMap::new();
        disputes.insert("oldest".to_string(), 1000);
        disputes.insert("middle".to_string(), 2000);
        disputes.insert("newest".to_string(), 3000);

        let config = PruneConfig {
            max_dispute_age_secs: DEFAULT_MAX_DISPUTE_AGE_SECS,
            max_disputes: 2,
        };
        let evicted = evict_excess_disputes(&mut disputes, &config);

        assert_eq!(evicted, 1);
        assert_eq!(disputes.len(), 2);
        assert!(!disputes.contains_key("oldest"));
        assert!(disputes.contains_key("middle"));
        assert!(disputes.contains_key("newest"));
    }

    #[test]
    fn test_evict_exactly_at_limit() {
        let mut disputes = HashMap::new();
        disputes.insert("d1".to_string(), 1000);
        disputes.insert("d2".to_string(), 2000);

        let config = PruneConfig {
            max_dispute_age_secs: DEFAULT_MAX_DISPUTE_AGE_SECS,
            max_disputes: 2,
        };
        let evicted = evict_excess_disputes(&mut disputes, &config);

        assert_eq!(evicted, 0);
        assert_eq!(disputes.len(), 2);
    }

    #[test]
    fn test_prune_and_evict_combined() {
        let mut disputes = HashMap::new();
        let now = now_secs();

        // 5 old disputes (60 days ago)
        for i in 0..5 {
            disputes.insert(format!("old-{}", i), now - 60 * 86400 + i as u64);
        }
        // 4 recent disputes
        for i in 0..4 {
            disputes.insert(format!("recent-{}", i), now - 86400 + i as u64);
        }

        let config = PruneConfig {
            max_dispute_age_secs: DEFAULT_MAX_DISPUTE_AGE_SECS,
            max_disputes: 3,
        };

        let (pruned, evicted) = prune_and_evict(&mut disputes, &config);

        assert_eq!(pruned, 5); // 5 old disputes removed
        assert_eq!(evicted, 1); // 4 remaining, max 3 → evict 1
        assert_eq!(disputes.len(), 3);
    }
}
