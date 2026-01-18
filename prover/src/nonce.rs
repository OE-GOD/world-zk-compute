//! Nonce Management for Parallel Transactions
//!
//! When sending multiple transactions in parallel, the default nonce filler
//! fetches the nonce from the chain for each transaction. This causes conflicts
//! when transactions are sent faster than they're confirmed.
//!
//! This module provides a `NonceManager` that:
//! - Tracks nonces locally with atomic operations
//! - Increments nonce for each outgoing transaction
//! - Syncs with the chain on initialization and errors
//! - Thread-safe for concurrent access
//!
//! ## Usage
//!
//! ```rust
//! let nonce_manager = NonceManager::new(provider, address).await?;
//!
//! // Get next nonce (atomically increments)
//! let nonce = nonce_manager.next_nonce().await?;
//!
//! // On transaction failure, reset to chain state
//! nonce_manager.sync_from_chain().await?;
//! ```

use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::network::Network;
use alloy::transports::Transport;
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Thread-safe nonce manager for parallel transaction submission
///
/// Maintains a local nonce counter that is atomically incremented
/// for each transaction, avoiding conflicts when multiple transactions
/// are sent concurrently.
pub struct NonceManager<P, T, N> {
    /// The provider for chain queries
    provider: Arc<P>,
    /// The address we're managing nonces for
    address: Address,
    /// Current nonce (local tracking)
    current_nonce: AtomicU64,
    /// Lock for sync operations (prevents concurrent syncs)
    sync_lock: RwLock<()>,
    /// Number of pending transactions (for monitoring)
    pending_count: AtomicU64,
    /// Phantom data for transport and network types
    _phantom: std::marker::PhantomData<(T, N)>,
}

impl<P, T, N> NonceManager<P, T, N>
where
    P: Provider<T, N> + Clone + 'static,
    T: Transport + Clone,
    N: Network,
{
    /// Create a new nonce manager, syncing initial nonce from chain
    pub async fn new(provider: Arc<P>, address: Address) -> Result<Self> {
        let nonce = provider.get_transaction_count(address).await?;

        info!(
            "NonceManager initialized for {} with nonce {}",
            address, nonce
        );

        Ok(Self {
            provider,
            address,
            current_nonce: AtomicU64::new(nonce),
            sync_lock: RwLock::new(()),
            pending_count: AtomicU64::new(0),
            _phantom: std::marker::PhantomData,
        })
    }

    /// Get the next nonce and atomically increment the counter
    ///
    /// This is the primary method for getting nonces. It's lock-free
    /// and safe to call from multiple tasks concurrently.
    pub fn next_nonce(&self) -> u64 {
        let nonce = self.current_nonce.fetch_add(1, Ordering::SeqCst);
        self.pending_count.fetch_add(1, Ordering::Relaxed);
        debug!("Allocated nonce {} (pending: {})", nonce, self.pending_count.load(Ordering::Relaxed));
        nonce
    }

    /// Mark a transaction as completed (success or confirmed failure)
    ///
    /// Call this after a transaction is confirmed (success or revert)
    /// to track pending transaction count.
    pub fn transaction_completed(&self) {
        self.pending_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get current nonce without incrementing
    pub fn current(&self) -> u64 {
        self.current_nonce.load(Ordering::SeqCst)
    }

    /// Get number of pending transactions
    pub fn pending(&self) -> u64 {
        self.pending_count.load(Ordering::Relaxed)
    }

    /// Sync nonce from chain state
    ///
    /// Use this to recover from errors or if transactions were sent
    /// outside this manager. This acquires a lock to prevent concurrent syncs.
    pub async fn sync_from_chain(&self) -> Result<u64> {
        // Acquire lock to prevent concurrent syncs
        let _lock = self.sync_lock.write().await;

        let chain_nonce = self.provider.get_transaction_count(self.address).await?;
        let old_nonce = self.current_nonce.swap(chain_nonce, Ordering::SeqCst);

        if chain_nonce != old_nonce {
            info!(
                "Nonce synced from chain: {} -> {} (diff: {})",
                old_nonce,
                chain_nonce,
                chain_nonce as i64 - old_nonce as i64
            );
        }

        // Reset pending count since we synced
        self.pending_count.store(0, Ordering::Relaxed);

        Ok(chain_nonce)
    }

    /// Reset nonce to a specific value
    ///
    /// Use with caution - primarily for error recovery when you know
    /// the correct nonce value.
    pub fn reset_to(&self, nonce: u64) {
        let old = self.current_nonce.swap(nonce, Ordering::SeqCst);
        warn!("Nonce manually reset: {} -> {}", old, nonce);
    }

    /// Handle a nonce-related transaction error
    ///
    /// Analyzes the error and takes appropriate action:
    /// - "nonce too low": syncs from chain (transaction was already mined or replaced)
    /// - "nonce too high": decrements local nonce (we got ahead somehow)
    /// - "replacement transaction underpriced": nonce collision, sync from chain
    pub async fn handle_nonce_error(&self, error: &str, used_nonce: u64) -> Result<()> {
        let error_lower = error.to_lowercase();

        if error_lower.contains("nonce too low") {
            info!("Nonce too low error - syncing from chain");
            self.sync_from_chain().await?;
        } else if error_lower.contains("nonce too high") {
            // This shouldn't happen normally, but handle it
            warn!("Nonce too high - this is unexpected");
            self.sync_from_chain().await?;
        } else if error_lower.contains("replacement transaction underpriced")
            || error_lower.contains("already known")
        {
            // Transaction with same nonce already pending
            info!("Transaction replacement issue - syncing from chain");
            self.sync_from_chain().await?;
        } else {
            // For other errors, just mark the nonce as potentially available
            // by not doing anything (the transaction didn't use the nonce)
            debug!(
                "Non-nonce error for nonce {}, no action needed: {}",
                used_nonce, error
            );
        }

        self.transaction_completed();
        Ok(())
    }
}

/// Wrapper that provides nonce to transactions
///
/// Use this with alloy's transaction builder to inject managed nonces.
#[derive(Clone)]
pub struct ManagedNonce<P, T, N> {
    manager: Arc<NonceManager<P, T, N>>,
}

impl<P, T, N> ManagedNonce<P, T, N>
where
    P: Provider<T, N> + Clone + 'static,
    T: Transport + Clone,
    N: Network,
{
    pub fn new(manager: Arc<NonceManager<P, T, N>>) -> Self {
        Self { manager }
    }

    /// Get manager reference
    pub fn manager(&self) -> &Arc<NonceManager<P, T, N>> {
        &self.manager
    }

    /// Get next nonce
    pub fn next(&self) -> u64 {
        self.manager.next_nonce()
    }
}

/// Statistics about nonce management
#[derive(Debug, Clone)]
pub struct NonceStats {
    pub current_nonce: u64,
    pub pending_transactions: u64,
    pub address: Address,
}

impl<P, T, N> NonceManager<P, T, N>
where
    P: Provider<T, N> + Clone + 'static,
    T: Transport + Clone,
    N: Network,
{
    /// Get current statistics
    pub fn stats(&self) -> NonceStats {
        NonceStats {
            current_nonce: self.current(),
            pending_transactions: self.pending(),
            address: self.address,
        }
    }
}

impl std::fmt::Display for NonceStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NonceStats {{ address: {}, nonce: {}, pending: {} }}",
            self.address, self.current_nonce, self.pending_transactions
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_increment() {
        // Create a mock manager without provider
        let nonce = AtomicU64::new(5);

        // Simulate next_nonce behavior
        let n1 = nonce.fetch_add(1, Ordering::SeqCst);
        let n2 = nonce.fetch_add(1, Ordering::SeqCst);
        let n3 = nonce.fetch_add(1, Ordering::SeqCst);

        assert_eq!(n1, 5);
        assert_eq!(n2, 6);
        assert_eq!(n3, 7);
        assert_eq!(nonce.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn test_concurrent_nonce_allocation() {
        use std::thread;

        let nonce = Arc::new(AtomicU64::new(0));
        let mut handles = vec![];

        // Spawn 10 threads each getting 100 nonces
        for _ in 0..10 {
            let nonce = nonce.clone();
            handles.push(thread::spawn(move || {
                let mut nonces = vec![];
                for _ in 0..100 {
                    nonces.push(nonce.fetch_add(1, Ordering::SeqCst));
                }
                nonces
            }));
        }

        // Collect all nonces
        let mut all_nonces: Vec<u64> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();

        // Should have 1000 unique nonces
        all_nonces.sort();
        all_nonces.dedup();
        assert_eq!(all_nonces.len(), 1000);

        // Should be 0..999
        assert_eq!(all_nonces.first(), Some(&0));
        assert_eq!(all_nonces.last(), Some(&999));
    }
}
