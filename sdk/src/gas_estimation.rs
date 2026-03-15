//! Gas estimation utilities for DAG batch verification.
//!
//! Provides pure-computation helpers that estimate the gas cost of a full
//! multi-transaction DAG batch verification session on the `RemainderVerifier`
//! contract. No RPC calls are made; all calculations use on-chain constants and
//! empirical gas measurements from the 88-layer XGBoost reference circuit.
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
    (num_compute_layers + LAYERS_PER_BATCH - 1) / LAYERS_PER_BATCH
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
    (num_eval_groups + GROUPS_PER_FINALIZE_BATCH - 1) / GROUPS_PER_FINALIZE_BATCH
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

    let per_layer =
        (CONTINUE_GAS_UPPER - CONTINUE_GAS_LOWER) / (full_batch as u64);
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
    let per_group =
        (FINALIZE_GAS_UPPER - FINALIZE_GAS_LOWER) / (full_batch as u64);
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
        (num_layers + layers_per_batch - 1) / layers_per_batch
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
    (num_layers * 2 + 4) / 5
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
        assert!(fin >= 2 && fin <= 4, "Expected 2-4 finalize batches, got {fin}");
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
}
