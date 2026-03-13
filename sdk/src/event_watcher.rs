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

/// Watches TEEMLVerifier events via `eth_getLogs` polling.
///
/// Supports one-shot polling via [`poll_events`](TEEEventWatcher::poll_events) and
/// continuous background watching via [`watch`](TEEEventWatcher::watch).
pub struct TEEEventWatcher {
    rpc_url: String,
    contract_address: Address,
    stop_flag: Arc<AtomicBool>,
}

impl TEEEventWatcher {
    /// Create a new event watcher for the given RPC endpoint and contract address.
    pub fn new(rpc_url: &str, contract_address: Address) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            contract_address,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
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
    pub async fn poll_events(&self, from_block: u64) -> anyhow::Result<(Vec<TEEEvent>, u64)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let latest = provider.get_block_number().await?;
        if from_block > latest {
            return Ok((vec![], from_block));
        }

        let filter = Filter::new()
            .address(self.contract_address)
            .from_block(from_block)
            .to_block(latest);

        let logs = provider.get_logs(&filter).await?;
        let events: Vec<TEEEvent> = logs.iter().filter_map(parse_log).collect();

        Ok((events, latest + 1))
    }

    /// Start a continuous polling loop in a background task.
    ///
    /// Calls `callback` for each new event. The loop polls every `poll_interval`.
    /// Use [`stop`](TEEEventWatcher::stop) to signal the loop to exit.
    ///
    /// Returns a `JoinHandle` that resolves when the loop finishes.
    pub fn watch<F>(&self, mut callback: F, poll_interval: Duration) -> JoinHandle<()>
    where
        F: FnMut(TEEEvent) + Send + 'static,
    {
        let rpc_url = self.rpc_url.clone();
        let contract_address = self.contract_address;
        let stop_flag = self.stop_flag.clone();

        // Reset the stop flag so watch can be called again after a stop.
        stop_flag.store(false, Ordering::SeqCst);

        tokio::spawn(async move {
            let mut from_block: u64 = 0;

            // Attempt to get the current block number for the starting point.
            if let Ok(provider) = rpc_url
                .parse()
                .map(|url| ProviderBuilder::new().connect_http(url))
            {
                if let Ok(latest) = provider.get_block_number().await {
                    from_block = latest;
                }
            }

            while !stop_flag.load(Ordering::SeqCst) {
                if let Ok(provider) = rpc_url
                    .parse()
                    .map(|url| ProviderBuilder::new().connect_http(url))
                {
                    if let Ok(latest) = provider.get_block_number().await {
                        if from_block <= latest {
                            let filter = Filter::new()
                                .address(contract_address)
                                .from_block(from_block)
                                .to_block(latest);

                            if let Ok(logs) = provider.get_logs(&filter).await {
                                for log in &logs {
                                    if let Some(event) = parse_log(log) {
                                        callback(event);
                                    }
                                }
                            }

                            from_block = latest + 1;
                        }
                    }
                }

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
