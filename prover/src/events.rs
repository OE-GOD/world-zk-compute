//! Event Subscription for Instant Job Detection
//!
//! Subscribes to ExecutionRequested events via WebSocket for instant
//! detection of new jobs (vs 5-second polling delay).
//!
//! ## Benefits
//!
//! - **Instant detection**: New jobs detected within ~100ms vs 5s polling
//! - **Reduced RPC calls**: No repeated getPendingRequests calls
//! - **Lower latency**: Combined with prefetching, near-instant job processing
//!
//! ## Architecture
//!
//! ```text
//! WebSocket Connection
//!        ↓
//! [Event Received] → [Parse ExecutionRequested] → [Add to Job Queue]
//!        ↓                                              ↓
//! [Reconnect on Error]                           [Start Prefetching]
//! ```

use crate::contracts::IExecutionEngine;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Configuration for event subscription
#[derive(Clone, Debug)]
pub struct EventConfig {
    /// WebSocket URL for subscriptions
    pub ws_url: String,
    /// Contract address to monitor
    pub engine_address: Address,
    /// Reconnection delay (starts here, backs off exponentially)
    pub reconnect_delay: Duration,
    /// Maximum reconnection delay
    pub max_reconnect_delay: Duration,
    /// Minimum tip to accept
    pub min_tip: U256,
}

/// Event data extracted from ExecutionRequested event
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NewJobEvent {
    pub request_id: u64,
    pub requester: Address,
    pub image_id: B256,
    pub input_digest: B256,
    pub input_type: u8,
    pub tip: U256,
    pub expires_at: u64,
}

/// Event subscriber for ExecutionRequested events
pub struct EventSubscriber {
    config: EventConfig,
    /// Channel to send new job events
    event_tx: mpsc::Sender<NewJobEvent>,
    /// Allowed image IDs (empty = all)
    allowed_images: Vec<B256>,
}

impl EventSubscriber {
    /// Create a new event subscriber
    pub fn new(
        config: EventConfig,
        event_tx: mpsc::Sender<NewJobEvent>,
        allowed_images: Vec<B256>,
    ) -> Self {
        Self {
            config,
            event_tx,
            allowed_images,
        }
    }

    /// Start the subscription loop (runs forever, handles reconnection)
    pub async fn run(&self) {
        let mut reconnect_delay = self.config.reconnect_delay;
        let mut consecutive_errors = 0u32;

        loop {
            info!("Connecting to WebSocket: {}", self.config.ws_url);

            match self.subscribe_loop().await {
                Ok(()) => {
                    // Clean disconnect, reset backoff
                    reconnect_delay = self.config.reconnect_delay;
                    consecutive_errors = 0;
                    info!("WebSocket disconnected cleanly, reconnecting...");
                }
                Err(e) => {
                    consecutive_errors += 1;
                    error!("WebSocket error (attempt {}): {:?}", consecutive_errors, e);

                    // Exponential backoff
                    reconnect_delay = Self::next_reconnect_delay(
                        reconnect_delay,
                        self.config.max_reconnect_delay,
                    );
                }
            }

            // Wait before reconnecting
            info!("Reconnecting in {:?}...", reconnect_delay);
            tokio::time::sleep(reconnect_delay).await;
        }
    }

    /// Internal subscription loop
    async fn subscribe_loop(&self) -> anyhow::Result<()> {
        // Connect via WebSocket
        let ws = alloy::providers::WsConnect::new(&self.config.ws_url);
        let provider = ProviderBuilder::new().connect_ws(ws).await?;

        info!("WebSocket connected, subscribing to ExecutionRequested events...");

        // Create filter for ExecutionRequested events
        // Use SIGNATURE_HASH which is the keccak256 of the event signature
        let filter = Filter::new()
            .address(self.config.engine_address)
            .event_signature(IExecutionEngine::ExecutionRequested::SIGNATURE_HASH);

        // Subscribe to logs
        let sub = provider.subscribe_logs(&filter).await?;
        let mut stream = sub.into_stream();

        info!(
            "Subscribed to ExecutionRequested events on {}",
            self.config.engine_address
        );

        // Process events
        while let Some(log) = stream.next().await {
            match self.process_log(&log) {
                Ok(Some(event)) => {
                    // Check image filter
                    if !Self::is_image_accepted(&self.allowed_images, &event.image_id) {
                        debug!(
                            "Ignoring event for image {}: not in allowed list",
                            event.image_id
                        );
                        continue;
                    }

                    // Check tip threshold
                    if !Self::is_tip_sufficient(event.tip, self.config.min_tip) {
                        debug!(
                            "Ignoring event {}: tip {} below threshold {}",
                            event.request_id, event.tip, self.config.min_tip
                        );
                        continue;
                    }

                    info!(
                        "New ExecutionRequested event: request_id={}, image={}, tip={}",
                        event.request_id, event.image_id, event.tip
                    );

                    // Send to channel
                    if let Err(e) = self.event_tx.send(event).await {
                        error!("Failed to send event to channel: {}", e);
                    }
                }
                Ok(None) => {
                    debug!("Received non-matching log, skipping");
                }
                Err(e) => {
                    warn!("Failed to parse event log: {}", e);
                }
            }
        }

        // Stream ended
        Ok(())
    }

    /// Compute the next reconnection delay with exponential backoff.
    ///
    /// Multiplies the current delay by 1.5 and caps at `max_delay`.
    /// Extracted as a standalone pure function for testability.
    fn next_reconnect_delay(current: Duration, max_delay: Duration) -> Duration {
        Duration::from_secs_f64((current.as_secs_f64() * 1.5).min(max_delay.as_secs_f64()))
    }

    /// Check whether a job event passes the image filter.
    ///
    /// Returns `true` if the event's image ID is accepted (either the allow
    /// list is empty, or the image is in the list).
    fn is_image_accepted(allowed: &[B256], image_id: &B256) -> bool {
        allowed.is_empty() || allowed.contains(image_id)
    }

    /// Check whether a job event passes the tip threshold filter.
    fn is_tip_sufficient(tip: U256, min_tip: U256) -> bool {
        tip >= min_tip
    }

    /// Parse a log into a NewJobEvent
    fn process_log(&self, log: &alloy::rpc::types::Log) -> anyhow::Result<Option<NewJobEvent>> {
        // Decode the ExecutionRequested event
        let event = log
            .log_decode::<IExecutionEngine::ExecutionRequested>()
            .map_err(|e| anyhow::anyhow!("Failed to decode event: {:?}", e))?;

        let inner = event.inner.data;

        let request_id: u64 = match inner.requestId.try_into() {
            Ok(id) => id,
            Err(e) => {
                warn!(
                    "Request ID overflow ({}), skipping event: {:?}",
                    inner.requestId, e
                );
                return Ok(None);
            }
        };

        let expires_at: u64 = match inner.expiresAt.try_into() {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "expires_at overflow ({}), using u64::MAX: {:?}",
                    inner.expiresAt, e
                );
                u64::MAX
            }
        };

        Ok(Some(NewJobEvent {
            request_id,
            requester: inner.requester,
            image_id: inner.imageId,
            input_digest: inner.inputDigest,
            input_type: inner.inputType,
            tip: inner.tip,
            expires_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a B256 from a single byte (first byte set, rest zeros).
    fn b256_from_byte(b: u8) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[0] = b;
        B256::from(bytes)
    }

    // ========== next_reconnect_delay (exponential backoff) ==========

    #[test]
    fn test_backoff_basic_increase() {
        let current = Duration::from_secs(1);
        let max = Duration::from_secs(60);
        let next = EventSubscriber::next_reconnect_delay(current, max);
        // 1s * 1.5 = 1.5s
        assert_eq!(next, Duration::from_secs_f64(1.5));
    }

    #[test]
    fn test_backoff_successive_increases() {
        let max = Duration::from_secs(60);

        let d1 = Duration::from_secs(1);
        let d2 = EventSubscriber::next_reconnect_delay(d1, max); // 1.5
        let d3 = EventSubscriber::next_reconnect_delay(d2, max); // 2.25
        let d4 = EventSubscriber::next_reconnect_delay(d3, max); // 3.375

        assert!(d2 > d1);
        assert!(d3 > d2);
        assert!(d4 > d3);
        // Verify the 1.5x multiplier
        let ratio = d2.as_secs_f64() / d1.as_secs_f64();
        assert!((ratio - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_backoff_capped_at_max() {
        let current = Duration::from_secs(50);
        let max = Duration::from_secs(60);
        let next = EventSubscriber::next_reconnect_delay(current, max);
        // 50 * 1.5 = 75, but capped at 60
        assert_eq!(next, Duration::from_secs(60));
    }

    #[test]
    fn test_backoff_already_at_max() {
        let current = Duration::from_secs(60);
        let max = Duration::from_secs(60);
        let next = EventSubscriber::next_reconnect_delay(current, max);
        // 60 * 1.5 = 90, capped at 60
        assert_eq!(next, max);
    }

    #[test]
    fn test_backoff_from_small_value() {
        let max = Duration::from_secs(60);
        let mut delay = Duration::from_millis(100);

        // Run backoff 20 times, should eventually reach max
        for _ in 0..20 {
            delay = EventSubscriber::next_reconnect_delay(delay, max);
        }
        assert_eq!(delay, max);
    }

    // ========== is_image_accepted ==========

    #[test]
    fn test_image_accepted_empty_list_accepts_all() {
        let allowed: Vec<B256> = vec![];
        assert!(EventSubscriber::is_image_accepted(
            &allowed,
            &b256_from_byte(0xAA)
        ));
        assert!(EventSubscriber::is_image_accepted(&allowed, &B256::ZERO));
    }

    #[test]
    fn test_image_accepted_in_list() {
        let id = b256_from_byte(0x42);
        let allowed = vec![id];
        assert!(EventSubscriber::is_image_accepted(&allowed, &id));
    }

    #[test]
    fn test_image_rejected_not_in_list() {
        let id = b256_from_byte(0x42);
        let other = b256_from_byte(0x99);
        let allowed = vec![id];
        assert!(!EventSubscriber::is_image_accepted(&allowed, &other));
    }

    #[test]
    fn test_image_accepted_multiple_ids() {
        let id1 = b256_from_byte(0x01);
        let id2 = b256_from_byte(0x02);
        let id3 = b256_from_byte(0x03);
        let allowed = vec![id1, id2];
        assert!(EventSubscriber::is_image_accepted(&allowed, &id1));
        assert!(EventSubscriber::is_image_accepted(&allowed, &id2));
        assert!(!EventSubscriber::is_image_accepted(&allowed, &id3));
    }

    // ========== is_tip_sufficient ==========

    #[test]
    fn test_tip_sufficient_at_threshold() {
        let min = U256::from(1000u64);
        assert!(EventSubscriber::is_tip_sufficient(U256::from(1000u64), min));
    }

    #[test]
    fn test_tip_sufficient_above_threshold() {
        let min = U256::from(1000u64);
        assert!(EventSubscriber::is_tip_sufficient(U256::from(1001u64), min));
    }

    #[test]
    fn test_tip_insufficient_below_threshold() {
        let min = U256::from(1000u64);
        assert!(!EventSubscriber::is_tip_sufficient(U256::from(999u64), min));
    }

    #[test]
    fn test_tip_sufficient_zero_minimum() {
        assert!(EventSubscriber::is_tip_sufficient(U256::ZERO, U256::ZERO));
        assert!(EventSubscriber::is_tip_sufficient(
            U256::from(1u64),
            U256::ZERO
        ));
    }

    // ========== NewJobEvent struct construction ==========

    #[test]
    fn test_new_job_event_fields() {
        let event = NewJobEvent {
            request_id: 42,
            requester: Address::ZERO,
            image_id: b256_from_byte(0xAB),
            input_digest: B256::ZERO,
            input_type: 1,
            tip: U256::from(5000u64),
            expires_at: 1700000000,
        };

        assert_eq!(event.request_id, 42);
        assert_eq!(event.input_type, 1);
        assert_eq!(event.tip, U256::from(5000u64));
        assert_eq!(event.expires_at, 1700000000);
    }

    // ========== EventConfig construction ==========

    #[test]
    fn test_event_config_fields() {
        let config = EventConfig {
            ws_url: "wss://example.com".to_string(),
            engine_address: Address::ZERO,
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(60),
            min_tip: U256::from(100u64),
        };

        assert_eq!(config.ws_url, "wss://example.com");
        assert_eq!(config.reconnect_delay, Duration::from_secs(1));
        assert_eq!(config.max_reconnect_delay, Duration::from_secs(60));
        assert_eq!(config.min_tip, U256::from(100u64));
    }
}
