//! Graceful Degradation System
//!
//! Allows the prover to continue operating in reduced capacity when
//! some components fail, rather than complete system failure.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Service operational mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum OperationalMode {
    /// Full functionality
    Normal,
    /// Reduced functionality due to degraded dependencies
    Degraded,
    /// Minimal functionality - only essential operations
    Minimal,
    /// Service is unavailable
    Unavailable,
}

impl OperationalMode {
    pub fn is_operational(&self) -> bool {
        !matches!(self, Self::Unavailable)
    }

    pub fn allows_new_jobs(&self) -> bool {
        matches!(self, Self::Normal | Self::Degraded)
    }

    pub fn allows_proof_generation(&self) -> bool {
        matches!(self, Self::Normal | Self::Degraded | Self::Minimal)
    }
}

/// Feature flag for graceful degradation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Feature {
    /// Accept new proof requests
    AcceptNewJobs,
    /// Generate proofs locally
    LocalProving,
    /// Use Bonsai cloud proving
    CloudProving,
    /// Submit proofs to chain
    ProofSubmission,
    /// Fetch inputs from IPFS
    IpfsFetch,
    /// Use proof caching
    ProofCache,
    /// Enable batching
    Batching,
    /// P2P coordination
    P2PCoordination,
    /// Metrics collection
    Metrics,
    /// Alerting
    Alerting,
}

impl Feature {
    /// Check if this feature is essential for operation
    pub fn is_essential(&self) -> bool {
        matches!(
            self,
            Self::LocalProving | Self::ProofSubmission
        )
    }

    /// Get fallback feature if available
    pub fn fallback(&self) -> Option<Feature> {
        match self {
            Self::CloudProving => Some(Self::LocalProving),
            Self::IpfsFetch => None, // No fallback, use cached/inline data
            Self::P2PCoordination => None, // Fall back to standalone mode
            _ => None,
        }
    }
}

/// Status of a feature
#[derive(Debug, Clone, serde::Serialize)]
pub struct FeatureStatus {
    pub feature: Feature,
    pub enabled: bool,
    pub available: bool,
    pub degraded_at: Option<u64>,
    pub reason: Option<String>,
    pub fallback_active: bool,
}

/// Degradation policy
#[derive(Debug, Clone)]
pub struct DegradationPolicy {
    /// Features to disable first when degrading
    pub disable_order: Vec<Feature>,
    /// Features that must remain enabled
    pub essential_features: Vec<Feature>,
    /// Auto-recovery timeout
    pub recovery_timeout: Duration,
    /// Minimum time between mode changes
    pub mode_change_cooldown: Duration,
}

impl Default for DegradationPolicy {
    fn default() -> Self {
        Self {
            disable_order: vec![
                Feature::P2PCoordination,
                Feature::Batching,
                Feature::Alerting,
                Feature::Metrics,
                Feature::ProofCache,
                Feature::CloudProving,
                Feature::IpfsFetch,
            ],
            essential_features: vec![
                Feature::LocalProving,
                Feature::ProofSubmission,
            ],
            recovery_timeout: Duration::from_secs(300),
            mode_change_cooldown: Duration::from_secs(30),
        }
    }
}

/// Degradation manager
pub struct DegradationManager {
    mode: RwLock<OperationalMode>,
    features: RwLock<HashMap<Feature, FeatureStatus>>,
    policy: DegradationPolicy,
    last_mode_change: RwLock<Instant>,
    failure_counts: RwLock<HashMap<Feature, u32>>,
}

impl DegradationManager {
    /// Create a new degradation manager
    pub fn new(policy: DegradationPolicy) -> Self {
        let mut features = HashMap::new();

        // Initialize all features as enabled and available
        for feature in [
            Feature::AcceptNewJobs,
            Feature::LocalProving,
            Feature::CloudProving,
            Feature::ProofSubmission,
            Feature::IpfsFetch,
            Feature::ProofCache,
            Feature::Batching,
            Feature::P2PCoordination,
            Feature::Metrics,
            Feature::Alerting,
        ] {
            features.insert(
                feature,
                FeatureStatus {
                    feature,
                    enabled: true,
                    available: true,
                    degraded_at: None,
                    reason: None,
                    fallback_active: false,
                },
            );
        }

        Self {
            mode: RwLock::new(OperationalMode::Normal),
            features: RwLock::new(features),
            policy,
            last_mode_change: RwLock::new(Instant::now()),
            failure_counts: RwLock::new(HashMap::new()),
        }
    }

    /// Get current operational mode
    pub async fn mode(&self) -> OperationalMode {
        *self.mode.read().await
    }

    /// Check if a feature is enabled
    pub async fn is_enabled(&self, feature: Feature) -> bool {
        let features = self.features.read().await;
        features.get(&feature).map(|f| f.enabled).unwrap_or(false)
    }

    /// Check if a feature is available (enabled and working)
    pub async fn is_available(&self, feature: Feature) -> bool {
        let features = self.features.read().await;
        features
            .get(&feature)
            .map(|f| f.enabled && f.available)
            .unwrap_or(false)
    }

    /// Report a feature failure
    pub async fn report_failure(&self, feature: Feature, reason: String) {
        let mut failure_counts = self.failure_counts.write().await;
        let count = failure_counts.entry(feature).or_insert(0);
        *count += 1;

        tracing::warn!(
            feature = ?feature,
            failures = *count,
            reason = %reason,
            "Feature failure reported"
        );

        // After 3 failures, mark as unavailable
        if *count >= 3 {
            drop(failure_counts);
            self.mark_unavailable(feature, reason).await;
        }
    }

    /// Mark a feature as unavailable
    pub async fn mark_unavailable(&self, feature: Feature, reason: String) {
        let mut features = self.features.write().await;

        if let Some(status) = features.get_mut(&feature) {
            status.available = false;
            status.degraded_at = Some(current_timestamp());
            status.reason = Some(reason.clone());

            // Activate fallback if available
            if let Some(fallback) = feature.fallback() {
                if let Some(fallback_status) = features.get_mut(&fallback) {
                    fallback_status.fallback_active = true;
                    tracing::info!(
                        feature = ?feature,
                        fallback = ?fallback,
                        "Activated fallback feature"
                    );
                }
            }
        }

        drop(features);

        // Recalculate operational mode
        self.recalculate_mode().await;

        tracing::warn!(
            feature = ?feature,
            reason = %reason,
            mode = ?self.mode().await,
            "Feature marked unavailable"
        );
    }

    /// Mark a feature as recovered
    pub async fn mark_recovered(&self, feature: Feature) {
        let mut features = self.features.write().await;

        if let Some(status) = features.get_mut(&feature) {
            status.available = true;
            status.degraded_at = None;
            status.reason = None;

            // Deactivate fallback if this was the original feature
            if let Some(fallback) = feature.fallback() {
                if let Some(fallback_status) = features.get_mut(&fallback) {
                    fallback_status.fallback_active = false;
                }
            }
        }

        // Reset failure count
        let mut failure_counts = self.failure_counts.write().await;
        failure_counts.remove(&feature);

        drop(features);
        drop(failure_counts);

        // Recalculate operational mode
        self.recalculate_mode().await;

        tracing::info!(
            feature = ?feature,
            mode = ?self.mode().await,
            "Feature recovered"
        );
    }

    /// Manually disable a feature
    pub async fn disable_feature(&self, feature: Feature, reason: String) {
        let mut features = self.features.write().await;

        if let Some(status) = features.get_mut(&feature) {
            status.enabled = false;
            status.reason = Some(reason);
        }

        drop(features);
        self.recalculate_mode().await;
    }

    /// Manually enable a feature
    pub async fn enable_feature(&self, feature: Feature) {
        let mut features = self.features.write().await;

        if let Some(status) = features.get_mut(&feature) {
            status.enabled = true;
            status.reason = None;
        }

        drop(features);
        self.recalculate_mode().await;
    }

    /// Get status of all features
    pub async fn feature_status(&self) -> Vec<FeatureStatus> {
        let features = self.features.read().await;
        features.values().cloned().collect()
    }

    /// Get degradation summary
    pub async fn summary(&self) -> DegradationSummary {
        let mode = self.mode().await;
        let features = self.features.read().await;

        let available_count = features.values().filter(|f| f.available && f.enabled).count();
        let degraded_features: Vec<Feature> = features
            .values()
            .filter(|f| !f.available || !f.enabled)
            .map(|f| f.feature)
            .collect();

        DegradationSummary {
            mode,
            available_features: available_count,
            total_features: features.len(),
            degraded_features,
            since: self.last_mode_change.read().await.elapsed(),
        }
    }

    /// Recalculate operational mode based on feature availability
    async fn recalculate_mode(&self) {
        let features = self.features.read().await;

        // Check essential features
        let essential_ok = self.policy.essential_features.iter().all(|f| {
            features.get(f).map(|s| s.available && s.enabled).unwrap_or(false)
        });

        if !essential_ok {
            self.set_mode(OperationalMode::Unavailable).await;
            return;
        }

        // Count unavailable non-essential features
        let unavailable_count = features
            .values()
            .filter(|f| !f.available || !f.enabled)
            .count();

        let new_mode = match unavailable_count {
            0 => OperationalMode::Normal,
            1..=2 => OperationalMode::Degraded,
            _ => OperationalMode::Minimal,
        };

        self.set_mode(new_mode).await;
    }

    async fn set_mode(&self, new_mode: OperationalMode) {
        let mut mode = self.mode.write().await;
        let mut last_change = self.last_mode_change.write().await;

        // Check cooldown
        if last_change.elapsed() < self.policy.mode_change_cooldown {
            return;
        }

        if *mode != new_mode {
            tracing::info!(
                old_mode = ?*mode,
                new_mode = ?new_mode,
                "Operational mode changed"
            );

            *mode = new_mode;
            *last_change = Instant::now();
        }
    }

    /// Create a guard for a feature operation
    pub async fn feature_guard(&self, feature: Feature) -> Option<FeatureGuard> {
        if self.is_available(feature).await {
            Some(FeatureGuard {
                feature,
                manager: self,
                succeeded: false,
            })
        } else {
            None
        }
    }
}

impl Default for DegradationManager {
    fn default() -> Self {
        Self::new(DegradationPolicy::default())
    }
}

/// Degradation summary
#[derive(Debug, Clone, serde::Serialize)]
pub struct DegradationSummary {
    pub mode: OperationalMode,
    pub available_features: usize,
    pub total_features: usize,
    #[serde(skip)]
    pub since: Duration,
    pub degraded_features: Vec<Feature>,
}

/// Guard for feature operations
pub struct FeatureGuard<'a> {
    feature: Feature,
    manager: &'a DegradationManager,
    succeeded: bool,
}

impl<'a> FeatureGuard<'a> {
    /// Mark the operation as successful
    pub fn success(mut self) {
        self.succeeded = true;
    }

    /// Mark the operation as failed
    pub async fn failure(self, reason: String) {
        self.manager.report_failure(self.feature, reason).await;
    }
}

impl<'a> Drop for FeatureGuard<'a> {
    fn drop(&mut self) {
        // If not explicitly marked as success, consider it a potential failure
        // (though we don't auto-report to avoid false positives from panics)
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_mode() {
        let manager = DegradationManager::default();
        assert_eq!(manager.mode().await, OperationalMode::Normal);
    }

    #[tokio::test]
    async fn test_feature_availability() {
        let manager = DegradationManager::default();

        assert!(manager.is_enabled(Feature::LocalProving).await);
        assert!(manager.is_available(Feature::LocalProving).await);
    }

    #[tokio::test]
    async fn test_degradation() {
        let policy = DegradationPolicy {
            mode_change_cooldown: Duration::from_millis(0),
            ..Default::default()
        };
        let manager = DegradationManager::new(policy);

        // Mark a non-essential feature as unavailable
        manager.mark_unavailable(Feature::P2PCoordination, "test".into()).await;

        assert!(!manager.is_available(Feature::P2PCoordination).await);
        assert_eq!(manager.mode().await, OperationalMode::Degraded);
    }

    #[tokio::test]
    async fn test_recovery() {
        let policy = DegradationPolicy {
            mode_change_cooldown: Duration::from_millis(0),
            ..Default::default()
        };
        let manager = DegradationManager::new(policy);

        manager.mark_unavailable(Feature::P2PCoordination, "test".into()).await;
        assert_eq!(manager.mode().await, OperationalMode::Degraded);

        manager.mark_recovered(Feature::P2PCoordination).await;
        assert!(manager.is_available(Feature::P2PCoordination).await);
        assert_eq!(manager.mode().await, OperationalMode::Normal);
    }

    #[tokio::test]
    async fn test_essential_feature_unavailable() {
        let policy = DegradationPolicy {
            mode_change_cooldown: Duration::from_millis(0),
            ..Default::default()
        };
        let manager = DegradationManager::new(policy);

        // Mark an essential feature as unavailable
        manager.mark_unavailable(Feature::LocalProving, "test".into()).await;

        assert_eq!(manager.mode().await, OperationalMode::Unavailable);
    }

    #[tokio::test]
    async fn test_failure_threshold() {
        let policy = DegradationPolicy {
            mode_change_cooldown: Duration::from_millis(0),
            ..Default::default()
        };
        let manager = DegradationManager::new(policy);

        // Report failures below threshold
        manager.report_failure(Feature::CloudProving, "error 1".into()).await;
        manager.report_failure(Feature::CloudProving, "error 2".into()).await;
        assert!(manager.is_available(Feature::CloudProving).await);

        // Third failure should trigger unavailability
        manager.report_failure(Feature::CloudProving, "error 3".into()).await;
        assert!(!manager.is_available(Feature::CloudProving).await);
    }

    #[tokio::test]
    async fn test_manual_disable() {
        let manager = DegradationManager::default();

        manager.disable_feature(Feature::Batching, "maintenance".into()).await;
        assert!(!manager.is_enabled(Feature::Batching).await);

        manager.enable_feature(Feature::Batching).await;
        assert!(manager.is_enabled(Feature::Batching).await);
    }

    #[test]
    fn test_operational_mode() {
        assert!(OperationalMode::Normal.is_operational());
        assert!(OperationalMode::Degraded.is_operational());
        assert!(OperationalMode::Minimal.is_operational());
        assert!(!OperationalMode::Unavailable.is_operational());

        assert!(OperationalMode::Normal.allows_new_jobs());
        assert!(OperationalMode::Degraded.allows_new_jobs());
        assert!(!OperationalMode::Minimal.allows_new_jobs());
    }

    #[test]
    fn test_feature_fallbacks() {
        assert_eq!(Feature::CloudProving.fallback(), Some(Feature::LocalProving));
        assert_eq!(Feature::LocalProving.fallback(), None);
    }

    #[tokio::test]
    async fn test_summary() {
        let manager = DegradationManager::default();
        let summary = manager.summary().await;

        assert_eq!(summary.mode, OperationalMode::Normal);
        assert_eq!(summary.available_features, summary.total_features);
        assert!(summary.degraded_features.is_empty());
    }
}
