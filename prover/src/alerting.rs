//! Alerting System
//!
//! Sends alerts when critical events occur:
//! - Proof generation failures
//! - Transaction submission failures
//! - High error rates
//! - Resource exhaustion
//!
//! Supports multiple backends: Slack, PagerDuty, webhooks, logs

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational - no action needed
    Info,
    /// Warning - investigate when convenient
    Warning,
    /// Error - investigate soon
    Error,
    /// Critical - immediate action required
    Critical,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
            Self::Critical => "CRITICAL",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Info => "ℹ️",
            Self::Warning => "⚠️",
            Self::Error => "🔴",
            Self::Critical => "🚨",
        }
    }
}

/// Alert types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlertType {
    /// Proof generation failed
    ProofFailure,
    /// Transaction submission failed
    TransactionFailure,
    /// High error rate detected
    HighErrorRate,
    /// RPC connection issues
    RpcConnectionError,
    /// Memory usage high
    HighMemoryUsage,
    /// Disk usage high
    HighDiskUsage,
    /// Queue backlog growing
    QueueBacklog,
    /// No jobs processed recently
    IdleProver,
    /// Nonce synchronization issue
    NonceSyncError,
    /// Cache hit rate low
    LowCacheHitRate,
    /// Custom alert
    Custom(String),
}

impl AlertType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ProofFailure => "proof_failure",
            Self::TransactionFailure => "transaction_failure",
            Self::HighErrorRate => "high_error_rate",
            Self::RpcConnectionError => "rpc_connection_error",
            Self::HighMemoryUsage => "high_memory_usage",
            Self::HighDiskUsage => "high_disk_usage",
            Self::QueueBacklog => "queue_backlog",
            Self::IdleProver => "idle_prover",
            Self::NonceSyncError => "nonce_sync_error",
            Self::LowCacheHitRate => "low_cache_hit_rate",
            Self::Custom(name) => name,
        }
    }
}

/// An alert to be sent
#[derive(Debug, Clone)]
pub struct Alert {
    pub alert_type: AlertType,
    pub severity: Severity,
    pub title: String,
    pub message: String,
    pub details: HashMap<String, String>,
    pub timestamp: Instant,
}

impl Alert {
    pub fn new(alert_type: AlertType, severity: Severity, title: &str, message: &str) -> Self {
        Self {
            alert_type,
            severity,
            title: title.to_string(),
            message: message.to_string(),
            details: HashMap::new(),
            timestamp: Instant::now(),
        }
    }

    pub fn with_detail(mut self, key: &str, value: &str) -> Self {
        self.details.insert(key.to_string(), value.to_string());
        self
    }

    /// Format for Slack
    pub fn to_slack_message(&self) -> String {
        let mut msg = format!(
            "{} *{}* - {}\n\n{}",
            self.severity.emoji(),
            self.severity.as_str(),
            self.title,
            self.message
        );

        if !self.details.is_empty() {
            msg.push_str("\n\n*Details:*\n");
            for (key, value) in &self.details {
                msg.push_str(&format!("• {}: {}\n", key, value));
            }
        }

        msg
    }

    /// Format for PagerDuty
    pub fn to_pagerduty_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "routing_key": "", // Filled in by sender
            "event_action": "trigger",
            "dedup_key": format!("{}_{}", self.alert_type.as_str(), self.title),
            "payload": {
                "summary": format!("{}: {}", self.severity.as_str(), self.title),
                "severity": match self.severity {
                    Severity::Critical => "critical",
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "info",
                },
                "source": "world-zk-prover",
                "custom_details": self.details,
            }
        })
    }
}

/// Alert backend configuration
#[derive(Debug, Clone)]
pub enum AlertBackend {
    /// Log alerts (always enabled as fallback)
    Log,
    /// Send to Slack webhook
    Slack { webhook_url: String },
    /// Send to PagerDuty
    PagerDuty { routing_key: String },
    /// Send to generic webhook
    Webhook { url: String, headers: HashMap<String, String> },
}

/// Alerting configuration
#[derive(Debug, Clone)]
pub struct AlertConfig {
    /// Alert backends to use
    pub backends: Vec<AlertBackend>,
    /// Minimum severity to alert on
    pub min_severity: Severity,
    /// Cooldown between same alert type (prevent spam)
    pub cooldown: Duration,
    /// Error rate threshold (percentage)
    pub error_rate_threshold: f64,
    /// Memory usage threshold (percentage)
    pub memory_threshold: f64,
    /// Disk usage threshold (percentage)
    pub disk_threshold: f64,
    /// Queue size threshold
    pub queue_threshold: usize,
    /// Idle timeout (no jobs processed)
    pub idle_timeout: Duration,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            backends: vec![AlertBackend::Log],
            min_severity: Severity::Warning,
            cooldown: Duration::from_secs(300), // 5 minutes
            error_rate_threshold: 10.0,         // 10% error rate
            memory_threshold: 90.0,             // 90% memory
            disk_threshold: 85.0,               // 85% disk
            queue_threshold: 100,               // 100 pending jobs
            idle_timeout: Duration::from_secs(3600), // 1 hour
        }
    }
}

impl AlertConfig {
    /// Create config from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Slack webhook
        if let Ok(url) = std::env::var("SLACK_WEBHOOK_URL") {
            config.backends.push(AlertBackend::Slack { webhook_url: url });
        }

        // PagerDuty
        if let Ok(key) = std::env::var("PAGERDUTY_ROUTING_KEY") {
            config.backends.push(AlertBackend::PagerDuty { routing_key: key });
        }

        // Generic webhook
        if let Ok(url) = std::env::var("ALERT_WEBHOOK_URL") {
            config.backends.push(AlertBackend::Webhook {
                url,
                headers: HashMap::new(),
            });
        }

        // Thresholds
        if let Ok(val) = std::env::var("ALERT_ERROR_RATE_THRESHOLD") {
            if let Ok(threshold) = val.parse() {
                config.error_rate_threshold = threshold;
            }
        }

        config
    }
}

/// Alert manager
pub struct AlertManager {
    config: AlertConfig,
    http_client: reqwest::Client,
    /// Last alert time by type (for cooldown)
    last_alerts: RwLock<HashMap<String, Instant>>,
    /// Alert counters
    alerts_sent: AtomicU64,
    alerts_suppressed: AtomicU64,
}

impl AlertManager {
    /// Create a new alert manager
    pub fn new(config: AlertConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            http_client: reqwest::Client::new(),
            last_alerts: RwLock::new(HashMap::new()),
            alerts_sent: AtomicU64::new(0),
            alerts_suppressed: AtomicU64::new(0),
        })
    }

    /// Check if alert should be sent (cooldown check)
    async fn should_send(&self, alert: &Alert) -> bool {
        // Check severity
        if alert.severity < self.config.min_severity {
            return false;
        }

        // Check cooldown
        let key = format!("{}_{}", alert.alert_type.as_str(), alert.title);
        let last_alerts = self.last_alerts.read().await;

        if let Some(last_time) = last_alerts.get(&key) {
            if last_time.elapsed() < self.config.cooldown {
                return false;
            }
        }

        true
    }

    /// Record that an alert was sent
    async fn record_sent(&self, alert: &Alert) {
        let key = format!("{}_{}", alert.alert_type.as_str(), alert.title);
        let mut last_alerts = self.last_alerts.write().await;
        last_alerts.insert(key, Instant::now());
        self.alerts_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Send an alert
    pub async fn send(&self, alert: Alert) {
        if !self.should_send(&alert).await {
            self.alerts_suppressed.fetch_add(1, Ordering::Relaxed);
            return;
        }

        for backend in &self.config.backends {
            match backend {
                AlertBackend::Log => {
                    self.send_log(&alert);
                }
                AlertBackend::Slack { webhook_url } => {
                    if let Err(e) = self.send_slack(&alert, webhook_url).await {
                        error!("Failed to send Slack alert: {}", e);
                    }
                }
                AlertBackend::PagerDuty { routing_key } => {
                    if let Err(e) = self.send_pagerduty(&alert, routing_key).await {
                        error!("Failed to send PagerDuty alert: {}", e);
                    }
                }
                AlertBackend::Webhook { url, headers } => {
                    if let Err(e) = self.send_webhook(&alert, url, headers).await {
                        error!("Failed to send webhook alert: {}", e);
                    }
                }
            }
        }

        self.record_sent(&alert).await;
    }

    /// Send to log (fallback)
    fn send_log(&self, alert: &Alert) {
        match alert.severity {
            Severity::Info => info!("[ALERT] {}: {}", alert.title, alert.message),
            Severity::Warning => warn!("[ALERT] {}: {}", alert.title, alert.message),
            Severity::Error | Severity::Critical => {
                error!("[ALERT] {}: {}", alert.title, alert.message)
            }
        }
    }

    /// Send to Slack
    async fn send_slack(&self, alert: &Alert, webhook_url: &str) -> Result<(), reqwest::Error> {
        let payload = serde_json::json!({
            "text": alert.to_slack_message(),
        });

        self.http_client
            .post(webhook_url)
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    /// Send to PagerDuty
    async fn send_pagerduty(
        &self,
        alert: &Alert,
        routing_key: &str,
    ) -> Result<(), reqwest::Error> {
        let mut payload = alert.to_pagerduty_payload();
        payload["routing_key"] = serde_json::Value::String(routing_key.to_string());

        self.http_client
            .post("https://events.pagerduty.com/v2/enqueue")
            .json(&payload)
            .send()
            .await?;

        Ok(())
    }

    /// Send to generic webhook
    async fn send_webhook(
        &self,
        alert: &Alert,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<(), reqwest::Error> {
        let payload = serde_json::json!({
            "type": alert.alert_type.as_str(),
            "severity": alert.severity.as_str(),
            "title": alert.title,
            "message": alert.message,
            "details": alert.details,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        let mut request = self.http_client.post(url).json(&payload);

        for (key, value) in headers {
            request = request.header(key, value);
        }

        request.send().await?;

        Ok(())
    }

    /// Get alert statistics
    pub fn stats(&self) -> AlertStats {
        AlertStats {
            alerts_sent: self.alerts_sent.load(Ordering::Relaxed),
            alerts_suppressed: self.alerts_suppressed.load(Ordering::Relaxed),
        }
    }

    // === Convenience methods for common alerts ===

    /// Alert on proof failure
    pub async fn proof_failure(&self, task_id: &str, error: &str) {
        self.send(
            Alert::new(
                AlertType::ProofFailure,
                Severity::Error,
                "Proof Generation Failed",
                &format!("Failed to generate proof for task {}", task_id),
            )
            .with_detail("task_id", task_id)
            .with_detail("error", error),
        )
        .await;
    }

    /// Alert on transaction failure
    pub async fn transaction_failure(&self, tx_type: &str, error: &str) {
        self.send(
            Alert::new(
                AlertType::TransactionFailure,
                Severity::Error,
                "Transaction Failed",
                &format!("{} transaction failed", tx_type),
            )
            .with_detail("tx_type", tx_type)
            .with_detail("error", error),
        )
        .await;
    }

    /// Alert on high error rate
    pub async fn high_error_rate(&self, rate: f64, window: &str) {
        self.send(
            Alert::new(
                AlertType::HighErrorRate,
                Severity::Critical,
                "High Error Rate Detected",
                &format!("Error rate is {:.1}% over {}", rate, window),
            )
            .with_detail("error_rate", &format!("{:.1}%", rate))
            .with_detail("window", window),
        )
        .await;
    }

    /// Alert on RPC connection issues
    pub async fn rpc_connection_error(&self, endpoint: &str, error: &str) {
        self.send(
            Alert::new(
                AlertType::RpcConnectionError,
                Severity::Warning,
                "RPC Connection Error",
                &format!("Failed to connect to RPC endpoint"),
            )
            .with_detail("endpoint", endpoint)
            .with_detail("error", error),
        )
        .await;
    }

    /// Alert on high memory usage
    pub async fn high_memory_usage(&self, usage_percent: f64) {
        self.send(
            Alert::new(
                AlertType::HighMemoryUsage,
                if usage_percent > 95.0 {
                    Severity::Critical
                } else {
                    Severity::Warning
                },
                "High Memory Usage",
                &format!("Memory usage is at {:.1}%", usage_percent),
            )
            .with_detail("usage", &format!("{:.1}%", usage_percent)),
        )
        .await;
    }

    /// Alert on queue backlog
    pub async fn queue_backlog(&self, queue_size: usize) {
        self.send(
            Alert::new(
                AlertType::QueueBacklog,
                Severity::Warning,
                "Queue Backlog Growing",
                &format!("Job queue has {} pending items", queue_size),
            )
            .with_detail("queue_size", &queue_size.to_string()),
        )
        .await;
    }

    /// Alert on idle prover
    pub async fn idle_prover(&self, idle_duration: Duration) {
        self.send(
            Alert::new(
                AlertType::IdleProver,
                Severity::Info,
                "Prover Idle",
                &format!(
                    "No jobs processed for {:.0} minutes",
                    idle_duration.as_secs_f64() / 60.0
                ),
            )
            .with_detail("idle_minutes", &format!("{:.0}", idle_duration.as_secs_f64() / 60.0)),
        )
        .await;
    }

    /// Alert on nonce sync error
    pub async fn nonce_sync_error(&self, expected: u64, actual: u64) {
        self.send(
            Alert::new(
                AlertType::NonceSyncError,
                Severity::Warning,
                "Nonce Synchronization Issue",
                &format!("Local nonce {} doesn't match chain nonce {}", expected, actual),
            )
            .with_detail("expected", &expected.to_string())
            .with_detail("actual", &actual.to_string()),
        )
        .await;
    }
}

/// Alert statistics
#[derive(Debug, Clone)]
pub struct AlertStats {
    pub alerts_sent: u64,
    pub alerts_suppressed: u64,
}

/// Global alert manager instance
static ALERT_MANAGER: once_cell::sync::OnceCell<Arc<AlertManager>> = once_cell::sync::OnceCell::new();

/// Initialize the global alert manager
pub fn init_alerting(config: AlertConfig) -> Arc<AlertManager> {
    ALERT_MANAGER.get_or_init(|| AlertManager::new(config)).clone()
}

/// Get the global alert manager
pub fn alerts() -> Arc<AlertManager> {
    ALERT_MANAGER
        .get()
        .cloned()
        .unwrap_or_else(|| init_alerting(AlertConfig::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Error < Severity::Critical);
    }

    #[test]
    fn test_alert_creation() {
        let alert = Alert::new(
            AlertType::ProofFailure,
            Severity::Error,
            "Test Alert",
            "Test message",
        )
        .with_detail("key", "value");

        assert_eq!(alert.title, "Test Alert");
        assert_eq!(alert.details.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_slack_format() {
        let alert = Alert::new(
            AlertType::ProofFailure,
            Severity::Error,
            "Test",
            "Message",
        );

        let msg = alert.to_slack_message();
        assert!(msg.contains("ERROR"));
        assert!(msg.contains("Test"));
    }

    #[test]
    fn test_pagerduty_payload() {
        let alert = Alert::new(
            AlertType::ProofFailure,
            Severity::Critical,
            "Test",
            "Message",
        );

        let payload = alert.to_pagerduty_payload();
        assert_eq!(payload["payload"]["severity"], "critical");
    }

    #[tokio::test]
    async fn test_cooldown() {
        let config = AlertConfig {
            cooldown: Duration::from_millis(100),
            min_severity: Severity::Info,
            ..Default::default()
        };
        let manager = AlertManager::new(config);

        let alert = Alert::new(AlertType::ProofFailure, Severity::Error, "Test", "Msg");

        // First should send
        assert!(manager.should_send(&alert).await);
        manager.record_sent(&alert).await;

        // Immediate second should not send (cooldown)
        assert!(!manager.should_send(&alert).await);

        // After cooldown should send again
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(manager.should_send(&alert).await);
    }

    #[tokio::test]
    async fn test_severity_filter() {
        let config = AlertConfig {
            min_severity: Severity::Error,
            ..Default::default()
        };
        let manager = AlertManager::new(config);

        let warning = Alert::new(AlertType::ProofFailure, Severity::Warning, "Test", "Msg");
        let error = Alert::new(AlertType::ProofFailure, Severity::Error, "Test", "Msg");

        assert!(!manager.should_send(&warning).await);
        assert!(manager.should_send(&error).await);
    }

    #[test]
    fn test_config_from_env() {
        // Just test it doesn't panic
        let _config = AlertConfig::from_env();
    }
}
