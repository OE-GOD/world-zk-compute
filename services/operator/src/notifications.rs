use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Webhook health status for monitoring endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookHealthStatus {
    pub webhook_url: String,
    pub total_failures: u64,
}

/// JSON payload sent to the webhook endpoint.
///
/// The `text` field provides a human-readable summary that is compatible
/// with Slack incoming webhooks (Slack reads the `text` field by default).
#[derive(Debug, Clone, Serialize)]
pub struct WebhookPayload {
    pub event: String,
    pub result_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
    pub text: String,
}

/// Sends webhook notifications for dispute-related events.
///
/// Notifications are fire-and-forget: each send is dispatched via
/// `tokio::spawn` so the caller (e.g., the event watcher loop) is
/// never blocked by network latency or failures.
///
/// When no `webhook_url` is configured, all notification methods are
/// no-ops.
pub struct WebhookNotifier {
    client: reqwest::Client,
    webhook_url: String,
    failures: Arc<AtomicU64>,
}

/// A disabled notifier placeholder returned when no webhook URL is set.
/// All notification methods on `Option<WebhookNotifier>` are no-ops
/// when the value is `None`.
impl WebhookNotifier {
    /// Create a new `WebhookNotifier` that posts JSON payloads to the
    /// given `webhook_url`.
    ///
    /// The HTTP client uses a 5-second timeout for each request.
    pub fn new(webhook_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            webhook_url: webhook_url.to_string(),
            failures: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create a `WebhookNotifier` from an optional URL.
    ///
    /// Returns `Some(notifier)` when a URL is provided, `None` otherwise.
    /// Callers should use `Option<WebhookNotifier>` and call the
    /// free-standing helper methods which handle the `None` case as a
    /// no-op.
    pub fn from_optional(webhook_url: Option<&str>) -> Option<Self> {
        webhook_url.map(Self::new)
    }

    /// Returns the total number of notification send failures since
    /// this notifier was created.
    pub fn notification_failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Returns a health status summary for monitoring.
    pub fn health_status(&self) -> WebhookHealthStatus {
        WebhookHealthStatus {
            webhook_url: self.webhook_url.clone(),
            total_failures: self.notification_failures(),
        }
    }

    /// Returns a clone of the internal failure counter for sharing
    /// with spawned tasks.
    fn failures_handle(&self) -> Arc<AtomicU64> {
        self.failures.clone()
    }

    /// Fire-and-forget: spawn a background task that POSTs the payload
    /// to the webhook URL with retry and exponential backoff.
    ///
    /// Retries up to 3 times with delays of 1s, 2s, 4s on transient failures
    /// (connection errors and 5xx responses). Non-retryable failures (4xx)
    /// are reported immediately without retry.
    fn send_async(&self, payload: WebhookPayload) {
        let client = self.client.clone();
        let url = self.webhook_url.clone();
        let failures = self.failures_handle();

        tokio::spawn(async move {
            const MAX_RETRIES: u32 = 3;
            let base_delay = Duration::from_secs(1);

            for attempt in 0..=MAX_RETRIES {
                let result = client.post(&url).json(&payload).send().await;
                match result {
                    Ok(resp) if resp.status().is_success() => {
                        if attempt > 0 {
                            tracing::debug!(
                                event = %payload.event,
                                attempt = attempt + 1,
                                "Webhook notification sent after retry"
                            );
                        } else {
                            tracing::debug!(
                                event = %payload.event,
                                result_id = %payload.result_id,
                                "Webhook notification sent"
                            );
                        }
                        return;
                    }
                    Ok(resp) if resp.status().is_server_error() => {
                        // 5xx — retryable
                        if attempt < MAX_RETRIES {
                            let delay = base_delay * 2u32.pow(attempt);
                            tracing::warn!(
                                event = %payload.event,
                                status = %resp.status(),
                                attempt = attempt + 1,
                                retry_in_ms = delay.as_millis() as u64,
                                "Webhook returned server error, retrying"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        failures.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            event = %payload.event,
                            status = %resp.status(),
                            "Webhook notification failed after {} retries",
                            MAX_RETRIES
                        );
                        return;
                    }
                    Ok(resp) => {
                        // 4xx — not retryable
                        failures.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            event = %payload.event,
                            status = %resp.status(),
                            "Webhook notification returned non-retryable error"
                        );
                        return;
                    }
                    Err(e) => {
                        // Connection error — retryable
                        if attempt < MAX_RETRIES {
                            let delay = base_delay * 2u32.pow(attempt);
                            tracing::warn!(
                                event = %payload.event,
                                error = %e,
                                attempt = attempt + 1,
                                retry_in_ms = delay.as_millis() as u64,
                                "Webhook notification failed, retrying"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        failures.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            event = %payload.event,
                            error = %e,
                            "Webhook notification failed after {} retries",
                            MAX_RETRIES
                        );
                        return;
                    }
                }
            }
        });
    }

    /// Notify that a result has been challenged.
    pub fn notify_challenge(
        &self,
        result_id: &str,
        challenger: &str,
        timestamp: u64,
        deadline: u64,
    ) {
        let short_result = abbreviate_hex(result_id);
        let short_challenger = abbreviate_hex(challenger);

        let payload = WebhookPayload {
            event: "dispute".to_string(),
            result_id: result_id.to_string(),
            challenger: Some(challenger.to_string()),
            timestamp: Some(timestamp),
            deadline: Some(deadline),
            tx_hash: None,
            winner: None,
            text: format!(
                "\u{1f6a8} Result {} challenged by {}",
                short_result, short_challenger
            ),
        };

        self.send_async(payload);
    }

    /// Notify that a ZK proof has been submitted for a disputed result.
    pub fn notify_proof_submitted(&self, result_id: &str, tx_hash: &str) {
        let short_result = abbreviate_hex(result_id);
        let short_tx = abbreviate_hex(tx_hash);

        let payload = WebhookPayload {
            event: "proof_submitted".to_string(),
            result_id: result_id.to_string(),
            challenger: None,
            timestamp: None,
            deadline: None,
            tx_hash: Some(tx_hash.to_string()),
            winner: None,
            text: format!(
                "\u{2705} Proof submitted for result {} (tx: {})",
                short_result, short_tx
            ),
        };

        self.send_async(payload);
    }

    /// Notify that a dispute has been resolved.
    pub fn notify_dispute_resolved(&self, result_id: &str, winner: &str) {
        let short_result = abbreviate_hex(result_id);
        let short_winner = abbreviate_hex(winner);

        let payload = WebhookPayload {
            event: "dispute_resolved".to_string(),
            result_id: result_id.to_string(),
            challenger: None,
            timestamp: None,
            deadline: None,
            tx_hash: None,
            winner: Some(winner.to_string()),
            text: format!(
                "\u{1f3c6} Dispute resolved for result {}. Winner: {}",
                short_result, short_winner
            ),
        };

        self.send_async(payload);
    }
}

/// Abbreviate a hex string for human-readable display.
///
/// "0x1234567890abcdef1234567890abcdef12345678" becomes "0x1234...5678"
fn abbreviate_hex(hex: &str) -> String {
    if hex.len() <= 12 {
        return hex.to_string();
    }

    let prefix = if hex.starts_with("0x") {
        &hex[..6]
    } else {
        &hex[..4]
    };
    let suffix = &hex[hex.len() - 4..];
    format!("{}...{}", prefix, suffix)
}

// ---------------------------------------------------------------------------
// Convenience helpers for `Option<WebhookNotifier>` so callers don't need
// to match on the option at every call site.
// ---------------------------------------------------------------------------

/// Notify challenge if notifier is present, otherwise no-op.
pub fn maybe_notify_challenge(
    notifier: &Option<WebhookNotifier>,
    result_id: &str,
    challenger: &str,
    timestamp: u64,
    deadline: u64,
) {
    if let Some(n) = notifier {
        n.notify_challenge(result_id, challenger, timestamp, deadline);
    }
}

/// Notify proof submitted if notifier is present, otherwise no-op.
pub fn maybe_notify_proof_submitted(
    notifier: &Option<WebhookNotifier>,
    result_id: &str,
    tx_hash: &str,
) {
    if let Some(n) = notifier {
        n.notify_proof_submitted(result_id, tx_hash);
    }
}

/// Notify dispute resolved if notifier is present, otherwise no-op.
pub fn maybe_notify_dispute_resolved(
    notifier: &Option<WebhookNotifier>,
    result_id: &str,
    winner: &str,
) {
    if let Some(n) = notifier {
        n.notify_dispute_resolved(result_id, winner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_notifier_construction() {
        let notifier = WebhookNotifier::new("https://hooks.slack.com/services/T00/B00/XXXX");
        assert_eq!(
            notifier.webhook_url,
            "https://hooks.slack.com/services/T00/B00/XXXX"
        );
        assert_eq!(notifier.notification_failures(), 0);
    }

    #[test]
    fn test_from_optional_some() {
        let notifier =
            WebhookNotifier::from_optional(Some("https://hooks.slack.com/services/T00/B00/XXXX"));
        assert!(notifier.is_some());
    }

    #[test]
    fn test_from_optional_none() {
        let notifier = WebhookNotifier::from_optional(None);
        assert!(notifier.is_none());
    }

    #[test]
    fn test_disabled_notifications_no_panic() {
        let notifier: Option<WebhookNotifier> = None;
        // These should all be no-ops and not panic
        maybe_notify_challenge(
            &notifier,
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "0x5678ef0123456789abcdef0123456789abcdef01",
            1700000000,
            1700003600,
        );
        maybe_notify_proof_submitted(
            &notifier,
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "0xaabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344",
        );
        maybe_notify_dispute_resolved(
            &notifier,
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            "0x5678ef0123456789abcdef0123456789abcdef01",
        );
    }

    #[test]
    fn test_challenge_payload_serialization() {
        let payload = WebhookPayload {
            event: "dispute".to_string(),
            result_id: "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
                .to_string(),
            challenger: Some("0x5678ef0123456789abcdef0123456789abcdef01".to_string()),
            timestamp: Some(1700000000),
            deadline: Some(1700003600),
            tx_hash: None,
            winner: None,
            text: "test challenge".to_string(),
        };

        let json_str = serde_json::to_string(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["event"], "dispute");
        assert_eq!(
            parsed["result_id"],
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
        );
        assert_eq!(
            parsed["challenger"],
            "0x5678ef0123456789abcdef0123456789abcdef01"
        );
        assert_eq!(parsed["timestamp"], 1700000000);
        assert_eq!(parsed["deadline"], 1700003600);
        assert_eq!(parsed["text"], "test challenge");
        // tx_hash and winner should be absent (not null) due to skip_serializing_if
        assert!(parsed.get("tx_hash").is_none());
        assert!(parsed.get("winner").is_none());
    }

    #[test]
    fn test_proof_submitted_payload_serialization() {
        let payload = WebhookPayload {
            event: "proof_submitted".to_string(),
            result_id: "0xabcd".to_string(),
            challenger: None,
            timestamp: None,
            deadline: None,
            tx_hash: Some("0xdeadbeef".to_string()),
            winner: None,
            text: "Proof submitted".to_string(),
        };

        let json_str = serde_json::to_string(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["event"], "proof_submitted");
        assert_eq!(parsed["tx_hash"], "0xdeadbeef");
        assert!(parsed.get("challenger").is_none());
        assert!(parsed.get("timestamp").is_none());
        assert!(parsed.get("deadline").is_none());
        assert!(parsed.get("winner").is_none());
    }

    #[test]
    fn test_dispute_resolved_payload_serialization() {
        let payload = WebhookPayload {
            event: "dispute_resolved".to_string(),
            result_id: "0xabcd".to_string(),
            challenger: None,
            timestamp: None,
            deadline: None,
            tx_hash: None,
            winner: Some("0x1111222233334444555566667777888899990000".to_string()),
            text: "Dispute resolved".to_string(),
        };

        let json_str = serde_json::to_string(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["event"], "dispute_resolved");
        assert_eq!(
            parsed["winner"],
            "0x1111222233334444555566667777888899990000"
        );
        assert!(parsed.get("challenger").is_none());
        assert!(parsed.get("tx_hash").is_none());
    }

    #[test]
    fn test_notification_failure_counter_initial() {
        let notifier = WebhookNotifier::new("https://example.com/webhook");
        assert_eq!(notifier.notification_failures(), 0);
    }

    #[test]
    fn test_notification_failure_counter_increment() {
        let notifier = WebhookNotifier::new("https://example.com/webhook");
        // Manually increment to test the counter mechanism
        notifier.failures.fetch_add(1, Ordering::Relaxed);
        assert_eq!(notifier.notification_failures(), 1);
        notifier.failures.fetch_add(1, Ordering::Relaxed);
        assert_eq!(notifier.notification_failures(), 2);
    }

    #[test]
    fn test_abbreviate_hex_long() {
        let hex = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        assert_eq!(abbreviate_hex(hex), "0x1234...cdef");
    }

    #[test]
    fn test_abbreviate_hex_short() {
        assert_eq!(abbreviate_hex("0xabcd"), "0xabcd");
        assert_eq!(abbreviate_hex("short"), "short");
    }

    #[test]
    fn test_abbreviate_hex_address() {
        let addr = "0x5678ef0123456789abcdef0123456789abcdef01";
        assert_eq!(abbreviate_hex(addr), "0x5678...ef01");
    }

    #[test]
    fn test_abbreviate_hex_no_prefix() {
        let hex = "1234567890abcdef1234567890abcdef";
        assert_eq!(abbreviate_hex(hex), "1234...cdef");
    }

    #[tokio::test]
    async fn test_send_to_unreachable_increments_failure() {
        // Use localhost port 1 which returns "connection refused" instantly
        // (unlike non-routable IPs which wait for timeout).
        // With 3 retries at 1s/2s/4s backoff, total wait is ~7s.
        let notifier = WebhookNotifier::new("http://127.0.0.1:1/webhook");
        notifier.notify_challenge(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000001",
            1700000000,
            1700003600,
        );

        // Give the spawned task time to exhaust retries (1s + 2s + 4s + margin)
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            notifier.notification_failures() >= 1,
            "Expected at least 1 failure, got {}",
            notifier.notification_failures()
        );
    }

    #[test]
    fn test_health_status() {
        let notifier = WebhookNotifier::new("https://example.com/webhook");
        notifier.failures.fetch_add(5, Ordering::Relaxed);

        let status = notifier.health_status();
        assert_eq!(status.webhook_url, "https://example.com/webhook");
        assert_eq!(status.total_failures, 5);
    }

    #[test]
    fn test_health_status_serialization() {
        let status = WebhookHealthStatus {
            webhook_url: "https://hooks.slack.com/test".to_string(),
            total_failures: 3,
        };

        let json_str = serde_json::to_string(&status).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["webhook_url"], "https://hooks.slack.com/test");
        assert_eq!(parsed["total_failures"], 3);
    }
}
