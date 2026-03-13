//! WebSocket event streaming for real-time push notifications.
//!
//! Provides an [`EventBroadcaster`] that distributes events to all connected
//! WebSocket clients, with per-client subscription filtering and heartbeat
//! keep-alive.
//!
//! # Usage
//!
//! ```ignore
//! use std::sync::Arc;
//! use axum::Router;
//! use axum::routing::get;
//! use crate::websocket::{EventBroadcaster, ws_handler};
//!
//! let broadcaster = Arc::new(EventBroadcaster::new(256));
//! let app = Router::new()
//!     .route("/ws/events", get(ws_handler))
//!     .with_state(broadcaster);
//! ```
//!
//! # Protocol
//!
//! After the WebSocket connection is established, the client may send a JSON
//! subscribe message to filter which event types it receives:
//!
//! ```json
//! {"subscribe": ["ResultSubmitted", "ResultChallenged"]}
//! ```
//!
//! If no subscribe message is sent, all events are forwarded. The server sends
//! a ping frame every 30 seconds and disconnects if no pong is received within
//! 10 seconds.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/// Default maximum number of concurrent WebSocket connections.
const DEFAULT_MAX_WS_CONNECTIONS: usize = 100;

/// Interval between heartbeat ping frames.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time to wait for a pong response after sending a ping.
/// If no pong arrives within this window, the connection is dropped.
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// Internal state tracking for heartbeat pong checks.
enum HeartbeatState {
    /// Idle -- waiting for next heartbeat interval to send a ping.
    Idle,
    /// Waiting for pong -- ping was sent, waiting up to PONG_TIMEOUT for response.
    WaitingForPong,
}

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// A JSON-serializable event pushed to WebSocket clients.
///
/// This mirrors the on-chain event types from [`tee_watcher::TEEEvent`] but
/// uses a flat JSON representation suitable for WebSocket transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WsEvent {
    /// Event type name, e.g. "ResultSubmitted", "ResultChallenged".
    pub event_type: String,
    /// Arbitrary key-value payload carrying event-specific data.
    pub data: serde_json::Value,
}

/// Client-to-server message for subscribing to specific event types.
#[derive(Debug, Clone, Deserialize)]
pub struct SubscribeMessage {
    /// List of event type names to subscribe to. An empty list means "all".
    pub subscribe: Vec<String>,
}

// ---------------------------------------------------------------------------
// EventBroadcaster
// ---------------------------------------------------------------------------

/// Central hub that distributes [`WsEvent`]s to all connected WebSocket
/// clients via a [`tokio::sync::broadcast`] channel.
///
/// Each WebSocket handler task subscribes to the broadcast channel and
/// forwards matching events to its client.
pub struct EventBroadcaster {
    sender: broadcast::Sender<WsEvent>,
    /// Number of currently active WebSocket connections.
    active_connections: AtomicUsize,
    /// Maximum allowed concurrent connections.
    max_connections: usize,
}

impl EventBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    ///
    /// `capacity` determines how many events can be buffered before slow
    /// receivers start losing messages.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let max_connections = std::env::var("MAX_WS_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_WS_CONNECTIONS);
        Self {
            sender,
            active_connections: AtomicUsize::new(0),
            max_connections,
        }
    }

    /// Create a new broadcaster with explicit max connection limit (for testing).
    pub fn with_max_connections(capacity: usize, max_connections: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            active_connections: AtomicUsize::new(0),
            max_connections,
        }
    }

    /// Broadcast an event to all connected clients.
    ///
    /// Returns the number of receivers that received the event. Returns 0 if
    /// there are no active subscribers (this is not an error).
    pub fn broadcast(&self, event: WsEvent) -> usize {
        match self.sender.send(event) {
            Ok(n) => n,
            Err(_) => 0, // No active receivers
        }
    }

    /// Subscribe to the event stream. Returns a receiver that yields cloned
    /// events as they are broadcast.
    pub fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.sender.subscribe()
    }

    /// Returns the current number of active WebSocket connections.
    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Returns the configured maximum number of connections.
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Try to increment the connection count. Returns `true` if the connection
    /// is accepted (under the limit), `false` if the limit is reached.
    fn try_acquire_slot(&self) -> bool {
        loop {
            let current = self.active_connections.load(Ordering::Relaxed);
            if current >= self.max_connections {
                return false;
            }
            if self
                .active_connections
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
            // CAS failed, retry
        }
    }

    /// Decrement the connection count when a client disconnects.
    fn release_slot(&self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
    }
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

/// Axum handler for `GET /ws/events`.
///
/// Upgrades the HTTP connection to a WebSocket and spawns a task to handle
/// event streaming. Use this with `axum::routing::get`:
///
/// ```ignore
/// Router::new()
///     .route("/ws/events", get(ws_handler))
///     .with_state(Arc::new(EventBroadcaster::new(256)));
/// ```
pub async fn ws_handler(
    State(broadcaster): State<Arc<EventBroadcaster>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if !broadcaster.try_acquire_slot() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many WebSocket connections",
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| handle_ws_connection(socket, broadcaster))
        .into_response()
}

/// Core WebSocket connection handler. Runs in a spawned task per connection.
///
/// The heartbeat uses a two-phase approach:
/// 1. Every `HEARTBEAT_INTERVAL` (30s), send a ping frame.
/// 2. After sending the ping, wait up to `PONG_TIMEOUT` (10s) for a pong.
/// 3. If no pong arrives within the timeout, disconnect the client.
async fn handle_ws_connection(socket: WebSocket, broadcaster: Arc<EventBroadcaster>) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut event_rx = broadcaster.subscribe();
    let mut filter: Option<HashSet<String>> = None;
    let mut heartbeat_state = HeartbeatState::Idle;
    let mut heartbeat_deadline = tokio::time::Instant::now() + HEARTBEAT_INTERVAL;

    info!(
        "WebSocket client connected (active: {})",
        broadcaster.active_connections()
    );

    loop {
        tokio::select! {
            // --- Heartbeat timer ---
            _ = tokio::time::sleep_until(heartbeat_deadline) => {
                match heartbeat_state {
                    HeartbeatState::Idle => {
                        // Time to send a ping
                        if ws_sender.send(Message::Ping(vec![].into())).await.is_err() {
                            break;
                        }
                        heartbeat_state = HeartbeatState::WaitingForPong;
                        heartbeat_deadline = tokio::time::Instant::now() + PONG_TIMEOUT;
                    }
                    HeartbeatState::WaitingForPong => {
                        // Pong timeout expired without receiving pong
                        debug!("WebSocket client missed pong within {}s, disconnecting", PONG_TIMEOUT.as_secs());
                        break;
                    }
                }
            }

            // --- Incoming event from broadcast channel ---
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Apply subscription filter
                        if let Some(ref allowed) = filter {
                            if !allowed.contains(&event.event_type) {
                                continue;
                            }
                        }
                        match serde_json::to_string(&event) {
                            Ok(json) => {
                                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to serialize event: {}", e);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket client lagged, skipped {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Broadcast channel closed");
                        break;
                    }
                }
            }

            // --- Incoming message from WebSocket client ---
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<SubscribeMessage>(&text) {
                            Ok(sub) => {
                                if sub.subscribe.is_empty() {
                                    filter = None;
                                    debug!("WebSocket client subscribed to all events");
                                } else {
                                    debug!(
                                        "WebSocket client subscribed to: {:?}",
                                        sub.subscribe
                                    );
                                    filter = Some(sub.subscribe.into_iter().collect());
                                }
                            }
                            Err(e) => {
                                debug!("Ignoring invalid subscribe message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // Pong received -- reset heartbeat to idle and schedule next ping
                        heartbeat_state = HeartbeatState::Idle;
                        heartbeat_deadline = tokio::time::Instant::now() + HEARTBEAT_INTERVAL;
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!("WebSocket client sent close frame");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Ignore binary, ping (axum auto-responds to pings)
                    }
                    Some(Err(e)) => {
                        debug!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        // Stream ended
                        break;
                    }
                }
            }
        }
    }

    broadcaster.release_slot();
    info!(
        "WebSocket client disconnected (active: {})",
        broadcaster.active_connections()
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_broadcaster_creation() {
        let broadcaster = EventBroadcaster::new(64);
        assert_eq!(broadcaster.active_connections(), 0);
        assert!(broadcaster.max_connections() > 0);
    }

    #[test]
    fn test_broadcast_with_no_receivers_returns_zero() {
        let broadcaster = EventBroadcaster::new(64);
        let event = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"result_id": "0xabc"}),
        };
        let count = broadcaster.broadcast(event);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_broadcast_delivers_to_subscribers() {
        let broadcaster = EventBroadcaster::new(64);
        let mut rx1 = broadcaster.subscribe();
        let mut rx2 = broadcaster.subscribe();

        let event = WsEvent {
            event_type: "ResultChallenged".to_string(),
            data: serde_json::json!({"result_id": "0xdef", "challenger": "0x123"}),
        };

        let count = broadcaster.broadcast(event.clone());
        assert_eq!(count, 2);

        let received1 = rx1.try_recv().unwrap();
        assert_eq!(received1.event_type, "ResultChallenged");
        assert_eq!(received1, event);

        let received2 = rx2.try_recv().unwrap();
        assert_eq!(received2, event);
    }

    #[test]
    fn test_subscription_filter_allows_matching_events() {
        let allowed: HashSet<String> = ["ResultSubmitted", "ResultChallenged"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let event_submitted = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({}),
        };
        let event_finalized = WsEvent {
            event_type: "ResultFinalized".to_string(),
            data: serde_json::json!({}),
        };
        let event_challenged = WsEvent {
            event_type: "ResultChallenged".to_string(),
            data: serde_json::json!({}),
        };

        assert!(allowed.contains(&event_submitted.event_type));
        assert!(!allowed.contains(&event_finalized.event_type));
        assert!(allowed.contains(&event_challenged.event_type));
    }

    #[test]
    fn test_connection_limit_enforcement() {
        let broadcaster = EventBroadcaster::with_max_connections(64, 3);
        assert_eq!(broadcaster.max_connections(), 3);

        // Acquire 3 slots (the max)
        assert!(broadcaster.try_acquire_slot());
        assert_eq!(broadcaster.active_connections(), 1);
        assert!(broadcaster.try_acquire_slot());
        assert_eq!(broadcaster.active_connections(), 2);
        assert!(broadcaster.try_acquire_slot());
        assert_eq!(broadcaster.active_connections(), 3);

        // 4th should fail
        assert!(!broadcaster.try_acquire_slot());
        assert_eq!(broadcaster.active_connections(), 3);

        // Release one and try again
        broadcaster.release_slot();
        assert_eq!(broadcaster.active_connections(), 2);
        assert!(broadcaster.try_acquire_slot());
        assert_eq!(broadcaster.active_connections(), 3);
    }

    #[test]
    fn test_subscribe_message_parsing() {
        let json = r#"{"subscribe": ["ResultSubmitted", "ResultChallenged"]}"#;
        let msg: SubscribeMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.subscribe.len(), 2);
        assert_eq!(msg.subscribe[0], "ResultSubmitted");
        assert_eq!(msg.subscribe[1], "ResultChallenged");

        // Empty subscribe means "all events"
        let json_all = r#"{"subscribe": []}"#;
        let msg_all: SubscribeMessage = serde_json::from_str(json_all).unwrap();
        assert!(msg_all.subscribe.is_empty());
    }

    #[test]
    fn test_ws_event_serialization_roundtrip() {
        let event = WsEvent {
            event_type: "DisputeResolved".to_string(),
            data: serde_json::json!({
                "result_id": "0xabcdef",
                "prover_won": true
            }),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: WsEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_subscribe_message_invalid_json_fails() {
        let bad_json = r#"{"not_subscribe": true}"#;
        // serde will produce a default missing-field error
        let result = serde_json::from_str::<SubscribeMessage>(bad_json);
        assert!(result.is_err());

        let also_bad = r#"hello world"#;
        let result2 = serde_json::from_str::<SubscribeMessage>(also_bad);
        assert!(result2.is_err());
    }

    #[test]
    fn test_broadcast_channel_capacity() {
        let broadcaster = EventBroadcaster::new(2);
        let _rx = broadcaster.subscribe();

        // Fill the channel to capacity
        let event1 = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"id": 1}),
        };
        let event2 = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"id": 2}),
        };
        // Both should succeed
        assert_eq!(broadcaster.broadcast(event1), 1);
        assert_eq!(broadcaster.broadcast(event2), 1);
    }

    #[tokio::test]
    async fn test_concurrent_slot_acquisition() {
        let broadcaster = Arc::new(EventBroadcaster::with_max_connections(64, 10));
        let mut handles = Vec::new();

        for _ in 0..20 {
            let b = Arc::clone(&broadcaster);
            handles.push(tokio::spawn(async move { b.try_acquire_slot() }));
        }

        let mut accepted = 0;
        let mut rejected = 0;
        for handle in handles {
            if handle.await.unwrap() {
                accepted += 1;
            } else {
                rejected += 1;
            }
        }

        assert_eq!(accepted, 10);
        assert_eq!(rejected, 10);
        assert_eq!(broadcaster.active_connections(), 10);
    }
}
