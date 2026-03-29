//! Webhook notification system for proof lifecycle events.
//!
//! Sends HTTP POST notifications when proofs are submitted, verified, or deleted.
//! Supports retry with exponential backoff.
//!
//! ## Configuration
//!
//! Webhooks are registered via `POST /webhooks` with a callback URL.
//! Each webhook receives JSON payloads for proof events.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Maximum number of retry attempts for failed webhook deliveries.
const MAX_RETRIES: u32 = 3;
/// Base delay between retries (doubles each attempt).
const BASE_RETRY_DELAY_MS: u64 = 1000;

/// Webhook registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    /// Unique webhook ID.
    pub id: String,
    /// Callback URL to POST events to.
    pub url: String,
    /// Event types to subscribe to (empty = all events).
    pub events: Vec<String>,
    /// Whether this webhook is active.
    pub active: bool,
}

/// Event payload sent to webhook endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookEvent {
    /// Event type: "proof.submitted", "proof.verified", "proof.deleted".
    pub event: String,
    /// Proof ID.
    pub proof_id: String,
    /// Timestamp (Unix seconds).
    pub timestamp: u64,
    /// Additional event-specific data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// In-memory webhook store.
pub struct WebhookStore {
    hooks: RwLock<HashMap<String, Webhook>>,
    client: reqwest::Client,
}

impl WebhookStore {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Register a new webhook. Returns the webhook ID.
    pub fn register(&self, url: &str, events: Vec<String>) -> String {
        let id = format!("wh-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let webhook = Webhook {
            id: id.clone(),
            url: url.to_string(),
            events,
            active: true,
        };
        if let Ok(mut hooks) = self.hooks.write() {
            hooks.insert(id.clone(), webhook);
        }
        id
    }

    /// Unregister a webhook by ID.
    pub fn unregister(&self, id: &str) -> bool {
        self.hooks
            .write()
            .map(|mut h| h.remove(id).is_some())
            .unwrap_or(false)
    }

    /// List all registered webhooks.
    pub fn list(&self) -> Vec<Webhook> {
        self.hooks
            .read()
            .map(|h| h.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Notify all matching webhooks about an event (fire-and-forget).
    pub fn notify(&self, event: WebhookEvent) {
        let hooks: Vec<Webhook> = self
            .hooks
            .read()
            .map(|h| {
                h.values()
                    .filter(|w| {
                        w.active && (w.events.is_empty() || w.events.contains(&event.event))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if hooks.is_empty() {
            return;
        }

        let client = self.client.clone();
        let event = Arc::new(event);

        for hook in hooks {
            let client = client.clone();
            let event = event.clone();
            tokio::spawn(async move {
                deliver_with_retry(&client, &hook.url, &event, MAX_RETRIES).await;
            });
        }
    }
}

/// Deliver a webhook event with exponential backoff retry.
async fn deliver_with_retry(
    client: &reqwest::Client,
    url: &str,
    event: &WebhookEvent,
    max_retries: u32,
) {
    let mut delay = BASE_RETRY_DELAY_MS;

    for attempt in 0..=max_retries {
        match client.post(url).json(event).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(
                    url = %url,
                    event = %event.event,
                    proof_id = %event.proof_id,
                    attempt = attempt,
                    "Webhook delivered"
                );
                return;
            }
            Ok(resp) => {
                tracing::warn!(
                    url = %url,
                    status = %resp.status(),
                    attempt = attempt,
                    "Webhook delivery failed (HTTP error)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    url = %url,
                    error = %e,
                    attempt = attempt,
                    "Webhook delivery failed (network error)"
                );
            }
        }

        if attempt < max_retries {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            delay *= 2;
        }
    }

    tracing::error!(
        url = %url,
        event = %event.event,
        proof_id = %event.proof_id,
        "Webhook delivery exhausted all retries"
    );
}

/// Helper to create a timestamp.
pub fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_list() {
        let store = WebhookStore::new();
        let id = store.register("https://example.com/hook", vec!["proof.submitted".into()]);
        assert!(id.starts_with("wh-"));

        let hooks = store.list();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].url, "https://example.com/hook");
        assert_eq!(hooks[0].events, vec!["proof.submitted"]);
        assert!(hooks[0].active);
    }

    #[test]
    fn test_unregister() {
        let store = WebhookStore::new();
        let id = store.register("https://example.com/hook", vec![]);
        assert!(store.unregister(&id));
        assert!(!store.unregister(&id)); // already removed
        assert!(store.list().is_empty());
    }

    #[test]
    fn test_register_multiple() {
        let store = WebhookStore::new();
        store.register("https://a.com/hook", vec![]);
        store.register("https://b.com/hook", vec!["proof.verified".into()]);
        assert_eq!(store.list().len(), 2);
    }

    #[test]
    fn test_event_serialization() {
        let event = WebhookEvent {
            event: "proof.submitted".into(),
            proof_id: "abc-123".into(),
            timestamp: 1700000000,
            data: Some(serde_json::json!({"circuit_hash": "0xdead"})),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("proof.submitted"));
        assert!(json.contains("abc-123"));
    }
}
