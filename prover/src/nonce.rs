//! Nonce Management for Parallel Transactions
//!
//! When sending multiple transactions in parallel, the default nonce filler
//! fetches the nonce from the chain for each transaction. This causes conflicts
//! when transactions are sent faster than they're confirmed.
//!
//! This module provides a `NonceManager` that:
//! - Sends transactions one at a time (estimate, sign and broadcast happen
//!   under a lock; receipts are awaited outside of it)
//! - Uses up a nonce only for a transaction the node accepted
//! - Re-reads the next nonce from the chain (pending count) on initialization
//!   and after a failed send
//!
//! ## Usage
//!
//! ```rust
//! let nonce_manager = NonceManager::new(provider, address).await?;
//!
//! // Send with the next nonce
//! let pending_tx = nonce_manager
//!     .send(|nonce| async move { call.nonce(nonce).send().await })
//!     .await?;
//!
//! // When a sent transaction may have been dropped, re-read the next nonce
//! nonce_manager.sync_from_chain().await;
//! ```

use crate::redact;
use alloy::primitives::Address;
use alloy::providers::Provider;
use anyhow::Result;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Nonce manager for parallel transaction submission
///
/// Holds the nonce of the next transaction. Sends take turns, so that a nonce
/// is either used by a transaction the node accepted or given to the next send.
pub struct NonceManager<P> {
    /// The provider for chain queries
    provider: Arc<P>,
    /// The address we're managing nonces for
    address: Address,
    /// Nonce of the next transaction; locked while a transaction is sent
    next_nonce: Mutex<u64>,
}

impl<P> NonceManager<P>
where
    P: Provider + Clone + 'static,
{
    /// Create a new nonce manager, reading the initial nonce from the chain
    pub async fn new(provider: Arc<P>, address: Address) -> Result<Self> {
        let nonce = provider.get_transaction_count(address).pending().await?;

        info!(
            "NonceManager initialized for {} with nonce {}",
            address, nonce
        );

        Ok(Self {
            provider,
            address,
            next_nonce: Mutex::new(nonce),
        })
    }

    /// Send a transaction with the next nonce
    ///
    /// `send` is called with the nonce and must return once the node has
    /// accepted or refused the transaction, without waiting for the receipt.
    /// Other sends wait meanwhile. On success the nonce is used up; on an
    /// error the next nonce is re-read from the chain.
    pub async fn send<T, E, F, Fut>(&self, send: F) -> Result<T, E>
    where
        F: FnOnce(u64) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut next_nonce = self.next_nonce.lock().await;

        let result = send(*next_nonce).await;
        match result {
            Ok(_) => *next_nonce += 1,
            Err(_) => self.read_pending_count(&mut next_nonce).await,
        }

        result
    }

    /// Get the nonce of the next transaction
    pub async fn current(&self) -> u64 {
        *self.next_nonce.lock().await
    }

    /// Re-read the next nonce from the chain
    ///
    /// Use this when a transaction that was sent may have been dropped.
    pub async fn sync_from_chain(&self) {
        let mut next_nonce = self.next_nonce.lock().await;
        self.read_pending_count(&mut next_nonce).await;
    }

    /// Set the next nonce to the pending transaction count. If that cannot be
    /// read, the nonce stays as it is.
    async fn read_pending_count(&self, next_nonce: &mut u64) {
        match self
            .provider
            .get_transaction_count(self.address)
            .pending()
            .await
        {
            Ok(chain_nonce) => {
                if chain_nonce != *next_nonce {
                    info!("Nonce synced from chain: {} -> {}", next_nonce, chain_nonce);
                }
                *next_nonce = chain_nonce;
            }
            Err(e) => warn!(
                "Failed to read nonce from chain, keeping {}: {}",
                next_nonce,
                redact::redact_urls(&e.to_string())
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{MockChain, MockNode};

    #[tokio::test]
    async fn test_starts_from_pending_count() {
        let node = MockNode::start(MockChain {
            mined_count: 3,
            pending_count: 5,
            ..Default::default()
        })
        .await;

        assert_eq!(node.nonce_manager.current().await, 5);
    }

    #[tokio::test]
    async fn test_accepted_send_uses_up_the_nonce() {
        let node = MockNode::start(MockChain::default()).await;

        for expected in 0..3 {
            let nonce = node
                .nonce_manager
                .send(|nonce| async move { Ok::<_, ()>(nonce) })
                .await;
            assert_eq!(nonce, Ok(expected));
        }
        assert_eq!(node.nonce_manager.current().await, 3);
    }

    #[tokio::test]
    async fn test_failed_send_rereads_pending_count() {
        let node = MockNode::start(MockChain::default()).await;

        // Nothing was broadcast: the nonce is free again
        let result = node
            .nonce_manager
            .send(|nonce| async move { Err::<(), _>(nonce) })
            .await;
        assert_eq!(result, Err(0));
        assert_eq!(node.nonce_manager.current().await, 0);

        // Transactions of the same account are in flight: their nonces are not
        node.chain().pending_count = 4;
        let result = node
            .nonce_manager
            .send(|nonce| async move { Err::<(), _>(nonce) })
            .await;
        assert_eq!(result, Err(0));
        assert_eq!(node.nonce_manager.current().await, 4);
    }

    #[tokio::test]
    async fn test_nonce_is_kept_when_the_chain_cannot_be_read() {
        let node = MockNode::start(MockChain {
            pending_count: 5,
            ..Default::default()
        })
        .await;
        node.chain().fail_transaction_count = true;

        let result = node
            .nonce_manager
            .send(|nonce| async move { Err::<(), _>(nonce) })
            .await;
        assert_eq!(result, Err(5));
        assert_eq!(node.nonce_manager.current().await, 5);

        node.nonce_manager.sync_from_chain().await;
        assert_eq!(node.nonce_manager.current().await, 5);
    }

    #[tokio::test]
    async fn test_sync_from_chain_frees_nonces_of_dropped_transactions() {
        let node = MockNode::start(MockChain::default()).await;
        for _ in 0..2 {
            node.nonce_manager
                .send(|_| async { Ok::<_, ()>(()) })
                .await
                .unwrap();
        }

        // The chain knows only one of the two
        node.chain().pending_count = 1;
        node.nonce_manager.sync_from_chain().await;
        assert_eq!(node.nonce_manager.current().await, 1);
    }

    /// Sends do not overlap: each one is finished before the next one gets
    /// its nonce.
    #[tokio::test]
    async fn test_concurrent_sends_take_turns() {
        let node = MockNode::start(MockChain::default()).await;
        let events = std::sync::Mutex::new(Vec::new());

        let sends = (0..10).map(|_| {
            node.nonce_manager.send(|nonce| {
                let events = &events;
                async move {
                    events.lock().unwrap().push(("start", nonce));
                    // Let the other sends run
                    tokio::task::yield_now().await;
                    events.lock().unwrap().push(("end", nonce));
                    Ok::<_, ()>(())
                }
            })
        });
        futures::future::join_all(sends).await;

        let expected: Vec<_> = (0..10)
            .flat_map(|nonce| [("start", nonce), ("end", nonce)])
            .collect();
        assert_eq!(*events.lock().unwrap(), expected);
    }
}
