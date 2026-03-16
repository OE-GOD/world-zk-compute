//! WebSocket event streaming for real-time push notifications.
//!
//! Provides an [`EventBroadcaster`] that distributes events to all connected
//! WebSocket clients, with per-client subscription filtering, heartbeat
//! keep-alive, and cursor-based pagination via monotonic sequence IDs.
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
//!
//! # Cursor-based Pagination
//!
//! Every event sent over WebSocket includes a monotonic `sequence_id`. Clients
//! can reconnect and replay missed events by sending:
//!
//! ```json
//! {"subscribe": "results", "since_id": 12345}
//! ```
//!
//! The server keeps the last [`DEFAULT_EVENT_BUFFER_SIZE`] events in memory.
//! On receiving a `since_id`, all buffered events with `sequence_id > since_id`
//! are replayed before switching to live streaming.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
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

/// Default maximum number of WebSocket connections from a single IP address.
const DEFAULT_MAX_WS_PER_IP: usize = 5;

/// Default number of recent events kept in the replay buffer.
const DEFAULT_EVENT_BUFFER_SIZE: usize = 1000;

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

/// A [`WsEvent`] augmented with a monotonic sequence ID for cursor-based
/// pagination. This is what gets broadcast on the channel and stored in the
/// replay buffer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SequencedEvent {
    /// Monotonically increasing sequence identifier. Never reused, never zero.
    pub sequence_id: u64,
    /// Event type name, e.g. "ResultSubmitted", "ResultChallenged".
    pub event_type: String,
    /// Arbitrary key-value payload carrying event-specific data.
    pub data: serde_json::Value,
}

impl SequencedEvent {
    /// Convert from a [`WsEvent`] and a sequence ID.
    fn from_ws_event(event: WsEvent, sequence_id: u64) -> Self {
        Self {
            sequence_id,
            event_type: event.event_type,
            data: event.data,
        }
    }
}

/// Client-to-server message for subscribing to specific event types.
///
/// Supports two forms:
/// - Array form: `{"subscribe": ["ResultSubmitted", "ResultChallenged"]}`
/// - String form with cursor: `{"subscribe": "results", "since_id": 12345}`
///
/// An empty `subscribe` array means "all events". When `since_id` is provided,
/// all buffered events with `sequence_id > since_id` are replayed immediately.
#[derive(Debug, Clone, Deserialize)]
pub struct SubscribeMessage {
    /// Event type filter. Can be a list of event type names, or the string
    /// "results" to subscribe to all result-related events.
    #[serde(default)]
    pub subscribe: SubscribeFilter,
    /// Optional cursor for replaying missed events. When set, the server sends
    /// all buffered events with `sequence_id > since_id` before switching to
    /// live streaming.
    #[serde(default)]
    pub since_id: Option<u64>,
}

/// The `subscribe` field can be either a list of event types or a single
/// string (for backward compatibility and the `"results"` shorthand).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SubscribeFilter {
    /// List of specific event type names.
    List(Vec<String>),
    /// Single string, e.g. "results" to subscribe to all.
    Single(String),
}

impl Default for SubscribeFilter {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

impl SubscribeFilter {
    /// Convert to a HashSet filter. Returns `None` if all events should pass.
    fn to_filter_set(&self) -> Option<HashSet<String>> {
        match self {
            SubscribeFilter::List(list) if list.is_empty() => None,
            SubscribeFilter::List(list) => Some(list.iter().cloned().collect()),
            SubscribeFilter::Single(s) if s == "results" || s.is_empty() => None,
            SubscribeFilter::Single(s) => {
                let mut set = HashSet::new();
                set.insert(s.clone());
                Some(set)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EventBroadcaster
// ---------------------------------------------------------------------------

/// Central hub that distributes [`SequencedEvent`]s to all connected WebSocket
/// clients via a [`tokio::sync::broadcast`] channel.
///
/// Each WebSocket handler task subscribes to the broadcast channel and
/// forwards matching events to its client. A bounded replay buffer stores
/// recent events for cursor-based pagination.
pub struct EventBroadcaster {
    sender: broadcast::Sender<SequencedEvent>,
    /// Monotonically increasing sequence counter.
    next_sequence_id: AtomicU64,
    /// Bounded buffer of recent events for replay.
    event_buffer: RwLock<VecDeque<SequencedEvent>>,
    /// Maximum number of events to keep in the replay buffer.
    buffer_capacity: usize,
    /// Number of currently active WebSocket connections.
    active_connections: AtomicUsize,
    /// Maximum allowed concurrent connections.
    max_connections: usize,
    /// Per-IP connection counts for preventing a single IP from exhausting all slots.
    per_ip_connections: Mutex<HashMap<IpAddr, usize>>,
    /// Maximum connections allowed from a single IP address.
    max_per_ip: usize,
}

impl EventBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    ///
    /// `capacity` determines how many events can be buffered in the broadcast
    /// channel before slow receivers start losing messages. The replay buffer
    /// defaults to [`DEFAULT_EVENT_BUFFER_SIZE`] (1000) events.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let max_connections = std::env::var("MAX_WS_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_WS_CONNECTIONS);
        let max_per_ip = std::env::var("MAX_WS_CONNECTIONS_PER_IP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_WS_PER_IP);
        Self {
            sender,
            next_sequence_id: AtomicU64::new(1),
            event_buffer: RwLock::new(VecDeque::with_capacity(DEFAULT_EVENT_BUFFER_SIZE)),
            buffer_capacity: DEFAULT_EVENT_BUFFER_SIZE,
            active_connections: AtomicUsize::new(0),
            max_connections,
            per_ip_connections: Mutex::new(HashMap::new()),
            max_per_ip,
        }
    }

    /// Create a new broadcaster with explicit max connection limit (for testing).
    pub fn with_max_connections(capacity: usize, max_connections: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            next_sequence_id: AtomicU64::new(1),
            event_buffer: RwLock::new(VecDeque::with_capacity(DEFAULT_EVENT_BUFFER_SIZE)),
            buffer_capacity: DEFAULT_EVENT_BUFFER_SIZE,
            active_connections: AtomicUsize::new(0),
            max_connections,
            per_ip_connections: Mutex::new(HashMap::new()),
            max_per_ip: DEFAULT_MAX_WS_PER_IP,
        }
    }

    /// Create a new broadcaster with a custom replay buffer size (for testing).
    pub fn with_buffer_size(capacity: usize, buffer_size: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let max_connections = std::env::var("MAX_WS_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_WS_CONNECTIONS);
        Self {
            sender,
            next_sequence_id: AtomicU64::new(1),
            event_buffer: RwLock::new(VecDeque::with_capacity(buffer_size)),
            buffer_capacity: buffer_size,
            active_connections: AtomicUsize::new(0),
            max_connections,
            per_ip_connections: Mutex::new(HashMap::new()),
            max_per_ip: DEFAULT_MAX_WS_PER_IP,
        }
    }

    /// Broadcast an event to all connected clients.
    ///
    /// Assigns a monotonic `sequence_id` to the event, stores it in the replay
    /// buffer, and sends it on the broadcast channel.
    ///
    /// Returns the number of receivers that received the event. Returns 0 if
    /// there are no active subscribers (this is not an error).
    pub fn broadcast(&self, event: WsEvent) -> usize {
        let seq_id = self.next_sequence_id.fetch_add(1, Ordering::SeqCst);
        let sequenced = SequencedEvent::from_ws_event(event, seq_id);

        // Store in replay buffer
        if let Ok(mut buf) = self.event_buffer.write() {
            if buf.len() >= self.buffer_capacity {
                buf.pop_front();
            }
            buf.push_back(sequenced.clone());
        }

        self.sender.send(sequenced).unwrap_or_default()
    }

    /// Subscribe to the event stream. Returns a receiver that yields cloned
    /// events as they are broadcast.
    pub fn subscribe(&self) -> broadcast::Receiver<SequencedEvent> {
        self.sender.subscribe()
    }

    /// Returns the current sequence ID counter value (next ID to be assigned).
    pub fn current_sequence_id(&self) -> u64 {
        self.next_sequence_id.load(Ordering::SeqCst)
    }

    /// Retrieve all buffered events with `sequence_id > since_id`.
    ///
    /// Returns events in chronological order (oldest first). If `since_id` is
    /// 0, returns all buffered events.
    pub fn events_since(&self, since_id: u64) -> Vec<SequencedEvent> {
        if let Ok(buf) = self.event_buffer.read() {
            buf.iter()
                .filter(|e| e.sequence_id > since_id)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Returns the number of events currently in the replay buffer.
    pub fn buffer_len(&self) -> usize {
        self.event_buffer.read().map(|buf| buf.len()).unwrap_or(0)
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

    /// Try to acquire a per-IP connection slot. Returns `true` if the IP is
    /// under its per-IP limit, `false` if the limit is reached.
    ///
    /// Also acquires a global slot. If the global limit is reached, returns
    /// `false` without incrementing the per-IP counter.
    fn try_acquire_slot_for_ip(&self, ip: IpAddr) -> bool {
        // Check global limit first
        if !self.try_acquire_slot() {
            return false;
        }

        let mut map = self.per_ip_connections.lock().unwrap_or_else(|e| e.into_inner());
        let count = map.entry(ip).or_insert(0);
        if *count >= self.max_per_ip {
            // Release the global slot we just acquired
            self.release_slot();
            return false;
        }
        *count += 1;
        true
    }

    /// Release a per-IP connection slot and the corresponding global slot.
    fn release_slot_for_ip(&self, ip: IpAddr) {
        self.release_slot();
        let mut map = self.per_ip_connections.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = map.get_mut(&ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(&ip);
            }
        }
    }

    /// Returns the current number of connections from the given IP.
    pub fn connections_for_ip(&self, ip: IpAddr) -> usize {
        let map = self.per_ip_connections.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&ip).copied().unwrap_or(0)
    }

    /// Returns the configured maximum per-IP connection limit.
    pub fn max_per_ip(&self) -> usize {
        self.max_per_ip
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
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    State(broadcaster): State<Arc<EventBroadcaster>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let peer_ip = connect_info
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    if !broadcaster.try_acquire_slot_for_ip(peer_ip) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many WebSocket connections",
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| handle_ws_connection(socket, broadcaster, peer_ip))
        .into_response()
}

/// Core WebSocket connection handler. Runs in a spawned task per connection.
///
/// The heartbeat uses a two-phase approach:
/// 1. Every `HEARTBEAT_INTERVAL` (30s), send a ping frame.
/// 2. After sending the ping, wait up to `PONG_TIMEOUT` (10s) for a pong.
/// 3. If no pong arrives within the timeout, disconnect the client.
async fn handle_ws_connection(socket: WebSocket, broadcaster: Arc<EventBroadcaster>, peer_ip: IpAddr) {
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
                        if ws_sender.send(Message::Ping(vec![])).await.is_err() {
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
                                if ws_sender.send(Message::Text(json)).await.is_err() {
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
                                // Update filter
                                filter = sub.subscribe.to_filter_set();

                                if filter.is_none() {
                                    debug!("WebSocket client subscribed to all events");
                                } else {
                                    debug!(
                                        "WebSocket client subscribed to: {:?}",
                                        filter
                                    );
                                }

                                // Replay buffered events if since_id is provided
                                if let Some(since_id) = sub.since_id {
                                    let missed = broadcaster.events_since(since_id);
                                    debug!(
                                        "Replaying {} buffered events since sequence_id {}",
                                        missed.len(),
                                        since_id
                                    );
                                    for event in missed {
                                        // Apply filter to replayed events too
                                        if let Some(ref allowed) = filter {
                                            if !allowed.contains(&event.event_type) {
                                                continue;
                                            }
                                        }
                                        match serde_json::to_string(&event) {
                                            Ok(json) => {
                                                if ws_sender.send(Message::Text(json)).await.is_err() {
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                warn!("Failed to serialize replayed event: {}", e);
                                            }
                                        }
                                    }
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

    broadcaster.release_slot_for_ip(peer_ip);
    info!(
        "WebSocket client disconnected (active: {}, ip: {})",
        broadcaster.active_connections(),
        peer_ip,
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
        assert_eq!(broadcaster.buffer_len(), 0);
        assert_eq!(broadcaster.current_sequence_id(), 1);
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
        // Event should still be buffered
        assert_eq!(broadcaster.buffer_len(), 1);
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

        let count = broadcaster.broadcast(event);
        assert_eq!(count, 2);

        let received1 = rx1.try_recv().unwrap();
        assert_eq!(received1.event_type, "ResultChallenged");
        assert_eq!(received1.sequence_id, 1);

        let received2 = rx2.try_recv().unwrap();
        assert_eq!(received2.event_type, "ResultChallenged");
        assert_eq!(received2.sequence_id, 1);
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
        assert_eq!(
            msg.subscribe,
            SubscribeFilter::List(vec![
                "ResultSubmitted".to_string(),
                "ResultChallenged".to_string(),
            ])
        );
        assert!(msg.since_id.is_none());

        // Empty subscribe means "all events"
        let json_all = r#"{"subscribe": []}"#;
        let msg_all: SubscribeMessage = serde_json::from_str(json_all).unwrap();
        assert_eq!(msg_all.subscribe, SubscribeFilter::List(Vec::new()));
    }

    #[test]
    fn test_subscribe_message_with_since_id() {
        let json = r#"{"subscribe": "results", "since_id": 42}"#;
        let msg: SubscribeMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg.subscribe,
            SubscribeFilter::Single("results".to_string())
        );
        assert_eq!(msg.since_id, Some(42));
    }

    #[test]
    fn test_subscribe_message_string_form() {
        let json = r#"{"subscribe": "results"}"#;
        let msg: SubscribeMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg.subscribe,
            SubscribeFilter::Single("results".to_string())
        );
        assert!(msg.since_id.is_none());
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
    fn test_sequenced_event_serialization_roundtrip() {
        let event = SequencedEvent {
            sequence_id: 42,
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"result_id": "0xabc"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"sequence_id\":42"));
        let deserialized: SequencedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_subscribe_message_invalid_json_fails() {
        let bad_json = r#"{"not_subscribe": true}"#;
        // With default, this should parse but subscribe will be default (empty list)
        let result = serde_json::from_str::<SubscribeMessage>(bad_json);
        assert!(result.is_ok()); // now succeeds because subscribe has a default

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

    // -----------------------------------------------------------------------
    // Sequence ID and replay buffer tests (T454)
    // -----------------------------------------------------------------------

    #[test]
    fn test_sequence_ids_are_monotonic() {
        let broadcaster = EventBroadcaster::new(64);

        for i in 0..10 {
            let event = WsEvent {
                event_type: "ResultSubmitted".to_string(),
                data: serde_json::json!({"idx": i}),
            };
            broadcaster.broadcast(event);
        }

        let events = broadcaster.events_since(0);
        assert_eq!(events.len(), 10);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.sequence_id, (i + 1) as u64);
        }

        // Verify monotonicity
        for window in events.windows(2) {
            assert!(
                window[1].sequence_id > window[0].sequence_id,
                "Sequence IDs must be strictly increasing"
            );
        }
    }

    #[test]
    fn test_sequence_ids_start_at_one() {
        let broadcaster = EventBroadcaster::new(64);
        let mut rx = broadcaster.subscribe();

        let event = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({}),
        };
        broadcaster.broadcast(event);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.sequence_id, 1, "First sequence ID should be 1");
    }

    #[test]
    fn test_events_since_returns_events_after_cursor() {
        let broadcaster = EventBroadcaster::new(64);

        for i in 0..5 {
            broadcaster.broadcast(WsEvent {
                event_type: format!("Event{}", i),
                data: serde_json::json!({"i": i}),
            });
        }

        // since_id=0 returns all
        let all = broadcaster.events_since(0);
        assert_eq!(all.len(), 5);

        // since_id=3 returns events 4 and 5
        let after3 = broadcaster.events_since(3);
        assert_eq!(after3.len(), 2);
        assert_eq!(after3[0].sequence_id, 4);
        assert_eq!(after3[1].sequence_id, 5);

        // since_id=5 returns nothing
        let after5 = broadcaster.events_since(5);
        assert_eq!(after5.len(), 0);

        // since_id=999 returns nothing
        let after999 = broadcaster.events_since(999);
        assert_eq!(after999.len(), 0);
    }

    #[test]
    fn test_buffer_bounded_to_capacity() {
        let broadcaster = EventBroadcaster::with_buffer_size(64, 5);

        for i in 0..10 {
            broadcaster.broadcast(WsEvent {
                event_type: "ResultSubmitted".to_string(),
                data: serde_json::json!({"i": i}),
            });
        }

        // Buffer should contain only last 5 events
        assert_eq!(broadcaster.buffer_len(), 5);

        let events = broadcaster.events_since(0);
        assert_eq!(events.len(), 5);
        // Oldest should be sequence_id 6 (events 1-5 were evicted)
        assert_eq!(events[0].sequence_id, 6);
        assert_eq!(events[4].sequence_id, 10);
    }

    #[test]
    fn test_buffer_empty_initially() {
        let broadcaster = EventBroadcaster::new(64);
        assert_eq!(broadcaster.buffer_len(), 0);
        assert!(broadcaster.events_since(0).is_empty());
    }

    #[test]
    fn test_events_since_with_filter() {
        let broadcaster = EventBroadcaster::new(64);

        broadcaster.broadcast(WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({}),
        });
        broadcaster.broadcast(WsEvent {
            event_type: "ResultChallenged".to_string(),
            data: serde_json::json!({}),
        });
        broadcaster.broadcast(WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({}),
        });

        // events_since returns all types (filtering is per-client in handler)
        let events = broadcaster.events_since(0);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_current_sequence_id_advances() {
        let broadcaster = EventBroadcaster::new(64);
        assert_eq!(broadcaster.current_sequence_id(), 1);

        broadcaster.broadcast(WsEvent {
            event_type: "Test".to_string(),
            data: serde_json::json!({}),
        });
        assert_eq!(broadcaster.current_sequence_id(), 2);

        broadcaster.broadcast(WsEvent {
            event_type: "Test".to_string(),
            data: serde_json::json!({}),
        });
        assert_eq!(broadcaster.current_sequence_id(), 3);
    }

    #[test]
    fn test_subscribe_filter_to_filter_set() {
        // Empty list = all
        let f = SubscribeFilter::List(Vec::new());
        assert!(f.to_filter_set().is_none());

        // Non-empty list = specific types
        let f = SubscribeFilter::List(vec!["A".to_string(), "B".to_string()]);
        let set = f.to_filter_set().unwrap();
        assert!(set.contains("A"));
        assert!(set.contains("B"));
        assert!(!set.contains("C"));

        // "results" string = all
        let f = SubscribeFilter::Single("results".to_string());
        assert!(f.to_filter_set().is_none());

        // Empty string = all
        let f = SubscribeFilter::Single("".to_string());
        assert!(f.to_filter_set().is_none());

        // Specific string = filter
        let f = SubscribeFilter::Single("ResultSubmitted".to_string());
        let set = f.to_filter_set().unwrap();
        assert!(set.contains("ResultSubmitted"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_sequenced_event_from_ws_event() {
        let ws = WsEvent {
            event_type: "ResultSubmitted".to_string(),
            data: serde_json::json!({"key": "value"}),
        };
        let seq = SequencedEvent::from_ws_event(ws.clone(), 42);
        assert_eq!(seq.sequence_id, 42);
        assert_eq!(seq.event_type, ws.event_type);
        assert_eq!(seq.data, ws.data);
    }

    #[test]
    fn test_subscribe_message_backwards_compatible() {
        // Old format (array subscribe, no since_id) should still work
        let json = r#"{"subscribe": ["ResultSubmitted"]}"#;
        let msg: SubscribeMessage = serde_json::from_str(json).unwrap();
        assert!(msg.since_id.is_none());
        match msg.subscribe {
            SubscribeFilter::List(list) => {
                assert_eq!(list.len(), 1);
                assert_eq!(list[0], "ResultSubmitted");
            }
            _ => panic!("Expected List variant"),
        }
    }

    #[test]
    fn test_buffer_eviction_order() {
        let broadcaster = EventBroadcaster::with_buffer_size(64, 3);

        // Insert events 1, 2, 3
        for i in 1..=3 {
            broadcaster.broadcast(WsEvent {
                event_type: format!("Event{}", i),
                data: serde_json::json!({}),
            });
        }
        assert_eq!(broadcaster.buffer_len(), 3);

        // Insert event 4 -- should evict event 1
        broadcaster.broadcast(WsEvent {
            event_type: "Event4".to_string(),
            data: serde_json::json!({}),
        });
        assert_eq!(broadcaster.buffer_len(), 3);

        let events = broadcaster.events_since(0);
        assert_eq!(events[0].sequence_id, 2);
        assert_eq!(events[0].event_type, "Event2");
        assert_eq!(events[2].sequence_id, 4);
        assert_eq!(events[2].event_type, "Event4");
    }

    #[test]
    fn test_events_since_partial_buffer() {
        let broadcaster = EventBroadcaster::with_buffer_size(64, 5);

        // Insert 8 events, buffer keeps last 5
        for i in 1..=8 {
            broadcaster.broadcast(WsEvent {
                event_type: "Test".to_string(),
                data: serde_json::json!({"i": i}),
            });
        }

        // since_id=3 -- event 3 was evicted, but 4-8 are in buffer
        let events = broadcaster.events_since(3);
        assert_eq!(events.len(), 5); // 4, 5, 6, 7, 8
        assert_eq!(events[0].sequence_id, 4);

        // since_id=6 -- only 7, 8
        let events = broadcaster.events_since(6);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence_id, 7);
        assert_eq!(events[1].sequence_id, 8);
    }

    #[test]
    fn test_default_buffer_size_is_1000() {
        let broadcaster = EventBroadcaster::new(64);
        assert_eq!(broadcaster.buffer_capacity, DEFAULT_EVENT_BUFFER_SIZE);
        assert_eq!(DEFAULT_EVENT_BUFFER_SIZE, 1000);
    }

    // -----------------------------------------------------------------------
    // Per-IP connection limit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_per_ip_limit_enforced() {
        let broadcaster = EventBroadcaster::with_max_connections(64, 100);
        // Default per-IP limit is 5
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));

        for i in 0..5 {
            assert!(
                broadcaster.try_acquire_slot_for_ip(ip),
                "slot {} should be acquired",
                i
            );
        }
        // 6th from same IP should fail
        assert!(!broadcaster.try_acquire_slot_for_ip(ip));
        assert_eq!(broadcaster.active_connections(), 5);
        assert_eq!(broadcaster.connections_for_ip(ip), 5);
    }

    #[test]
    fn test_per_ip_different_ips_independent() {
        let broadcaster = EventBroadcaster::with_max_connections(64, 100);
        let ip1 = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 2));

        // Fill up ip1's quota
        for _ in 0..5 {
            assert!(broadcaster.try_acquire_slot_for_ip(ip1));
        }
        assert!(!broadcaster.try_acquire_slot_for_ip(ip1));

        // ip2 should still be able to connect
        for _ in 0..5 {
            assert!(broadcaster.try_acquire_slot_for_ip(ip2));
        }
        assert!(!broadcaster.try_acquire_slot_for_ip(ip2));
        assert_eq!(broadcaster.active_connections(), 10);
    }

    #[test]
    fn test_per_ip_release_decrements() {
        let broadcaster = EventBroadcaster::with_max_connections(64, 100);
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));

        // Acquire all 5 slots
        for _ in 0..5 {
            broadcaster.try_acquire_slot_for_ip(ip);
        }
        assert_eq!(broadcaster.connections_for_ip(ip), 5);
        assert!(!broadcaster.try_acquire_slot_for_ip(ip));

        // Release one
        broadcaster.release_slot_for_ip(ip);
        assert_eq!(broadcaster.connections_for_ip(ip), 4);
        assert_eq!(broadcaster.active_connections(), 4);

        // Should be able to acquire again
        assert!(broadcaster.try_acquire_slot_for_ip(ip));
        assert_eq!(broadcaster.connections_for_ip(ip), 5);
    }

    #[test]
    fn test_per_ip_global_limit_still_applies() {
        // Global limit of 3, per-IP limit of 5
        let broadcaster = EventBroadcaster::with_max_connections(64, 3);
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));

        // Should hit global limit before per-IP limit
        assert!(broadcaster.try_acquire_slot_for_ip(ip));
        assert!(broadcaster.try_acquire_slot_for_ip(ip));
        assert!(broadcaster.try_acquire_slot_for_ip(ip));
        assert!(!broadcaster.try_acquire_slot_for_ip(ip)); // global limit hit
        assert_eq!(broadcaster.connections_for_ip(ip), 3);
    }

    #[test]
    fn test_per_ip_cleanup_on_full_release() {
        let broadcaster = EventBroadcaster::with_max_connections(64, 100);
        let ip = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));

        broadcaster.try_acquire_slot_for_ip(ip);
        assert_eq!(broadcaster.connections_for_ip(ip), 1);

        broadcaster.release_slot_for_ip(ip);
        assert_eq!(broadcaster.connections_for_ip(ip), 0);

        // Entry should be cleaned up from the map
        let map = broadcaster.per_ip_connections.lock().unwrap();
        assert!(!map.contains_key(&ip));
    }

    #[test]
    fn test_default_per_ip_limit() {
        let broadcaster = EventBroadcaster::new(64);
        assert_eq!(broadcaster.max_per_ip(), DEFAULT_MAX_WS_PER_IP);
        assert_eq!(DEFAULT_MAX_WS_PER_IP, 5);
    }
}
