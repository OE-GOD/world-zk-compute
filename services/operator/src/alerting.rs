//! Advanced alerting module with multi-channel routing, dedup, and escalation.
//!
//! Supports Slack webhooks, PagerDuty Events API v2, email (SMTP), and
//! generic webhooks. Alerts are routed by severity:
//!
//! - **Critical** -- PagerDuty + Slack + GenericWebhook
//! - **Warning** -- Slack + GenericWebhook
//! - **Info** -- log only (no external notification)
//!
//! Duplicate alerts (same `alert_type` + `source`) are suppressed within a
//! configurable dedup window (default 5 minutes). Un-acknowledged critical
//! alerts trigger escalation after the escalation timeout (default 15 minutes).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Alert Severity
// ---------------------------------------------------------------------------

/// Severity level for an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Critical,
    Warning,
    Info,
}

impl AlertSeverity {
    /// Returns a lowercase string label for the severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertSeverity::Critical => "critical",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Info => "info",
        }
    }
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Alert Channels
// ---------------------------------------------------------------------------

/// A notification channel that alerts can be routed to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AlertChannel {
    SlackWebhook {
        url: String,
        #[serde(default)]
        channel: Option<String>,
    },
    PagerDuty {
        routing_key: String,
        service_name: String,
    },
    Email {
        smtp_host: String,
        smtp_port: u16,
        from: String,
        to: Vec<String>,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    GenericWebhook {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

impl AlertChannel {
    /// Returns a human-readable label for this channel type.
    pub fn channel_type(&self) -> &'static str {
        match self {
            AlertChannel::SlackWebhook { .. } => "slack",
            AlertChannel::PagerDuty { .. } => "pagerduty",
            AlertChannel::Email { .. } => "email",
            AlertChannel::GenericWebhook { .. } => "generic_webhook",
        }
    }

    /// Returns true if this channel should receive critical alerts.
    fn receives_critical(&self) -> bool {
        matches!(
            self,
            AlertChannel::PagerDuty { .. }
                | AlertChannel::SlackWebhook { .. }
                | AlertChannel::GenericWebhook { .. }
        )
    }

    /// Returns true if this channel should receive warning alerts.
    fn receives_warning(&self) -> bool {
        matches!(
            self,
            AlertChannel::SlackWebhook { .. } | AlertChannel::GenericWebhook { .. }
        )
    }
}

impl std::fmt::Display for AlertChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertChannel::SlackWebhook { url, .. } => write!(f, "Slack({})", url),
            AlertChannel::PagerDuty { service_name, .. } => {
                write!(f, "PagerDuty({})", service_name)
            }
            AlertChannel::Email { from, .. } => write!(f, "Email({})", from),
            AlertChannel::GenericWebhook { url, .. } => write!(f, "Webhook({})", url),
        }
    }
}

// ---------------------------------------------------------------------------
// Alert Channel Sender Trait
// ---------------------------------------------------------------------------

/// Trait abstracting the actual sending of alerts to a channel.
///
/// Production implementations perform HTTP requests; tests use a mock that
/// records sent alerts without network access.
pub trait AlertSender: Send + Sync {
    /// Send `alert` to `channel`. Returns `Ok(())` on success.
    fn send(&self, channel: &AlertChannel, alert: &Alert) -> Result<(), String>;
}

/// Default sender that logs the alert but does not perform real HTTP calls.
/// Replace with a real implementation for production use.
#[derive(Debug, Default)]
pub struct LogOnlySender;

impl AlertSender for LogOnlySender {
    fn send(&self, channel: &AlertChannel, alert: &Alert) -> Result<(), String> {
        tracing::info!(
            channel_type = channel.channel_type(),
            alert_id = %alert.id,
            severity = %alert.severity,
            alert_type = %alert.alert_type,
            source = %alert.source,
            message = %alert.message,
            "Alert dispatched to channel (log-only sender)"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Alert
// ---------------------------------------------------------------------------

/// A single alert instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    /// Unique identifier (format: `alert-XXXXXXXX`).
    pub id: String,
    /// Severity level.
    pub severity: AlertSeverity,
    /// Machine-readable alert type, e.g. `"dispute_detected"`, `"rpc_error"`.
    pub alert_type: String,
    /// Origin component, e.g. `"operator"`, `"enclave"`.
    pub source: String,
    /// Human-readable description.
    pub message: String,
    /// Unix timestamp (seconds) when the alert was created.
    pub timestamp: u64,
    /// Arbitrary key-value metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Alert Config
// ---------------------------------------------------------------------------

/// Configuration for the alert manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    /// Configured notification channels.
    #[serde(default)]
    pub channels: Vec<AlertChannel>,
    /// Seconds within which duplicate alerts are suppressed (default: 300 = 5 min).
    #[serde(default = "default_dedup_window")]
    pub dedup_window_secs: u64,
    /// Seconds after which an un-acknowledged critical alert escalates (default: 900 = 15 min).
    #[serde(default = "default_escalation_timeout")]
    pub escalation_timeout_secs: u64,
}

fn default_dedup_window() -> u64 {
    300
}

fn default_escalation_timeout() -> u64 {
    900
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            channels: Vec::new(),
            dedup_window_secs: default_dedup_window(),
            escalation_timeout_secs: default_escalation_timeout(),
        }
    }
}

impl AlertConfig {
    /// Validate alert configuration values.
    ///
    /// Emits `tracing::warn!` for likely-unintended but technically valid settings.
    /// Returns `Ok(())` on success, `Err(Vec<String>)` for hard errors.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let errors: Vec<String> = Vec::new();

        // escalation_timeout_secs == 0 means immediate escalation on every
        // alert tick, which is almost certainly unintended.
        if self.escalation_timeout_secs == 0 {
            tracing::warn!(
                "escalation_timeout_secs is 0 -- this causes immediate escalation on every \
                 tick, which is likely unintended. Set a positive value (e.g. 900 for 15 min)."
            );
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// ---------------------------------------------------------------------------
// Alert Stats
// ---------------------------------------------------------------------------

/// Aggregate statistics about alert activity.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlertStats {
    pub total_sent: u64,
    pub total_suppressed: u64,
    pub total_acknowledged: u64,
    pub by_severity: HashMap<String, u64>,
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Dedup key: (alert_type, source).
type DedupKey = (String, String);

/// Tracks a sent alert for dedup and escalation.
#[derive(Debug, Clone)]
struct TrackedAlert {
    alert: Alert,
    acknowledged: bool,
    /// Unix timestamp when escalation was last attempted (or first sent).
    last_escalation: u64,
}

/// Interior-mutable state protected by a mutex.
#[derive(Debug, Default)]
struct AlertState {
    /// All tracked alerts by alert ID.
    alerts: HashMap<String, TrackedAlert>,
    /// Most recent alert timestamp by (alert_type, source) for dedup.
    dedup_map: HashMap<DedupKey, u64>,
    /// Running statistics.
    stats: AlertStats,
    /// Monotonic counter for generating unique IDs.
    next_id: u64,
}

// ---------------------------------------------------------------------------
// AlertManager
// ---------------------------------------------------------------------------

/// Manages multi-channel alert routing, dedup, and escalation.
pub struct AlertManager {
    config: AlertConfig,
    sender: Box<dyn AlertSender>,
    state: Mutex<AlertState>,
}

impl AlertManager {
    /// Create a new `AlertManager` with the given config and the default
    /// log-only sender. Use [`AlertManager::with_sender`] to supply a
    /// real HTTP sender.
    pub fn new(config: AlertConfig) -> Self {
        Self::with_sender(config, Box::new(LogOnlySender))
    }

    /// Create a new `AlertManager` with a custom sender (useful for testing).
    pub fn with_sender(config: AlertConfig, sender: Box<dyn AlertSender>) -> Self {
        Self {
            config,
            sender,
            state: Mutex::new(AlertState::default()),
        }
    }

    /// Send an alert. Returns the alert ID on success, or an error description.
    ///
    /// Routing rules:
    /// - **Critical**: PagerDuty + Slack + GenericWebhook
    /// - **Warning**: Slack + GenericWebhook
    /// - **Info**: log only (no channel dispatch)
    ///
    /// Duplicate alerts (same `alert_type` + `source`) within the dedup window
    /// are suppressed and an `Err` is returned.
    pub fn send_alert(
        &self,
        severity: AlertSeverity,
        alert_type: &str,
        source: &str,
        message: &str,
        metadata: HashMap<String, String>,
    ) -> Result<String, String> {
        let now = now_secs();
        let mut state = self.state.lock().map_err(|e| format!("lock error: {e}"))?;

        // --- Dedup check ---
        let dedup_key = (alert_type.to_string(), source.to_string());
        if let Some(&last_ts) = state.dedup_map.get(&dedup_key) {
            if now.saturating_sub(last_ts) < self.config.dedup_window_secs {
                state.stats.total_suppressed += 1;
                return Err(format!(
                    "alert suppressed (dedup): type={alert_type} source={source}"
                ));
            }
        }

        // --- Build alert ---
        state.next_id += 1;
        let alert_id = format!("alert-{:08x}", state.next_id);

        let alert = Alert {
            id: alert_id.clone(),
            severity,
            alert_type: alert_type.to_string(),
            source: source.to_string(),
            message: message.to_string(),
            timestamp: now,
            metadata,
        };

        // --- Route to channels ---
        let channels_to_send: Vec<&AlertChannel> = match severity {
            AlertSeverity::Critical => self
                .config
                .channels
                .iter()
                .filter(|ch| ch.receives_critical())
                .collect(),
            AlertSeverity::Warning => self
                .config
                .channels
                .iter()
                .filter(|ch| ch.receives_warning())
                .collect(),
            AlertSeverity::Info => {
                // Info alerts are logged only, not dispatched to channels.
                tracing::info!(
                    alert_id = %alert_id,
                    alert_type = %alert_type,
                    source = %source,
                    message = %message,
                    "Alert [INFO] (log only)"
                );
                Vec::new()
            }
        };

        // Dispatch (errors are logged but do not block the alert from being tracked).
        for ch in &channels_to_send {
            if let Err(e) = self.sender.send(ch, &alert) {
                tracing::warn!(
                    channel = ch.channel_type(),
                    alert_id = %alert.id,
                    error = %e,
                    "Failed to send alert to channel"
                );
            }
        }

        // --- Track ---
        state.dedup_map.insert(dedup_key, now);
        state.alerts.insert(
            alert_id.clone(),
            TrackedAlert {
                alert,
                acknowledged: false,
                last_escalation: now,
            },
        );

        // Update stats.
        state.stats.total_sent += 1;
        *state
            .stats
            .by_severity
            .entry(severity.as_str().to_string())
            .or_insert(0) += 1;

        Ok(alert_id)
    }

    /// Acknowledge an alert by ID. Returns `true` if the alert was found and
    /// not already acknowledged; `false` otherwise.
    pub fn acknowledge(&self, alert_id: &str) -> bool {
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return false,
        };
        if let Some(tracked) = state.alerts.get_mut(alert_id) {
            if tracked.acknowledged {
                return false;
            }
            tracked.acknowledged = true;
            state.stats.total_acknowledged += 1;
            true
        } else {
            false
        }
    }

    /// Returns all active (un-acknowledged) alerts, ordered by timestamp descending.
    pub fn get_active_alerts(&self) -> Vec<Alert> {
        let state = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut active: Vec<Alert> = state
            .alerts
            .values()
            .filter(|t| !t.acknowledged)
            .map(|t| t.alert.clone())
            .collect();
        active.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        active
    }

    /// Returns aggregate statistics.
    pub fn get_alert_stats(&self) -> AlertStats {
        match self.state.lock() {
            Ok(s) => s.stats.clone(),
            Err(_) => AlertStats::default(),
        }
    }

    /// Check for un-acknowledged critical alerts past the escalation timeout
    /// and re-send them to all channels. Returns the number of escalated alerts.
    pub fn check_escalations(&self) -> usize {
        let now = now_secs();
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let mut to_escalate: Vec<Alert> = Vec::new();
        let mut ids_to_update: Vec<String> = Vec::new();

        for (id, tracked) in state.alerts.iter() {
            if tracked.acknowledged {
                continue;
            }
            if tracked.alert.severity != AlertSeverity::Critical {
                continue;
            }
            if now.saturating_sub(tracked.last_escalation) >= self.config.escalation_timeout_secs {
                to_escalate.push(tracked.alert.clone());
                ids_to_update.push(id.clone());
            }
        }

        // Update last_escalation timestamps.
        for id in &ids_to_update {
            if let Some(tracked) = state.alerts.get_mut(id) {
                tracked.last_escalation = now;
            }
        }

        // Re-send to all channels. Drop the lock before I/O to avoid holding
        // it during potentially slow sender calls.
        let channels: Vec<AlertChannel> = self.config.channels.clone();
        drop(state);

        for alert in &to_escalate {
            for ch in &channels {
                if let Err(e) = self.sender.send(ch, alert) {
                    tracing::warn!(
                        channel = ch.channel_type(),
                        alert_id = %alert.id,
                        error = %e,
                        "Failed to escalate alert to channel"
                    );
                }
            }
        }

        to_escalate.len()
    }

    /// Returns the number of un-acknowledged critical alerts that are past
    /// the escalation timeout. Useful for monitoring whether escalation is
    /// needed without actually sending anything.
    pub fn pending_escalations(&self) -> usize {
        let now = now_secs();
        let state = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return 0,
        };
        state
            .alerts
            .values()
            .filter(|t| {
                !t.acknowledged
                    && t.alert.severity == AlertSeverity::Critical
                    && now.saturating_sub(t.last_escalation) >= self.config.escalation_timeout_secs
            })
            .count()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    /// A mock sender that records every (channel_type, alert_id) pair sent.
    #[derive(Debug, Default, Clone)]
    struct MockSender {
        sent: Arc<StdMutex<Vec<(String, String)>>>,
    }

    impl AlertSender for MockSender {
        fn send(&self, channel: &AlertChannel, alert: &Alert) -> Result<(), String> {
            let mut log = self.sent.lock().expect("mock sender lock poisoned");
            log.push((channel.channel_type().to_string(), alert.id.clone()));
            Ok(())
        }
    }

    /// Helper: build a config with one of each channel type.
    fn full_config() -> AlertConfig {
        AlertConfig {
            channels: vec![
                AlertChannel::SlackWebhook {
                    url: "https://hooks.slack.com/test".into(),
                    channel: Some("#alerts".into()),
                },
                AlertChannel::PagerDuty {
                    routing_key: "test-key".into(),
                    service_name: "operator".into(),
                },
                AlertChannel::Email {
                    smtp_host: "smtp.example.com".into(),
                    smtp_port: 587,
                    from: "alerts@example.com".into(),
                    to: vec!["oncall@example.com".into()],
                    username: None,
                    password: None,
                },
                AlertChannel::GenericWebhook {
                    url: "https://webhooks.example.com/alerts".into(),
                    headers: HashMap::new(),
                },
            ],
            dedup_window_secs: 300,
            escalation_timeout_secs: 900,
        }
    }

    // ------------------------------------------------------------------
    // 1. test_alert_routing_critical
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_routing_critical() {
        let mock = MockSender::default();
        let mgr = AlertManager::with_sender(full_config(), Box::new(mock.clone()));

        let id = mgr
            .send_alert(
                AlertSeverity::Critical,
                "dispute_detected",
                "operator",
                "Dispute detected on result 0xabc",
                HashMap::new(),
            )
            .expect("should send");

        let sent = mock.sent.lock().expect("mock sender lock poisoned");
        // Critical routes to: Slack, PagerDuty, GenericWebhook (NOT Email).
        let channel_types: Vec<&str> = sent.iter().map(|(ct, _)| ct.as_str()).collect();
        assert!(
            channel_types.contains(&"slack"),
            "critical should route to slack"
        );
        assert!(
            channel_types.contains(&"pagerduty"),
            "critical should route to pagerduty"
        );
        assert!(
            channel_types.contains(&"generic_webhook"),
            "critical should route to generic_webhook"
        );
        assert!(
            !channel_types.contains(&"email"),
            "critical should NOT route to email"
        );
        assert_eq!(sent.len(), 3, "critical should send to exactly 3 channels");
        // All entries should reference our alert.
        assert!(sent.iter().all(|(_, aid)| aid == &id));
    }

    // ------------------------------------------------------------------
    // 2. test_alert_routing_warning
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_routing_warning() {
        let mock = MockSender::default();
        let mgr = AlertManager::with_sender(full_config(), Box::new(mock.clone()));

        mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_error",
            "operator",
            "RPC endpoint returned 503",
            HashMap::new(),
        )
        .expect("should send");

        let sent = mock.sent.lock().expect("mock sender lock poisoned");
        let channel_types: Vec<&str> = sent.iter().map(|(ct, _)| ct.as_str()).collect();
        assert!(
            channel_types.contains(&"slack"),
            "warning should route to slack"
        );
        assert!(
            channel_types.contains(&"generic_webhook"),
            "warning should route to generic_webhook"
        );
        assert!(
            !channel_types.contains(&"pagerduty"),
            "warning should NOT route to pagerduty"
        );
        assert!(
            !channel_types.contains(&"email"),
            "warning should NOT route to email"
        );
        assert_eq!(sent.len(), 2);
    }

    // ------------------------------------------------------------------
    // 3. test_alert_routing_info
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_routing_info() {
        let mock = MockSender::default();
        let mgr = AlertManager::with_sender(full_config(), Box::new(mock.clone()));

        mgr.send_alert(
            AlertSeverity::Info,
            "heartbeat",
            "enclave",
            "Enclave is healthy",
            HashMap::new(),
        )
        .expect("should send");

        let sent = mock.sent.lock().expect("mock sender lock poisoned");
        assert!(
            sent.is_empty(),
            "info alerts should not be dispatched to any channel"
        );
    }

    // ------------------------------------------------------------------
    // 4. test_alert_dedup
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_dedup() {
        let mock = MockSender::default();
        let config = AlertConfig {
            channels: vec![AlertChannel::SlackWebhook {
                url: "https://hooks.slack.com/test".into(),
                channel: None,
            }],
            dedup_window_secs: 300,
            escalation_timeout_secs: 900,
        };
        let mgr = AlertManager::with_sender(config, Box::new(mock.clone()));

        // First alert should succeed.
        let r1 = mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_error",
            "operator",
            "RPC error #1",
            HashMap::new(),
        );
        assert!(r1.is_ok(), "first alert should succeed");

        // Second alert with same type+source should be suppressed.
        let r2 = mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_error",
            "operator",
            "RPC error #2",
            HashMap::new(),
        );
        assert!(r2.is_err(), "duplicate alert should be suppressed");
        assert!(
            r2.unwrap_err().contains("suppressed"),
            "error should mention suppression"
        );

        // Different alert_type should NOT be suppressed.
        let r3 = mgr.send_alert(
            AlertSeverity::Warning,
            "proof_failed",
            "operator",
            "Proof generation failed",
            HashMap::new(),
        );
        assert!(r3.is_ok(), "different alert_type should not be suppressed");

        // Different source should NOT be suppressed.
        let r4 = mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_error",
            "enclave",
            "RPC error from enclave",
            HashMap::new(),
        );
        assert!(r4.is_ok(), "different source should not be suppressed");

        // Verify stats.
        let stats = mgr.get_alert_stats();
        assert_eq!(stats.total_sent, 3);
        assert_eq!(stats.total_suppressed, 1);
    }

    // ------------------------------------------------------------------
    // 5. test_alert_acknowledge
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_acknowledge() {
        let mgr = AlertManager::new(AlertConfig::default());

        let id = mgr
            .send_alert(
                AlertSeverity::Critical,
                "dispute_detected",
                "operator",
                "Dispute on 0xabc",
                HashMap::new(),
            )
            .expect("should send");

        // Active alerts should contain our alert.
        let active = mgr.get_active_alerts();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id);

        // Acknowledge.
        assert!(mgr.acknowledge(&id), "acknowledge should return true");

        // After acknowledgment, active alerts should be empty.
        let active = mgr.get_active_alerts();
        assert!(active.is_empty(), "acknowledged alert should not be active");

        // Double-acknowledge should return false.
        assert!(
            !mgr.acknowledge(&id),
            "double-acknowledge should return false"
        );

        // Non-existent alert.
        assert!(
            !mgr.acknowledge("does-not-exist"),
            "non-existent alert should return false"
        );

        // Stats.
        let stats = mgr.get_alert_stats();
        assert_eq!(stats.total_acknowledged, 1);
    }

    // ------------------------------------------------------------------
    // 6. test_alert_stats
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_stats() {
        let mock = MockSender::default();
        let config = AlertConfig {
            dedup_window_secs: 0, // disable dedup for this test
            ..full_config()
        };
        let mgr = AlertManager::with_sender(config, Box::new(mock));

        mgr.send_alert(
            AlertSeverity::Critical,
            "dispute_detected",
            "op",
            "msg",
            HashMap::new(),
        )
        .expect("critical alert should send");
        mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_error",
            "op",
            "msg",
            HashMap::new(),
        )
        .expect("warning alert should send");
        mgr.send_alert(
            AlertSeverity::Info,
            "heartbeat",
            "op",
            "msg",
            HashMap::new(),
        )
        .expect("info alert should send");
        mgr.send_alert(
            AlertSeverity::Critical,
            "proof_failed",
            "enc",
            "msg",
            HashMap::new(),
        )
        .expect("second critical alert should send");

        let stats = mgr.get_alert_stats();
        assert_eq!(stats.total_sent, 4);
        assert_eq!(stats.total_suppressed, 0);
        assert_eq!(*stats.by_severity.get("critical").unwrap_or(&0), 2);
        assert_eq!(*stats.by_severity.get("warning").unwrap_or(&0), 1);
        assert_eq!(*stats.by_severity.get("info").unwrap_or(&0), 1);
    }

    // ------------------------------------------------------------------
    // 7. test_config_defaults
    // ------------------------------------------------------------------
    #[test]
    fn test_config_defaults() {
        let config = AlertConfig::default();
        assert!(config.channels.is_empty());
        assert_eq!(config.dedup_window_secs, 300);
        assert_eq!(config.escalation_timeout_secs, 900);

        // Also verify serde round-trip with defaults.
        let json = r#"{"channels":[]}"#;
        let parsed: AlertConfig =
            serde_json::from_str(json).expect("AlertConfig should deserialize from valid JSON");
        assert_eq!(parsed.dedup_window_secs, 300);
        assert_eq!(parsed.escalation_timeout_secs, 900);
    }

    // ------------------------------------------------------------------
    // 8. test_escalation_tracking
    // ------------------------------------------------------------------
    #[test]
    fn test_escalation_tracking() {
        let mock = MockSender::default();
        let config = AlertConfig {
            channels: vec![AlertChannel::SlackWebhook {
                url: "https://hooks.slack.com/test".into(),
                channel: None,
            }],
            dedup_window_secs: 300,
            // Set to 0 so escalation triggers immediately.
            escalation_timeout_secs: 0,
        };
        let mgr = AlertManager::with_sender(config, Box::new(mock.clone()));

        // Send a critical alert.
        let id = mgr
            .send_alert(
                AlertSeverity::Critical,
                "dispute_detected",
                "operator",
                "Critical dispute",
                HashMap::new(),
            )
            .expect("should send");

        // With timeout=0, the alert should be eligible for escalation immediately.
        let pending = mgr.pending_escalations();
        assert!(
            pending >= 1,
            "un-acked critical alert should be pending escalation"
        );

        // Check escalations -- should re-send.
        let escalated = mgr.check_escalations();
        assert_eq!(escalated, 1, "one alert should be escalated");

        // After acknowledgment, no more escalations.
        mgr.acknowledge(&id);
        let pending = mgr.pending_escalations();
        assert_eq!(
            pending, 0,
            "acknowledged alert should not be pending escalation"
        );

        // Warning alert should never escalate.
        mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_warning",
            "enclave",
            "Warning",
            HashMap::new(),
        )
        .expect("warning alert should send");
        let pending = mgr.pending_escalations();
        assert_eq!(pending, 0, "warning alerts should not escalate");
    }

    // ------------------------------------------------------------------
    // 9. test_alert_metadata
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_metadata() {
        let mgr = AlertManager::new(AlertConfig::default());

        let mut meta = HashMap::new();
        meta.insert("result_id".to_string(), "0xdeadbeef".to_string());
        meta.insert("chain_id".to_string(), "11155111".to_string());

        let id = mgr
            .send_alert(
                AlertSeverity::Warning,
                "proof_failed",
                "operator",
                "Proof generation timeout",
                meta.clone(),
            )
            .expect("alert with metadata should send");

        let active = mgr.get_active_alerts();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id);
        assert_eq!(
            active[0]
                .metadata
                .get("result_id")
                .expect("result_id should be in metadata"),
            "0xdeadbeef"
        );
        assert_eq!(
            active[0]
                .metadata
                .get("chain_id")
                .expect("chain_id should be in metadata"),
            "11155111"
        );
    }

    // ------------------------------------------------------------------
    // 10. test_alert_serialization
    // ------------------------------------------------------------------
    #[test]
    fn test_alert_serialization() {
        let alert = Alert {
            id: "alert-00000001".into(),
            severity: AlertSeverity::Critical,
            alert_type: "dispute_detected".into(),
            source: "operator".into(),
            message: "Dispute on result 0xabc".into(),
            timestamp: 1700000000,
            metadata: {
                let mut m = HashMap::new();
                m.insert("key".into(), "value".into());
                m
            },
        };

        let json = serde_json::to_string(&alert).expect("Alert should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("serialized Alert should parse as JSON Value");

        assert_eq!(parsed["id"], "alert-00000001");
        assert_eq!(parsed["severity"], "critical");
        assert_eq!(parsed["alert_type"], "dispute_detected");
        assert_eq!(parsed["source"], "operator");
        assert_eq!(parsed["timestamp"], 1700000000);
        assert_eq!(parsed["metadata"]["key"], "value");
    }

    // ------------------------------------------------------------------
    // 11. test_channel_serialization
    // ------------------------------------------------------------------
    #[test]
    fn test_channel_serialization() {
        let channel = AlertChannel::PagerDuty {
            routing_key: "abc123".into(),
            service_name: "operator".into(),
        };
        let json = serde_json::to_string(&channel).expect("AlertChannel should serialize to JSON");
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .expect("serialized AlertChannel should parse as JSON Value");
        assert_eq!(parsed["type"], "pager_duty");
        assert_eq!(parsed["routing_key"], "abc123");
        assert_eq!(parsed["service_name"], "operator");

        // Round-trip.
        let deser: AlertChannel =
            serde_json::from_str(&json).expect("AlertChannel should round-trip through JSON");
        assert_eq!(deser.channel_type(), "pagerduty");
    }

    // ------------------------------------------------------------------
    // 12. test_sender_failure_does_not_block
    // ------------------------------------------------------------------
    #[test]
    fn test_sender_failure_does_not_block() {
        /// A sender that always fails.
        struct FailingSender;
        impl AlertSender for FailingSender {
            fn send(&self, _channel: &AlertChannel, _alert: &Alert) -> Result<(), String> {
                Err("network timeout".into())
            }
        }

        let config = AlertConfig {
            channels: vec![AlertChannel::SlackWebhook {
                url: "https://hooks.slack.com/test".into(),
                channel: None,
            }],
            ..AlertConfig::default()
        };
        let mgr = AlertManager::with_sender(config, Box::new(FailingSender));

        // Even though the sender fails, the alert should still be tracked.
        let result = mgr.send_alert(
            AlertSeverity::Warning,
            "rpc_error",
            "operator",
            "RPC error",
            HashMap::new(),
        );
        assert!(result.is_ok(), "sender failure should not prevent tracking");

        let stats = mgr.get_alert_stats();
        assert_eq!(stats.total_sent, 1);

        let active = mgr.get_active_alerts();
        assert_eq!(active.len(), 1);
    }
}
