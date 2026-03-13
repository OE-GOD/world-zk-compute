//! Shared TEE event watching library.
//!
//! Provides [`EventWatcher`] for polling TEEMLVerifier contract events via
//! `eth_getLogs`, along with the [`TEEEvent`] enum and log-parsing helpers.
//! Used by both the operator service and the indexer service.

use alloy::primitives::{keccak256, Address, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{Filter, Log};

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Parsed event from a TEEMLVerifier log.
#[derive(Debug, Clone)]
pub enum TEEEvent {
    ResultSubmitted {
        result_id: B256,
        model_hash: B256,
        input_hash: B256,
        submitter: Address,
        /// Block number the event was emitted in (0 if unavailable).
        block_number: u64,
    },
    ResultChallenged {
        result_id: B256,
        challenger: Address,
    },
    ResultFinalized {
        result_id: B256,
    },
    /// Emitted when finalize() is called after challenge window with no challenge.
    ResultExpired {
        result_id: B256,
    },
    /// Emitted when a dispute is resolved on-chain.
    DisputeResolved {
        result_id: B256,
        prover_won: bool,
    },
}

/// A [`TEEEvent`] tagged with the contract address it originated from.
/// Useful when watching multiple contracts to know which contract emitted the event.
#[derive(Debug, Clone)]
pub struct TaggedEvent {
    /// The contract address that emitted this event.
    pub contract_address: Address,
    /// The parsed event.
    pub event: TEEEvent,
}

// ---------------------------------------------------------------------------
// Topic hashes
// ---------------------------------------------------------------------------

/// Keccak-256 topic hash for `ResultSubmitted(bytes32,bytes32,bytes32,address)`.
pub fn topic_result_submitted() -> B256 {
    keccak256("ResultSubmitted(bytes32,bytes32,bytes32,address)")
}

/// Keccak-256 topic hash for `ResultChallenged(bytes32,address)`.
pub fn topic_result_challenged() -> B256 {
    keccak256("ResultChallenged(bytes32,address)")
}

/// Keccak-256 topic hash for `ResultFinalized(bytes32)`.
pub fn topic_result_finalized() -> B256 {
    keccak256("ResultFinalized(bytes32)")
}

/// Keccak-256 topic hash for `ResultExpired(bytes32)`.
pub fn topic_result_expired() -> B256 {
    keccak256("ResultExpired(bytes32)")
}

/// Keccak-256 topic hash for `DisputeResolved(bytes32,bool)`.
pub fn topic_dispute_resolved() -> B256 {
    keccak256("DisputeResolved(bytes32,bool)")
}

// ---------------------------------------------------------------------------
// Log parsing
// ---------------------------------------------------------------------------

/// Parse a single [`Log`] into a [`TEEEvent`], returning `None` if the log
/// does not match any known event signature.
pub fn parse_log(log: &Log) -> Option<TEEEvent> {
    let topics = log.topics();
    if topics.is_empty() {
        return None;
    }

    let topic0 = topics[0];
    let block_number = log.block_number.unwrap_or(0);

    if topic0 == topic_result_submitted() {
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
        let result_id = *topics.get(1)?;
        let data = log.data().data.as_ref();
        if data.len() < 32 {
            return None;
        }
        let challenger = Address::from_slice(&data[12..32]);
        Some(TEEEvent::ResultChallenged {
            result_id,
            challenger,
        })
    } else if topic0 == topic_result_finalized() {
        let result_id = *topics.get(1)?;
        Some(TEEEvent::ResultFinalized { result_id })
    } else if topic0 == topic_result_expired() {
        let result_id = *topics.get(1)?;
        Some(TEEEvent::ResultExpired { result_id })
    } else if topic0 == topic_dispute_resolved() {
        let result_id = *topics.get(1)?;
        let data = log.data().data.as_ref();
        if data.len() < 32 {
            return None;
        }
        let prover_won = data[31] != 0;
        Some(TEEEvent::DisputeResolved {
            result_id,
            prover_won,
        })
    } else {
        None
    }
}

/// Parse a log into a [`TaggedEvent`], extracting the emitting contract address.
pub fn parse_log_tagged(log: &Log) -> Option<TaggedEvent> {
    let contract_address = log.address();
    parse_log(log).map(|event| TaggedEvent {
        contract_address,
        event,
    })
}

// ---------------------------------------------------------------------------
// EventWatcher
// ---------------------------------------------------------------------------

/// Watches TEEMLVerifier events via `eth_getLogs` polling.
/// Supports watching one or more contract addresses.
pub struct EventWatcher {
    rpc_url: String,
    contract_addresses: Vec<Address>,
}

impl EventWatcher {
    /// Create a new `EventWatcher` for a single contract address.
    pub fn new(rpc_url: &str, contract_address: Address) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            contract_addresses: vec![contract_address],
        }
    }

    /// Create a new `EventWatcher` for multiple contract addresses.
    ///
    /// The filter will match events from any of the provided addresses.
    /// Each returned [`TaggedEvent`] includes the `contract_address` that emitted it.
    ///
    /// # Panics
    /// Panics if `addresses` is empty.
    pub fn new_multi(rpc_url: &str, addresses: Vec<Address>) -> Self {
        assert!(
            !addresses.is_empty(),
            "EventWatcher requires at least one contract address"
        );
        Self {
            rpc_url: rpc_url.to_string(),
            contract_addresses: addresses,
        }
    }

    /// Returns the first contract address (for backward compatibility).
    pub fn contract_address(&self) -> Address {
        self.contract_addresses[0]
    }

    /// Returns all watched contract addresses.
    pub fn contract_addresses(&self) -> &[Address] {
        &self.contract_addresses
    }

    /// Build a filter for the watched addresses.
    fn build_address_filter(&self) -> Filter {
        if self.contract_addresses.len() == 1 {
            Filter::new().address(self.contract_addresses[0])
        } else {
            Filter::new().address(self.contract_addresses.clone())
        }
    }

    /// Poll for new events since `from_block`. Returns events and the next block to poll from.
    pub async fn poll_events(&self, from_block: u64) -> anyhow::Result<(Vec<TEEEvent>, u64)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let latest = provider.get_block_number().await?;
        if from_block > latest {
            return Ok((vec![], from_block));
        }

        let filter = self
            .build_address_filter()
            .from_block(from_block)
            .to_block(latest);

        let logs = provider.get_logs(&filter).await?;
        let events: Vec<TEEEvent> = logs.iter().filter_map(parse_log).collect();

        Ok((events, latest + 1))
    }

    /// Poll for new events since `from_block`, tagging each with its source contract address.
    ///
    /// Returns tagged events and the next block to poll from.
    pub async fn poll_events_tagged(
        &self,
        from_block: u64,
    ) -> anyhow::Result<(Vec<TaggedEvent>, u64)> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let latest = provider.get_block_number().await?;
        if from_block > latest {
            return Ok((vec![], from_block));
        }

        let filter = self
            .build_address_filter()
            .from_block(from_block)
            .to_block(latest);

        let logs = provider.get_logs(&filter).await?;
        let events: Vec<TaggedEvent> = logs.iter().filter_map(parse_log_tagged).collect();

        Ok((events, latest + 1))
    }

    /// Get all `ResultChallenged` events in a block range.
    pub async fn get_challenges(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> anyhow::Result<Vec<(B256, Address)>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let filter = self
            .build_address_filter()
            .event_signature(topic_result_challenged())
            .from_block(from_block)
            .to_block(to_block);

        let logs = provider.get_logs(&filter).await?;
        let mut results = Vec::new();
        for log in &logs {
            if let Some(TEEEvent::ResultChallenged {
                result_id,
                challenger,
            }) = parse_log(log)
            {
                results.push((result_id, challenger));
            }
        }
        Ok(results)
    }

    /// Get all `ResultSubmitted` events in a block range.
    pub async fn get_submissions(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> anyhow::Result<Vec<B256>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let filter = self
            .build_address_filter()
            .event_signature(topic_result_submitted())
            .from_block(from_block)
            .to_block(to_block);

        let logs = provider.get_logs(&filter).await?;
        let mut results = Vec::new();
        for log in &logs {
            if let Some(TEEEvent::ResultSubmitted { result_id, .. }) = parse_log(log) {
                results.push(result_id);
            }
        }
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_hashes_unique() {
        let t1 = topic_result_submitted();
        let t2 = topic_result_challenged();
        let t3 = topic_result_finalized();
        let t4 = topic_result_expired();
        let t5 = topic_dispute_resolved();
        let all = [t1, t2, t3, t4, t5];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "topics {} and {} collide", i, j);
            }
        }
    }

    #[test]
    fn test_topic_hashes_deterministic() {
        assert_eq!(topic_result_submitted(), topic_result_submitted());
        assert_eq!(topic_result_challenged(), topic_result_challenged());
        assert_eq!(topic_result_finalized(), topic_result_finalized());
        assert_eq!(topic_result_expired(), topic_result_expired());
        assert_eq!(topic_dispute_resolved(), topic_dispute_resolved());
    }

    #[test]
    fn test_watcher_new() {
        let addr = Address::ZERO;
        let w = EventWatcher::new("http://localhost:8545", addr);
        assert_eq!(w.contract_address(), addr);
        assert_eq!(w.contract_addresses().len(), 1);
        assert_eq!(w.contract_addresses()[0], addr);
    }

    #[test]
    fn test_watcher_new_multi() {
        let addr1: Address = "0x1111111111111111111111111111111111111111"
            .parse()
            .unwrap();
        let addr2: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .unwrap();
        let w = EventWatcher::new_multi("http://localhost:8545", vec![addr1, addr2]);
        assert_eq!(w.contract_addresses().len(), 2);
        assert_eq!(w.contract_addresses()[0], addr1);
        assert_eq!(w.contract_addresses()[1], addr2);
        assert_eq!(w.contract_address(), addr1);
    }

    #[test]
    #[should_panic(expected = "EventWatcher requires at least one contract address")]
    fn test_watcher_new_multi_empty_panics() {
        EventWatcher::new_multi("http://localhost:8545", vec![]);
    }

    #[test]
    fn test_single_address_backward_compat() {
        let addr: Address = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap();
        let w_old = EventWatcher::new("http://localhost:8545", addr);
        let w_new = EventWatcher::new_multi("http://localhost:8545", vec![addr]);

        assert_eq!(w_old.contract_address(), w_new.contract_address());
        assert_eq!(w_old.contract_addresses(), w_new.contract_addresses());
    }

    #[test]
    fn test_tagged_event_debug() {
        let addr: Address = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .parse()
            .unwrap();
        let tagged = TaggedEvent {
            contract_address: addr,
            event: TEEEvent::ResultFinalized {
                result_id: B256::ZERO,
            },
        };
        let debug_str = format!("{:?}", tagged);
        assert!(debug_str.contains("TaggedEvent"));
        assert!(debug_str.contains("ResultFinalized"));
    }

    #[test]
    fn test_build_address_filter_single() {
        let addr: Address = "0xcccccccccccccccccccccccccccccccccccccccc"
            .parse()
            .unwrap();
        let w = EventWatcher::new("http://localhost:8545", addr);
        let _filter = w.build_address_filter();
    }

    #[test]
    fn test_build_address_filter_multi() {
        let addr1: Address = "0x1111111111111111111111111111111111111111"
            .parse()
            .unwrap();
        let addr2: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .unwrap();
        let addr3: Address = "0x3333333333333333333333333333333333333333"
            .parse()
            .unwrap();
        let w = EventWatcher::new_multi("http://localhost:8545", vec![addr1, addr2, addr3]);
        let _filter = w.build_address_filter();
    }

    #[test]
    fn test_event_clone() {
        let event = TEEEvent::ResultSubmitted {
            result_id: B256::ZERO,
            model_hash: B256::ZERO,
            input_hash: B256::ZERO,
            submitter: Address::ZERO,
            block_number: 42,
        };
        let cloned = event.clone();
        if let TEEEvent::ResultSubmitted { block_number, .. } = cloned {
            assert_eq!(block_number, 42);
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn test_dispute_resolved_variant() {
        let event = TEEEvent::DisputeResolved {
            result_id: B256::ZERO,
            prover_won: true,
        };
        if let TEEEvent::DisputeResolved { prover_won, .. } = event {
            assert!(prover_won);
        } else {
            panic!("unexpected variant");
        }
    }
}
