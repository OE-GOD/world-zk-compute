//! ProverRegistry SDK bindings.
//!
//! Provides a Rust client for the `ProverRegistry` contract, which manages
//! prover registration, staking, reputation, and slashing in the decentralized
//! proving network.
//!
//! # Example
//!
//! ```rust,no_run
//! use world_zk_sdk::{Client, ProverRegistryClient};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = Client::new(
//!         "http://localhost:8545",
//!         "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
//!         "0x5FbDB2315678afecb367f032d93F642f64180aa3",
//!     )?;
//!
//!     let registry = ProverRegistryClient::new(client);
//!
//!     // Query active provers
//!     let provers = registry.get_active_provers().await?;
//!     println!("Active provers: {:?}", provers);
//!
//!     Ok(())
//! }
//! ```

use alloy::primitives::{Address, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;

use crate::client::Client;

// ---------------------------------------------------------------------------
// ABI bindings generated via alloy sol! macro
// ---------------------------------------------------------------------------

sol! {
    #[sol(rpc)]
    contract ProverRegistry {
        // --- Structs ---

        struct Prover {
            address owner;
            uint256 stake;
            uint256 reputation;
            uint256 proofsSubmitted;
            uint256 proofsFailed;
            uint256 totalEarnings;
            uint256 registeredAt;
            uint256 lastActiveAt;
            bool active;
            string endpoint;
        }

        // --- Events ---

        event ProverRegistered(address indexed prover, uint256 stake, string endpoint);
        event ProverDeactivated(address indexed prover);
        event ProverReactivated(address indexed prover);
        event StakeAdded(address indexed prover, uint256 amount, uint256 newTotal);
        event StakeWithdrawn(address indexed prover, uint256 amount, uint256 newTotal);
        event ProverSlashed(address indexed prover, uint256 amount, string reason);
        event ReputationUpdated(address indexed prover, uint256 oldRep, uint256 newRep);
        event RewardDistributed(address indexed prover, uint256 amount);
        event SlasherUpdated(address indexed slasher, bool authorized);

        // --- Errors ---

        error InsufficientStake();
        error ProverNotRegistered();
        error ProverAlreadyRegistered();
        error ProverNotActive();
        error UnauthorizedSlasher();
        error WithdrawalWouldBreachMinimum();
        error NoStakeToWithdraw();

        // --- Mutating Functions ---

        function register(uint256 stake, string calldata endpoint) external;
        function addStake(uint256 amount) external;
        function withdrawStake(uint256 amount) external;
        function deactivate() external;
        function reactivate() external;

        // --- Slashing & Rewards (authorized callers) ---

        function slash(address prover, string calldata reason) external;
        function recordSuccess(address prover, uint256 reward) external;

        // --- Admin Functions (onlyOwner) ---

        function setMinStake(uint256 _minStake) external;
        function setSlashBasisPoints(uint256 _slashBasisPoints) external;
        function setSlasher(address slasher, bool authorized) external;

        // --- View Functions ---

        function getProver(address prover) external view returns (Prover memory);
        function activeProverCount() external view returns (uint256);
        function getActiveProvers() external view returns (address[] memory);
        function isProver(address addr) external view returns (bool);
        function isActive(address addr) external view returns (bool);
        function getWeight(address prover) external view returns (uint256);
        function selectProver(uint256 seed) external view returns (address);
        function getTopProvers(uint256 n) external view returns (address[] memory);

        // --- State accessors ---

        function stakingToken() external view returns (address);
        function minStake() external view returns (uint256);
        function slashBasisPoints() external view returns (uint256);
        function totalStaked() external view returns (uint256);
    }
}

// ---------------------------------------------------------------------------
// ProverRegistryClient
// ---------------------------------------------------------------------------

/// Client for the `ProverRegistry` contract.
///
/// Wraps the alloy-generated bindings and provides ergonomic methods for
/// prover registration, staking, reputation queries, and admin operations.
pub struct ProverRegistryClient {
    client: Client,
}

impl ProverRegistryClient {
    /// Create a new `ProverRegistryClient` from an SDK `Client`.
    ///
    /// The client's contract address should point to a deployed `ProverRegistry`.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Returns a reference to the underlying SDK client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    // =====================================================================
    // Prover Registration
    // =====================================================================

    /// Register as a prover with an initial stake and optional endpoint.
    ///
    /// The caller must have approved the staking token for transfer beforehand.
    /// Initial reputation is set to 5000 (50%).
    ///
    /// # Arguments
    ///
    /// * `stake` - Amount of staking tokens to deposit (must be >= minStake).
    /// * `endpoint` - Optional P2P endpoint for coordination.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn register_prover(
        &self,
        stake: U256,
        endpoint: &str,
    ) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .register(stake, endpoint.to_string())
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Deactivate the caller's prover registration.
    ///
    /// Stops receiving new jobs. The prover can later withdraw stake below
    /// the minimum while deactivated.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn deregister_prover(&self) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .deactivate()
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Reactivate the caller's prover registration.
    ///
    /// The prover must have at least `minStake` to reactivate.
    pub async fn reactivate_prover(&self) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .reactivate()
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Staking
    // =====================================================================

    /// Add additional stake to the caller's prover registration.
    ///
    /// If the prover was deactivated due to low stake and the new total
    /// exceeds `minStake`, the prover is automatically reactivated.
    pub async fn add_stake(&self, amount: U256) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .addStake(amount)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Withdraw stake from the caller's prover registration.
    ///
    /// If the prover is active, the remaining stake must be >= `minStake`.
    pub async fn withdraw_stake(&self, amount: U256) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .withdrawStake(amount)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Query Functions
    // =====================================================================

    /// Get the full prover info for a given address.
    ///
    /// Returns the on-chain `Prover` struct with stake, reputation, stats, etc.
    pub async fn get_prover_info(
        &self,
        prover: Address,
    ) -> anyhow::Result<ProverRegistry::Prover> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let info = contract.getProver(prover).call().await?;
        Ok(info)
    }

    /// Check if an address is an active prover.
    pub async fn is_prover_active(&self, addr: Address) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let active = contract.isActive(addr).call().await?;
        Ok(active)
    }

    /// Check if an address is a registered prover (active or inactive).
    pub async fn is_prover(&self, addr: Address) -> anyhow::Result<bool> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let registered = contract.isProver(addr).call().await?;
        Ok(registered)
    }

    /// Get all active prover addresses.
    pub async fn get_active_provers(&self) -> anyhow::Result<Vec<Address>> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let provers = contract.getActiveProvers().call().await?;
        Ok(provers)
    }

    /// Get the number of active provers.
    pub async fn active_prover_count(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let count = contract.activeProverCount().call().await?;
        Ok(count)
    }

    /// Get the effective weight of a prover (stake * reputation / 10000).
    pub async fn get_weight(&self, prover: Address) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let weight = contract.getWeight(prover).call().await?;
        Ok(weight)
    }

    /// Select a prover using weighted random selection based on stake and reputation.
    ///
    /// # Arguments
    ///
    /// * `seed` - Random seed (e.g., blockhash or VRF output).
    pub async fn select_prover(&self, seed: U256) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let selected = contract.selectProver(seed).call().await?;
        Ok(selected)
    }

    /// Get the top N provers by reputation.
    pub async fn get_top_provers(&self, n: u64) -> anyhow::Result<Vec<Address>> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let top = contract.getTopProvers(U256::from(n)).call().await?;
        Ok(top)
    }

    // =====================================================================
    // State Accessors
    // =====================================================================

    /// Get the staking token address.
    pub async fn staking_token(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let addr = contract.stakingToken().call().await?;
        Ok(addr)
    }

    /// Get the minimum stake required to be an active prover.
    pub async fn min_stake(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let min = contract.minStake().call().await?;
        Ok(min)
    }

    /// Get the slash basis points (e.g., 500 = 5%).
    pub async fn slash_basis_points(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let bps = contract.slashBasisPoints().call().await?;
        Ok(bps)
    }

    /// Get the total amount staked across all provers.
    pub async fn total_staked(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let total = contract.totalStaked().call().await?;
        Ok(total)
    }

    // =====================================================================
    // Admin Functions (onlyOwner)
    // =====================================================================

    /// Set the minimum stake requirement (owner only).
    pub async fn set_min_stake(
        &self,
        min_stake: U256,
    ) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .setMinStake(min_stake)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Set the slash percentage in basis points (owner only, max 50% = 5000).
    pub async fn set_slash_basis_points(
        &self,
        bps: U256,
    ) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .setSlashBasisPoints(bps)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Authorize or deauthorize a slasher address (owner only).
    pub async fn set_slasher(
        &self,
        slasher: Address,
        authorized: bool,
    ) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .setSlasher(slasher, authorized)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Slashing & Rewards (authorized callers)
    // =====================================================================

    /// Slash a prover for misbehavior (authorized slashers or owner only).
    ///
    /// Reduces the prover's stake by `slashBasisPoints` percent and decreases
    /// reputation by 5%. If stake falls below minStake, the prover is deactivated.
    pub async fn slash(
        &self,
        prover: Address,
        reason: &str,
    ) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .slash(prover, reason.to_string())
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Record a successful proof submission and update reputation (authorized callers).
    ///
    /// Increases proofsSubmitted, adds reward to totalEarnings, and increases
    /// reputation by 0.5% (capped at 100%).
    pub async fn record_success(
        &self,
        prover: Address,
        reward: U256,
    ) -> anyhow::Result<alloy::primitives::B256> {
        let provider = self.build_provider();
        let contract = ProverRegistry::new(self.client.contract_address(), provider);

        let receipt = contract
            .recordSuccess(prover, reward)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Internal
    // =====================================================================

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
    use alloy::primitives::Address;

    fn test_client() -> Client {
        Client::new(
            "http://localhost:8545",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "0x5FbDB2315678afecb367f032d93F642f64180aa3",
        )
        .expect("should create test client")
    }

    #[test]
    fn test_prover_registry_client_construction() {
        let client = test_client();
        let registry = ProverRegistryClient::new(client);
        assert_eq!(
            registry.client().contract_address(),
            "0x5FbDB2315678afecb367f032d93F642f64180aa3"
                .parse::<Address>()
                .unwrap()
        );
    }

    #[test]
    fn test_prover_registry_client_signer_address() {
        let client = test_client();
        let signer = client.signer_address();
        let registry = ProverRegistryClient::new(client);
        assert_eq!(registry.client().signer_address(), signer);
        assert_ne!(registry.client().signer_address(), Address::ZERO);
    }

    #[test]
    fn test_prover_struct_fields() {
        // Verify the generated Prover struct has expected fields.
        let prover = ProverRegistry::Prover {
            owner: Address::ZERO,
            stake: U256::from(1_000_000_000_000_000_000u64), // 1 ETH
            reputation: U256::from(5000u64),                  // 50%
            proofsSubmitted: U256::from(10u64),
            proofsFailed: U256::from(1u64),
            totalEarnings: U256::from(500_000_000_000_000u64),
            registeredAt: U256::from(1700000000u64),
            lastActiveAt: U256::from(1700001000u64),
            active: true,
            endpoint: "https://prover.example.com:8080".to_string(),
        };

        assert_eq!(prover.stake, U256::from(1_000_000_000_000_000_000u64));
        assert_eq!(prover.reputation, U256::from(5000u64));
        assert_eq!(prover.proofsSubmitted, U256::from(10u64));
        assert_eq!(prover.proofsFailed, U256::from(1u64));
        assert!(prover.active);
        assert_eq!(prover.endpoint, "https://prover.example.com:8080");
    }

    #[test]
    fn test_prover_struct_inactive() {
        let prover = ProverRegistry::Prover {
            owner: Address::ZERO,
            stake: U256::ZERO,
            reputation: U256::from(2500u64),
            proofsSubmitted: U256::from(5u64),
            proofsFailed: U256::from(3u64),
            totalEarnings: U256::ZERO,
            registeredAt: U256::from(1600000000u64),
            lastActiveAt: U256::from(1600000500u64),
            active: false,
            endpoint: String::new(),
        };

        assert!(!prover.active);
        assert_eq!(prover.stake, U256::ZERO);
        assert_eq!(prover.reputation, U256::from(2500u64));
        assert!(prover.endpoint.is_empty());
    }

    #[test]
    fn test_multiple_clients_from_different_keys() {
        let client1 = Client::new(
            "http://localhost:8545",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "0x5FbDB2315678afecb367f032d93F642f64180aa3",
        )
        .unwrap();
        let client2 = Client::new(
            "http://localhost:8545",
            "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
            "0x5FbDB2315678afecb367f032d93F642f64180aa3",
        )
        .unwrap();

        let registry1 = ProverRegistryClient::new(client1);
        let registry2 = ProverRegistryClient::new(client2);

        assert_ne!(
            registry1.client().signer_address(),
            registry2.client().signer_address()
        );
    }

    #[test]
    fn test_prover_struct_default_reputation() {
        // New provers start with reputation = 5000 (50%)
        let prover = ProverRegistry::Prover {
            owner: Address::ZERO,
            stake: U256::from(1_000_000u64),
            reputation: U256::from(5000u64),
            proofsSubmitted: U256::ZERO,
            proofsFailed: U256::ZERO,
            totalEarnings: U256::ZERO,
            registeredAt: U256::from(1700000000u64),
            lastActiveAt: U256::from(1700000000u64),
            active: true,
            endpoint: String::new(),
        };

        assert_eq!(prover.reputation, U256::from(5000u64));
        assert_eq!(prover.proofsSubmitted, U256::ZERO);
        assert_eq!(prover.proofsFailed, U256::ZERO);
    }

    #[test]
    fn test_client_contract_address_preserved() {
        let addr = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512";
        let client = Client::new(
            "http://localhost:8545",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            addr,
        )
        .unwrap();

        let registry = ProverRegistryClient::new(client);
        assert_eq!(
            registry.client().contract_address(),
            addr.parse::<Address>().unwrap()
        );
    }
}
