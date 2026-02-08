//! Security Audit Logging
//!
//! Provides comprehensive audit logging for security-relevant events:
//! - Authentication attempts
//! - Key access
//! - Transaction submissions
//! - Configuration changes
//! - Error conditions

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};

/// Audit event severity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// Informational events
    Info,
    /// Warning conditions
    Warning,
    /// Security-relevant events
    Security,
    /// Critical security events
    Critical,
}

/// Audit event category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventCategory {
    /// Key management events
    KeyManagement,
    /// Authentication events
    Authentication,
    /// Transaction events
    Transaction,
    /// Configuration changes
    Configuration,
    /// Access control events
    AccessControl,
    /// System events
    System,
    /// Network events
    Network,
    /// Error events
    Error,
}

/// Audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique event ID
    pub id: String,
    /// Timestamp (Unix seconds)
    pub timestamp: u64,
    /// Event severity
    pub severity: Severity,
    /// Event category
    pub category: EventCategory,
    /// Event action
    pub action: String,
    /// Actor (who performed the action)
    pub actor: Option<String>,
    /// Target (what was affected)
    pub target: Option<String>,
    /// Outcome (success/failure)
    pub outcome: Outcome,
    /// Additional details
    pub details: Option<String>,
    /// Source IP (if applicable)
    pub source_ip: Option<String>,
    /// Request ID for correlation
    pub request_id: Option<String>,
}

/// Event outcome
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Success,
    Failure,
    Pending,
    Unknown,
}

impl AuditEvent {
    /// Create a new audit event
    pub fn new(
        severity: Severity,
        category: EventCategory,
        action: impl Into<String>,
    ) -> Self {
        Self {
            id: generate_event_id(),
            timestamp: current_timestamp(),
            severity,
            category,
            action: action.into(),
            actor: None,
            target: None,
            outcome: Outcome::Unknown,
            details: None,
            source_ip: None,
            request_id: None,
        }
    }

    /// Set the actor
    pub fn actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    /// Set the target
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Set the outcome
    pub fn outcome(mut self, outcome: Outcome) -> Self {
        self.outcome = outcome;
        self
    }

    /// Set additional details
    pub fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Set source IP
    pub fn source_ip(mut self, ip: impl Into<String>) -> Self {
        self.source_ip = Some(ip.into());
        self
    }

    /// Set request ID for correlation
    pub fn request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// Format as log line
    pub fn to_log_line(&self) -> String {
        let actor = self.actor.as_deref().unwrap_or("-");
        let target = self.target.as_deref().unwrap_or("-");
        let details = self.details.as_deref().unwrap_or("");

        format!(
            "[AUDIT] {} {:?} {:?} action={} actor={} target={} outcome={:?} {}",
            self.timestamp,
            self.severity,
            self.category,
            self.action,
            actor,
            target,
            self.outcome,
            details
        )
    }
}

/// Handler type enum (avoids async trait dyn-compatibility issues)
#[derive(Clone)]
pub enum HandlerType {
    File(FileAuditHandler),
    Memory(Arc<MemoryAuditHandler>),
}

impl HandlerType {
    async fn handle(&self, event: &AuditEvent) {
        match self {
            HandlerType::File(h) => h.handle(event).await,
            HandlerType::Memory(h) => h.handle(event).await,
        }
    }
}

/// Audit logger
pub struct AuditLogger {
    /// In-memory buffer for recent events
    buffer: RwLock<VecDeque<AuditEvent>>,
    /// Maximum buffer size
    max_buffer_size: usize,
    /// Minimum severity to log
    min_severity: Severity,
    /// Event handlers
    handlers: RwLock<Vec<HandlerType>>,
}

impl AuditLogger {
    /// Create a new audit logger
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            buffer: RwLock::new(VecDeque::with_capacity(max_buffer_size)),
            max_buffer_size,
            min_severity: Severity::Info,
            handlers: RwLock::new(Vec::new()),
        }
    }

    /// Set minimum severity
    pub fn with_min_severity(mut self, severity: Severity) -> Self {
        self.min_severity = severity;
        self
    }

    /// Add an event handler
    pub async fn add_handler(&self, handler: HandlerType) {
        let mut handlers = self.handlers.write().await;
        handlers.push(handler);
    }

    /// Add a file handler
    pub async fn add_file_handler(&self, path: impl Into<String>) {
        self.add_handler(HandlerType::File(FileAuditHandler::new(path))).await;
    }

    /// Add a memory handler
    pub async fn add_memory_handler(&self, handler: Arc<MemoryAuditHandler>) {
        self.add_handler(HandlerType::Memory(handler)).await;
    }

    /// Log an audit event
    pub async fn log(&self, event: AuditEvent) {
        // Check severity filter
        if !self.should_log(&event) {
            return;
        }

        // Add to buffer
        {
            let mut buffer = self.buffer.write().await;
            if buffer.len() >= self.max_buffer_size {
                buffer.pop_front();
            }
            buffer.push_back(event.clone());
        }

        // Call handlers
        let handlers = self.handlers.read().await;
        for handler in handlers.iter() {
            handler.handle(&event).await;
        }

        // Also log to tracing
        match event.severity {
            Severity::Critical => tracing::error!("{}", event.to_log_line()),
            Severity::Security => tracing::warn!("{}", event.to_log_line()),
            Severity::Warning => tracing::warn!("{}", event.to_log_line()),
            Severity::Info => tracing::info!("{}", event.to_log_line()),
        }
    }

    /// Get recent events
    pub async fn recent_events(&self, limit: usize) -> Vec<AuditEvent> {
        let buffer = self.buffer.read().await;
        buffer.iter().rev().take(limit).cloned().collect()
    }

    /// Get events by category
    pub async fn events_by_category(&self, category: EventCategory) -> Vec<AuditEvent> {
        let buffer = self.buffer.read().await;
        buffer.iter().filter(|e| e.category == category).cloned().collect()
    }

    /// Get security events
    pub async fn security_events(&self) -> Vec<AuditEvent> {
        let buffer = self.buffer.read().await;
        buffer
            .iter()
            .filter(|e| matches!(e.severity, Severity::Security | Severity::Critical))
            .cloned()
            .collect()
    }

    fn should_log(&self, event: &AuditEvent) -> bool {
        match (self.min_severity, event.severity) {
            (Severity::Info, _) => true,
            (Severity::Warning, Severity::Info) => false,
            (Severity::Warning, _) => true,
            (Severity::Security, Severity::Info | Severity::Warning) => false,
            (Severity::Security, _) => true,
            (Severity::Critical, Severity::Critical) => true,
            (Severity::Critical, _) => false,
        }
    }

    // Convenience methods for common events

    /// Log key access
    pub async fn key_accessed(&self, key_id: &str, purpose: &str) {
        self.log(
            AuditEvent::new(Severity::Security, EventCategory::KeyManagement, "key_access")
                .target(key_id)
                .details(purpose)
                .outcome(Outcome::Success)
        ).await;
    }

    /// Log key rotation
    pub async fn key_rotated(&self, old_key_id: &str, new_key_id: &str) {
        self.log(
            AuditEvent::new(Severity::Security, EventCategory::KeyManagement, "key_rotation")
                .target(old_key_id)
                .details(format!("rotated to {}", new_key_id))
                .outcome(Outcome::Success)
        ).await;
    }

    /// Log transaction submission
    pub async fn transaction_submitted(&self, tx_hash: &str, request_id: u64) {
        self.log(
            AuditEvent::new(Severity::Info, EventCategory::Transaction, "tx_submit")
                .target(tx_hash)
                .request_id(request_id.to_string())
                .outcome(Outcome::Pending)
        ).await;
    }

    /// Log transaction confirmed
    pub async fn transaction_confirmed(&self, tx_hash: &str, success: bool) {
        self.log(
            AuditEvent::new(
                if success { Severity::Info } else { Severity::Warning },
                EventCategory::Transaction,
                "tx_confirm"
            )
                .target(tx_hash)
                .outcome(if success { Outcome::Success } else { Outcome::Failure })
        ).await;
    }

    /// Log configuration change
    pub async fn config_changed(&self, setting: &str, old_value: &str, new_value: &str) {
        self.log(
            AuditEvent::new(Severity::Security, EventCategory::Configuration, "config_change")
                .target(setting)
                .details(format!("changed from '{}' to '{}'", old_value, new_value))
                .outcome(Outcome::Success)
        ).await;
    }

    /// Log authentication attempt
    pub async fn auth_attempt(&self, method: &str, success: bool, source_ip: Option<&str>) {
        let mut event = AuditEvent::new(
            if success { Severity::Info } else { Severity::Security },
            EventCategory::Authentication,
            "auth_attempt"
        )
            .details(method)
            .outcome(if success { Outcome::Success } else { Outcome::Failure });

        if let Some(ip) = source_ip {
            event = event.source_ip(ip);
        }

        self.log(event).await;
    }

    /// Log security violation
    pub async fn security_violation(&self, violation_type: &str, details: &str) {
        self.log(
            AuditEvent::new(Severity::Critical, EventCategory::AccessControl, "security_violation")
                .details(format!("{}: {}", violation_type, details))
                .outcome(Outcome::Failure)
        ).await;
    }

    /// Log rate limit exceeded
    pub async fn rate_limit_exceeded(&self, source: &str, limit_type: &str) {
        self.log(
            AuditEvent::new(Severity::Warning, EventCategory::AccessControl, "rate_limit")
                .actor(source)
                .details(limit_type)
                .outcome(Outcome::Failure)
        ).await;
    }

    /// Log system startup
    pub async fn system_startup(&self, version: &str) {
        self.log(
            AuditEvent::new(Severity::Info, EventCategory::System, "startup")
                .details(format!("version {}", version))
                .outcome(Outcome::Success)
        ).await;
    }

    /// Log system shutdown
    pub async fn system_shutdown(&self, reason: &str) {
        self.log(
            AuditEvent::new(Severity::Info, EventCategory::System, "shutdown")
                .details(reason)
                .outcome(Outcome::Success)
        ).await;
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new(10000)
    }
}

/// File-based audit handler
#[derive(Clone)]
pub struct FileAuditHandler {
    path: String,
}

impl FileAuditHandler {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    async fn handle(&self, event: &AuditEvent) {
        let line = format!("{}\n", serde_json::to_string(event).unwrap_or_default());

        // Append to file (in production, use async file I/O)
        if let Err(e) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()))
        {
            tracing::error!("Failed to write audit log: {}", e);
        }
    }
}

/// In-memory audit handler for testing
pub struct MemoryAuditHandler {
    events: RwLock<Vec<AuditEvent>>,
}

impl MemoryAuditHandler {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
        }
    }

    pub async fn events(&self) -> Vec<AuditEvent> {
        self.events.read().await.clone()
    }

    async fn handle(&self, event: &AuditEvent) {
        let mut events = self.events.write().await;
        events.push(event.clone());
    }
}

impl Default for MemoryAuditHandler {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions

fn generate_event_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let count = COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = current_timestamp();
    format!("evt_{}_{}", timestamp, count)
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Global audit logger instance
static AUDIT_LOGGER: std::sync::OnceLock<AuditLogger> = std::sync::OnceLock::new();

/// Get the global audit logger
pub fn audit() -> &'static AuditLogger {
    AUDIT_LOGGER.get_or_init(AuditLogger::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_event() {
        let event = AuditEvent::new(Severity::Security, EventCategory::KeyManagement, "key_access")
            .actor("prover_1")
            .target("key_abc123")
            .outcome(Outcome::Success)
            .details("signing transaction");

        assert_eq!(event.action, "key_access");
        assert_eq!(event.actor, Some("prover_1".to_string()));
        assert_eq!(event.outcome, Outcome::Success);
    }

    #[tokio::test]
    async fn test_audit_logger() {
        let logger = AuditLogger::new(100);

        logger.log(
            AuditEvent::new(Severity::Info, EventCategory::System, "test_event")
                .outcome(Outcome::Success)
        ).await;

        let recent = logger.recent_events(10).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].action, "test_event");
    }

    #[tokio::test]
    async fn test_buffer_limit() {
        let logger = AuditLogger::new(3);

        for i in 0..5 {
            logger.log(
                AuditEvent::new(Severity::Info, EventCategory::System, format!("event_{}", i))
            ).await;
        }

        let recent = logger.recent_events(10).await;
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].action, "event_4");
    }

    #[tokio::test]
    async fn test_severity_filter() {
        let logger = AuditLogger::new(100).with_min_severity(Severity::Security);

        logger.log(
            AuditEvent::new(Severity::Info, EventCategory::System, "info_event")
        ).await;

        logger.log(
            AuditEvent::new(Severity::Security, EventCategory::System, "security_event")
        ).await;

        let recent = logger.recent_events(10).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].action, "security_event");
    }

    #[tokio::test]
    async fn test_memory_handler() {
        let logger = AuditLogger::new(100);
        let handler = Arc::new(MemoryAuditHandler::new());
        logger.add_memory_handler(handler.clone()).await;

        logger.key_accessed("key_123", "signing").await;

        let events = handler.events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "key_access");
    }

    #[tokio::test]
    async fn test_convenience_methods() {
        let logger = AuditLogger::new(100);

        logger.transaction_submitted("0xabc123", 42).await;
        logger.transaction_confirmed("0xabc123", true).await;
        logger.config_changed("max_concurrent", "4", "8").await;

        let recent = logger.recent_events(10).await;
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_security_events_filter() {
        let logger = AuditLogger::new(100);

        logger.log(AuditEvent::new(Severity::Info, EventCategory::System, "info")).await;
        logger.log(AuditEvent::new(Severity::Security, EventCategory::KeyManagement, "security")).await;
        logger.log(AuditEvent::new(Severity::Critical, EventCategory::AccessControl, "critical")).await;

        let security = logger.security_events().await;
        assert_eq!(security.len(), 2);
    }

    #[test]
    fn test_event_id_generation() {
        let id1 = generate_event_id();
        let id2 = generate_event_id();

        assert!(id1.starts_with("evt_"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_log_line_format() {
        let event = AuditEvent::new(Severity::Security, EventCategory::KeyManagement, "key_access")
            .actor("user_1")
            .target("key_abc")
            .outcome(Outcome::Success);

        let line = event.to_log_line();
        assert!(line.contains("[AUDIT]"));
        assert!(line.contains("key_access"));
        assert!(line.contains("user_1"));
    }
}
