//! Gas estimation utilities for DAG batch verification.
//!
//! This module provides two categories of gas estimation:
//!
//! 1. **Pure-computation helpers** ([`estimate_batch_session`], [`estimate_start_gas`],
//!    [`estimate_continue_gas`], [`estimate_finalize_gas`], etc.) that use empirical
//!    constants from the 88-layer XGBoost reference circuit. No RPC calls required.
//!
//! 2. **RPC-based estimators** ([`RpcGasEstimator`]) that call `eth_estimateGas`
//!    against a live provider for accurate per-transaction gas estimates. Requires
//!    an RPC connection and a deployed contract with the circuit registered.
//!
//! # On-chain constants
//!
//! | Constant | Value | Source |
//! |---|---|---|
//! | `LAYERS_PER_BATCH` | 8 | `DAGBatchVerifier.sol` |
//! | `GROUPS_PER_FINALIZE_BATCH` | 16 | `DAGBatchVerifier.sol` |
//!
//! # Gas profile (88-layer XGBoost circuit)
//!
//! | Phase | Gas range | Notes |
//! |---|---|---|
//! | Start | ~14-17.5M | transcript setup + proof decode + storage writes, NO compute |
//! | Continue (x11) | ~13-28M | proof decode + 8 layers + storage read/write |
//! | Finalize (x3) | ~9-28M | proof decode + eval groups + bindings reconstruction |
//! | Cleanup | ~0.1-0.5M | storage deletion |
//!
//! # Example
//!
//! ```rust
//! use world_zk_sdk::gas_estimation::{
//!     estimate_batch_session, estimate_total_cost_eth, BatchGasEstimate,
//! };
//!
//! // 88 compute layers, 34 eval groups (XGBoost reference)
//! let est = estimate_batch_session(88, 34);
//! assert_eq!(est.num_continue_txs, 11);
//! assert_eq!(est.num_finalize_txs, 3);
//! assert_eq!(est.verification_txs(), 15); // 1 start + 11 continue + 3 finalize
//! assert_eq!(est.total_txs(), 16); // ... + 1 cleanup
//!
//! // Cost at 30 gwei gas price
//! let cost_eth = estimate_total_cost_eth(est.total_gas_upper, 30.0);
//! println!("Upper bound cost: {cost_eth:.6} ETH");
//! ```

// ---------------------------------------------------------------------------
// On-chain constants (mirrored from DAGBatchVerifier.sol)
// ---------------------------------------------------------------------------

/// Number of compute layers processed per `continueDAGBatchVerify` transaction.
pub const LAYERS_PER_BATCH: u64 = 8;

/// Number of committed input eval groups processed per `finalizeDAGBatchVerify`
/// transaction.
pub const GROUPS_PER_FINALIZE_BATCH: u64 = 16;

// ---------------------------------------------------------------------------
// Empirical gas constants (from 88-layer XGBoost circuit benchmarks)
// ---------------------------------------------------------------------------

/// Gas used by `startDAGBatchVerify` -- lower bound (observed ~14M).
pub const START_GAS_LOWER: u64 = 14_000_000;
/// Gas used by `startDAGBatchVerify` -- upper bound (observed ~17.5M).
pub const START_GAS_UPPER: u64 = 17_500_000;
/// Typical (average) gas for the start transaction.
pub const START_GAS_TYPICAL: u64 = 15_500_000;

/// Gas per `continueDAGBatchVerify` call -- lower bound (observed ~13M).
pub const CONTINUE_GAS_LOWER: u64 = 13_000_000;
/// Gas per `continueDAGBatchVerify` call -- upper bound (observed ~28M).
pub const CONTINUE_GAS_UPPER: u64 = 28_000_000;
/// Typical (average) gas for a continue transaction.
pub const CONTINUE_GAS_TYPICAL: u64 = 20_000_000;

/// Gas per `finalizeDAGBatchVerify` call -- lower bound (observed ~9M).
pub const FINALIZE_GAS_LOWER: u64 = 9_000_000;
/// Gas per `finalizeDAGBatchVerify` call -- upper bound (observed ~28M).
pub const FINALIZE_GAS_UPPER: u64 = 28_000_000;
/// Typical (average) gas for a finalize transaction.
pub const FINALIZE_GAS_TYPICAL: u64 = 22_000_000;

/// Gas for `cleanupDAGBatchSession` (storage deletion refunds make this cheap).
pub const CLEANUP_GAS: u64 = 500_000;

/// Ethereum mainnet block gas limit (used as a sanity reference).
pub const BLOCK_GAS_LIMIT: u64 = 30_000_000;

// ---------------------------------------------------------------------------
// Estimate types
// ---------------------------------------------------------------------------

/// Detailed gas estimate for a complete DAG batch verification session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchGasEstimate {
    /// Number of compute layers in the circuit.
    pub num_compute_layers: u64,
    /// Number of committed input eval groups.
    pub num_eval_groups: u64,

    /// Number of `continueDAGBatchVerify` transactions required.
    pub num_continue_txs: u64,
    /// Number of `finalizeDAGBatchVerify` transactions required.
    pub num_finalize_txs: u64,

    /// Lower-bound total gas (optimistic).
    pub total_gas_lower: u64,
    /// Upper-bound total gas (pessimistic).
    pub total_gas_upper: u64,
    /// Typical total gas (average-case).
    pub total_gas_typical: u64,

    /// Per-phase breakdown (lower, upper, typical) for start.
    pub start_gas: GasRange,
    /// Per-phase breakdown (lower, upper, typical) for all continue txs combined.
    pub continue_gas: GasRange,
    /// Per-phase breakdown (lower, upper, typical) for all finalize txs combined.
    pub finalize_gas: GasRange,
    /// Gas for the cleanup transaction.
    pub cleanup_gas: u64,
}

/// A (lower, upper, typical) gas range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GasRange {
    pub lower: u64,
    pub upper: u64,
    pub typical: u64,
}

impl BatchGasEstimate {
    /// Total number of transactions in the session (start + continue + finalize + cleanup).
    pub fn total_txs(&self) -> u64 {
        1 + self.num_continue_txs + self.num_finalize_txs + 1
    }

    /// Total number of transactions excluding cleanup (start + continue + finalize).
    pub fn verification_txs(&self) -> u64 {
        1 + self.num_continue_txs + self.num_finalize_txs
    }

    /// Returns `true` if every individual transaction fits within the Ethereum
    /// block gas limit (30M).
    pub fn fits_in_blocks(&self) -> bool {
        START_GAS_UPPER < BLOCK_GAS_LIMIT
            && CONTINUE_GAS_UPPER < BLOCK_GAS_LIMIT
            && FINALIZE_GAS_UPPER < BLOCK_GAS_LIMIT
    }
}

// ---------------------------------------------------------------------------
// Core estimation functions
// ---------------------------------------------------------------------------

/// Compute the number of `continueDAGBatchVerify` transactions needed for a
/// circuit with `num_compute_layers` compute layers.
///
/// This mirrors the on-chain calculation:
/// `ceil(num_compute_layers / LAYERS_PER_BATCH)`.
pub fn compute_num_continue_batches(num_compute_layers: u64) -> u64 {
    if num_compute_layers == 0 {
        return 0;
    }
    num_compute_layers.div_ceil(LAYERS_PER_BATCH)
}

/// Compute the number of `finalizeDAGBatchVerify` transactions needed for a
/// circuit with `num_eval_groups` committed input eval groups.
///
/// This mirrors the on-chain calculation:
/// `ceil(num_eval_groups / GROUPS_PER_FINALIZE_BATCH)`.
/// A minimum of 1 finalize tx is always needed (even with 0 groups) because
/// finalize also handles public-input verification.
pub fn compute_num_finalize_batches(num_eval_groups: u64) -> u64 {
    if num_eval_groups == 0 {
        return 1;
    }
    num_eval_groups.div_ceil(GROUPS_PER_FINALIZE_BATCH)
}

/// Estimate gas for a complete DAG batch verification session.
///
/// # Arguments
///
/// * `num_compute_layers` -- Number of compute layers in the DAG circuit
///   (88 for the reference XGBoost model).
/// * `num_eval_groups` -- Number of committed input eval groups
///   (34 for the reference XGBoost model).
///
/// # Returns
///
/// A [`BatchGasEstimate`] with lower/upper/typical bounds and per-phase
/// breakdowns.
pub fn estimate_batch_session(num_compute_layers: u64, num_eval_groups: u64) -> BatchGasEstimate {
    let num_continue = compute_num_continue_batches(num_compute_layers);
    let num_finalize = compute_num_finalize_batches(num_eval_groups);

    let start = GasRange {
        lower: START_GAS_LOWER,
        upper: START_GAS_UPPER,
        typical: START_GAS_TYPICAL,
    };

    let continue_total = GasRange {
        lower: num_continue.saturating_mul(CONTINUE_GAS_LOWER),
        upper: num_continue.saturating_mul(CONTINUE_GAS_UPPER),
        typical: num_continue.saturating_mul(CONTINUE_GAS_TYPICAL),
    };

    let finalize_total = GasRange {
        lower: num_finalize.saturating_mul(FINALIZE_GAS_LOWER),
        upper: num_finalize.saturating_mul(FINALIZE_GAS_UPPER),
        typical: num_finalize.saturating_mul(FINALIZE_GAS_TYPICAL),
    };

    let total_lower = start
        .lower
        .saturating_add(continue_total.lower)
        .saturating_add(finalize_total.lower)
        .saturating_add(CLEANUP_GAS);

    let total_upper = start
        .upper
        .saturating_add(continue_total.upper)
        .saturating_add(finalize_total.upper)
        .saturating_add(CLEANUP_GAS);

    let total_typical = start
        .typical
        .saturating_add(continue_total.typical)
        .saturating_add(finalize_total.typical)
        .saturating_add(CLEANUP_GAS);

    BatchGasEstimate {
        num_compute_layers,
        num_eval_groups,
        num_continue_txs: num_continue,
        num_finalize_txs: num_finalize,
        total_gas_lower: total_lower,
        total_gas_upper: total_upper,
        total_gas_typical: total_typical,
        start_gas: start,
        continue_gas: continue_total,
        finalize_gas: finalize_total,
        cleanup_gas: CLEANUP_GAS,
    }
}

/// Estimate the cost in ETH for a given amount of gas at a given gas price.
///
/// # Arguments
///
/// * `gas` -- Total gas units.
/// * `gas_price_gwei` -- Gas price in Gwei (e.g. 30.0 for 30 Gwei).
///
/// # Returns
///
/// Cost in ETH as `f64`.
///
/// # Example
///
/// ```rust
/// use world_zk_sdk::gas_estimation::estimate_total_cost_eth;
///
/// let cost = estimate_total_cost_eth(250_000_000, 30.0);
/// assert!((cost - 7.5).abs() < 0.0001);
/// ```
pub fn estimate_total_cost_eth(gas: u64, gas_price_gwei: f64) -> f64 {
    let gas_price_eth = gas_price_gwei * 1e-9;
    gas as f64 * gas_price_eth
}

/// Estimate cost in USD given gas, gas price, and ETH/USD price.
///
/// # Arguments
///
/// * `gas` -- Total gas units.
/// * `gas_price_gwei` -- Gas price in Gwei.
/// * `eth_usd_price` -- Current ETH price in USD.
pub fn estimate_total_cost_usd(gas: u64, gas_price_gwei: f64, eth_usd_price: f64) -> f64 {
    estimate_total_cost_eth(gas, gas_price_gwei) * eth_usd_price
}

/// Pre-built estimate for the reference 88-layer XGBoost circuit (34 eval groups).
///
/// This is a convenience function that returns the estimate for the most common
/// circuit configuration used in this project.
pub fn xgboost_reference_estimate() -> BatchGasEstimate {
    estimate_batch_session(88, 34)
}

// ---------------------------------------------------------------------------
// Per-step gas estimation helpers (T446)
// ---------------------------------------------------------------------------

/// Estimate gas for `startDAGBatchVerify`.
///
/// The start phase decodes the proof transcript and stores initial session state.
/// Returns the upper-bound gas estimate (pessimistic), which is the safe choice
/// for transaction gas limits.
///
/// # Arguments
///
/// * `num_layers` - Total number of compute layers in the DAG circuit.
///
/// # Example
///
/// ```rust
/// use world_zk_sdk::gas_estimation::estimate_start_gas;
/// let gas = estimate_start_gas(88);
/// assert!(gas >= 17_500_000);
/// assert!(gas < 30_000_000);
/// ```
pub fn estimate_start_gas(num_layers: u32) -> u64 {
    // Start gas is relatively constant but we add a mild per-layer overhead
    // for transcript parsing (each layer adds sumcheck data to decode).
    let per_layer_overhead: u64 = 30_000;
    START_GAS_UPPER + (num_layers as u64) * per_layer_overhead
}

/// Estimate gas for a single `continueDAGBatchVerify` call.
///
/// Each continue call processes up to `LAYERS_PER_BATCH` compute layers.
/// Gas varies depending on layer complexity (oracle expressions, num_vars).
///
/// # Arguments
///
/// * `batch_idx` - Zero-based index of the continue batch.
/// * `layers_per_batch` - Number of layers in this batch (typically
///   [`LAYERS_PER_BATCH`], may be fewer for the last batch).
///
/// # Returns
///
/// Upper-bound gas estimate for this specific batch.
///
/// # Example
///
/// ```rust
/// use world_zk_sdk::gas_estimation::estimate_continue_gas;
/// let gas = estimate_continue_gas(0, 8);
/// assert!(gas >= 13_000_000);
/// ```
pub fn estimate_continue_gas(batch_idx: u32, layers_per_batch: u32) -> u64 {
    // Scale linearly between lower and upper based on how many layers are in
    // this batch. Full batch = upper bound, partial batch = proportionally less.
    let full_batch = LAYERS_PER_BATCH as u32;
    if full_batch == 0 || layers_per_batch == 0 {
        return CONTINUE_GAS_LOWER;
    }

    // Later batches tend to verify layers with more complex oracle expressions.
    // Apply a mild complexity factor based on batch index.
    let complexity_bump = (batch_idx as u64).min(10) * 500_000;

    let per_layer = (CONTINUE_GAS_UPPER - CONTINUE_GAS_LOWER) / (full_batch as u64);
    let base = CONTINUE_GAS_LOWER + (layers_per_batch as u64) * per_layer + complexity_bump;

    // Clamp to observed upper bound
    base.min(CONTINUE_GAS_UPPER)
}

/// Estimate gas for a single `finalizeDAGBatchVerify` call.
///
/// Each finalize call processes up to `GROUPS_PER_FINALIZE_BATCH` input groups,
/// performing tensor products, RLC, and PODP verification.
///
/// # Arguments
///
/// * `groups_per_batch` - Number of groups to process in this finalize step
///   (typically [`GROUPS_PER_FINALIZE_BATCH`], may be fewer for the last batch).
///
/// # Returns
///
/// Upper-bound gas estimate for this finalize step.
///
/// # Example
///
/// ```rust
/// use world_zk_sdk::gas_estimation::estimate_finalize_gas;
/// let gas = estimate_finalize_gas(16);
/// assert!(gas >= 9_000_000);
/// assert!(gas < 30_000_000);
/// ```
pub fn estimate_finalize_gas(groups_per_batch: u32) -> u64 {
    if groups_per_batch == 0 {
        return FINALIZE_GAS_LOWER;
    }
    let full_batch = GROUPS_PER_FINALIZE_BATCH as u32;
    let per_group = (FINALIZE_GAS_UPPER - FINALIZE_GAS_LOWER) / (full_batch as u64);
    let base = FINALIZE_GAS_LOWER + (groups_per_batch as u64) * per_group;
    base.min(FINALIZE_GAS_UPPER)
}

/// Calculate the number of continue and finalize batches needed.
///
/// # Arguments
///
/// * `num_layers` - Total number of compute layers in the DAG circuit.
/// * `layers_per_batch` - Number of layers per continue batch (typically 8).
///
/// # Returns
///
/// A tuple `(num_continue, num_finalize)`:
/// - `num_continue`: Number of `continueDAGBatchVerify` transactions needed.
/// - `num_finalize`: Number of `finalizeDAGBatchVerify` transactions needed
///   (estimated from layer count using an empirical heuristic).
///
/// For precise `num_finalize`, use [`compute_num_finalize_batches`] with the
/// actual number of eval groups from the on-chain circuit description.
///
/// # Example
///
/// ```rust
/// use world_zk_sdk::gas_estimation::estimate_total_batches;
/// let (cont, fin) = estimate_total_batches(88, 8);
/// assert_eq!(cont, 11);
/// assert!(fin >= 2);
/// ```
pub fn estimate_total_batches(num_layers: u32, layers_per_batch: u32) -> (u32, u32) {
    let num_continue = if layers_per_batch == 0 {
        0
    } else {
        num_layers.div_ceil(layers_per_batch)
    };

    // Estimate eval groups from layer count. The XGBoost 88-layer circuit has
    // 34 eval groups (~39% ratio). We use ceil(num_layers * 2 / 5).
    let estimated_groups = estimate_input_groups(num_layers);
    let num_finalize = compute_num_finalize_batches(estimated_groups as u64) as u32;

    (num_continue, num_finalize)
}

/// Estimate the number of input eval groups from the compute layer count.
///
/// Uses an empirical heuristic: 88 layers produce ~34 groups (ratio ~0.386).
/// Returns `ceil(num_layers * 2 / 5)`.
///
/// For precise values, query the on-chain circuit description instead.
pub fn estimate_input_groups(num_layers: u32) -> u32 {
    if num_layers == 0 {
        return 0;
    }
    (num_layers * 2).div_ceil(5)
}

// ---------------------------------------------------------------------------
// RPC-based gas estimation (eth_estimateGas)
// ---------------------------------------------------------------------------

use alloy::primitives::{Bytes, B256};
use alloy::providers::ProviderBuilder;

use crate::abi::RemainderVerifier;
use crate::client::Client;
use crate::fixture::ProofData;

/// Result of an RPC-based gas estimate for a full DAG batch verification session.
///
/// Unlike [`BatchGasEstimate`] (which uses empirical constants), this struct
/// contains actual gas values returned by `eth_estimateGas` calls against a
/// live node (Anvil, testnet, or mainnet fork).
///
/// # Gas profile reference (88-layer XGBoost circuit)
///
/// | Phase | Gas range |
/// |---|---|
/// | Start | ~14-17.5M |
/// | Continue (per step) | ~13-28M |
/// | Finalize (per step) | ~9-22M |
/// | Cleanup | ~0.1-0.5M |
#[derive(Debug, Clone)]
pub struct RpcBatchGasEstimate {
    /// Gas estimate for `startDAGBatchVerify`.
    pub start_gas: u64,

    /// Gas estimate for each `continueDAGBatchVerify` call.
    /// Length equals the number of continue batches.
    pub continue_gas_per_step: Vec<u64>,

    /// Gas estimate for each `finalizeDAGBatchVerify` call.
    /// Length equals the number of finalize batches.
    pub finalize_gas_per_step: Vec<u64>,

    /// Gas estimate for `cleanupDAGBatchSession`.
    pub cleanup_gas: u64,

    /// Sum of start + all continue + all finalize + cleanup gas.
    pub total_gas: u64,

    /// Estimated cost in ETH at the given gas price (Gwei).
    /// `None` if no gas price was provided.
    pub estimated_eth_cost: Option<f64>,
}

impl RpcBatchGasEstimate {
    /// Total number of transactions: 1 start + N continue + M finalize + 1 cleanup.
    pub fn total_txs(&self) -> usize {
        1 + self.continue_gas_per_step.len() + self.finalize_gas_per_step.len() + 1
    }

    /// Returns the average gas per continue step, or 0 if there are none.
    pub fn avg_continue_gas(&self) -> u64 {
        if self.continue_gas_per_step.is_empty() {
            return 0;
        }
        let sum: u64 = self.continue_gas_per_step.iter().sum();
        sum / self.continue_gas_per_step.len() as u64
    }

    /// Returns the average gas per finalize step, or 0 if there are none.
    pub fn avg_finalize_gas(&self) -> u64 {
        if self.finalize_gas_per_step.is_empty() {
            return 0;
        }
        let sum: u64 = self.finalize_gas_per_step.iter().sum();
        sum / self.finalize_gas_per_step.len() as u64
    }

    /// Returns the maximum gas used by any single transaction in the session.
    pub fn max_single_tx_gas(&self) -> u64 {
        let mut max = self.start_gas;
        for &g in &self.continue_gas_per_step {
            if g > max {
                max = g;
            }
        }
        for &g in &self.finalize_gas_per_step {
            if g > max {
                max = g;
            }
        }
        // cleanup is typically small, but include for completeness
        if self.cleanup_gas > max {
            max = self.cleanup_gas;
        }
        max
    }

    /// Returns `true` if every individual transaction fits within the Ethereum
    /// block gas limit (30M).
    pub fn fits_in_blocks(&self) -> bool {
        self.max_single_tx_gas() < BLOCK_GAS_LIMIT
    }
}

/// RPC-based gas estimator for DAG batch verification.
///
/// Uses `eth_estimateGas` against a live provider to obtain accurate gas
/// estimates for each phase of the batch verification protocol. This is
/// more accurate than the pure-computation helpers (which use empirical
/// constants) but requires an RPC connection and a deployed contract with
/// the circuit already registered.
///
/// # Gas profile reference (88-layer XGBoost circuit)
///
/// | Phase | Observed gas |
/// |---|---|
/// | `startDAGBatchVerify` | ~14-17.5M |
/// | `continueDAGBatchVerify` (x11) | ~13-28M each |
/// | `finalizeDAGBatchVerify` (x3) | ~9-22M each |
/// | `cleanupDAGBatchSession` | ~0.1-0.5M |
///
/// # Example
///
/// ```rust,no_run
/// use world_zk_sdk::{Client, DAGFixture};
/// use world_zk_sdk::gas_estimation::RpcGasEstimator;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let client = Client::new(
///         "http://localhost:8545",
///         "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
///         "0x5FbDB2315678afecb367f032d93F642f64180aa3",
///     )?;
///     let fixture = DAGFixture::load("path/to/fixture.json")?;
///     let proof = fixture.to_proof_data()?;
///
///     let estimator = RpcGasEstimator::new(&client);
///
///     // Estimate gas for the start transaction
///     let start_gas = estimator.estimate_start_gas(&proof).await?;
///     println!("Start gas: {start_gas}");
///
///     Ok(())
/// }
/// ```
pub struct RpcGasEstimator<'a> {
    client: &'a Client,
}

impl<'a> RpcGasEstimator<'a> {
    /// Create a new RPC gas estimator wrapping an SDK [`Client`].
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Estimate gas for `startDAGBatchVerify`.
    ///
    /// Calls `eth_estimateGas` against the provider to simulate the start
    /// transaction. The circuit must already be registered on-chain for this
    /// to succeed.
    ///
    /// # Gas profile reference
    ///
    /// Start typically uses ~14-17.5M gas for the 88-layer XGBoost circuit.
    /// The start phase decodes the proof transcript and stores initial session
    /// state but does NOT process any compute layers.
    ///
    /// # Errors
    ///
    /// Returns an error if the RPC call fails or the contract reverts (e.g.,
    /// circuit not registered, invalid proof format).
    pub async fn estimate_start_gas(&self, proof: &ProofData) -> anyhow::Result<u64> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), &provider);

        let gas = contract
            .startDAGBatchVerify(
                Bytes::copy_from_slice(&proof.proof_bytes),
                proof.circuit_hash,
                Bytes::copy_from_slice(&proof.public_inputs),
                Bytes::copy_from_slice(&proof.gens_data),
            )
            .estimate_gas()
            .await?;

        Ok(gas)
    }

    /// Estimate gas for `continueDAGBatchVerify`.
    ///
    /// Calls `eth_estimateGas` to simulate a continue transaction for the
    /// given batch session. The session must already exist on-chain (i.e.,
    /// `startDAGBatchVerify` must have been called and mined).
    ///
    /// # Gas profile reference
    ///
    /// Each continue step processes up to `LAYERS_PER_BATCH` (8) compute
    /// layers. Observed gas ranges from ~13M (simpler layers) to ~28M
    /// (complex oracle expressions).
    ///
    /// # Arguments
    ///
    /// * `session_id` -- The batch session ID returned by `startDAGBatchVerify`.
    /// * `proof` -- The same proof data used for the start call.
    ///
    /// # Errors
    ///
    /// Returns an error if the session does not exist, all batches are already
    /// processed, or the RPC call fails.
    pub async fn estimate_continue_gas(
        &self,
        session_id: B256,
        proof: &ProofData,
    ) -> anyhow::Result<u64> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), &provider);

        let gas = contract
            .continueDAGBatchVerify(
                session_id,
                Bytes::copy_from_slice(&proof.proof_bytes),
                Bytes::copy_from_slice(&proof.public_inputs),
                Bytes::copy_from_slice(&proof.gens_data),
            )
            .estimate_gas()
            .await?;

        Ok(gas)
    }

    /// Estimate gas for `finalizeDAGBatchVerify`.
    ///
    /// Calls `eth_estimateGas` to simulate a finalize transaction. The session
    /// must have all compute batches processed (all continue calls done).
    ///
    /// # Gas profile reference
    ///
    /// Each finalize step processes up to `GROUPS_PER_FINALIZE_BATCH` (16)
    /// input eval groups. Observed gas ranges from ~9M to ~22M depending
    /// on group complexity.
    ///
    /// # Arguments
    ///
    /// * `session_id` -- The batch session ID.
    /// * `proof` -- The same proof data used for the start call.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not ready for finalization,
    /// already finalized, or the RPC call fails.
    pub async fn estimate_finalize_gas(
        &self,
        session_id: B256,
        proof: &ProofData,
    ) -> anyhow::Result<u64> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), &provider);

        let gas = contract
            .finalizeDAGBatchVerify(
                session_id,
                Bytes::copy_from_slice(&proof.proof_bytes),
                Bytes::copy_from_slice(&proof.public_inputs),
                Bytes::copy_from_slice(&proof.gens_data),
            )
            .estimate_gas()
            .await?;

        Ok(gas)
    }

    /// Estimate gas for `cleanupDAGBatchSession`.
    ///
    /// Cleanup deletes session storage and is typically very cheap (~0.1-0.5M gas)
    /// due to storage deletion refunds.
    ///
    /// # Arguments
    ///
    /// * `session_id` -- The batch session ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the session does not exist or the RPC call fails.
    pub async fn estimate_cleanup_gas(&self, session_id: B256) -> anyhow::Result<u64> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), &provider);

        let gas = contract
            .cleanupDAGBatchSession(session_id)
            .estimate_gas()
            .await?;

        Ok(gas)
    }

    /// Estimate total gas cost for a complete DAG batch verification session.
    ///
    /// This is a **dry-run** estimator that simulates the full multi-transaction
    /// flow: start -> continue x N -> finalize x M -> cleanup. It executes
    /// each step on a forked/simulated state by actually sending transactions
    /// (not just estimating), so the session progresses through all phases.
    ///
    /// **Important**: This function sends real transactions to the provider.
    /// Use it against a local Anvil node or a forked environment, NOT against
    /// a live network where gas costs real ETH.
    ///
    /// # Gas profile reference (88-layer XGBoost circuit)
    ///
    /// | Phase | Typical gas | Count |
    /// |---|---|---|
    /// | Start | ~17.5M | 1 |
    /// | Continue | ~13-28M | 11 |
    /// | Finalize | ~9-22M | 3 |
    /// | Cleanup | ~0.5M | 1 |
    /// | **Total** | **~250M** | **16 txs** |
    ///
    /// # Arguments
    ///
    /// * `proof` -- Proof data for the circuit.
    /// * `gas_price_gwei` -- Optional gas price in Gwei for ETH cost calculation.
    ///   If `None`, `estimated_eth_cost` in the result will be `None`.
    ///
    /// # Returns
    ///
    /// An [`RpcBatchGasEstimate`] with per-step gas values, total gas, and
    /// optional ETH cost.
    ///
    /// # Errors
    ///
    /// Returns an error if any transaction fails (circuit not registered,
    /// invalid proof, RPC errors, etc.).
    pub async fn estimate_total_batch_cost(
        &self,
        proof: &ProofData,
        gas_price_gwei: Option<f64>,
    ) -> anyhow::Result<RpcBatchGasEstimate> {
        let provider = self.build_provider();
        let contract = RemainderVerifier::new(self.client.contract_address(), &provider);

        let proof_bytes = Bytes::copy_from_slice(&proof.proof_bytes);
        let pub_inputs = Bytes::copy_from_slice(&proof.public_inputs);
        let gens = Bytes::copy_from_slice(&proof.gens_data);

        // 1. Start — estimate then send to advance state
        let start_gas = contract
            .startDAGBatchVerify(
                proof_bytes.clone(),
                proof.circuit_hash,
                pub_inputs.clone(),
                gens.clone(),
            )
            .estimate_gas()
            .await?;

        let receipt = contract
            .startDAGBatchVerify(
                proof_bytes.clone(),
                proof.circuit_hash,
                pub_inputs.clone(),
                gens.clone(),
            )
            .send()
            .await?
            .get_receipt()
            .await?;

        // Extract session ID from event log
        let session_id = receipt
            .inner
            .logs()
            .iter()
            .find_map(|log| {
                log.log_decode::<RemainderVerifier::DAGBatchSessionStarted>()
                    .ok()
                    .map(|e| e.inner.data.sessionId)
            })
            .ok_or_else(|| anyhow::anyhow!("DAGBatchSessionStarted event not found"))?;

        // Query session for total batches
        let session_result = contract.getDAGBatchSession(session_id).call().await?;
        let total_batches = session_result.totalBatches.to::<u64>();

        // 2. Continue — estimate then send each batch
        let mut continue_gas_per_step = Vec::with_capacity(total_batches as usize);
        for _batch in 0..total_batches {
            let gas = contract
                .continueDAGBatchVerify(
                    session_id,
                    proof_bytes.clone(),
                    pub_inputs.clone(),
                    gens.clone(),
                )
                .estimate_gas()
                .await?;

            continue_gas_per_step.push(gas);

            // Send the actual transaction to advance session state
            contract
                .continueDAGBatchVerify(
                    session_id,
                    proof_bytes.clone(),
                    pub_inputs.clone(),
                    gens.clone(),
                )
                .send()
                .await?
                .get_receipt()
                .await?;
        }

        // 3. Finalize — estimate then send until fully finalized
        let mut finalize_gas_per_step = Vec::new();
        let mut finalize_count = 0u64;
        loop {
            let gas = contract
                .finalizeDAGBatchVerify(
                    session_id,
                    proof_bytes.clone(),
                    pub_inputs.clone(),
                    gens.clone(),
                )
                .estimate_gas()
                .await?;

            finalize_gas_per_step.push(gas);

            // Send the actual transaction
            contract
                .finalizeDAGBatchVerify(
                    session_id,
                    proof_bytes.clone(),
                    pub_inputs.clone(),
                    gens.clone(),
                )
                .send()
                .await?
                .get_receipt()
                .await?;

            finalize_count += 1;

            // Check if fully finalized
            let session_result = contract.getDAGBatchSession(session_id).call().await?;
            if session_result.finalized {
                break;
            }

            if finalize_count > 100 {
                anyhow::bail!("Finalization exceeded 100 steps during gas estimation");
            }
        }

        // 4. Cleanup — estimate then send
        let cleanup_gas = contract
            .cleanupDAGBatchSession(session_id)
            .estimate_gas()
            .await?;

        contract
            .cleanupDAGBatchSession(session_id)
            .send()
            .await?
            .get_receipt()
            .await?;

        // Sum totals
        let total_gas = start_gas
            + continue_gas_per_step.iter().sum::<u64>()
            + finalize_gas_per_step.iter().sum::<u64>()
            + cleanup_gas;

        let estimated_eth_cost =
            gas_price_gwei.map(|gwei| estimate_total_cost_eth(total_gas, gwei));

        Ok(RpcBatchGasEstimate {
            start_gas,
            continue_gas_per_step,
            finalize_gas_per_step,
            cleanup_gas,
            total_gas,
            estimated_eth_cost,
        })
    }

    /// Build an alloy HTTP provider with the client's wallet.
    fn build_provider(&self) -> impl alloy::providers::Provider + Clone {
        ProviderBuilder::new()
            .wallet(self.client.wallet.clone())
            .connect_http(self.client.rpc_url.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_continue_batches_88_layers() {
        // 88 / 8 = 11 exactly
        assert_eq!(compute_num_continue_batches(88), 11);
    }

    #[test]
    fn test_continue_batches_non_divisible() {
        // 90 / 8 = 11.25 -> ceil = 12
        assert_eq!(compute_num_continue_batches(90), 12);
    }

    #[test]
    fn test_continue_batches_exact_multiple() {
        assert_eq!(compute_num_continue_batches(16), 2);
        assert_eq!(compute_num_continue_batches(8), 1);
    }

    #[test]
    fn test_continue_batches_zero() {
        assert_eq!(compute_num_continue_batches(0), 0);
    }

    #[test]
    fn test_continue_batches_one() {
        assert_eq!(compute_num_continue_batches(1), 1);
    }

    #[test]
    fn test_finalize_batches_34_groups() {
        // 34 / 16 = 2.125 -> ceil = 3
        assert_eq!(compute_num_finalize_batches(34), 3);
    }

    #[test]
    fn test_finalize_batches_exact_multiple() {
        assert_eq!(compute_num_finalize_batches(16), 1);
        assert_eq!(compute_num_finalize_batches(32), 2);
    }

    #[test]
    fn test_finalize_batches_zero_gives_one() {
        // Minimum 1 finalize tx even with 0 groups
        assert_eq!(compute_num_finalize_batches(0), 1);
    }

    #[test]
    fn test_finalize_batches_one_group() {
        assert_eq!(compute_num_finalize_batches(1), 1);
    }

    #[test]
    fn test_xgboost_reference_estimate() {
        let est = xgboost_reference_estimate();
        assert_eq!(est.num_compute_layers, 88);
        assert_eq!(est.num_eval_groups, 34);
        assert_eq!(est.num_continue_txs, 11);
        assert_eq!(est.num_finalize_txs, 3);
        // 1 start + 11 continue + 3 finalize + 1 cleanup = 16
        assert_eq!(est.total_txs(), 16);
        // verification txs = 15 (no cleanup)
        assert_eq!(est.verification_txs(), 15);
    }

    #[test]
    fn test_estimate_batch_session_gas_bounds() {
        let est = estimate_batch_session(88, 34);

        // Lower < typical < upper
        assert!(est.total_gas_lower < est.total_gas_typical);
        assert!(est.total_gas_typical < est.total_gas_upper);

        // Verify start gas is the constant ranges
        assert_eq!(est.start_gas.lower, START_GAS_LOWER);
        assert_eq!(est.start_gas.upper, START_GAS_UPPER);

        // Verify continue gas = num_continue * per-batch gas
        assert_eq!(est.continue_gas.lower, 11 * CONTINUE_GAS_LOWER);
        assert_eq!(est.continue_gas.upper, 11 * CONTINUE_GAS_UPPER);

        // Verify finalize gas = num_finalize * per-finalize gas
        assert_eq!(est.finalize_gas.lower, 3 * FINALIZE_GAS_LOWER);
        assert_eq!(est.finalize_gas.upper, 3 * FINALIZE_GAS_UPPER);

        // Total = start + continue + finalize + cleanup
        assert_eq!(
            est.total_gas_lower,
            START_GAS_LOWER + 11 * CONTINUE_GAS_LOWER + 3 * FINALIZE_GAS_LOWER + CLEANUP_GAS
        );
        assert_eq!(
            est.total_gas_upper,
            START_GAS_UPPER + 11 * CONTINUE_GAS_UPPER + 3 * FINALIZE_GAS_UPPER + CLEANUP_GAS
        );
    }

    #[test]
    fn test_estimate_batch_session_small_circuit() {
        let est = estimate_batch_session(4, 2);
        assert_eq!(est.num_continue_txs, 1);
        assert_eq!(est.num_finalize_txs, 1);
        assert_eq!(est.total_txs(), 4); // start + 1 continue + 1 finalize + cleanup
    }

    #[test]
    fn test_estimate_batch_session_zero_layers() {
        let est = estimate_batch_session(0, 0);
        assert_eq!(est.num_continue_txs, 0);
        assert_eq!(est.num_finalize_txs, 1); // always at least 1 finalize
        assert_eq!(est.total_txs(), 3); // start + 0 continue + 1 finalize + cleanup
    }

    #[test]
    fn test_estimate_total_cost_eth_basic() {
        // 250M gas at 30 gwei = 250_000_000 * 30 * 1e-9 = 7.5 ETH
        let cost = estimate_total_cost_eth(250_000_000, 30.0);
        assert!((cost - 7.5).abs() < 1e-6);
    }

    #[test]
    fn test_estimate_total_cost_eth_zero_gas() {
        let cost = estimate_total_cost_eth(0, 30.0);
        assert!((cost - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_estimate_total_cost_eth_zero_price() {
        let cost = estimate_total_cost_eth(250_000_000, 0.0);
        assert!((cost - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_estimate_total_cost_eth_low_gas_price() {
        // 1M gas at 1 gwei = 0.001 ETH
        let cost = estimate_total_cost_eth(1_000_000, 1.0);
        assert!((cost - 0.001).abs() < 1e-9);
    }

    #[test]
    fn test_estimate_total_cost_usd() {
        // 250M gas at 30 gwei = 7.5 ETH, ETH at $2000 = $15,000
        let cost = estimate_total_cost_usd(250_000_000, 30.0, 2000.0);
        assert!((cost - 15_000.0).abs() < 0.01);
    }

    #[test]
    fn test_fits_in_blocks() {
        let est = estimate_batch_session(88, 34);
        // All individual tx upper bounds are < 30M
        assert!(est.fits_in_blocks());
    }

    #[test]
    fn test_gas_range_ordering_invariant() {
        let est = estimate_batch_session(88, 34);
        assert!(est.start_gas.lower <= est.start_gas.typical);
        assert!(est.start_gas.typical <= est.start_gas.upper);
        assert!(est.continue_gas.lower <= est.continue_gas.typical);
        assert!(est.continue_gas.typical <= est.continue_gas.upper);
        assert!(est.finalize_gas.lower <= est.finalize_gas.typical);
        assert!(est.finalize_gas.typical <= est.finalize_gas.upper);
    }

    #[test]
    fn test_constants_match_contract() {
        // These must stay in sync with DAGBatchVerifier.sol
        assert_eq!(LAYERS_PER_BATCH, 8);
        assert_eq!(GROUPS_PER_FINALIZE_BATCH, 16);
    }

    #[test]
    fn test_large_circuit_does_not_overflow() {
        // Stress test with a very large circuit
        let est = estimate_batch_session(10_000, 1_000);
        assert_eq!(est.num_continue_txs, 1250);
        assert_eq!(est.num_finalize_txs, 63); // ceil(1000/16)
        assert!(est.total_gas_upper > 0);
        // Should not overflow u64 for reasonable circuit sizes
        assert!(est.total_gas_upper < u64::MAX);
    }

    #[test]
    fn test_estimate_consistency_with_verifier_flow() {
        // The verifier.rs verify_batch flow does:
        //   1 start + total_batches continue + N finalize + 1 cleanup
        // Our estimate should produce matching tx counts.
        let est = estimate_batch_session(88, 34);
        // verify_batch loop: for batch in 0..total_batches
        assert_eq!(est.num_continue_txs, 11);
        // finalize loop: repeats until finalized
        assert_eq!(est.num_finalize_txs, 3);
    }

    #[test]
    fn test_xgboost_typical_gas_in_expected_range() {
        let est = xgboost_reference_estimate();
        // Total typical gas for 88-layer XGBoost should be roughly 250-300M
        // (based on observed ~252M for single-tx direct verification)
        assert!(est.total_gas_typical > 200_000_000);
        assert!(est.total_gas_typical < 500_000_000);
    }

    #[test]
    fn test_cleanup_gas_included() {
        let est = estimate_batch_session(8, 16);
        let expected_lower =
            START_GAS_LOWER + CONTINUE_GAS_LOWER + FINALIZE_GAS_LOWER + CLEANUP_GAS;
        assert_eq!(est.total_gas_lower, expected_lower);
    }

    #[test]
    fn test_per_batch_gas_estimates() {
        // Verify that per-continue-tx gas is within expected range
        assert!(CONTINUE_GAS_LOWER >= 10_000_000);
        assert!(CONTINUE_GAS_UPPER <= BLOCK_GAS_LIMIT);

        // Verify that per-finalize-tx gas is within expected range
        assert!(FINALIZE_GAS_LOWER >= 5_000_000);
        assert!(FINALIZE_GAS_UPPER <= BLOCK_GAS_LIMIT);

        // Start should fit in a block
        assert!(START_GAS_UPPER < BLOCK_GAS_LIMIT);
    }

    // -----------------------------------------------------------------------
    // Per-step helper tests (T446)
    // -----------------------------------------------------------------------

    #[test]
    fn test_estimate_start_gas_baseline() {
        let gas = estimate_start_gas(0);
        // Should be at least the upper baseline
        assert!(gas >= START_GAS_UPPER);
    }

    #[test]
    fn test_estimate_start_gas_scales_with_layers() {
        let gas_small = estimate_start_gas(10);
        let gas_large = estimate_start_gas(88);
        assert!(gas_large > gas_small, "More layers should cost more gas");
    }

    #[test]
    fn test_estimate_start_gas_under_block_limit() {
        // 88-layer circuit start should fit in a single block
        let gas = estimate_start_gas(88);
        assert!(
            gas < BLOCK_GAS_LIMIT,
            "Start gas {gas} should be under {BLOCK_GAS_LIMIT}"
        );
    }

    #[test]
    fn test_estimate_continue_gas_full_batch() {
        let gas = estimate_continue_gas(0, 8);
        assert!(gas >= CONTINUE_GAS_LOWER);
        assert!(gas <= CONTINUE_GAS_UPPER);
    }

    #[test]
    fn test_estimate_continue_gas_partial_batch() {
        let gas_full = estimate_continue_gas(0, 8);
        let gas_partial = estimate_continue_gas(0, 3);
        assert!(
            gas_partial < gas_full,
            "Partial batch ({gas_partial}) should cost less than full ({gas_full})"
        );
    }

    #[test]
    fn test_estimate_continue_gas_later_batches_higher() {
        let gas_early = estimate_continue_gas(0, 8);
        let gas_late = estimate_continue_gas(8, 8);
        assert!(
            gas_late >= gas_early,
            "Later batches should cost at least as much"
        );
    }

    #[test]
    fn test_estimate_continue_gas_zero_layers() {
        let gas = estimate_continue_gas(0, 0);
        assert_eq!(gas, CONTINUE_GAS_LOWER);
    }

    #[test]
    fn test_estimate_finalize_gas_full_batch() {
        let gas = estimate_finalize_gas(16);
        assert!(gas >= FINALIZE_GAS_LOWER);
        assert!(gas <= FINALIZE_GAS_UPPER);
    }

    #[test]
    fn test_estimate_finalize_gas_scales_with_groups() {
        let gas_few = estimate_finalize_gas(4);
        let gas_many = estimate_finalize_gas(16);
        assert!(
            gas_many > gas_few,
            "More groups ({gas_many}) should cost more than fewer ({gas_few})"
        );
    }

    #[test]
    fn test_estimate_finalize_gas_zero_groups() {
        let gas = estimate_finalize_gas(0);
        assert_eq!(gas, FINALIZE_GAS_LOWER);
    }

    #[test]
    fn test_estimate_finalize_gas_under_block_limit() {
        let gas = estimate_finalize_gas(16);
        assert!(
            gas < BLOCK_GAS_LIMIT,
            "Finalize gas {gas} should be under {BLOCK_GAS_LIMIT}"
        );
    }

    #[test]
    fn test_estimate_total_batches_88_layers_lpb8() {
        let (cont, fin) = estimate_total_batches(88, 8);
        assert_eq!(cont, 11); // ceil(88 / 8)
                              // Estimated groups from 88 layers: (88*2+4)/5 = 36, ceil(36/16) = 3
        assert!(
            fin >= 2 && fin <= 4,
            "Expected 2-4 finalize batches, got {fin}"
        );
    }

    #[test]
    fn test_estimate_total_batches_small_circuit() {
        let (cont, fin) = estimate_total_batches(4, 8);
        assert_eq!(cont, 1); // ceil(4 / 8)
        assert!(fin >= 1);
    }

    #[test]
    fn test_estimate_total_batches_zero_layers() {
        let (cont, fin) = estimate_total_batches(0, 8);
        assert_eq!(cont, 0);
        assert_eq!(fin, 1); // At least 1 finalize
    }

    #[test]
    fn test_estimate_total_batches_zero_layers_per_batch() {
        let (cont, _) = estimate_total_batches(88, 0);
        assert_eq!(cont, 0);
    }

    #[test]
    fn test_estimate_input_groups_88_layers() {
        let groups = estimate_input_groups(88);
        // Should be close to 34 (actual for XGBoost)
        assert!(
            groups >= 30 && groups <= 40,
            "Expected ~34 groups for 88 layers, got {groups}"
        );
    }

    #[test]
    fn test_estimate_input_groups_zero() {
        assert_eq!(estimate_input_groups(0), 0);
    }

    #[test]
    fn test_estimate_input_groups_small() {
        let groups = estimate_input_groups(5);
        assert!(groups >= 1);
    }

    #[test]
    fn test_per_step_helpers_consistent_with_session_estimate() {
        // The per-step helpers should produce estimates in the same range
        // as the session-level estimate.
        let session = estimate_batch_session(88, 34);

        let start = estimate_start_gas(88);
        // Start gas should be in the range of session start gas
        assert!(
            start >= session.start_gas.lower,
            "Per-step start {start} should be >= session lower {}",
            session.start_gas.lower
        );

        let cont_first = estimate_continue_gas(0, 8);
        assert!(
            cont_first >= CONTINUE_GAS_LOWER,
            "Continue gas should be >= lower bound"
        );
        assert!(
            cont_first <= CONTINUE_GAS_UPPER,
            "Continue gas should be <= upper bound"
        );

        let fin_full = estimate_finalize_gas(16);
        assert!(
            fin_full >= FINALIZE_GAS_LOWER,
            "Finalize gas should be >= lower bound"
        );
        assert!(
            fin_full <= FINALIZE_GAS_UPPER,
            "Finalize gas should be <= upper bound"
        );
    }

    // -----------------------------------------------------------------------
    // RpcBatchGasEstimate struct tests
    // -----------------------------------------------------------------------

    fn make_rpc_estimate(
        start: u64,
        continues: Vec<u64>,
        finalizes: Vec<u64>,
        cleanup: u64,
        gas_price_gwei: Option<f64>,
    ) -> RpcBatchGasEstimate {
        let total = start + continues.iter().sum::<u64>() + finalizes.iter().sum::<u64>() + cleanup;
        let estimated_eth_cost = gas_price_gwei.map(|g| estimate_total_cost_eth(total, g));
        RpcBatchGasEstimate {
            start_gas: start,
            continue_gas_per_step: continues,
            finalize_gas_per_step: finalizes,
            cleanup_gas: cleanup,
            total_gas: total,
            estimated_eth_cost,
        }
    }

    #[test]
    fn test_rpc_estimate_total_txs() {
        let est = make_rpc_estimate(
            17_500_000,
            vec![20_000_000; 11],
            vec![15_000_000; 3],
            500_000,
            None,
        );
        // 1 start + 11 continue + 3 finalize + 1 cleanup = 16
        assert_eq!(est.total_txs(), 16);
    }

    #[test]
    fn test_rpc_estimate_total_gas() {
        let est = make_rpc_estimate(
            17_500_000,
            vec![20_000_000; 11],
            vec![15_000_000; 3],
            500_000,
            None,
        );
        let expected = 17_500_000 + 11 * 20_000_000 + 3 * 15_000_000 + 500_000;
        assert_eq!(est.total_gas, expected);
    }

    #[test]
    fn test_rpc_estimate_avg_continue_gas() {
        let est = make_rpc_estimate(
            17_000_000,
            vec![13_000_000, 20_000_000, 28_000_000],
            vec![15_000_000],
            400_000,
            None,
        );
        // avg = (13M + 20M + 28M) / 3 = 20_333_333
        assert_eq!(est.avg_continue_gas(), 20_333_333);
    }

    #[test]
    fn test_rpc_estimate_avg_continue_gas_empty() {
        let est = make_rpc_estimate(17_000_000, vec![], vec![15_000_000], 400_000, None);
        assert_eq!(est.avg_continue_gas(), 0);
    }

    #[test]
    fn test_rpc_estimate_avg_finalize_gas() {
        let est = make_rpc_estimate(
            17_000_000,
            vec![20_000_000],
            vec![9_000_000, 22_000_000],
            400_000,
            None,
        );
        // avg = (9M + 22M) / 2 = 15_500_000
        assert_eq!(est.avg_finalize_gas(), 15_500_000);
    }

    #[test]
    fn test_rpc_estimate_avg_finalize_gas_empty() {
        let est = make_rpc_estimate(17_000_000, vec![20_000_000], vec![], 400_000, None);
        assert_eq!(est.avg_finalize_gas(), 0);
    }

    #[test]
    fn test_rpc_estimate_max_single_tx_gas() {
        let est = make_rpc_estimate(
            17_000_000,
            vec![13_000_000, 28_000_000, 20_000_000],
            vec![9_000_000, 22_000_000],
            400_000,
            None,
        );
        assert_eq!(est.max_single_tx_gas(), 28_000_000);
    }

    #[test]
    fn test_rpc_estimate_max_single_tx_start_is_max() {
        let est = make_rpc_estimate(29_000_000, vec![13_000_000], vec![9_000_000], 400_000, None);
        assert_eq!(est.max_single_tx_gas(), 29_000_000);
    }

    #[test]
    fn test_rpc_estimate_fits_in_blocks_true() {
        let est = make_rpc_estimate(
            17_000_000,
            vec![28_000_000],
            vec![22_000_000],
            400_000,
            None,
        );
        assert!(est.fits_in_blocks());
    }

    #[test]
    fn test_rpc_estimate_fits_in_blocks_false() {
        let est = make_rpc_estimate(
            17_000_000,
            vec![31_000_000], // exceeds 30M block gas limit
            vec![22_000_000],
            400_000,
            None,
        );
        assert!(!est.fits_in_blocks());
    }

    #[test]
    fn test_rpc_estimate_eth_cost_present() {
        let est = make_rpc_estimate(
            17_500_000,
            vec![20_000_000; 11],
            vec![15_000_000; 3],
            500_000,
            Some(30.0),
        );
        assert!(est.estimated_eth_cost.is_some());
        let cost = est.estimated_eth_cost.unwrap();
        // total_gas = 17.5M + 220M + 45M + 0.5M = 283M
        // cost = 283M * 30 * 1e-9 = 8.49
        let expected = 283_000_000.0 * 30.0 * 1e-9;
        assert!(
            (cost - expected).abs() < 0.01,
            "Expected ~{expected:.4}, got {cost:.4}"
        );
    }

    #[test]
    fn test_rpc_estimate_eth_cost_absent() {
        let est = make_rpc_estimate(
            17_500_000,
            vec![20_000_000],
            vec![15_000_000],
            500_000,
            None,
        );
        assert!(est.estimated_eth_cost.is_none());
    }

    #[test]
    fn test_rpc_estimate_xgboost_like_session() {
        // Simulate a realistic XGBoost 88-layer session
        let continues = vec![
            13_500_000, 18_000_000, 20_000_000, 22_000_000, 24_000_000, 25_000_000, 26_000_000,
            27_000_000, 27_500_000, 28_000_000, 15_000_000, // last batch with fewer layers
        ];
        let finalizes = vec![22_000_000, 20_000_000, 9_500_000];
        let est = make_rpc_estimate(17_500_000, continues, finalizes, 300_000, Some(30.0));

        assert_eq!(est.total_txs(), 16); // 1 + 11 + 3 + 1
        assert!(est.total_gas > 200_000_000);
        assert!(est.total_gas < 400_000_000);
        assert!(est.fits_in_blocks());
        assert!(est.estimated_eth_cost.is_some());
    }
}
