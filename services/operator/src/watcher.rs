use alloy::primitives::{keccak256, Address, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{Filter, Log};

/// Watches TEEMLVerifier events via eth_getLogs polling.
pub struct EventWatcher {
    rpc_url: String,
    contract_address: Address,
}

/// Parsed event from a TEEMLVerifier log.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum TEEEvent {
    ResultSubmitted {
        result_id: B256,
        #[allow(dead_code)]
        model_hash: B256,
        #[allow(dead_code)]
        input_hash: B256,
        #[allow(dead_code)]
        submitter: Address,
    },
    ResultChallenged {
        result_id: B256,
        challenger: Address,
    },
    ResultFinalized {
        result_id: B256,
    },
    /// Emitted when finalize() is called after challenge window with no challenge.
    /// The result expired without being challenged, indicating normal happy-path completion.
    ResultExpired {
        result_id: B256,
    },
}

fn topic_result_submitted() -> B256 {
    keccak256("ResultSubmitted(bytes32,bytes32,bytes32,address)")
}

fn topic_result_challenged() -> B256 {
    keccak256("ResultChallenged(bytes32,address)")
}

fn topic_result_finalized() -> B256 {
    keccak256("ResultFinalized(bytes32)")
}

fn topic_result_expired() -> B256 {
    keccak256("ResultExpired(bytes32)")
}

fn parse_log(log: &Log) -> Option<TEEEvent> {
    let topics = &log.topics();
    if topics.is_empty() {
        return None;
    }

    let topic0 = topics[0];

    if topic0 == topic_result_challenged() {
        let result_id = *topics.get(1)?;
        // challenger is ABI-encoded in data (non-indexed)
        let data = log.data().data.as_ref();
        if data.len() < 32 {
            return None;
        }
        let challenger = Address::from_slice(&data[12..32]);
        Some(TEEEvent::ResultChallenged {
            result_id,
            challenger,
        })
    } else if topic0 == topic_result_submitted() {
        // Updated layout: resultId (topic[1]), modelHash (topic[2]), submitter (topic[3])
        // inputHash is non-indexed, in data
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
        })
    } else if topic0 == topic_result_finalized() {
        let result_id = *topics.get(1)?;
        Some(TEEEvent::ResultFinalized { result_id })
    } else if topic0 == topic_result_expired() {
        let result_id = *topics.get(1)?;
        Some(TEEEvent::ResultExpired { result_id })
    } else {
        None
    }
}

impl EventWatcher {
    pub fn new(rpc_url: &str, contract_address: Address) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            contract_address,
        }
    }

    /// Poll for new events since `from_block`. Returns events and the next block to poll from.
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

    /// Get all ResultChallenged events in a block range.
    #[allow(dead_code)]
    pub async fn get_challenges(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> anyhow::Result<Vec<(B256, Address)>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let filter = Filter::new()
            .address(self.contract_address)
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

    /// Get all ResultSubmitted events in a block range.
    #[allow(dead_code)]
    pub async fn get_submissions(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> anyhow::Result<Vec<B256>> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let filter = Filter::new()
            .address(self.contract_address)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_hashes() {
        // Verify topic hashes are deterministic and unique
        let t1 = topic_result_submitted();
        let t2 = topic_result_challenged();
        let t3 = topic_result_finalized();
        let t4 = topic_result_expired();
        assert_ne!(t1, t2);
        assert_ne!(t2, t3);
        assert_ne!(t1, t3);
        assert_ne!(t1, t4);
        assert_ne!(t2, t4);
        assert_ne!(t3, t4);
    }

    #[test]
    fn test_watcher_new() {
        let addr = Address::ZERO;
        let w = EventWatcher::new("http://localhost:8545", addr);
        assert_eq!(w.rpc_url, "http://localhost:8545");
        assert_eq!(w.contract_address, addr);
    }
}
