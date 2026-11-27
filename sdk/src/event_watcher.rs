//! Event watcher for TEEMLVerifier contract events.
//!
//! Provides polling and continuous watching of on-chain events emitted by the
//! TEEMLVerifier contract. Supports `ResultSubmitted`, `ResultChallenged`,
//! `ResultFinalized`, and `ResultExpired` events.
//!
//! # Example
//!
//! ```rust,no_run
//! use alloy::primitives::Address;
//! use world_zk_sdk::event_watcher::TEEEventWatcher;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let contract: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3".parse()?;
//!     let watcher = TEEEventWatcher::new("http://localhost:8545", contract);
//!
//!     // One-shot poll
//!     let (events, next_block) = watcher.poll_events(0).await?;
//!     for event in &events {
//!         println!("{:?}", event);
//!     }
//!
//!     // Continuous watch
//!     let handle = watcher.watch(
//!         |event| println!("Event: {:?}", event),
//!         Duration::from_secs(2),
//!     );
//!     // ... later ...
//!     watcher.stop();
//!     handle.await.unwrap();
//!     Ok(())
//! }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{keccak256, Address, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{Filter, Log};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Topic hash constants (keccak256 of each Solidity event signature)
// ---------------------------------------------------------------------------

/// Topic hash for `ResultSubmitted(bytes32 indexed, bytes32 indexed, bytes32, address indexed)`.
pub fn topic_result_submitted() -> B256 {
    keccak256("ResultSubmitted(bytes32,bytes32,bytes32,address)")
}

/// Topic hash for `ResultChallenged(bytes32 indexed, address)`.
pub fn topic_result_challenged() -> B256 {
    keccak256("ResultChallenged(bytes32,address)")
}

/// Topic hash for `ResultFinalized(bytes32 indexed)`.
pub fn topic_result_finalized() -> B256 {
    keccak256("ResultFinalized(bytes32)")
}

/// Topic hash for `ResultExpired(bytes32 indexed)`.
pub fn topic_result_expired() -> B256 {
    keccak256("ResultExpired(bytes32)")
}

// ---------------------------------------------------------------------------
// TEEEvent enum
// ---------------------------------------------------------------------------

/// Parsed event from the TEEMLVerifier contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum TEEEvent {
    /// A new ML inference result was submitted with a prover stake.
    ResultSubmitted {
        /// Unique result identifier (indexed topic).
        result_id: B256,
        /// Hash of the ML model used (indexed topic).
        model_hash: B256,
        /// Hash of the input data (non-indexed, in log data).
        input_hash: B256,
        /// Address of the submitter (indexed topic).
        submitter: Address,
        /// Block number at which the event was emitted.
        block_number: u64,
    },
    /// A submitted result was challenged.
    ResultChallenged {
        /// The challenged result identifier (indexed topic).
        result_id: B256,
        /// Address of the challenger (non-indexed, in log data).
        challenger: Address,
        /// Block number at which the event was emitted.
        block_number: u64,
    },
    /// An unchallenged result was finalized after the challenge window.
    ResultFinalized {
        /// The finalized result identifier (indexed topic).
        result_id: B256,
        /// Block number at which the event was emitted.
        block_number: u64,
    },
    /// A result expired without being challenged (emitted alongside ResultFinalized).
    ResultExpired {
        /// The expired result identifier (indexed topic).
        result_id: B256,
        /// Block number at which the event was emitted.
        block_number: u64,
    },
}

// ---------------------------------------------------------------------------
// Log parsing
// ---------------------------------------------------------------------------

/// Parse an alloy `Log` into a `TEEEvent`.
///
/// Returns `None` if the log does not match any known TEEMLVerifier event.
pub fn parse_log(log: &Log) -> Option<TEEEvent> {
    let topics = log.topics();
    if topics.is_empty() {
        return None;
    }

    let topic0 = topics[0];
    let block_number = log.block_number.unwrap_or(0);

    if topic0 == topic_result_submitted() {
        // Layout:
        //   topic[0] = event signature
        //   topic[1] = resultId (indexed)
        //   topic[2] = modelHash (indexed)
        //   topic[3] = submitter (indexed, left-padded address)
        //   data[0..32] = inputHash (non-indexed)
        let result_id = *topics.get(1)?;
        let model_hash = *topics.get(2)?;
        let submitter_topic = *topics.get(3)?;
        let submitter = Address::from_slice(&submitter_topic.as_slice()[12..32]);
        let data = log.data().data.as_ref();
        if data.len() < 32 {
            return None;
        }
        let input_hash = B256::from_slice(&data[0..32]);
        Some(TEEEvent::ResultSubmitted {
            result_id,
            model_hash,
            input_hash,
            submitter,
            block_number,
        })
    } else if topic0 == topic_result_challenged() {
        // Layout:
        //   topic[0] = event signature
        //   topic[1] = resultId (indexed)
        //   data[0..32] = challenger (non-indexed, left-padded address)
        let result_id = *topics.get(1)?;
        let data = log.data().data.as_ref();
        if data.len() < 32 {
            return None;
        }
        let challenger = Address::from_slice(&data[12..32]);
        Some(TEEEvent::ResultChallenged {
            result_id,
            challenger,
            block_number,
        })
    } else if topic0 == topic_result_finalized() {
        // Layout:
        //   topic[0] = event signature
        //   topic[1] = resultId (indexed)
        let result_id = *topics.get(1)?;
        Some(TEEEvent::ResultFinalized {
            result_id,
            block_number,
        })
    } else if topic0 == topic_result_expired() {
        // Layout:
        //   topic[0] = event signature
        //   topic[1] = resultId (indexed)
        let result_id = *topics.get(1)?;
        Some(TEEEvent::ResultExpired {
            result_id,
            block_number,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TEEEventWatcher
// ---------------------------------------------------------------------------

/// Default for [`TEEEventWatcher::with_max_block_range`]: the widest block span
/// requested from `eth_getLogs` in one call.
pub const DEFAULT_MAX_BLOCK_RANGE: u64 = 10_000;

/// Fetch and parse the watched contract's events in `from_block..=to_block`,
/// asking for at most `max_block_range` blocks per `eth_getLogs` call.
async fn fetch_events<P: Provider>(
    provider: &P,
    contract_address: Address,
    from_block: u64,
    to_block: u64,
    max_block_range: u64,
) -> anyhow::Result<Vec<TEEEvent>> {
    let span = max_block_range.max(1);
    let mut events = Vec::new();
    let mut start = from_block;
    while start <= to_block {
        let end = to_block.min(start.saturating_add(span - 1));
        let filter = Filter::new()
            .address(contract_address)
            .from_block(start)
            .to_block(end);
        let logs = provider.get_logs(&filter).await?;
        events.extend(logs.iter().filter_map(parse_log));
        match end.checked_add(1) {
            Some(next) => start = next,
            None => break,
        }
    }
    Ok(events)
}

/// Position of the background watch loop.
struct WatchCursor {
    /// First block not yet delivered (`None` until the chain head has been
    /// read once; watching starts at that head).
    next_block: Option<u64>,
    /// Blocks asked for in the next `eth_getLogs` call, between 1 and
    /// `max_block_range`.
    span: u64,
    max_block_range: u64,
}

impl WatchCursor {
    fn new(max_block_range: u64) -> Self {
        let max_block_range = max_block_range.max(1);
        Self {
            next_block: None,
            span: max_block_range,
            max_block_range,
        }
    }
}

/// One iteration of the background watch loop: pass the events from
/// `cursor.next_block` up to the chain head to `deliver`.
///
/// The range is fetched in chunks of `cursor.span` blocks. After each chunk
/// its events are delivered, `cursor.next_block` moves past it and the span
/// doubles (up to `cursor.max_block_range`). A failed `eth_getLogs` halves the
/// span (down to 1) and ends the iteration with the error; the next iteration
/// resumes at the chunk that failed, so no block is skipped or read twice.
async fn poll_new_events<P: Provider>(
    provider: &P,
    contract_address: Address,
    cursor: &mut WatchCursor,
    deliver: &mut impl FnMut(TEEEvent),
) -> anyhow::Result<()> {
    let latest = provider.get_block_number().await?;
    let mut start = *cursor.next_block.get_or_insert(latest);
    while start <= latest {
        let end = latest.min(start.saturating_add(cursor.span - 1));
        let filter = Filter::new()
            .address(contract_address)
            .from_block(start)
            .to_block(end);
        let logs = match provider.get_logs(&filter).await {
            Ok(logs) => logs,
            Err(err) => {
                cursor.span = (cursor.span / 2).max(1);
                return Err(err.into());
            }
        };
        logs.iter().filter_map(parse_log).for_each(&mut *deliver);
        cursor.span = cursor.span.saturating_mul(2).min(cursor.max_block_range);
        cursor.next_block = Some(end.saturating_add(1));
        match end.checked_add(1) {
            Some(next) => start = next,
            None => break,
        }
    }
    Ok(())
}

/// [`poll_new_events`], with a failure handed to `on_error` instead of returned.
async fn poll_and_report<P: Provider>(
    provider: &P,
    contract_address: Address,
    cursor: &mut WatchCursor,
    callback: &mut impl FnMut(TEEEvent),
    on_error: &mut impl FnMut(anyhow::Error),
) {
    if let Err(err) = poll_new_events(provider, contract_address, cursor, callback).await {
        #[cfg(feature = "tracing")]
        tracing::warn!(error = %err, "event watcher: poll failed, will retry");
        on_error(err);
    }
}

/// Watches TEEMLVerifier events via `eth_getLogs` polling.
///
/// Supports one-shot polling via [`poll_events`](TEEEventWatcher::poll_events) and
/// continuous background watching via [`watch`](TEEEventWatcher::watch).
pub struct TEEEventWatcher {
    rpc_url: String,
    contract_address: Address,
    max_block_range: u64,
    stop_flag: Arc<AtomicBool>,
}

impl TEEEventWatcher {
    /// Create a new event watcher for the given RPC endpoint and contract address.
    pub fn new(rpc_url: &str, contract_address: Address) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            contract_address,
            max_block_range: DEFAULT_MAX_BLOCK_RANGE,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the widest block span requested from `eth_getLogs` in one call
    /// (default [`DEFAULT_MAX_BLOCK_RANGE`], minimum 1).
    ///
    /// RPC providers reject log queries over too many blocks, and the limit
    /// differs per provider. Longer spans are fetched in consecutive chunks.
    pub fn with_max_block_range(mut self, max_block_range: u64) -> Self {
        self.max_block_range = max_block_range.max(1);
        self
    }

    /// Returns the watched contract address.
    pub fn contract_address(&self) -> Address {
        self.contract_address
    }

    /// Poll for events from `from_block` to the latest block.
    ///
    /// Returns a tuple of `(events, next_block)` where `next_block` is the block
    /// number to use for the next call (latest + 1). If `from_block` is beyond the
    /// latest block, returns an empty vec and `from_block` unchanged.
    ///
    /// The range is fetched in chunks of at most
    /// [`with_max_block_range`](TEEEventWatcher::with_max_block_range) blocks.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(from_block)))]
    pub async fn poll_events(&self, from_block: u64) -> anyhow::Result<(Vec<TEEEvent>, u64)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let latest = provider.get_block_number().await?;
        if from_block > latest {
            return Ok((vec![], from_block));
        }

        let events = fetch_events(
            &provider,
            self.contract_address,
            from_block,
            latest,
            self.max_block_range,
        )
        .await?;

        Ok((events, latest.saturating_add(1)))
    }

    /// Start a continuous polling loop in a background task.
    ///
    /// Calls `callback` for each new event, starting at the chain head as of the
    /// first successful poll. The loop polls every `poll_interval`. Each poll
    /// reads the new blocks in chunks of at most
    /// [`with_max_block_range`](TEEEventWatcher::with_max_block_range) blocks and
    /// delivers the events of a chunk as soon as it was read.
    ///
    /// A poll that fails (RPC error) stops at the failing chunk, and the next
    /// poll resumes there: events are delayed rather than dropped, and none is
    /// delivered twice. After a failed `eth_getLogs` the next call asks for
    /// half as many blocks (down to 1); after a successful one the span doubles
    /// again, up to the configured maximum. So a provider that rejects wide
    /// ranges slows the watcher down instead of stopping it.
    ///
    /// Poll errors are not reported to the caller (with the `tracing` feature
    /// they are logged as warnings). Use
    /// [`watch_with_errors`](TEEEventWatcher::watch_with_errors) to receive them.
    ///
    /// Use [`stop`](TEEEventWatcher::stop) to signal the loop to exit.
    ///
    /// Returns a `JoinHandle` that resolves when the loop finishes. It resolves
    /// immediately if the RPC URL cannot be parsed.
    pub fn watch<F>(&self, callback: F, poll_interval: Duration) -> JoinHandle<()>
    where
        F: FnMut(TEEEvent) + Send + 'static,
    {
        self.watch_with_errors(callback, |_| {}, poll_interval)
    }

    /// Like [`watch`](TEEEventWatcher::watch), and calls `on_error` with the
    /// error of every poll that fails.
    ///
    /// The loop keeps polling after an error. A watcher whose polls fail every
    /// time delivers nothing, and `on_error` is the only signal of that.
    pub fn watch_with_errors<F, E>(
        &self,
        mut callback: F,
        mut on_error: E,
        poll_interval: Duration,
    ) -> JoinHandle<()>
    where
        F: FnMut(TEEEvent) + Send + 'static,
        E: FnMut(anyhow::Error) + Send + 'static,
    {
        let rpc_url = self.rpc_url.clone();
        let contract_address = self.contract_address;
        let max_block_range = self.max_block_range;
        let stop_flag = self.stop_flag.clone();

        // Reset the stop flag so watch can be called again after a stop.
        stop_flag.store(false, Ordering::SeqCst);

        tokio::spawn(async move {
            let Ok(url) = rpc_url.parse() else {
                #[cfg(feature = "tracing")]
                tracing::error!(%rpc_url, "event watcher: invalid RPC URL, not watching");
                return;
            };
            let provider = ProviderBuilder::new().connect_http(url);
            let mut cursor = WatchCursor::new(max_block_range);

            while !stop_flag.load(Ordering::SeqCst) {
                poll_and_report(
                    &provider,
                    contract_address,
                    &mut cursor,
                    &mut callback,
                    &mut on_error,
                )
                .await;

                tokio::time::sleep(poll_interval).await;
            }
        })
    }

    /// Signal the background watch loop to stop.
    ///
    /// The loop will exit after the current sleep interval completes.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Returns whether the watcher has been signaled to stop.
    pub fn is_stopped(&self) -> bool {
        self.stop_flag.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Bytes, LogData, B256};
    use alloy::rpc::types::Log;

    /// Helper: build a mock Log with the given topics and data.
    fn mock_log(topics: Vec<B256>, data: Vec<u8>, block_number: u64) -> Log {
        let log_data = LogData::new(topics, Bytes::from(data)).expect("valid log data");
        Log {
            inner: alloy::primitives::Log {
                address: Address::ZERO,
                data: log_data,
            },
            block_hash: None,
            block_number: Some(block_number),
            block_timestamp: None,
            transaction_hash: None,
            transaction_index: None,
            log_index: None,
            removed: false,
        }
    }

    // -- Topic hash tests --

    #[test]
    fn test_topic_hashes_are_deterministic() {
        // Calling the same function twice should yield the same value.
        assert_eq!(topic_result_submitted(), topic_result_submitted());
        assert_eq!(topic_result_challenged(), topic_result_challenged());
        assert_eq!(topic_result_finalized(), topic_result_finalized());
        assert_eq!(topic_result_expired(), topic_result_expired());
    }

    #[test]
    fn test_topic_hashes_are_unique() {
        let t1 = topic_result_submitted();
        let t2 = topic_result_challenged();
        let t3 = topic_result_finalized();
        let t4 = topic_result_expired();
        assert_ne!(t1, t2);
        assert_ne!(t1, t3);
        assert_ne!(t1, t4);
        assert_ne!(t2, t3);
        assert_ne!(t2, t4);
        assert_ne!(t3, t4);
    }

    #[test]
    fn test_topic_hash_values() {
        // Verify topic hashes match the expected keccak256 of the Solidity event signatures.
        assert_eq!(
            topic_result_submitted(),
            keccak256("ResultSubmitted(bytes32,bytes32,bytes32,address)")
        );
        assert_eq!(
            topic_result_challenged(),
            keccak256("ResultChallenged(bytes32,address)")
        );
        assert_eq!(
            topic_result_finalized(),
            keccak256("ResultFinalized(bytes32)")
        );
        assert_eq!(topic_result_expired(), keccak256("ResultExpired(bytes32)"));
    }

    // -- parse_log tests --

    #[test]
    fn test_parse_log_empty_topics() {
        let log = mock_log(vec![], vec![], 100);
        assert!(parse_log(&log).is_none());
    }

    #[test]
    fn test_parse_log_unknown_topic() {
        let unknown = keccak256("UnknownEvent(uint256)");
        let log = mock_log(vec![unknown], vec![], 100);
        assert!(parse_log(&log).is_none());
    }

    #[test]
    fn test_parse_log_result_submitted() {
        let result_id = B256::from([0x11; 32]);
        let model_hash = B256::from([0x22; 32]);
        // submitter is an address, left-padded to 32 bytes in the topic
        let submitter_addr: Address = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap();
        let mut submitter_topic = [0u8; 32];
        submitter_topic[12..32].copy_from_slice(submitter_addr.as_slice());
        let submitter_b256 = B256::from(submitter_topic);

        // inputHash is in data (non-indexed)
        let input_hash = B256::from([0x33; 32]);
        let data = input_hash.as_slice().to_vec();

        let topics = vec![
            topic_result_submitted(),
            result_id,
            model_hash,
            submitter_b256,
        ];
        let log = mock_log(topics, data, 42);

        let event = parse_log(&log).expect("should parse ResultSubmitted");
        match event {
            TEEEvent::ResultSubmitted {
                result_id: rid,
                model_hash: mh,
                input_hash: ih,
                submitter: sub,
                block_number: bn,
            } => {
                assert_eq!(rid, result_id);
                assert_eq!(mh, model_hash);
                assert_eq!(ih, input_hash);
                assert_eq!(sub, submitter_addr);
                assert_eq!(bn, 42);
            }
            _ => panic!("expected ResultSubmitted"),
        }
    }

    #[test]
    fn test_parse_log_result_submitted_insufficient_data() {
        let result_id = B256::from([0x11; 32]);
        let model_hash = B256::from([0x22; 32]);
        let submitter_b256 = B256::from([0x00; 32]);

        let topics = vec![
            topic_result_submitted(),
            result_id,
            model_hash,
            submitter_b256,
        ];
        // data too short (less than 32 bytes for inputHash)
        let log = mock_log(topics, vec![0u8; 16], 10);
        assert!(parse_log(&log).is_none());
    }

    #[test]
    fn test_parse_log_result_challenged() {
        let result_id = B256::from([0x44; 32]);
        let challenger_addr: Address = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .parse()
            .unwrap();

        // challenger is non-indexed, ABI-encoded in data (left-padded to 32 bytes)
        let mut data = [0u8; 32];
        data[12..32].copy_from_slice(challenger_addr.as_slice());

        let topics = vec![topic_result_challenged(), result_id];
        let log = mock_log(topics, data.to_vec(), 55);

        let event = parse_log(&log).expect("should parse ResultChallenged");
        match event {
            TEEEvent::ResultChallenged {
                result_id: rid,
                challenger,
                block_number: bn,
            } => {
                assert_eq!(rid, result_id);
                assert_eq!(challenger, challenger_addr);
                assert_eq!(bn, 55);
            }
            _ => panic!("expected ResultChallenged"),
        }
    }

    #[test]
    fn test_parse_log_result_challenged_insufficient_data() {
        let result_id = B256::from([0x44; 32]);
        let topics = vec![topic_result_challenged(), result_id];
        let log = mock_log(topics, vec![0u8; 10], 55);
        assert!(parse_log(&log).is_none());
    }

    #[test]
    fn test_parse_log_result_finalized() {
        let result_id = B256::from([0x55; 32]);
        let topics = vec![topic_result_finalized(), result_id];
        let log = mock_log(topics, vec![], 77);

        let event = parse_log(&log).expect("should parse ResultFinalized");
        match event {
            TEEEvent::ResultFinalized {
                result_id: rid,
                block_number: bn,
            } => {
                assert_eq!(rid, result_id);
                assert_eq!(bn, 77);
            }
            _ => panic!("expected ResultFinalized"),
        }
    }

    #[test]
    fn test_parse_log_result_expired() {
        let result_id = B256::from([0x66; 32]);
        let topics = vec![topic_result_expired(), result_id];
        let log = mock_log(topics, vec![], 88);

        let event = parse_log(&log).expect("should parse ResultExpired");
        match event {
            TEEEvent::ResultExpired {
                result_id: rid,
                block_number: bn,
            } => {
                assert_eq!(rid, result_id);
                assert_eq!(bn, 88);
            }
            _ => panic!("expected ResultExpired"),
        }
    }

    #[test]
    fn test_parse_log_missing_result_id_topic() {
        // Only topic0, no topic[1] for resultId
        let topics = vec![topic_result_finalized()];
        let log = mock_log(topics, vec![], 100);
        assert!(parse_log(&log).is_none());
    }

    // -- Serialization round-trip tests --

    #[test]
    fn test_event_serialization_result_submitted() {
        let event = TEEEvent::ResultSubmitted {
            result_id: B256::from([0x11; 32]),
            model_hash: B256::from([0x22; 32]),
            input_hash: B256::from([0x33; 32]),
            submitter: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .parse()
                .unwrap(),
            block_number: 42,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: TEEEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serialization_result_challenged() {
        let event = TEEEvent::ResultChallenged {
            result_id: B256::from([0x44; 32]),
            challenger: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .parse()
                .unwrap(),
            block_number: 55,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: TEEEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serialization_result_finalized() {
        let event = TEEEvent::ResultFinalized {
            result_id: B256::from([0x55; 32]),
            block_number: 77,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: TEEEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serialization_result_expired() {
        let event = TEEEvent::ResultExpired {
            result_id: B256::from([0x66; 32]),
            block_number: 88,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: TEEEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_event_serialization_includes_type_tag() {
        let event = TEEEvent::ResultFinalized {
            result_id: B256::ZERO,
            block_number: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ResultFinalized\""));
    }

    // -- TEEEventWatcher unit tests --

    #[test]
    fn test_watcher_new() {
        let addr: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();
        let watcher = TEEEventWatcher::new("http://localhost:8545", addr);
        assert_eq!(watcher.contract_address(), addr);
        assert!(!watcher.is_stopped());
    }

    #[test]
    fn test_watcher_stop_flag() {
        let watcher = TEEEventWatcher::new("http://localhost:8545", Address::ZERO);
        assert!(!watcher.is_stopped());
        watcher.stop();
        assert!(watcher.is_stopped());
    }

    #[test]
    fn test_watcher_max_block_range() {
        let watcher = TEEEventWatcher::new("http://localhost:8545", Address::ZERO);
        assert_eq!(watcher.max_block_range, DEFAULT_MAX_BLOCK_RANGE);
        assert_eq!(watcher.with_max_block_range(0).max_block_range, 1);
    }

    // -- Polling against a mocked RPC transport --

    /// A `ResultFinalized` log as `eth_getLogs` would return it.
    fn finalized_log(id: u8, block_number: u64) -> Log {
        mock_log(
            vec![topic_result_finalized(), B256::from([id; 32])],
            vec![],
            block_number,
        )
    }

    fn mocked_provider(asserter: &alloy::transports::mock::Asserter) -> impl Provider {
        ProviderBuilder::new().connect_mocked_client(asserter.clone())
    }

    #[tokio::test]
    async fn test_fetch_events_splits_wide_ranges() {
        let asserter = alloy::transports::mock::Asserter::new();
        let provider = mocked_provider(&asserter);

        // Blocks 0..=30 with at most 10 blocks per call: 0-9, 10-19, 20-29, 30.
        // Chunks of 11 blocks would cover the range in three calls.
        asserter.push_success(&vec![finalized_log(1, 3)]);
        asserter.push_success(&vec![finalized_log(2, 12)]);
        asserter.push_success(&vec![finalized_log(3, 24)]);
        asserter.push_success(&vec![finalized_log(4, 30)]);

        let events = fetch_events(&provider, Address::ZERO, 0, 30, 10)
            .await
            .expect("four eth_getLogs calls");
        let blocks: Vec<u64> = events
            .iter()
            .map(|event| match event {
                TEEEvent::ResultFinalized { block_number, .. } => *block_number,
                other => panic!("unexpected event {other:?}"),
            })
            .collect();
        assert_eq!(blocks, vec![3, 12, 24, 30]);
        assert!(asserter.read_q().is_empty(), "one response per chunk");

        // A range that fits is a single call, also at the top of the u64 range.
        asserter.push_success(&Vec::<Log>::new());
        let events = fetch_events(&provider, Address::ZERO, u64::MAX - 2, u64::MAX, 10)
            .await
            .expect("one eth_getLogs call");
        assert!(events.is_empty());
        assert!(asserter.read_q().is_empty());
    }

    /// Block numbers of `ResultFinalized` events, in delivery order.
    fn finalized_blocks(events: &[TEEEvent]) -> Vec<u64> {
        events
            .iter()
            .map(|event| match event {
                TEEEvent::ResultFinalized { block_number, .. } => *block_number,
                other => panic!("unexpected event {other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn test_poll_new_events_retries_range_after_rpc_error() {
        let asserter = alloy::transports::mock::Asserter::new();
        let provider = mocked_provider(&asserter);
        let mut cursor = WatchCursor::new(100);
        let mut events = Vec::new();
        let mut deliver = |event| events.push(event);

        // First poll: learns the chain head (block 7) and reads that block.
        asserter.push_success(&7u64);
        asserter.push_success(&Vec::<Log>::new());
        poll_new_events(&provider, Address::ZERO, &mut cursor, &mut deliver)
            .await
            .unwrap();
        assert_eq!(cursor.next_block, Some(8));

        // Second poll: the head moved to 9 but eth_getLogs fails.
        asserter.push_success(&9u64);
        asserter.push_failure_msg("upstream unavailable");
        let err = poll_new_events(&provider, Address::ZERO, &mut cursor, &mut deliver).await;
        assert!(err.is_err());
        assert_eq!(
            cursor.next_block,
            Some(8),
            "a failed poll must not skip blocks"
        );

        // Third poll: blocks 8..=9 are fetched again and the event is delivered.
        asserter.push_success(&9u64);
        asserter.push_success(&vec![finalized_log(5, 8)]);
        poll_new_events(&provider, Address::ZERO, &mut cursor, &mut deliver)
            .await
            .unwrap();
        assert_eq!(cursor.next_block, Some(10));

        // Fourth poll: nothing new, no eth_getLogs call at all.
        asserter.push_success(&9u64);
        poll_new_events(&provider, Address::ZERO, &mut cursor, &mut deliver)
            .await
            .unwrap();
        assert_eq!(cursor.next_block, Some(10));
        assert!(asserter.read_q().is_empty());
        assert_eq!(
            events,
            vec![TEEEvent::ResultFinalized {
                result_id: B256::from([5; 32]),
                block_number: 8,
            }]
        );
    }

    #[tokio::test]
    async fn test_watch_poll_halves_span_after_failed_request() {
        let asserter = alloy::transports::mock::Asserter::new();
        let provider = mocked_provider(&asserter);
        let mut cursor = WatchCursor::new(8);
        let mut events = Vec::new();
        let mut callback = |event| events.push(event);
        let mut errors = Vec::new();
        let mut on_error = |err: anyhow::Error| errors.push(format!("{err:#}"));

        // First poll: learns the chain head (block 10) and reads that block.
        asserter.push_success(&10u64);
        asserter.push_success(&Vec::<Log>::new());
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert_eq!((cursor.next_block, cursor.span), (Some(11), 8));

        // Second poll: the head is at 26 and the request for 11..=18 is rejected.
        asserter.push_success(&26u64);
        asserter.push_failure_msg("block range too wide");
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert_eq!((cursor.next_block, cursor.span), (Some(11), 4));

        // Third poll: 11..=14 (4 blocks), then 15..=22 and 23..=26 with the
        // span doubled again. With the span still at 8 two calls would do, and
        // the third response would be left in the queue.
        asserter.push_success(&26u64);
        asserter.push_success(&vec![finalized_log(1, 12)]);
        asserter.push_success(&vec![finalized_log(2, 20)]);
        asserter.push_success(&vec![finalized_log(3, 26)]);
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert_eq!((cursor.next_block, cursor.span), (Some(27), 8));
        assert!(asserter.read_q().is_empty(), "one response per chunk");

        // Fourth poll: nothing new.
        asserter.push_success(&26u64);
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert!(asserter.read_q().is_empty());

        assert_eq!(finalized_blocks(&events), vec![12, 20, 26]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("block range too wide"), "{errors:?}");
    }

    #[tokio::test]
    async fn test_watch_poll_keeps_chunks_before_a_failed_one() {
        let asserter = alloy::transports::mock::Asserter::new();
        let provider = mocked_provider(&asserter);
        let mut cursor = WatchCursor::new(4);
        let mut events = Vec::new();
        let mut callback = |event| events.push(event);
        let mut errors = 0;
        let mut on_error = |_err: anyhow::Error| errors += 1;

        // First poll: learns the chain head (block 0) and reads that block.
        asserter.push_success(&0u64);
        asserter.push_success(&Vec::<Log>::new());
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert_eq!((cursor.next_block, cursor.span), (Some(1), 4));

        // Second poll: the head is at 12; 1..=4 is read, 5..=8 fails.
        asserter.push_success(&12u64);
        asserter.push_success(&vec![finalized_log(1, 2)]);
        asserter.push_failure_msg("upstream unavailable");
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert_eq!((cursor.next_block, cursor.span), (Some(5), 2));

        // Third poll: resumes at block 5 with 5..=6, 7..=10 and 11..=12. A
        // poll that started at block 1 again would need a fourth response.
        asserter.push_success(&12u64);
        asserter.push_success(&vec![finalized_log(2, 5)]);
        asserter.push_success(&vec![finalized_log(3, 9)]);
        asserter.push_success(&Vec::<Log>::new());
        poll_and_report(
            &provider,
            Address::ZERO,
            &mut cursor,
            &mut callback,
            &mut on_error,
        )
        .await;
        assert_eq!((cursor.next_block, cursor.span), (Some(13), 4));
        assert!(asserter.read_q().is_empty(), "one response per chunk");

        assert_eq!(finalized_blocks(&events), vec![2, 5, 9]);
        assert_eq!(errors, 1);
    }

    #[tokio::test]
    async fn test_watch_with_errors_reports_failed_polls() {
        // A port nothing listens on: every poll fails.
        let closed_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let watcher =
            TEEEventWatcher::new(&format!("http://127.0.0.1:{closed_port}"), Address::ZERO);

        let (errors, mut reported) = tokio::sync::mpsc::unbounded_channel();
        let handle = watcher.watch_with_errors(
            |event| panic!("unexpected event {event:?}"),
            move |err| {
                let _ = errors.send(err);
            },
            Duration::from_millis(1),
        );
        // The time limit turns a watcher that never reports into a failure.
        for what in ["the first poll error", "the loop goes on after an error"] {
            tokio::time::timeout(Duration::from_secs(30), reported.recv())
                .await
                .unwrap_or_else(|_| panic!("on_error was not called: {what}"))
                .expect(what);
        }
        watcher.stop();
        handle.await.unwrap();
    }

    #[test]
    fn test_event_clone() {
        let event = TEEEvent::ResultSubmitted {
            result_id: B256::from([0x11; 32]),
            model_hash: B256::from([0x22; 32]),
            input_hash: B256::from([0x33; 32]),
            submitter: Address::ZERO,
            block_number: 1,
        };
        let cloned = event.clone();
        assert_eq!(event, cloned);
    }

    #[test]
    fn test_event_debug_format() {
        let event = TEEEvent::ResultFinalized {
            result_id: B256::ZERO,
            block_number: 99,
        };
        let debug = format!("{:?}", event);
        assert!(debug.contains("ResultFinalized"));
        assert!(debug.contains("99"));
    }
}
